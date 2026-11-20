use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::core::{IInspectable, Result, HSTRING};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationUpdate, DeviceWatcher};
use windows::Foundation::{EventRegistrationToken, TypedEventHandler};
use windows::Media::Audio::{
    AudioPlaybackConnection, AudioPlaybackConnectionOpenResultStatus, AudioPlaybackConnectionState,
};
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

const CONFIG_FILE_NAME: &str = "device_id.cfg";
const RECONNECT_DEBOUNCE: Duration = Duration::from_millis(800);

#[derive(Clone, Debug)]
struct PairedDevice {
    id: String,
    name: String,
    is_enabled: bool,
}

#[derive(Default)]
struct SignalState {
    exit_requested: bool,
    reconnect_requested: bool,
    state_sync_requested: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WakeEvent {
    Exit,
    Reconnect,
    StateSync,
}

struct AppSignal {
    state: Mutex<SignalState>,
    cv: Condvar,
}

impl AppSignal {
    fn new() -> Self {
        Self {
            state: Mutex::new(SignalState::default()),
            cv: Condvar::new(),
        }
    }

    fn request_exit(&self) {
        if let Ok(mut guard) = self.state.lock() {
            guard.exit_requested = true;
            self.cv.notify_all();
        }
    }

    fn request_reconnect(&self) {
        if let Ok(mut guard) = self.state.lock() {
            guard.reconnect_requested = true;
            self.cv.notify_all();
        }
    }

    fn request_state_sync(&self) {
        if let Ok(mut guard) = self.state.lock() {
            guard.state_sync_requested = true;
            self.cv.notify_all();
        }
    }

    fn is_exit_requested(&self) -> bool {
        self.state
            .lock()
            .map(|guard| guard.exit_requested)
            .unwrap_or(true)
    }

