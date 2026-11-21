use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::config::{load_config, save_adb_host};
use crate::log;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Creates an `adb` Command with `CREATE_NO_WINDOW` so background probes and
/// streaming under `#![windows_subsystem = "windows"]` never flash a console window.
fn adb_command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Pre-compiled 5.3 KB Android DEX agent embedded directly into the executable.
const AGENT_DEX_BYTES: &[u8] = include_bytes!("../../android/agent/aether-agent.dex");
const REMOTE_DEX_PATH: &str = "/data/local/tmp/aether-agent.dex";

pub struct AdbAgentProcess {
    child: Child,
}

impl Drop for AdbAgentProcess {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn find_adb_binary() -> Option<PathBuf> {
    if adb_command("adb")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
    {
        return Some(PathBuf::from("adb"));
    }

    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        let sdk_adb = PathBuf::from(local_app_data)
            .join("Android")
            .join("Sdk")
            .join("platform-tools")
            .join("adb.exe");
        if sdk_adb.is_file() {
            return Some(sdk_adb);
        }
    }

    None
}

pub fn pair_wireless_adb(host_port: &str, code: &str) -> std::result::Result<(), String> {
    let adb = find_adb_binary().ok_or_else(|| {
        "adb.exe not found in PATH or %LOCALAPPDATA%\\Android\\Sdk\\platform-tools".to_owned()
    })?;

    log::info(format!("[ADB Pair] Pairing with {host_port}..."));
    let output = adb_command(&adb)
        .args(["pair", host_port, code])
        .output()
        .map_err(|e| format!("Failed to execute adb pair: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() && stdout.to_ascii_lowercase().contains("successfully paired") {
        log::info(format!("[ADB Pair] {stdout}"));
        if let Some((ip, _)) = host_port.split_once(':') {
            let _ = save_adb_host(ip);
        }
        Ok(())
    } else {
        Err(format!("adb pair failed: {stdout} {stderr}").trim().to_owned())
    }
}

/// Non-invasive readiness check called ONLY after passive mDNS detects
/// `_adb-tls-connect._tcp` on LAN. Verifies an authorized device is reachable
/// BEFORE `Auto` mode drops an active Bluetooth session.
pub fn probe_wireless_adb_available(explicit_adb_host: Option<&str>) -> bool {
    let Some(adb) = find_adb_binary() else {
        return false;
    };

    if find_online_adb_device(&adb).is_some() {
        return true;
    }

    let saved_cfg = load_config();
    let candidate_host = explicit_adb_host.or(saved_cfg.adb_host.as_deref());

    if let Some(mdns_endpoint) = discover_mdns_wireless_endpoint(&adb, candidate_host) {
        if tcp_quick_reachable(&mdns_endpoint, Duration::from_millis(450)) {
            let _ = adb_command(&adb)
                .args(["connect", &mdns_endpoint])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .output();
            if find_online_adb_device(&adb).is_some() {
                let _ = save_adb_host(&mdns_endpoint);
                return true;
            }
        }
    }

    false
}

/// Fast probe (<= 1.2s) for an available ADB device (Wireless Debugging over 5 GHz LAN or USB).
/// If found, pushes the 5.3 KB `aether-agent.dex` and starts `com.aethersora.Agent` streaming
/// 48 kHz stereo PCM via UDP to `pc_lan_ip:udp_port`.
pub fn try_launch_android_agent(
    explicit_adb_host: Option<&str>,
    udp_port: u16,
) -> std::result::Result<AdbAgentProcess, String> {
    let adb = find_adb_binary().ok_or_else(|| "adb.exe not available".to_owned())?;

    let saved_cfg = load_config();
    let candidate_host = explicit_adb_host.or(saved_cfg.adb_host.as_deref());

    if let Some(host) = candidate_host {
        if host.contains(':') && tcp_quick_reachable(host, Duration::from_millis(450)) {
            let _ = adb_command(&adb)
                .args(["connect", host])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .output();
            if explicit_adb_host.is_some() {
                let _ = save_adb_host(host);
            }
        }
    }

    let mut serial = find_online_adb_device(&adb);

    if serial.is_none() {
        if let Some(mdns_endpoint) = discover_mdns_wireless_endpoint(&adb, candidate_host) {
            if tcp_quick_reachable(&mdns_endpoint, Duration::from_millis(450)) {
                let _ = adb_command(&adb)
                    .args(["connect", &mdns_endpoint])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .output();
                let _ = save_adb_host(&mdns_endpoint);
                serial = find_online_adb_device(&adb);
            }
        }
    }

    let device_serial =
        serial.ok_or_else(|| "No active Android Wireless/USB ADB device reachable on LAN".to_owned())?;

    let phone_ip_hint = extract_ipv4_prefix(&device_serial)
        .or_else(|| candidate_host.and_then(extract_ipv4_prefix));
    let pc_lan_ip = resolve_pc_lan_ip(phone_ip_hint.as_deref())
        .ok_or_else(|| "Could not determine PC LAN IPv4 address (is PC connected to Wi-Fi/LAN?)".to_owned())?;

    let local_dex = env::temp_dir().join("aether-agent.dex");
    fs::write(&local_dex, AGENT_DEX_BYTES)
        .map_err(|e| format!("Failed to write temp aether-agent.dex: {e}"))?;

    let push_status = adb_command(&adb)
        .args([
            "-s",
            &device_serial,
            "push",
            local_dex.to_str().unwrap_or("aether-agent.dex"),
            REMOTE_DEX_PATH,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("adb push failed: {e}"))?;

    if !push_status.success() {
        return Err("adb push aether-agent.dex failed (device offline or unauthorized)".to_owned());
    }

    let remote_cmd = format!(
        "CLASSPATH={REMOTE_DEX_PATH} app_process / com.aethersora.Agent {pc_lan_ip} {udp_port}"
    );
    let mut child = adb_command(&adb)
        .args(["-s", &device_serial, "shell", &remote_cmd])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn adb shell app_process: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Missing stdout pipe from adb shell".to_owned())?;

    let (ready_tx, ready_rx) = mpsc::sync_channel::<std::result::Result<(), String>>(1);
    thread::Builder::new()
        .name("aether-adb-stdout".to_owned())
        .spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                let trimmed = line.trim();
                if trimmed == "READY" {
                    let _ = ready_tx.send(Ok(()));
                } else if trimmed.starts_with("ERR:") {
                    let _ = ready_tx.send(Err(format!("Android agent reported {trimmed}")));
                    break;
                }
            }
        })
        .map_err(|e| format!("Failed to spawn adb stdout reader: {e}"))?;

    match ready_rx.recv_timeout(Duration::from_millis(1800)) {
        Ok(Ok(())) => {
            log::info(format!(
                "[Wi-Fi LAN] Android agent READY on '{device_serial}' -> streaming 48 kHz 16-bit Stereo PCM to {pc_lan_ip}:{udp_port}"
            ));
            Ok(AdbAgentProcess { child })
        }
        Ok(Err(agent_err)) => {
            drop(child.stdin.take());
            let _ = child.kill();
            let _ = child.wait();
            Err(agent_err)
        }
        Err(_) => {
            drop(child.stdin.take());
            let _ = child.kill();
            let _ = child.wait();
            Err("Timed out waiting for Android agent READY handshake".to_owned())
        }
    }
}

