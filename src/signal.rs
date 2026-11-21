use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{w, Result};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, BOOL, ERROR_ALREADY_EXISTS, FALSE, HANDLE, LPARAM, TRUE,
    WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::Console::{
    AllocConsole, AttachConsole, SetConsoleCtrlHandler, ATTACH_PARENT_PROCESS, CTRL_BREAK_EVENT,
    CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
};
use windows::Win32::System::Power::{
    SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, GetCurrentProcess, OpenEventW, ProcessPowerThrottling, SetEvent,
    SetProcessInformation, WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_STATE,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};

use crate::config::TransportMode;
use crate::tray::{TRAY_WND_CLASS_NAME, WM_TRAY_ALREADY_RUNNING};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoUpgradeTarget {
    WifiApp(SocketAddr),
    WifiAdb,
}

#[derive(Default)]
struct SignalState {
    exit_requested: bool,
    mode_switch: Option<TransportMode>,
    auto_upgrade: Option<AutoUpgradeTarget>,
    reconnect_requested: bool,
    state_sync_requested: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeEvent {
    Exit,
    SwitchMode(TransportMode),
    AutoUpgrade(AutoUpgradeTarget),
    Reconnect,
    StateSync,
}

pub struct AppSignal {
    state: Mutex<SignalState>,
    cv: Condvar,
}

impl AppSignal {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(SignalState::default()),
            cv: Condvar::new(),
        }
    }

    pub fn request_exit(&self) {
        if let Ok(mut guard) = self.state.lock() {
            guard.exit_requested = true;
            self.cv.notify_all();
        }
    }

    pub fn request_mode_switch(&self, mode: TransportMode) {
        if let Ok(mut guard) = self.state.lock() {
            guard.mode_switch = Some(mode);
            self.cv.notify_all();
        }
    }

    pub fn take_mode_switch(&self) -> Option<TransportMode> {
        self.state
            .lock()
            .ok()
            .and_then(|mut guard| guard.mode_switch.take())
    }

    pub fn request_auto_upgrade(&self, target: AutoUpgradeTarget) {
        if let Ok(mut guard) = self.state.lock() {
            guard.auto_upgrade = Some(target);
            self.cv.notify_all();
        }
    }

    pub fn take_auto_upgrade(&self) -> Option<AutoUpgradeTarget> {
        self.state
            .lock()
            .ok()
            .and_then(|mut guard| guard.auto_upgrade.take())
    }

    pub fn request_reconnect(&self) {
        if let Ok(mut guard) = self.state.lock() {
            guard.reconnect_requested = true;
            self.cv.notify_all();
        }
    }

    pub fn request_state_sync(&self) {
        if let Ok(mut guard) = self.state.lock() {
            guard.state_sync_requested = true;
            self.cv.notify_all();
        }
    }

    pub fn is_exit_requested(&self) -> bool {
        self.state
            .lock()
            .map(|guard| guard.exit_requested)
            .unwrap_or(true)
    }

    /// Blocks with 0% CPU until exit, mode switch, connection state change, or watcher reconnect.
    pub fn wait_for_event(&self) -> WakeEvent {
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(_) => return WakeEvent::Exit,
        };

        while !guard.exit_requested
            && guard.mode_switch.is_none()
            && guard.auto_upgrade.is_none()
            && !guard.reconnect_requested
            && !guard.state_sync_requested
        {
            guard = match self.cv.wait(guard) {
                Ok(g) => g,
                Err(_) => return WakeEvent::Exit,
            };
        }

        if guard.exit_requested {
            return WakeEvent::Exit;
        }
        if let Some(mode) = guard.mode_switch {
            return WakeEvent::SwitchMode(mode);
        }
        if let Some(target) = guard.auto_upgrade {
            return WakeEvent::AutoUpgrade(target);
        }
        if guard.state_sync_requested {
            guard.state_sync_requested = false;
            return WakeEvent::StateSync;
        }

        guard.reconnect_requested = false;
        WakeEvent::Reconnect
    }

    /// Waits for `duration` unless exit, mode switch, or auto-upgrade is requested earlier.
    /// Returns `true` if exiting, switching modes, or auto-upgrading.
    pub fn sleep_interruptible(&self, duration: Duration) -> bool {
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(_) => return true,
        };
        let deadline = Instant::now() + duration;

        while !guard.exit_requested && guard.mode_switch.is_none() && guard.auto_upgrade.is_none() {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let (next_guard, timeout) = match self.cv.wait_timeout(guard, deadline - now) {
                Ok(res) => res,
                Err(_) => return true,
            };
            guard = next_guard;
            if timeout.timed_out() {
                break;
            }
        }

        guard.exit_requested || guard.mode_switch.is_some() || guard.auto_upgrade.is_some()
    }
}

