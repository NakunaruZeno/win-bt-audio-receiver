#![cfg_attr(not(test), windows_subsystem = "windows")]

mod bluetooth;
mod config;
mod log;
mod signal;
mod tray;
mod wifi;

use std::process;
use std::sync::Arc;
use std::time::Duration;

use windows::core::Result;

use crate::bluetooth::{
    list_paired_devices, print_devices, resolve_target_device, BtResolveResult, BtSession,
    TargetQuery,
};
use crate::config::{load_config, TransportMode};
use crate::signal::{
    attach_parent_console, disable_power_throttling, ensure_console_for_cli, install_ctrl_handler,
    notify_existing_tray_instance, send_stop_signal, spawn_stop_event_listener, AppSignal,
    AutoUpgradeTarget, SingleInstanceGuard,
};
use crate::tray::{ToastKind, TrayController};
use crate::wifi::adb::pair_wireless_adb;
use crate::wifi::auto_watch::AutoUpgradeWatcher;
use crate::wifi::{WifiAdbSession, WifiAppEvent, WifiAppSession, WifiExitReason};

/// Compile-time mutually exclusive transport state machine.
/// Physically guarantees that Wi-Fi ADB, Wi-Fi Companion App, and Bluetooth A2DP Sink
/// NEVER coexist in memory or hold conflicting OS/radio handles simultaneously.
enum ActiveTransport {
    WifiAdb(WifiAdbSession),
    WifiApp(WifiAppSession),
    Bluetooth {
        session: BtSession,
        device_name: String,
    },
}

enum CommandAction {
    RunReceiver {
        initial_mode: TransportMode,
        bt_target: TargetQuery,
        auto_reconnect: bool,
        adb_host: Option<String>,
        buffer_ms: u32,
        use_tray: bool,
    },
    ListBtDevices,
    PairAdb {
        host_port: String,
        code: String,
    },
    StopRunning,
    Help,
}

fn print_help() {
    log::info(
        "AetherSora v0.3 - Universal Windows System Tray Audio Receiver\n\
         \n\
         USAGE:\n\
         \tAetherSora                                    Run silently in Windows System Tray (default)\n\
         \tAetherSora --tray                             Explicitly run in Windows System Tray\n\
         \tAetherSora --console                          Run attached to console window\n\
         \tAetherSora --stop                             Cleanly stop a running tray/background instance\n\
         \tAetherSora --mode auto|wifi-app|wifi-adb|bt   Select transport mode explicitly\n\
         \tAetherSora --adb-host <ip:port>               Connect/save Android Wireless Debugging address\n\
         \tAetherSora --pair <ip:port> <pin>             One-time Android Wireless Debugging pairing helper\n\
         \tAetherSora --buffer-ms <10..80>               Set Wi-Fi WASAPI jitter buffer target (default: 20 ms)\n\
         \tAetherSora --scan                             Scan paired Bluetooth devices interactively in console\n\
         \tAetherSora --list                             List paired Bluetooth audio source devices and exit\n\
         \tAetherSora --device <name_or_id>              Select Bluetooth device matching name/ID substring\n\
         \tAetherSora --no-reconnect                     Disable Bluetooth DeviceWatcher auto-reconnect\n\
         \tAetherSora --help                             Show this help message\n\
         \n\
         Right-click the System Tray icon to switch modes, pick Bluetooth devices, view Live Debug Log, or Exit.",
    );
}

