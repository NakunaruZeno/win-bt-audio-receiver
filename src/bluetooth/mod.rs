use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::{IInspectable, Result, HSTRING};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationUpdate, DeviceWatcher};
use windows::Foundation::{EventRegistrationToken, TypedEventHandler};
use windows::Media::Audio::{
    AudioPlaybackConnection, AudioPlaybackConnectionOpenResultStatus, AudioPlaybackConnectionState,
};

use crate::config::{load_saved_bt_device, save_bt_device, CONFIG_FILE_NAME};
use crate::signal::{set_streaming_keep_awake, AppSignal, WakeEvent};

const RECONNECT_DEBOUNCE: Duration = Duration::from_millis(800);

#[derive(Clone, Debug)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    pub is_enabled: bool,
}

#[derive(Clone, Debug)]
pub enum TargetQuery {
    DefaultOrCached,
    ForceScan,
    Query(String),
}

pub fn list_paired_devices() -> Result<Vec<PairedDevice>> {
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

pub fn print_devices(devices: &[PairedDevice]) {
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
        println!(
            "Invalid choice '{trimmed}'. Please enter a number between 1 and {}.",
            devices.len()
        );
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

fn persist_and_return(dev: Option<PairedDevice>) -> Option<(String, String)> {
    let dev = dev?;
    if let Err(e) = save_bt_device(&dev.id, &dev.name) {
        eprintln!("[Warning] Failed to save {CONFIG_FILE_NAME}: {e}");
    }
    Some((dev.id, dev.name))
}

fn scan_and_select_device() -> Result<Option<(String, String)>> {
    println!("[Bluetooth] Scanning paired Bluetooth audio devices...");
    let devices = list_paired_devices()?;
    Ok(persist_and_return(prompt_select_device(&devices)))
}

pub fn resolve_target_device(target: &TargetQuery) -> Result<Option<(String, String)>> {
    match target {
        TargetQuery::DefaultOrCached => {
            if let Some((id, name)) = load_saved_bt_device() {
                println!(
                    "[Bluetooth] Using cached device '{name}' from {CONFIG_FILE_NAME}"
                );
                return Ok(Some((id, name)));
            }
            println!("[Bluetooth] No saved {CONFIG_FILE_NAME} found.");
            scan_and_select_device()
        }
        TargetQuery::ForceScan => scan_and_select_device(),
        TargetQuery::Query(query) => {
            let devices = list_paired_devices()?;
            Ok(persist_and_return(find_device_by_query(&devices, query)))
        }
    }
}

struct ActiveConnection {
    handle: AudioPlaybackConnection,
    state_token: EventRegistrationToken,
}

impl ActiveConnection {
    fn close_cleanly(&mut self) {
        let _ = self.handle.RemoveStateChanged(self.state_token);
        let _ = self.handle.Close();
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
                if enum_flag_added.load(Ordering::Acquire) {
                    if let Some(dev) = info {
                        if let Ok(id) = dev.Id() {
                            if id == target_added {
                                println!(
                                    "[Bluetooth Watcher] Target device appeared; triggering reconnect."
                                );
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

    fn stop_cleanly(&mut self) {
        let _ = self.watcher.RemoveAdded(self.added_token);
        let _ = self.watcher.RemoveUpdated(self.updated_token);
        let _ = self.watcher.RemoveEnumerationCompleted(self.enum_token);
        let _ = self.watcher.Stop();
    }
}

/// Strictly isolated Bluetooth A2DP Sink session.
/// Holds WinRT `AudioPlaybackConnection` and on-demand `DeviceWatcher` handles.
/// Zero Wi-Fi, UDP, WASAPI, or ADB resources exist while `BtSession` is active.
pub struct BtSession {
    device_id: String,
    device_name: String,
    auto_reconnect: bool,
    signal: Arc<AppSignal>,
    was_opened: Arc<AtomicBool>,
    active_conn: Option<ActiveConnection>,
    watcher: Option<WatcherGuard>,
}

impl BtSession {
    pub fn start(
        device_id: String,
        device_name: String,
        auto_reconnect: bool,
        signal: Arc<AppSignal>,
    ) -> Result<Self> {
        println!("[Mode: Bluetooth A2DP Sink] Initializing WinRT AudioPlaybackConnection...");
        let was_opened = Arc::new(AtomicBool::new(false));
        let active_conn = match connect_and_open(&device_id, &device_name, &signal, &was_opened) {
            Ok((conn, _opened)) => Some(conn),
            Err(err) => {
                eprintln!(
                    "[Bluetooth Waiting] Initial TryCreateFromId/Start failed for '{device_name}': {err}. \
                     Waiting for device..."
                );
                None
            }
        };

        let mut watcher = None;
        sync_watcher_and_power(
            is_stream_opened(&active_conn),
            auto_reconnect,
            &device_id,
            &signal,
            &mut watcher,
        );

        Ok(Self {
            device_id,
            device_name,
            auto_reconnect,
            signal,
            was_opened,
            active_conn,
            watcher,
        })
    }

    pub fn run_until_exit(&mut self) {
        println!(
            "[Bluetooth] Running in background (0% CPU idle). Press Ctrl+C to disconnect and exit."
        );

        loop {
            match self.signal.wait_for_event() {
                WakeEvent::Exit => break,
                WakeEvent::StateSync => {
                    sync_watcher_and_power(
                        is_stream_opened(&self.active_conn),
                        self.auto_reconnect,
                        &self.device_id,
                        &self.signal,
                        &mut self.watcher,
                    );
                    continue;
                }
                WakeEvent::Reconnect => {}
            }

            if self.signal.sleep_interruptible(RECONNECT_DEBOUNCE) {
                break;
            }

            if is_stream_opened(&self.active_conn) {
                sync_watcher_and_power(
                    true,
                    self.auto_reconnect,
                    &self.device_id,
                    &self.signal,
                    &mut self.watcher,
                );
                continue;
            }

            match &self.active_conn {
                Some(conn) => {
                    println!(
                        "[Bluetooth Reconnect] Attempting to open audio stream for '{}'...",
                        self.device_name
                    );
                    if let Err(err) =
                        try_open_stream(&conn.handle, &self.device_name, &self.was_opened, None)
                    {
                        eprintln!(
                            "[Bluetooth Reconnect] OpenAsync error ({err}); recreating handle..."
                        );
                        if let Some(mut old) = self.active_conn.take() {
                            old.close_cleanly();
                        }
                        if let Ok((new_conn, _)) = connect_and_open(
                            &self.device_id,
                            &self.device_name,
                            &self.signal,
                            &self.was_opened,
                        ) {
                            self.active_conn = Some(new_conn);
                        }
                    }
                }
                None => {
                    println!(
                        "[Bluetooth Reconnect] Creating AudioPlaybackConnection for '{}'...",
                        self.device_name
                    );
                    match connect_and_open(
                        &self.device_id,
                        &self.device_name,
                        &self.signal,
                        &self.was_opened,
                    ) {
                        Ok((new_conn, _)) => self.active_conn = Some(new_conn),
                        Err(err) => eprintln!("[Bluetooth Reconnect] Still waiting ({err})."),
                    }
                }
            }

            sync_watcher_and_power(
                is_stream_opened(&self.active_conn),
                self.auto_reconnect,
                &self.device_id,
                &self.signal,
                &mut self.watcher,
            );
        }
    }
}

impl Drop for BtSession {
    fn drop(&mut self) {
        set_streaming_keep_awake(false);
        if let Some(mut guard) = self.watcher.take() {
            guard.stop_cleanly();
        }
        if let Some(mut conn) = self.active_conn.take() {
            conn.close_cleanly();
        }
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
        if let Some(mut guard) = watcher.take() {
            guard.stop_cleanly();
            println!("[Bluetooth Watcher] Stopped while streaming (zero background polling).");
        }
    } else if auto_reconnect && watcher.is_none() {
        match WatcherGuard::start(device_id.to_owned(), Arc::clone(signal)) {
            Ok(guard) => {
                *watcher = Some(guard);
                println!(
                    "[Bluetooth Watcher] Armed on-demand to auto-reconnect when device returns."
                );
            }
            Err(err) => {
                eprintln!("[Warning] Failed to start DeviceWatcher: {err}");
            }
        }
    }
}

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
                            println!("[Bluetooth State] '{name_owned}' -> Opened (stream active).");
                            signal_clone.request_state_sync();
                        }
                        AudioPlaybackConnectionState::Closed => {
                            let previously_opened = was_opened_clone.swap(false, Ordering::SeqCst);
                            if previously_opened && !signal_clone.is_exit_requested() {
                                println!(
                                    "[Bluetooth State] '{name_owned}' -> Closed (disconnected)."
                                );
                                signal_clone.request_state_sync();
                            }
                        }
                        other => {
                            println!("[Bluetooth State] '{name_owned}' -> Unknown ({})", other.0);
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
                "[Bluetooth Connected] '{device_name}' is already streaming audio! ({:.0} ms)",
                t0.elapsed().as_secs_f64() * 1000.0
            ),
            None => println!("[Bluetooth Reconnected] '{device_name}' is streaming audio!"),
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
                    "[Bluetooth Connected] '{device_name}' is streaming audio! ({:.0} ms)",
                    t0.elapsed().as_secs_f64() * 1000.0
                ),
                None => println!("[Bluetooth Reconnected] '{device_name}' is streaming audio!"),
            }
            Ok(true)
        }
        AudioPlaybackConnectionOpenResultStatus::RequestTimedOut => {
            println!(
                "[Bluetooth Waiting] OpenAsync timed out for '{device_name}'. \
                 Ensure Bluetooth is on and in range."
            );
            Ok(false)
        }
        AudioPlaybackConnectionOpenResultStatus::DeniedBySystem => {
            println!(
                "[Bluetooth Waiting] System denied audio connection for '{device_name}'. \
                 Will retry when device status updates."
            );
            Ok(false)
        }
        AudioPlaybackConnectionOpenResultStatus::UnknownFailure => {
            let ext = open_result.ExtendedError().map(|h| h.0).unwrap_or_default();
            println!(
                "[Bluetooth Waiting] OpenAsync failed for '{device_name}' (ExtendedError: 0x{ext:08X})."
            );
            Ok(false)
        }
        other => {
            println!(
                "[Bluetooth Waiting] Unexpected open status ({}) for '{device_name}'.",
                other.0
            );
            Ok(false)
        }
    }
}
