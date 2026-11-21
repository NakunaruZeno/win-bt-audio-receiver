use std::net::{Ipv4Addr, UdpSocket};
use std::os::windows::io::FromRawSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Networking::WinSock::{
    bind, closesocket, setsockopt, socket, AF_INET, IN_ADDR, IN_ADDR_0, IPPROTO_UDP, SOCKADDR,
    SOCKADDR_IN, SOCKET_ERROR, SOCK_DGRAM, SOL_SOCKET, SO_REUSEADDR,
};

use crate::log;
use crate::signal::{AppSignal, AutoUpgradeTarget};
use crate::wifi::adb::{probe_wireless_adb_available, resolve_pc_lan_ip};
use crate::wifi::discovery::DiscoveryResponder;
use crate::wifi::protocol::{PacketHeader, ACK_PACKET, DEFAULT_UDP_PORT};

const MDNS_MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_PORT: u16 = 5353;
const ADB_MDNS_NEEDLE: &[u8] = b"_adb-tls-connect";
const ADB_PROBE_COOLDOWN: Duration = Duration::from_secs(10);

/// Zero-overhead passive watcher active ONLY during `Auto` mode when currently
/// in Bluetooth fallback or idle standby.
///
/// - Listens on UDP 19481 (LAN discovery) + UDP 19480 (Companion `HELLO` sentinel)
///   with NO WASAPI and NO ring buffer (~0% CPU, ~0.1 MB RAM).
/// - Listens passively on mDNS multicast `224.0.0.251:5353` (`SO_REUSEADDR`) for
///   Android `_adb-tls-connect._tcp` announcements (zero `adb.exe` polling).
pub struct AutoUpgradeWatcher {
    stop_flag: Arc<AtomicBool>,
    _discovery: Option<DiscoveryResponder>,
    companion_thread: Option<JoinHandle<()>>,
    mdns_thread: Option<JoinHandle<()>>,
}

impl AutoUpgradeWatcher {
    pub fn start(signal: Arc<AppSignal>, adb_host_hint: Option<String>) -> Self {
        Self::start_with_ports(signal, adb_host_hint, DEFAULT_UDP_PORT, true)
    }

    fn start_with_ports(
        signal: Arc<AppSignal>,
        adb_host_hint: Option<String>,
        audio_port: u16,
        enable_discovery_and_mdns: bool,
    ) -> Self {
        let stop_flag = Arc::new(AtomicBool::new(false));

        let discovery = if enable_discovery_and_mdns {
            match DiscoveryResponder::start(audio_port) {
                Ok(d) => Some(d),
                Err(err) => {
                    log::error(format!(
                        "[Auto Watch] Could not bind LAN discovery UDP 19481: {err}"
                    ));
                    None
                }
            }
        } else {
            None
        };

        let companion_thread = spawn_companion_sentinel(
            audio_port,
            Arc::clone(&stop_flag),
            Arc::clone(&signal),
        );

        let mdns_thread = if enable_discovery_and_mdns {
            spawn_adb_mdns_watcher(adb_host_hint, Arc::clone(&stop_flag), signal)
        } else {
            None
        };

        Self {
            stop_flag,
            _discovery: discovery,
            companion_thread,
            mdns_thread,
        }
    }
}

impl Drop for AutoUpgradeWatcher {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        drop(self._discovery.take());
        if let Some(h) = self.companion_thread.take() {
            let _ = h.join();
        }
        if let Some(h) = self.mdns_thread.take() {
            let _ = h.join();
        }
    }
}

fn spawn_companion_sentinel(
    audio_port: u16,
    stop_flag: Arc<AtomicBool>,
    signal: Arc<AppSignal>,
) -> Option<JoinHandle<()>> {
    let socket = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, audio_port)) {
        Ok(s) => s,
        Err(err) => {
            log::error(format!(
                "[Auto Watch] Could not bind Companion sentinel UDP {audio_port}: {err}"
            ));
            return None;
        }
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(150)));

    thread::Builder::new()
        .name("aether-auto-app".to_owned())
        .spawn(move || {
            let mut buf = [0u8; 64];
            while !stop_flag.load(Ordering::Acquire) {
                let Ok((n, src)) = socket.recv_from(&mut buf) else {
                    continue;
                };
                let Some(hdr) = PacketHeader::decode(&buf[..n]) else {
                    continue;
                };
                if hdr.is_bye() {
                    continue;
                }
                if hdr.is_hello() {
                    let _ = socket.send_to(&ACK_PACKET, src);
                }
                stop_flag.store(true, Ordering::Release);
                drop(socket);
                signal.request_auto_upgrade(AutoUpgradeTarget::WifiApp(src));
                log::info(format!(
                    "[Auto Upgrade] Companion App handshake from {src} -> upgrading to Wi-Fi 5GHz Lossless..."
                ));
                break;
            }
        })
        .ok()
}

