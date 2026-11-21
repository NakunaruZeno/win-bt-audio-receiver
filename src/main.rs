mod bluetooth;
mod config;
mod signal;
mod wifi;

use std::process;
use std::sync::Arc;
use std::time::Duration;

use windows::core::Result;

use crate::bluetooth::{
    list_paired_devices, print_devices, resolve_target_device, BtSession, TargetQuery,
};
use crate::signal::{disable_power_throttling, install_ctrl_handler, AppSignal};
use crate::wifi::adb::pair_wireless_adb;
use crate::wifi::{WifiExitReason, WifiSession};

/// Compile-time mutually exclusive transport state machine.
/// Physically guarantees that Wi-Fi LAN (WASAPI/UDP/ADB) and Bluetooth A2DP Sink
/// (WinRT `AudioPlaybackConnection`/`DeviceWatcher`) never coexist in memory.
enum ActiveTransport {
    WifiLan(WifiSession),
    Bluetooth(BtSession),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransportPreference {
    Auto,
    WifiOnly,
    BluetoothOnly,
}

enum CommandAction {
    RunReceiver {
        preference: TransportPreference,
        bt_target: TargetQuery,
        auto_reconnect: bool,
        adb_host: Option<String>,
        buffer_ms: u32,
    },
    ListBtDevices,
    PairAdb {
        host_port: String,
        code: String,
    },
    Help,
}

fn print_help() {
    println!(
        "AetherSora v0.2 - Hybrid Wi-Fi 5GHz LAN & Bluetooth A2DP Sink Audio Receiver\n\
         \n\
         USAGE:\n\
         \taethersora                         Auto mode: try Wi-Fi 5GHz LAN first, fallback to Bluetooth\n\
         \taethersora --mode auto|wifi|bt     Select transport mode explicitly (default: auto)\n\
         \taethersora --adb-host <ip:port>    Connect/save Android Wireless Debugging address\n\
         \taethersora --pair <ip:port> <pin>  One-time Android Wireless Debugging pairing helper\n\
         \taethersora --buffer-ms <10..80>    Set Wi-Fi WASAPI jitter buffer target (default: 20 ms)\n\
         \taethersora --scan                  Scan paired Bluetooth devices and update device_id.cfg\n\
         \taethersora --list                  List paired Bluetooth audio source devices and exit\n\
         \taethersora --device <name_or_id>   Select Bluetooth device matching name/ID substring\n\
         \taethersora --no-reconnect          Disable Bluetooth DeviceWatcher auto-reconnect\n\
         \taethersora --help                  Show this help message\n\
         \n\
         Press Ctrl+C while running to close the active audio transport and exit cleanly."
    );
}

fn parse_args() -> std::result::Result<CommandAction, String> {
    let mut args = std::env::args().skip(1);
    let mut preference = TransportPreference::Auto;
    let mut bt_target = TargetQuery::DefaultOrCached;
    let mut auto_reconnect = true;
    let mut adb_host: Option<String> = None;
    let mut buffer_ms: u32 = 20;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(CommandAction::Help),
            "-l" | "--list" => return Ok(CommandAction::ListBtDevices),
            "-s" | "--scan" => {
                bt_target = TargetQuery::ForceScan;
            }
            "--no-reconnect" => {
                auto_reconnect = false;
            }
            "-m" | "--mode" => {
                let Some(val) = args.next() else {
                    return Err("Missing value for --mode <auto|wifi|bt>".to_owned());
                };
                preference = parse_mode_value(&val)?;
            }
            a if a.starts_with("--mode=") => {
                preference = parse_mode_value(a.trim_start_matches("--mode="))?;
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
                    return Err("Usage: aethersora --pair <ip:port> <6-digit-code>".to_owned());
                };
                let Some(code) = args.next() else {
                    return Err("Usage: aethersora --pair <ip:port> <6-digit-code>".to_owned());
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

    // If user explicitly passed `--scan` or `--device` without `--mode`, default to Bluetooth mode.
    if matches!(bt_target, TargetQuery::ForceScan | TargetQuery::Query(_))
        && preference == TransportPreference::Auto
        && adb_host.is_none()
    {
        preference = TransportPreference::BluetoothOnly;
    }

    Ok(CommandAction::RunReceiver {
        preference,
        bt_target,
        auto_reconnect,
        adb_host,
        buffer_ms,
    })
}

fn parse_mode_value(val: &str) -> std::result::Result<TransportPreference, String> {
    match val.to_ascii_lowercase().as_str() {
        "auto" => Ok(TransportPreference::Auto),
        "wifi" | "lan" => Ok(TransportPreference::WifiOnly),
        "bt" | "bluetooth" => Ok(TransportPreference::BluetoothOnly),
        _ => Err(format!(
            "Invalid mode '{val}'. Expected 'auto', 'wifi', or 'bt'."
        )),
    }
}

fn run() -> Result<i32> {
    let action = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("Error: {msg}\n");
            print_help();
            return Ok(1);
        }
    };