fn parse_args() -> std::result::Result<CommandAction, String> {
    let mut args = std::env::args().skip(1);
    let mut explicit_mode: Option<TransportMode> = None;
    let mut bt_target = TargetQuery::DefaultOrCached;
    let mut auto_reconnect = true;
    let mut adb_host: Option<String> = None;
    let mut buffer_ms: u32 = 20;
    let mut explicit_tray: Option<bool> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(CommandAction::Help),
            "-l" | "--list" => return Ok(CommandAction::ListBtDevices),
            "--stop" => return Ok(CommandAction::StopRunning),
            "--tray" | "--background" => explicit_tray = Some(true),
            "--console" => explicit_tray = Some(false),
            "-s" | "--scan" => {
                bt_target = TargetQuery::ForceScan;
            }
            "--no-reconnect" => {
                auto_reconnect = false;
            }
            "-m" | "--mode" => {
                let Some(val) = args.next() else {
                    return Err("Missing value for --mode <auto|wifi-app|wifi-adb|bt>".to_owned());
                };
                explicit_mode = Some(parse_mode_or_err(&val)?);
            }
            a if a.starts_with("--mode=") => {
                explicit_mode = Some(parse_mode_or_err(a.trim_start_matches("--mode="))?);
            }
            "--adb-host" => {
                let Some(h) = args.next() else {
                    return Err("Missing value for --adb-host <ip:port>".to_owned());
                };
                adb_host = Some(h);
            }
            a if a.starts_with("--adb-host=") => {
                adb_host = Some(a.trim_start_matches("--adb-host=").to_owned());
            }
            "--buffer-ms" => {
                let Some(ms_str) = args.next() else {
                    return Err("Missing value for --buffer-ms <10..80>".to_owned());
                };
                buffer_ms = ms_str
                    .parse::<u32>()
                    .map_err(|_| format!("Invalid --buffer-ms value '{ms_str}'"))?
                    .clamp(10, 80);
            }
            "--pair" => {
                let Some(host_port) = args.next() else {
                    return Err("Usage: AetherSora --pair <ip:port> <6-digit-code>".to_owned());
                };
                let Some(code) = args.next() else {
                    return Err("Usage: AetherSora --pair <ip:port> <6-digit-code>".to_owned());
                };
                return Ok(CommandAction::PairAdb { host_port, code });
            }
            "-d" | "--device" => match args.next() {
                Some(q) if !q.trim().is_empty() => bt_target = TargetQuery::Query(q),
                _ => return Err("Missing value for --device <name_or_id>".to_owned()),
            },
            a if a.starts_with("--device=") => {
                let q = a.trim_start_matches("--device=");
                if q.trim().is_empty() {
                    return Err("Missing value for --device=<name_or_id>".to_owned());
                }
                bt_target = TargetQuery::Query(q.to_owned());
            }
            other => return Err(format!("Unknown argument: {other}")),
        }
    }

    let saved_mode = load_config().default_mode;
    let mut initial_mode = explicit_mode.or(saved_mode).unwrap_or(TransportMode::Auto);

    if matches!(bt_target, TargetQuery::ForceScan | TargetQuery::Query(_))
        && explicit_mode.is_none()
        && adb_host.is_none()
    {
        initial_mode = TransportMode::Bluetooth;
    }

    let use_tray = explicit_tray.unwrap_or(!matches!(bt_target, TargetQuery::ForceScan));

    Ok(CommandAction::RunReceiver {
        initial_mode,
        bt_target,
        auto_reconnect,
        adb_host,
        buffer_ms,
        use_tray,
    })
}

fn parse_mode_or_err(val: &str) -> std::result::Result<TransportMode, String> {
    TransportMode::parse(val).ok_or_else(|| {
        format!("Invalid mode '{val}'. Expected 'auto', 'wifi-app', 'wifi-adb', or 'bt'.")
    })
}

fn run() -> Result<i32> {
    let action = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            let _ = attach_parent_console();
            log::error(format!("Error: {msg}\n"));
            print_help();
            return Ok(1);
        }
    };

    match action {
        CommandAction::Help => {
            let _ = attach_parent_console();
            print_help();
            Ok(0)
        }
        CommandAction::ListBtDevices => {
            let _ = attach_parent_console();
            let devices = list_paired_devices()?;
            if devices.is_empty() {
                log::info("No paired Bluetooth audio source devices found.");
            } else {
                print_devices(&devices);
            }
            Ok(0)
        }
        CommandAction::PairAdb { host_port, code } => {
            let _ = attach_parent_console();
            match pair_wireless_adb(&host_port, &code) {
                Ok(()) => Ok(0),
                Err(e) => {
                    log::error(format!("Error: {e}"));
                    Ok(1)
                }
            }
        }
        CommandAction::StopRunning => {
            let _ = attach_parent_console();
            match send_stop_signal() {
                Ok(()) => {
                    log::info("[Stop] Sent clean shutdown signal to running AetherSora instance.");
                    Ok(0)
                }
                Err(e) => {
                    log::error(e);
                    Ok(1)
                }
            }
        }
        CommandAction::RunReceiver {
            initial_mode,
            bt_target,
            auto_reconnect,
            adb_host,
            buffer_ms,
            use_tray,
        } => {
            if !use_tray {
                ensure_console_for_cli();
            }
            run_orchestrator(
                initial_mode,
                bt_target,
                auto_reconnect,
                adb_host,
                buffer_ms,
                use_tray,
            )
        }
    }
}