fn spawn_adb_mdns_watcher(
    adb_host_hint: Option<String>,
    stop_flag: Arc<AtomicBool>,
    signal: Arc<AppSignal>,
) -> Option<JoinHandle<()>> {
    let socket = match bind_reusable_mdns_socket() {
        Ok(s) => s,
        Err(err) => {
            log::info(format!(
                "[Auto Watch] Passive mDNS listener skipped ({err})."
            ));
            return None;
        }
    };

    thread::Builder::new()
        .name("aether-auto-mdns".to_owned())
        .spawn(move || {
            let mut buf = [0u8; 1500];
            let mut last_probe: Option<Instant> = None;

            while !stop_flag.load(Ordering::Acquire) {
                let Ok((n, _src)) = socket.recv_from(&mut buf) else {
                    continue;
                };
                if !is_adb_tls_mdns_response(&buf[..n]) {
                    continue;
                }
                if last_probe.is_some_and(|t| t.elapsed() < ADB_PROBE_COOLDOWN) {
                    continue;
                }
                last_probe = Some(Instant::now());

                log::info(
                    "[Auto Watch] Saw Android _adb-tls-connect mDNS announcement; verifying reachability...",
                );
                if !stop_flag.load(Ordering::Acquire)
                    && probe_wireless_adb_available(adb_host_hint.as_deref())
                {
                    log::info(
                        "[Auto Upgrade] Wireless Debugging ready -> upgrading to Wi-Fi 5GHz ADB Lossless...",
                    );
                    stop_flag.store(true, Ordering::Release);
                    signal.request_auto_upgrade(AutoUpgradeTarget::WifiAdb);
                    break;
                }
            }
        })
        .ok()
}

/// Binds UDP `0.0.0.0:5353` with `SO_REUSEADDR` enabled so we share the mDNS port
/// harmoniously with Windows `dnscache`, Bonjour, and `adb`.
fn bind_reusable_mdns_socket() -> std::result::Result<UdpSocket, String> {
    let _wsa_init = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0));

    unsafe {
        let raw = socket(AF_INET.0 as i32, SOCK_DGRAM, IPPROTO_UDP.0)
            .map_err(|e| format!("WinSock socket() failed: {e}"))?;

        let reuse: i32 = 1;
        let reuse_bytes = std::slice::from_raw_parts(
            (&reuse as *const i32).cast::<u8>(),
            std::mem::size_of::<i32>(),
        );
        if setsockopt(raw, SOL_SOCKET, SO_REUSEADDR, Some(reuse_bytes)) == SOCKET_ERROR {
            let _ = closesocket(raw);
            return Err("setsockopt(SO_REUSEADDR) failed".to_owned());
        }

        let addr = SOCKADDR_IN {
            sin_family: AF_INET,
            sin_port: MDNS_PORT.to_be(),
            sin_addr: IN_ADDR {
                S_un: IN_ADDR_0 { S_addr: 0 },
            },
            sin_zero: [0; 8],
        };
        if bind(
            raw,
            (&addr as *const SOCKADDR_IN).cast::<SOCKADDR>(),
            std::mem::size_of::<SOCKADDR_IN>() as i32,
        ) == SOCKET_ERROR
        {
            let _ = closesocket(raw);
            return Err("bind(0.0.0.0:5353) failed".to_owned());
        }

        let udp = UdpSocket::from_raw_socket(raw.0 as u64);
        let _ = udp.set_read_timeout(Some(Duration::from_millis(350)));

        let joined_lan = resolve_pc_lan_ip(None)
            .map(|lan_ip| udp.join_multicast_v4(&MDNS_MULTICAST_ADDR, &lan_ip).is_ok())
            .unwrap_or(false);
        if !joined_lan {
            udp.join_multicast_v4(&MDNS_MULTICAST_ADDR, &Ipv4Addr::UNSPECIFIED)
                .map_err(|e| format!("join_multicast_v4(224.0.0.251) failed: {e}"))?;
        }

        Ok(udp)
    }
}

/// Returns `true` when `pkt` is an mDNS response/announcement (`QR == 1`)
/// containing the Android Wireless Debugging service label `_adb-tls-connect`.
fn is_adb_tls_mdns_response(pkt: &[u8]) -> bool {
    if pkt.len() < 12 + ADB_MDNS_NEEDLE.len() {
        return false;
    }
    if (pkt[2] & 0x80) == 0 {
        return false;
    }
    contains_ascii_ci(&pkt[12..], ADB_MDNS_NEEDLE)
}

fn contains_ascii_ci(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi::protocol::{DEFAULT_CHANNELS, DEFAULT_SAMPLE_RATE, FLAG_HELLO, HEADER_SIZE};
    use std::net::SocketAddrV4;

    #[test]
    fn detects_adb_tls_mdns_response_and_ignores_queries() {
        let mut resp = vec![0u8; 12];
        resp[2] = 0x84;
        resp.extend_from_slice(b"\x10_adb-tls-connect\x04_tcp\x05local\x00");
        assert!(is_adb_tls_mdns_response(&resp));

        let mut query = resp.clone();
        query[2] = 0x00;
        assert!(!is_adb_tls_mdns_response(&query));

        let mut other = vec![0u8; 12];
        other[2] = 0x84;
        other.extend_from_slice(b"\x0b_googlecast\x04_tcp\x05local\x00");
        assert!(!is_adb_tls_mdns_response(&other));
    }

    #[test]
    fn companion_sentinel_replies_ack_and_signals_auto_upgrade() {
        let test_port = 19495;
        let signal = Arc::new(AppSignal::new());
        let watcher = AutoUpgradeWatcher::start_with_ports(
            Arc::clone(&signal),
            None,
            test_port,
            false,
        );

        let client = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let dest = SocketAddrV4::new(Ipv4Addr::LOCALHOST, test_port);

        let mut hello = [0u8; HEADER_SIZE];
        PacketHeader {
            seq: 0,
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: DEFAULT_CHANNELS,
            flags: FLAG_HELLO,
        }
        .encode(&mut hello);
        client.send_to(&hello, dest).unwrap();

        let mut ack_buf = [0u8; 16];
        let (n, _) = client.recv_from(&mut ack_buf).unwrap();
        assert_eq!(&ack_buf[..n], &ACK_PACKET);

        let evt = signal.wait_for_event();
        assert!(matches!(
            evt,
            crate::signal::WakeEvent::AutoUpgrade(AutoUpgradeTarget::WifiApp(_))
        ));
        drop(watcher);
    }
}