    /// Blocks with 0% CPU until exit, connection state change, or watcher reconnect is signaled.
    fn wait_for_event(&self) -> WakeEvent {
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
    fn sleep_interruptible(&self, duration: Duration) -> bool {
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

/// Opts out of Windows 11 EcoQoS background throttling so COM/WinRT callbacks
/// are not delayed when the console window is minimized or unfocused.
fn disable_power_throttling() {
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
fn set_streaming_keep_awake(active: bool) {
    let flags = if active {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    unsafe {
        let _ = SetThreadExecutionState(flags);
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

fn install_ctrl_handler(signal: Arc<AppSignal>) -> Result<()> {
    let _ = GLOBAL_SIGNAL.set(signal);
    unsafe { SetConsoleCtrlHandler(Some(console_ctrl_handler), TRUE) }
}

fn config_file_path() -> PathBuf {
    match std::env::current_exe() {
        Ok(exe) => match exe.parent() {
            Some(dir) => dir.join(CONFIG_FILE_NAME),
            None => PathBuf::from(CONFIG_FILE_NAME),
        },
        Err(_) => PathBuf::from(CONFIG_FILE_NAME),
    }
}

fn load_saved_device() -> Option<(String, String)> {
    let path = config_file_path();
    let content = fs::read_to_string(path).ok()?;
    let mut lines = content.lines();
    let id = lines.next()?.trim();
    if id.is_empty() {
        return None;
    }
    let name = lines
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Saved Bluetooth Device");
    Some((id.to_owned(), name.to_owned()))
}

fn save_device(id: &str, name: &str) -> io::Result<()> {
    let path = config_file_path();
    let sanitized_name = name.replace(['\r', '\n'], " ");
    fs::write(&path, format!("{id}\n{sanitized_name}\n"))?;
    println!("[Config] Saved device to {}", path.display());
    Ok(())
}

fn list_paired_devices() -> Result<Vec<PairedDevice>> {
    let selector = AudioPlaybackConnection::GetDeviceSelector()?;
    let collection = DeviceInformation::FindAllAsyncAqsFilter(&selector)?.get()?;
    let count = collection.Size()?;

    let mut devices = Vec::with_capacity(count as usize);
    for idx in 0..count {
        let info = collection.GetAt(idx)?;
        let id = info.Id()?.to_string_lossy();
        let raw_name = info.Name()?.to_string_lossy();
        let name = if raw_name.trim().is_empty() {
            "Unnamed Bluetooth Device".to_owned()
        } else {
            raw_name
        };
        let is_enabled = info.IsEnabled().unwrap_or(true);
        devices.push(PairedDevice {
            id,
            name,
            is_enabled,
        });
    }

    Ok(devices)
}

fn print_devices(devices: &[PairedDevice]) {
    println!("Paired Bluetooth audio source devices ({}):", devices.len());
    for (idx, dev) in devices.iter().enumerate() {
        let status = if dev.is_enabled { "enabled" } else { "disabled" };
        println!("  [{}] {} ({})", idx + 1, dev.name, status);
        println!("      ID: {}", dev.id);
    }
}

fn prompt_select_device(devices: &[PairedDevice]) -> Option<PairedDevice> {
    if devices.is_empty() {
        println!(
            "No paired Bluetooth audio devices found.\n\
             Pair your phone with Windows via Settings -> Bluetooth & devices first."
        );
        return None;
    }

    if devices.len() == 1 {
        let dev = devices[0].clone();
        println!("Auto-selected the only paired device: {}", dev.name);
        return Some(dev);
    }

    print_devices(devices);
    loop {
        print!("Select device number [1-{}] (or 'q' to quit): ", devices.len());
        let _ = io::stdout().flush();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            return None;
        }
        let trimmed = input.trim();
        if trimmed.eq_ignore_ascii_case("q") || trimmed.eq_ignore_ascii_case("quit") {
            return None;
        }
        if let Ok(choice) = trimmed.parse::<usize>() {
            if (1..=devices.len()).contains(&choice) {
                return Some(devices[choice - 1].clone());
            }
        }
        println!("Invalid choice '{trimmed}'. Please enter a number between 1 and {}.", devices.len());
    }
}

fn find_device_by_query(devices: &[PairedDevice], query: &str) -> Option<PairedDevice> {
    if let Some(exact) = devices.iter().find(|d| d.id.eq_ignore_ascii_case(query)) {
        return Some(exact.clone());
    }

    let query_lower = query.to_ascii_lowercase();
    let matches: Vec<PairedDevice> = devices
        .iter()
        .filter(|d| {
            d.name.to_ascii_lowercase().contains(&query_lower)
                || d.id.to_ascii_lowercase().contains(&query_lower)
        })
        .cloned()
        .collect();

    match matches.len() {
        0 => {
            eprintln!("No paired Bluetooth device matches '{query}'.");
            None
        }
        1 => Some(matches[0].clone()),
        _ => {
            println!("Multiple devices match '{query}':");
            prompt_select_device(&matches)
        }
    }
}

struct ActiveConnection {
    handle: AudioPlaybackConnection,
    state_token: EventRegistrationToken,
}

impl ActiveConnection {
    fn close_cleanly(self) {
        let _ = self.handle.RemoveStateChanged(self.state_token);
        let _ = self.handle.Close();
    }
}

/// Initializes an `AudioPlaybackConnection`, attaches `StateChanged`, calls `StartAsync`,
/// and attempts `OpenAsync`. Returns `Ok(Some(...))` if the connection object was created
/// (even if `OpenAsync` timed out because the phone is temporarily out of range), or `Err`
/// if `TryCreateFromId` / `StartAsync` itself failed.
fn connect_and_open(
    device_id: &str,
    device_name: &str,
    signal: &Arc<AppSignal>,
    was_opened: &Arc<AtomicBool>,
) -> Result<(ActiveConnection, bool)> {
    let t0 = Instant::now();
    let id_hstring = HSTRING::from(device_id);
    let connection = AudioPlaybackConnection::TryCreateFromId(&id_hstring)?;

    let signal_clone = Arc::clone(signal);
    let was_opened_clone = Arc::clone(was_opened);
    let name_owned = device_name.to_owned();

    let state_handler = TypedEventHandler::<AudioPlaybackConnection, IInspectable>::new(
        move |sender, _args| {
            if let Some(conn) = sender {
                if let Ok(state) = conn.State() {
                    match state {
                        AudioPlaybackConnectionState::Opened => {
                            was_opened_clone.store(true, Ordering::SeqCst);
                            println!("[State] '{name_owned}' -> Opened (audio stream active).");
                            signal_clone.request_state_sync();
                        }
                        AudioPlaybackConnectionState::Closed => {
                            let previously_opened = was_opened_clone.swap(false, Ordering::SeqCst);
                            if previously_opened && !signal_clone.is_exit_requested() {
                                println!(
                                    "[State] '{name_owned}' -> Closed (disconnected). Waiting for device..."
                                );
                                signal_clone.request_state_sync();
                            }
                        }
                        other => {
                            println!("[State] '{name_owned}' -> Unknown ({})", other.0);
                        }
                    }
                }
            }
            Ok(())
        },
    );

    let state_token = connection.StateChanged(&state_handler)?;

    connection.StartAsync()?.get()?;
    let opened_ok = try_open_stream(&connection, device_name, was_opened, Some(t0))?;

    Ok((
        ActiveConnection {
            handle: connection,
            state_token,
        },
        opened_ok,
    ))
}

fn try_open_stream(
    connection: &AudioPlaybackConnection,
    device_name: &str,
    was_opened: &Arc<AtomicBool>,
    start_time: Option<Instant>,
) -> Result<bool> {
    if matches!(connection.State(), Ok(AudioPlaybackConnectionState::Opened)) {
        was_opened.store(true, Ordering::SeqCst);
        match start_time {
            Some(t0) => println!(
                "[Connected] '{device_name}' is already streaming audio! ({:.0} ms)",
                t0.elapsed().as_secs_f64() * 1000.0
            ),
            None => println!("[Reconnected] '{device_name}' is streaming audio!"),
        }
        return Ok(true);
    }

    let open_result = connection.OpenAsync()?.get()?;
    let status = open_result.Status()?;
    match status {
        AudioPlaybackConnectionOpenResultStatus::Success => {
            was_opened.store(true, Ordering::SeqCst);
            match start_time {
                Some(t0) => println!(
                    "[Connected] '{device_name}' is streaming audio! ({:.0} ms)",
                    t0.elapsed().as_secs_f64() * 1000.0
                ),
                None => println!("[Reconnected] '{device_name}' is streaming audio!"),
            }
            Ok(true)
        }
        AudioPlaybackConnectionOpenResultStatus::RequestTimedOut => {
            println!(
                "[Waiting] OpenAsync timed out for '{device_name}'. \
                 Ensure Bluetooth is on and the device is in range; auto-reconnect is watching."
            );
            Ok(false)
        }
        AudioPlaybackConnectionOpenResultStatus::DeniedBySystem => {
            println!(
                "[Waiting] System denied audio connection for '{device_name}'. \
                 Will retry automatically when device status updates."
            );
            Ok(false)
        }
        AudioPlaybackConnectionOpenResultStatus::UnknownFailure => {
            let ext = open_result.ExtendedError().map(|h| h.0).unwrap_or_default();
            println!(
                "[Waiting] OpenAsync failed for '{device_name}' (ExtendedError: 0x{ext:08X}). \
                 Auto-reconnect is watching."
            );
            Ok(false)
        }
        other => {
            println!(
                "[Waiting] Unexpected open status ({}) for '{device_name}'.",
                other.0
            );
            Ok(false)
        }
    }
}

struct WatcherGuard {
    watcher: DeviceWatcher,
    added_token: EventRegistrationToken,
    updated_token: EventRegistrationToken,
    enum_token: EventRegistrationToken,
}

impl WatcherGuard {
    fn start(target_id: String, signal: Arc<AppSignal>) -> Result<Self> {
        let selector = AudioPlaybackConnection::GetDeviceSelector()?;
        let watcher = DeviceInformation::CreateWatcherAqsFilter(&selector)?;

        let target_added = HSTRING::from(target_id.as_str());
        let signal_added = Arc::clone(&signal);
        let initial_enum_done = Arc::new(AtomicBool::new(false));
        let enum_flag_added = Arc::clone(&initial_enum_done);

        let added_handler = TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(
            move |_sender, info| {
                // Skip initial DeviceWatcher enumeration burst since startup connects directly;
                // subsequent Added events signal a newly attached / reconnected Bluetooth endpoint.
                if enum_flag_added.load(Ordering::Acquire) {
                    if let Some(dev) = info {
                        if let Ok(id) = dev.Id() {
                            if id == target_added {
                                println!("[Watcher] Target device appeared; triggering reconnect.");
                                signal_added.request_reconnect();
                            }
                        }
                    }
                }
                Ok(())
            },
        );

        let target_updated = HSTRING::from(target_id);
        let signal_updated = Arc::clone(&signal);
        let updated_handler = TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(
            move |_sender, update| {
                if let Some(dev_update) = update {
                    if let Ok(id) = dev_update.Id() {
                        if id == target_updated {
                            signal_updated.request_reconnect();
                        }
                    }
                }
                Ok(())
            },
        );

        let enum_flag_done = Arc::clone(&initial_enum_done);
        let enum_completed_handler = TypedEventHandler::<DeviceWatcher, IInspectable>::new(
            move |_sender, _args| {
                enum_flag_done.store(true, Ordering::Release);
                Ok(())
            },
        );

        let added_token = watcher.Added(&added_handler)?;
        let updated_token = watcher.Updated(&updated_handler)?;
        let enum_token = watcher.EnumerationCompleted(&enum_completed_handler)?;
        watcher.Start()?;

        Ok(Self {
            watcher,
            added_token,
            updated_token,
            enum_token,
        })
    }

    fn stop_cleanly(self) {
        let _ = self.watcher.RemoveAdded(self.added_token);
        let _ = self.watcher.RemoveUpdated(self.updated_token);
        let _ = self.watcher.RemoveEnumerationCompleted(self.enum_token);
        let _ = self.watcher.Stop();
    }
}

fn print_help() {
    println!(
        "AetherSora - Lightweight Windows Bluetooth A2DP Sink\n\
         \n\
         USAGE:\n\
         \taethersora                   Fast-connect using saved device_id.cfg (or scan on first run)\n\
         \taethersora --scan            Scan paired devices, select one, and save to device_id.cfg\n\
         \taethersora --list            List all paired Bluetooth audio source devices and exit\n\
         \taethersora --device Q        Connect to device matching name/ID substring Q and save\n\
         \taethersora --no-reconnect    Disable DeviceWatcher auto-reconnect for minimal footprint\n\
         \taethersora --help            Show this help message\n\
         \n\
         Press Ctrl+C while running to close the audio connection and exit cleanly."
    );
}

enum Mode {
    Default,
    Scan,
    List,
    Device(String),
    Help,
}

struct CliOptions {
    mode: Mode,
    auto_reconnect: bool,
}

fn parse_args() -> std::result::Result<CliOptions, String> {
    let mut args = std::env::args().skip(1);
    let mut mode = Mode::Default;
    let mut auto_reconnect = true;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(CliOptions { mode: Mode::Help, auto_reconnect }),
            "-l" | "--list" => mode = Mode::List,
            "-s" | "--scan" => mode = Mode::Scan,
            "--no-reconnect" => auto_reconnect = false,
            "-d" | "--device" => match args.next() {
                Some(q) if !q.trim().is_empty() => mode = Mode::Device(q),
                _ => return Err("Missing value for --device <name_or_id>".to_owned()),
            },
            a if a.starts_with("--device=") => {
                let q = a.trim_start_matches("--device=");
                if q.trim().is_empty() {
                    return Err("Missing value for --device=<name_or_id>".to_owned());
                }
                mode = Mode::Device(q.to_owned());
            }
            other => return Err(format!("Unknown argument: {other}")),
        }
    }

    Ok(CliOptions {
        mode,
        auto_reconnect,
    })
}

fn persist_and_return(dev: Option<PairedDevice>) -> Option<(String, String)> {
    let dev = dev?;
    if let Err(e) = save_device(&dev.id, &dev.name) {
        eprintln!("[Warning] Failed to save {CONFIG_FILE_NAME}: {e}");
    }
    Some((dev.id, dev.name))
}

fn scan_and_select_device() -> Result<Option<(String, String)>> {
    println!("Scanning paired Bluetooth audio devices...");
    let devices = list_paired_devices()?;
    Ok(persist_and_return(prompt_select_device(&devices)))
}

fn resolve_target_device(mode: Mode) -> Result<Option<(String, String)>> {
    match mode {
        Mode::Default => {
            if let Some((id, name)) = load_saved_device() {
                println!("[Fast Connect] Using cached device '{name}' from {CONFIG_FILE_NAME}");
                return Ok(Some((id, name)));
            }
            println!("No saved {CONFIG_FILE_NAME} found.");
            scan_and_select_device()
        }
        Mode::Scan => scan_and_select_device(),
        Mode::Device(query) => {
            let devices = list_paired_devices()?;
            Ok(persist_and_return(find_device_by_query(&devices, &query)))
        }
        Mode::List | Mode::Help => Ok(None),
    }
}

fn is_stream_opened(active_conn: &Option<ActiveConnection>) -> bool {
    matches!(
        active_conn.as_ref().and_then(|c| c.handle.State().ok()),
        Some(AudioPlaybackConnectionState::Opened)
    )
}

fn sync_watcher_and_power(
    opened: bool,
    auto_reconnect: bool,
    device_id: &str,
    signal: &Arc<AppSignal>,
    watcher: &mut Option<WatcherGuard>,
) {
    set_streaming_keep_awake(opened);

    if opened {
        if let Some(guard) = watcher.take() {
            guard.stop_cleanly();
            println!("[Watcher] Stopped while streaming (zero background polling).");
        }
    } else if auto_reconnect && watcher.is_none() {
        match WatcherGuard::start(device_id.to_owned(), Arc::clone(signal)) {
            Ok(guard) => {
                *watcher = Some(guard);
                println!("[Watcher] Armed on-demand to auto-reconnect when device returns.");
            }
            Err(err) => {
                eprintln!("[Warning] Failed to start DeviceWatcher: {err}");
            }
        }
    }
}

fn run() -> Result<i32> {
    let options = match parse_args() {
        Ok(o) => o,
        Err(msg) => {
            eprintln!("Error: {msg}\n");
            print_help();
            return Ok(1);
        }
    };

    match options.mode {
        Mode::Help => {
            print_help();
            return Ok(0);
        }
        Mode::List => {
            let devices = list_paired_devices()?;
            if devices.is_empty() {
                println!("No paired Bluetooth audio source devices found.");
            } else {
                print_devices(&devices);
            }
            return Ok(0);
        }
        _ => {}
    }

    let Some((device_id, device_name)) = resolve_target_device(options.mode)? else {
        return Ok(1);
    };

    disable_power_throttling();

    let signal = Arc::new(AppSignal::new());
    install_ctrl_handler(Arc::clone(&signal))?;

    let was_opened = Arc::new(AtomicBool::new(false));
    let mut active_conn: Option<ActiveConnection> =
        match connect_and_open(&device_id, &device_name, &signal, &was_opened) {
            Ok((conn, _opened)) => Some(conn),
            Err(err) => {
                eprintln!(
                    "[Waiting] Initial TryCreateFromId/Start failed for '{device_name}': {err}. \
                     Waiting for device..."
                );
                None
            }
        };

    // Only keep DeviceWatcher alive while disconnected; pause it while streaming
    // to eliminate unnecessary background PnP/Bluetooth watcher overhead.
    let mut watcher: Option<WatcherGuard> = None;
    sync_watcher_and_power(
        is_stream_opened(&active_conn),
        options.auto_reconnect,
        &device_id,
        &signal,
        &mut watcher,
    );

    println!("Running in background (0% CPU idle). Press Ctrl+C to disconnect and exit.");

    loop {
        match signal.wait_for_event() {
            WakeEvent::Exit => break,
            WakeEvent::StateSync => {
                sync_watcher_and_power(
                    is_stream_opened(&active_conn),
                    options.auto_reconnect,
                    &device_id,
                    &signal,
                    &mut watcher,
                );
                continue;
            }
            WakeEvent::Reconnect => {}
        }

        // Coalesce bursty DeviceWatcher Updated events while Bluetooth settles.
        if signal.sleep_interruptible(RECONNECT_DEBOUNCE) {
            break;
        }

        if is_stream_opened(&active_conn) {
            sync_watcher_and_power(true, options.auto_reconnect, &device_id, &signal, &mut watcher);
            continue;
        }

        match &active_conn {
            Some(conn) => {
                println!("[Reconnect] Attempting to open audio stream for '{device_name}'...");
                if let Err(err) = try_open_stream(&conn.handle, &device_name, &was_opened, None) {
                    eprintln!("[Reconnect] OpenAsync error ({err}); recreating connection handle...");
                    if let Some(old) = active_conn.take() {
                        old.close_cleanly();
                    }
                    if let Ok((new_conn, _)) =
                        connect_and_open(&device_id, &device_name, &signal, &was_opened)
                    {
                        active_conn = Some(new_conn);
                    }
                }
            }
            None => {
                println!("[Reconnect] Creating AudioPlaybackConnection for '{device_name}'...");
                match connect_and_open(&device_id, &device_name, &signal, &was_opened) {
                    Ok((new_conn, _)) => active_conn = Some(new_conn),
                    Err(err) => eprintln!("[Reconnect] Still waiting ({err})."),
                }
            }
        }

        sync_watcher_and_power(
            is_stream_opened(&active_conn),
            options.auto_reconnect,
            &device_id,
            &signal,
            &mut watcher,
        );
    }

    println!("\n[Shutdown] Closing Bluetooth audio connection...");
    set_streaming_keep_awake(false);
    if let Some(guard) = watcher.take() {
        guard.stop_cleanly();
    }
    if let Some(conn) = active_conn.take() {
        conn.close_cleanly();
    }
    println!("[Shutdown] Disconnected cleanly. Bye!");

    Ok(0)
}

fn main() {
    match run() {
        Ok(code) => {
            if code != 0 {
                process::exit(code);
            }
        }
        Err(err) => {
            eprintln!("Fatal Windows API error: {err}");
            process::exit(1);
        }
    }
}