fn run_orchestrator(
    initial_mode: TransportMode,
    mut bt_target: TargetQuery,
    auto_reconnect: bool,
    adb_host: Option<String>,
    buffer_ms: u32,
    use_tray: bool,
) -> Result<i32> {
    let _instance_guard = match SingleInstanceGuard::acquire() {
        Ok(g) => g,
        Err(msg) => {
            if use_tray {
                let _ = notify_existing_tray_instance();
            }
            let _ = attach_parent_console();
            log::error(msg);
            return Ok(1);
        }
    };

    disable_power_throttling();

    let signal = Arc::new(AppSignal::new());
    let _ = install_ctrl_handler(Arc::clone(&signal), !use_tray);
    spawn_stop_event_listener(Arc::clone(&signal));

    let tray = if use_tray {
        Some(TrayController::start(Arc::clone(&signal), initial_mode))
    } else {
        None
    };

    log::info(format!(
        "[Startup] AetherSora v0.3 started (mode: {}, tray: {use_tray}).",
        initial_mode.as_str()
    ));

    let mut current_mode = initial_mode;
    let mut announce_state = true;
    let mut pending_auto_upgrade: Option<AutoUpgradeTarget> = None;
    let mut skip_next_adb_probe = false;
    let mut fallback_toast_reason: Option<String> = None;
    let interactive_cli = !use_tray;

    while !signal.is_exit_requested() {
        let _ = signal.take_mode_switch();
        let _ = signal.take_auto_upgrade();

        let mut auto_watcher: Option<AutoUpgradeWatcher> = None;

        let (mut active_transport, mode_used_for_fallback) = match current_mode {
            TransportMode::Bluetooth => {
                let target_res = resolve_target_device(&bt_target, interactive_cli)?;
                bt_target = TargetQuery::DefaultOrCached;
                match target_res {
                    BtResolveResult::Selected {
                        id: device_id,
                        name: device_name,
                    } => {
                        let session = BtSession::start(
                            device_id,
                            device_name.clone(),
                            auto_reconnect,
                            Arc::clone(&signal),
                        )?;
                        let connected = session.is_connected();
                        let status = if connected {
                            format!("Bluetooth: Connected ({device_name})")
                        } else {
                            format!("Bluetooth: Waiting ({device_name})")
                        };
                        update_tray(&tray, current_mode, status);
                        if announce_state {
                            if connected {
                                toast_tray(
                                    &tray,
                                    "Bluetooth A2DP Connected",
                                    format!("Receiving audio from {device_name}."),
                                    ToastKind::Info,
                                );
                            } else {
                                toast_tray(
                                    &tray,
                                    "AetherSora - Bluetooth Ready",
                                    format!("Running in System Tray. Waiting for {device_name}..."),
                                    ToastKind::Info,
                                );
                            }
                        }
                        (
                            Some(ActiveTransport::Bluetooth {
                                session,
                                device_name,
                            }),
                            false,
                        )
                    }
                    BtResolveResult::MultipleNeedChoice(devices) => {
                        if let Some(t) = &tray {
                            t.set_bt_devices(devices);
                        }
                        update_tray(&tray, current_mode, "Bluetooth: Select device in Tray");
                        if announce_state {
                            toast_tray(
                                &tray,
                                "Select Bluetooth Device",
                                "Right-click the AetherSora tray icon -> Bluetooth Device to pick your phone.",
                                ToastKind::Warning,
                            );
                        }
                        (None, false)
                    }
                    BtResolveResult::NonePaired => {
                        update_tray(&tray, current_mode, "Bluetooth: No paired device");
                        if announce_state {
                            toast_tray(
                                &tray,
                                "No Bluetooth Device Paired",
                                "Pair your phone in Windows Settings -> Bluetooth, then refresh in Tray.",
                                ToastKind::Warning,
                            );
                        }
                        (None, false)
                    }
                }
            }
            TransportMode::WifiApp => match WifiAppSession::start(buffer_ms) {
                Ok(session) => {
                    update_tray(
                        &tray,
                        current_mode,
                        "Wi-Fi Companion: Waiting for phone...",
                    );
                    if announce_state {
                        toast_tray(
                            &tray,
                            "AetherSora - Wi-Fi 5GHz Companion",
                            "Running in System Tray. Open AetherSora on your phone to connect.",
                            ToastKind::Info,
                        );
                    }
                    (Some(ActiveTransport::WifiApp(session)), false)
                }
                Err(err) => {
                    log::error(format!("[Wi-Fi Companion] Startup failed: {err}"));
                    update_tray(&tray, current_mode, "Wi-Fi Companion: Bind error");
                    if announce_state {
                        toast_tray(
                            &tray,
                            "Wi-Fi Companion Error",
                            format!("Could not bind UDP audio port: {err}"),
                            ToastKind::Warning,
                        );
                    }
                    (None, false)
                }
            },
            TransportMode::WifiAdb => {
                log::info("[Probe] Connecting via Wi-Fi 5GHz LAN (ADB Wireless)...");
                match WifiAdbSession::try_start(adb_host.clone(), buffer_ms) {
                    Ok(session) => {
                        update_tray(&tray, current_mode, "Wi-Fi 5GHz ADB: 48 kHz Lossless");
                        toast_tray(
                            &tray,
                            "Wi-Fi 5GHz Lossless Connected",
                            "Streaming 48 kHz 16-bit Stereo PCM via ADB Wireless.",
                            ToastKind::Info,
                        );
                        (Some(ActiveTransport::WifiAdb(session)), false)
                    }
                    Err(err) => {
                        log::error(format!("[Wi-Fi ADB] Could not start Wi-Fi session: {err}"));
                        update_tray(&tray, current_mode, "Wi-Fi ADB: Waiting / Offline");
                        if announce_state {
                            toast_tray(
                                &tray,
                                "Wi-Fi ADB Offline",
                                "Phone not reachable on LAN. Enable Wireless Debugging or switch mode in Tray.",
                                ToastKind::Warning,
                            );
                        }
                        (None, false)
                    }
                }
            }
            TransportMode::Auto => {
                let upgrade = pending_auto_upgrade.take();
                let do_skip_adb = skip_next_adb_probe;
                skip_next_adb_probe = false;
                let fb_reason = fallback_toast_reason.take();

                match upgrade {
                    Some(AutoUpgradeTarget::WifiApp(peer)) => {
                        match WifiAppSession::start_with_peer(buffer_ms, Some(peer)) {
                            Ok(session) => {
                                update_tray(
                                    &tray,
                                    current_mode,
                                    format!("Auto -> Wi-Fi Companion ({})", peer.ip()),
                                );
                                toast_tray(
                                    &tray,
                                    "Auto -> Wi-Fi 5GHz Lossless",
                                    format!(
                                        "Upgraded to Companion App ({}) - 48 kHz PCM.",
                                        peer.ip()
                                    ),
                                    ToastKind::Info,
                                );
                                (Some(ActiveTransport::WifiApp(session)), false)
                            }
                            Err(err) => {
                                log::error(format!(
                                    "[Auto Upgrade] Failed to start WifiAppSession: {err}"
                                ));
                                auto_watcher = Some(AutoUpgradeWatcher::start(
                                    Arc::clone(&signal),
                                    adb_host.clone(),
                                ));
                                (None, false)
                            }
                        }
                    }
                    other => {
                        let adb_attempt = if do_skip_adb {
                            Err("stream dropped".to_owned())
                        } else {
                            log::info("[Auto Mode] Probing Wi-Fi 5GHz LAN (ADB Wireless) first...");
                            WifiAdbSession::try_start(adb_host.clone(), buffer_ms)
                        };

                        match adb_attempt {
                            Ok(session) => {
                                update_tray(&tray, current_mode, "Auto -> Wi-Fi 5GHz (48 kHz)");
                                let toast_body = if matches!(other, Some(AutoUpgradeTarget::WifiAdb))
                                {
                                    "Wireless Debugging detected. Upgraded to 48 kHz Lossless PCM."
                                } else {
                                    "Auto mode streaming 48 kHz 16-bit PCM via ADB Wireless."
                                };
                                toast_tray(
                                    &tray,
                                    "Wi-Fi 5GHz Lossless Connected",
                                    toast_body,
                                    ToastKind::Info,
                                );
                                (Some(ActiveTransport::WifiAdb(session)), true)
                            }
                            Err(reason) => {
                                if !do_skip_adb {
                                    log::info(format!(
                                        "[Auto Fallback] Wi-Fi ADB unavailable ({reason}). Entering Bluetooth + passive Wi-Fi watch..."
                                    ));
                                }
                                let target_res =
                                    resolve_target_device(&bt_target, interactive_cli)?;
                                bt_target = TargetQuery::DefaultOrCached;

                                auto_watcher = Some(AutoUpgradeWatcher::start(
                                    Arc::clone(&signal),
                                    adb_host.clone(),
                                ));

                                match target_res {
                                    BtResolveResult::Selected {
                                        id: device_id,
                                        name: device_name,
                                    } => {
                                        let session = BtSession::start(
                                            device_id,
                                            device_name.clone(),
                                            auto_reconnect,
                                            Arc::clone(&signal),
                                        )?;
                                        let connected = session.is_connected();
                                        let status = if connected {
                                            format!("Auto -> Bluetooth ({device_name})")
                                        } else {
                                            format!("Auto -> Bluetooth Waiting ({device_name})")
                                        };
                                        update_tray(&tray, current_mode, status);
                                        if let Some(reason_msg) = fb_reason {
                                            toast_tray(
                                                &tray,
                                                "Switched to Bluetooth Fallback",
                                                format!("{reason_msg} Ready on {device_name}."),
                                                ToastKind::Warning,
                                            );
                                        } else if announce_state {
                                            if connected {
                                                toast_tray(
                                                    &tray,
                                                    "AetherSora - Auto (Bluetooth)",
                                                    format!(
                                                        "Connected to {device_name}. Auto-upgrades if you connect Companion App or ADB."
                                                    ),
                                                    ToastKind::Info,
                                                );
                                            } else {
                                                toast_tray(
                                                    &tray,
                                                    "AetherSora Running in Tray (Auto)",
                                                    format!(
                                                        "Ready for Companion App, ADB Wireless, or Bluetooth ({device_name})."
                                                    ),
                                                    ToastKind::Info,
                                                );
                                            }
                                        }
                                        (
                                            Some(ActiveTransport::Bluetooth {
                                                session,
                                                device_name,
                                            }),
                                            false,
                                        )
                                    }
                                    BtResolveResult::MultipleNeedChoice(devices) => {
                                        if let Some(t) = &tray {
                                            t.set_bt_devices(devices);
                                        }
                                        update_tray(
                                            &tray,
                                            current_mode,
                                            "Auto: Waiting (Wi-Fi / Pick BT in Tray)",
                                        );
                                        if announce_state {
                                            toast_tray(
                                                &tray,
                                                "AetherSora Running in Tray (Auto)",
                                                "Ready for Companion App / ADB Wireless, or pick Bluetooth device in Tray.",
                                                ToastKind::Info,
                                            );
                                        }
                                        (None, false)
                                    }
                                    BtResolveResult::NonePaired => {
                                        update_tray(
                                            &tray,
                                            current_mode,
                                            "Auto: Ready for Wi-Fi (No BT paired)",
                                        );
                                        if announce_state {
                                            toast_tray(
                                                &tray,
                                                "AetherSora Running in Tray (Auto)",
                                                "Ready for Companion App or ADB Wireless (no Bluetooth device paired).",
                                                ToastKind::Info,
                                            );
                                        }
                                        (None, false)
                                    }
                                }
                            }
                        }
                    }
                }
            }
        };

        announce_state = false;
        let mut auto_fallback_to_bt = false;

        match &mut active_transport {
            Some(ActiveTransport::WifiAdb(wifi_adb)) => {
                let reason = wifi_adb.run_until_exit_or_lost(&signal);
                if reason == WifiExitReason::StreamLost && mode_used_for_fallback {
                    auto_fallback_to_bt = true;
                }
            }
            Some(ActiveTransport::WifiApp(wifi_app)) => {
                let exit_on_disconnect = current_mode == TransportMode::Auto;
                wifi_app.run_until_exit(&signal, exit_on_disconnect, |evt| match evt {
                    WifiAppEvent::Connected(ip) => {
                        let status = if current_mode == TransportMode::Auto {
                            format!("Auto -> Wi-Fi Companion ({ip})")
                        } else {
                            format!("Wi-Fi Lossless: Connected ({ip})")
                        };
                        update_tray(&tray, current_mode, status);
                        toast_tray(
                            &tray,
                            "Wi-Fi 5GHz Lossless Connected",
                            format!("Streaming 48 kHz 16-bit PCM from {ip}."),
                            ToastKind::Info,
                        );
                    }
                    WifiAppEvent::Disconnected => {
                        let (status, body) = if current_mode == TransportMode::Auto {
                            (
                                "Auto: Returning to standby / Bluetooth...",
                                "Companion App disconnected. Returning to Auto standby.",
                            )
                        } else {
                            (
                                "Wi-Fi Companion: Waiting for phone...",
                                "Returned to 0% CPU standby in System Tray.",
                            )
                        };
                        update_tray(&tray, current_mode, status);
                        toast_tray(&tray, "Wi-Fi Companion Disconnected", body, ToastKind::Info);
                    }
                });
            }
            Some(ActiveTransport::Bluetooth {
                session,
                device_name,
            }) => {
                let dev_name = device_name.clone();
                session.run_until_exit(|connected| {
                    if connected {
                        let status = if current_mode == TransportMode::Auto {
                            format!("Auto -> Bluetooth ({dev_name})")
                        } else {
                            format!("Bluetooth: Connected ({dev_name})")
                        };
                        update_tray(&tray, current_mode, status);
                        toast_tray(
                            &tray,
                            "Bluetooth A2DP Connected",
                            format!("Receiving audio from {dev_name}."),
                            ToastKind::Info,
                        );
                    } else {
                        let status = if current_mode == TransportMode::Auto {
                            format!("Auto -> Bluetooth Waiting ({dev_name})")
                        } else {
                            format!("Bluetooth: Waiting ({dev_name})")
                        };
                        update_tray(&tray, current_mode, status);
                        toast_tray(
                            &tray,
                            "Bluetooth Disconnected",
                            format!("Waiting for {dev_name} to reconnect..."),
                            ToastKind::Info,
                        );
                    }
                });
            }
            None => {
                if tray.is_none() {
                    return Ok(1);
                }
                let _ = signal.wait_for_event();
            }
        }

        let was_auto_wifi_app =
            current_mode == TransportMode::Auto && matches!(active_transport, Some(ActiveTransport::WifiApp(_)));
        drop(active_transport.take());
        drop(auto_watcher.take());

        if signal.is_exit_requested() {
            break;
        }

        if let Some(next_mode) = signal.take_mode_switch() {
            log::info(format!(
                "[Switch] Dropped previous transport. Switching cleanly to '{}'...",
                next_mode.as_str()
            ));
            current_mode = next_mode;
            pending_auto_upgrade = None;
            skip_next_adb_probe = false;
            announce_state = true;
            if signal.sleep_interruptible(Duration::from_millis(200))
                && signal.is_exit_requested()
            {
                break;
            }
            continue;
        }

        if let Some(target) = signal.take_auto_upgrade() {
            pending_auto_upgrade = Some(target);
            skip_next_adb_probe = false;
            announce_state = true;
            continue;
        }

        if auto_fallback_to_bt {
            log::info(
                "[Fallback] Wi-Fi ADB session dropped completely. Returning to Bluetooth + passive Wi-Fi watch...",
            );
            skip_next_adb_probe = true;
            fallback_toast_reason = Some("Wi-Fi ADB dropped.".to_owned());
            if signal.sleep_interruptible(Duration::from_millis(200)) {
                continue;
            }
            continue;
        }

        if was_auto_wifi_app {
            skip_next_adb_probe = true;
            continue;
        }

        if tray.is_none() {
            break;
        }
        if signal.sleep_interruptible(Duration::from_secs(3)) {
            continue;
        }
    }

    log::info("[Shutdown] Disconnected cleanly. Bye!");
    drop(tray);
    Ok(0)
}

fn update_tray(tray: &Option<TrayController>, mode: TransportMode, status: impl Into<String>) {
    if let Some(t) = tray {
        t.update_status(mode, status);
    }
}

fn toast_tray(
    tray: &Option<TrayController>,
    title: impl Into<String>,
    body: impl Into<String>,
    kind: ToastKind,
) {
    if let Some(t) = tray {
        t.show_toast(title, body, kind);
    }
}

fn main() {
    match run() {
        Ok(code) => {
            if code != 0 {
                process::exit(code);
            }
        }
        Err(err) => {
            let _ = attach_parent_console();
            log::error(format!("Fatal Windows API error: {err}"));
            process::exit(1);
        }
    }
}