static GLOBAL_SIGNAL: OnceLock<Arc<AppSignal>> = OnceLock::new();
static EXIT_ON_CONSOLE_CTRL: AtomicBool = AtomicBool::new(true);

unsafe extern "system" fn console_ctrl_handler(ctrl_type: u32) -> BOOL {
    match ctrl_type {
        CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
            if let Some(signal) = GLOBAL_SIGNAL.get() {
                signal.request_exit();
            }
            TRUE
        }
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT => {
            if EXIT_ON_CONSOLE_CTRL.load(Ordering::Acquire) {
                if let Some(signal) = GLOBAL_SIGNAL.get() {
                    signal.request_exit();
                }
            }
            TRUE
        }
        _ => FALSE,
    }
}

pub fn install_ctrl_handler(signal: Arc<AppSignal>, exit_on_console_ctrl: bool) -> Result<()> {
    EXIT_ON_CONSOLE_CTRL.store(exit_on_console_ctrl, Ordering::Release);
    let _ = GLOBAL_SIGNAL.set(signal);
    unsafe { SetConsoleCtrlHandler(Some(console_ctrl_handler), TRUE) }
}

/// Attaches stdout/stderr/stdin to the parent terminal (PowerShell/CMD) when invoked with CLI flags
/// under `#![windows_subsystem = "windows"]`. Returns `true` if a parent console was attached.
pub fn attach_parent_console() -> bool {
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS).is_ok() }
}

/// Ensures a console exists for explicit `--console` or interactive `--scan` sessions.
pub fn ensure_console_for_cli() {
    if !attach_parent_console() {
        unsafe {
            let _ = AllocConsole();
        }
    }
}

/// Opts out of Windows 11 EcoQoS background throttling so COM/WinRT and audio
/// threads are not throttled when running in the System Tray.
pub fn disable_power_throttling() {
    let throttling = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: 0,
    };
    unsafe {
        let _ = SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            (&throttling as *const PROCESS_POWER_THROTTLING_STATE).cast(),
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

/// Prevents Windows idle sleep while audio is actively streaming; releases the
/// assertion as soon as the stream closes or the app exits.
pub fn set_streaming_keep_awake(active: bool) {
    let flags = if active {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    unsafe {
        let _ = SetThreadExecutionState(flags);
    }
}

/// Ensures only one AetherSora receiver instance runs per user session.
pub struct SingleInstanceGuard {
    handle: HANDLE,
}

impl SingleInstanceGuard {
    pub fn acquire() -> std::result::Result<Self, String> {
        unsafe {
            let handle = CreateMutexW(None, FALSE, w!("Local\\AetherSoraSingleInstance"))
                .map_err(|e| format!("Failed to create single-instance mutex: {e}"))?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                return Err(
                    "Another instance of AetherSora is already running. Use `AetherSora --stop` to stop it."
                        .to_owned(),
                );
            }
            Ok(Self { handle })
        }
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Notifies an already-running tray instance so it displays a native Windows Toast
/// when the user double-clicks `AetherSora.exe` a second time.
pub fn notify_existing_tray_instance() -> bool {
    unsafe {
        let Ok(hwnd) = FindWindowW(TRAY_WND_CLASS_NAME, None) else {
            return false;
        };
        if hwnd.0.is_null() {
            return false;
        }
        PostMessageW(hwnd, WM_TRAY_ALREADY_RUNNING, WPARAM(0), LPARAM(0)).is_ok()
    }
}

/// Spawns a 0% CPU listener waiting on `Local\AetherSoraStopEvent` so `AetherSora --stop`
/// cleanly shuts down a hidden System Tray instance.
pub fn spawn_stop_event_listener(signal: Arc<AppSignal>) {
    let _ = thread::Builder::new()
        .name("aether-stop-evt".to_owned())
        .spawn(move || unsafe {
            let Ok(evt) = CreateEventW(None, TRUE, FALSE, w!("Local\\AetherSoraStopEvent")) else {
                return;
            };
            if WaitForSingleObject(evt, INFINITE) == WAIT_OBJECT_0 {
                signal.request_exit();
            }
            let _ = CloseHandle(evt);
        });
}

/// Signals an already-running AetherSora instance to shut down cleanly.
pub fn send_stop_signal() -> std::result::Result<(), String> {
    unsafe {
        let evt = OpenEventW(EVENT_MODIFY_STATE, FALSE, w!("Local\\AetherSoraStopEvent"))
            .map_err(|_| "No running AetherSora instance found.".to_owned())?;
        let res = SetEvent(evt).map_err(|e| format!("Failed to signal AetherSora stop event: {e}"));
        let _ = CloseHandle(evt);
        res
    }
}