fn tcp_quick_reachable(host_port: &str, timeout: Duration) -> bool {
    let Ok(addr) = host_port.parse::<SocketAddr>() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, timeout).is_ok()
}

fn find_online_adb_device(adb: &PathBuf) -> Option<String> {
    let output = adb_command(adb).arg("devices").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);

    let mut usb_fallback = None;
    for line in text.lines().skip(1) {
        let mut parts = line.split_whitespace();
        let Some(serial) = parts.next() else { continue };
        let Some(state) = parts.next() else { continue };
        if state != "device" {
            continue;
        }
        if serial.contains(':') || serial.contains("_adb-tls-connect") {
            return Some(serial.to_owned());
        }
        if usb_fallback.is_none() {
            usb_fallback = Some(serial.to_owned());
        }
    }
    usb_fallback
}

fn discover_mdns_wireless_endpoint(adb: &PathBuf, preferred_ip_hint: Option<&str>) -> Option<String> {
    let output = adb_command(adb).args(["mdns", "services"]).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let ip_prefix = preferred_ip_hint.and_then(extract_ipv4_prefix);

    let mut first_match = None;
    for line in text.lines() {
        if !line.contains("_adb-tls-connect._tcp") {
            continue;
        }
        if let Some(host_port) = line.split_whitespace().last() {
            if host_port.contains(':') {
                if let Some(ref pref) = ip_prefix {
                    if host_port.starts_with(pref.as_str()) {
                        return Some(host_port.to_owned());
                    }
                }
                if first_match.is_none() {
                    first_match = Some(host_port.to_owned());
                }
            }
        }
    }
    first_match
}

fn extract_ipv4_prefix(s: &str) -> Option<String> {
    let ip_part = s.split(':').next()?;
    let addr: Ipv4Addr = ip_part.parse().ok()?;
    Some(addr.to_string())
}

pub(crate) fn resolve_pc_lan_ip(phone_ip_hint: Option<&str>) -> Option<Ipv4Addr> {
    let target_ip: Ipv4Addr = phone_ip_hint
        .and_then(|s| s.parse().ok())
        .unwrap_or(Ipv4Addr::new(192, 168, 1, 1));
    let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    probe.connect((IpAddr::V4(target_ip), 5555)).ok()?;
    match probe.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}