    match action {
        CommandAction::Help => {
            print_help();
            Ok(0)
        }
        CommandAction::ListBtDevices => {
            let devices = list_paired_devices()?;
            if devices.is_empty() {
                println!("No paired Bluetooth audio source devices found.");
            } else {
                print_devices(&devices);
            }
            Ok(0)
        }
        CommandAction::PairAdb { host_port, code } => match pair_wireless_adb(&host_port, &code) {
            Ok(()) => Ok(0),
            Err(e) => {
                eprintln!("Error: {e}");
                Ok(1)
            }
        },
        CommandAction::RunReceiver {
            preference,
            bt_target,
            auto_reconnect,
            adb_host,
            buffer_ms,
        } => run_orchestrator(preference, bt_target, auto_reconnect, adb_host, buffer_ms),
    }
}

fn run_orchestrator(
    preference: TransportPreference,
    bt_target: TargetQuery,
    auto_reconnect: bool,
    adb_host: Option<String>,
    buffer_ms: u32,
) -> Result<i32> {
    disable_power_throttling();

    let signal = Arc::new(AppSignal::new());
    install_ctrl_handler(Arc::clone(&signal))?;

    let mut active_transport: Option<ActiveTransport> = match preference {
        TransportPreference::BluetoothOnly => {
            let Some((device_id, device_name)) = resolve_target_device(&bt_target)? else {
                return Ok(1);
            };
            Some(ActiveTransport::Bluetooth(BtSession::start(
                device_id,
                device_name,
                auto_reconnect,
                Arc::clone(&signal),
            )?))
        }
        TransportPreference::WifiOnly => {
            println!("[Probe] Connecting via Wi-Fi 5GHz LAN (ADB Wireless)...");
            match WifiSession::try_start(adb_host.clone(), buffer_ms) {
                Ok(session) => Some(ActiveTransport::WifiLan(session)),
                Err(err) => {
                    eprintln!("[Wi-Fi LAN] Could not start Wi-Fi session: {err}");
                    return Ok(1);
                }
            }
        }
        TransportPreference::Auto => {
            println!("[Auto Mode] Probing Wi-Fi 5GHz LAN (ADB Wireless) first...");
            match WifiSession::try_start(adb_host.clone(), buffer_ms) {
                Ok(session) => Some(ActiveTransport::WifiLan(session)),
                Err(reason) => {
                    println!(
                        "[Auto Fallback] Wi-Fi LAN unavailable ({reason}). Switching cleanly to Bluetooth A2DP Sink..."
                    );
                    let Some((device_id, device_name)) = resolve_target_device(&bt_target)? else {
                        return Ok(1);
                    };
                    Some(ActiveTransport::Bluetooth(BtSession::start(
                        device_id,
                        device_name,
                        auto_reconnect,
                        Arc::clone(&signal),
                    )?))
                }
            }
        }
    };

    // Execute the active transport session; if Wi-Fi LAN drops in Auto mode
    // (e.g., phone switched to 4G/5G mobile data), tear down Wi-Fi 100% first,
    // wait a 200 ms settle window, and transition into Bluetooth A2DP Sink mode.
    let should_fallback_to_bt = match &mut active_transport {
        Some(ActiveTransport::WifiLan(wifi_session)) => {
            let reason = wifi_session.run_until_exit_or_lost(&signal);
            reason == WifiExitReason::StreamLost && preference == TransportPreference::Auto
        }
        Some(ActiveTransport::Bluetooth(bt_session)) => {
            bt_session.run_until_exit();
            false
        }
        None => false,
    };

    if should_fallback_to_bt && !signal.is_exit_requested() {
        println!(
            "\n[Fallback] Tearing down Wi-Fi LAN session (WASAPI + UDP + ADB) before starting Bluetooth..."
        );
        // Strictly drop WifiSession FIRST so zero sockets/WASAPI/ADB handles remain alive.
        drop(active_transport.take());

        if !signal.sleep_interruptible(Duration::from_millis(200)) {
            if let Some((device_id, device_name)) = resolve_target_device(&bt_target)? {
                let mut bt = BtSession::start(
                    device_id,
                    device_name,
                    auto_reconnect,
                    Arc::clone(&signal),
                )?;
                bt.run_until_exit();
                active_transport = Some(ActiveTransport::Bluetooth(bt));
            }
        }
    }

    println!("\n[Shutdown] Releasing active audio transport...");
    drop(active_transport.take());
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
