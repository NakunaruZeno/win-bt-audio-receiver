use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::core::Result;
use windows::Win32::Foundation::{BOOL, FALSE, TRUE};
use windows::Win32::System::Console::{
    SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT,
    CTRL_SHUTDOWN_EVENT,
};
use windows::Win32::System::Power::{
    SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_STATE,
};

#[derive(Default)]
struct SignalState {
    exit_requested: bool,
    reconnect_requested: bool,
    state_sync_requested: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeEvent {
    Exit,
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

    /// Blocks with 0% CPU until exit, connection state change, or watcher reconnect is signaled.
    pub fn wait_for_event(&self) -> WakeEvent {
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(_) => return WakeEvent::Exit,
        };

        while !guard.exit_requested
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
        if guard.state_sync_requested {
            guard.state_sync_requested = false;
            return WakeEvent::StateSync;
        }

        guard.reconnect_requested = false;
        WakeEvent::Reconnect
    }

    /// Waits for `duration` unless exit is requested earlier. Returns `true` if exiting.
    pub fn sleep_interruptible(&self, duration: Duration) -> bool {
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(_) => return true,
        };
        let deadline = Instant::now() + duration;

        while !guard.exit_requested {
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

        guard.exit_requested
    }
}

static GLOBAL_SIGNAL: OnceLock<Arc<AppSignal>> = OnceLock::new();

unsafe extern "system" fn console_ctrl_handler(ctrl_type: u32) -> BOOL {
    match ctrl_type {
        CTRL_C_EVENT
        | CTRL_BREAK_EVENT
        | CTRL_CLOSE_EVENT
        | CTRL_LOGOFF_EVENT
        | CTRL_SHUTDOWN_EVENT => {
            if let Some(signal) = GLOBAL_SIGNAL.get() {
                signal.request_exit();
            }
            TRUE
        }
        _ => FALSE,
    }
}

pub fn install_ctrl_handler(signal: Arc<AppSignal>) -> Result<()> {
    let _ = GLOBAL_SIGNAL.set(signal);
    unsafe { SetConsoleCtrlHandler(Some(console_ctrl_handler), TRUE) }
}

/// Opts out of Windows 11 EcoQoS background throttling so COM/WinRT and audio
/// threads are not throttled when the console window is minimized.
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
