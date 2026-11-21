use std::env;
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::wifi::protocol::{DEFAULT_UDP_PORT, DISCOVERY_UDP_PORT};

pub const DISCOVER_MAGIC: &[u8] = b"AETHERSORA_DISCOVER_V1";

/// Lightweight UDP LAN discovery responder on port 19481.
/// Sleeps on blocking `recv_from` (0% CPU) and replies with `AETHERSORA_PC_V1|<HOSTNAME>|<PORT>`
/// whenever the Android Companion APK scans the local Wi-Fi network.
pub struct DiscoveryResponder {
    stop_flag: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl DiscoveryResponder {
    pub fn start(audio_port: u16) -> std::result::Result<Self, String> {
        Self::start_on_port(DISCOVERY_UDP_PORT, audio_port)
    }

    pub fn start_on_port(discovery_port: u16, audio_port: u16) -> std::result::Result<Self, String> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, discovery_port))
            .map_err(|e| format!("Failed to bind discovery UDP port {discovery_port}: {e}"))?;
        let _ = socket.set_read_timeout(Some(Duration::from_millis(150)));

        let pc_name = env::var("COMPUTERNAME")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Windows-PC".to_owned());
        let reply_payload = format!("AETHERSORA_PC_V1|{pc_name}|{audio_port}");

        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop_flag);

        let handle = thread::Builder::new()
            .name("aether-lan-discover".to_owned())
            .spawn(move || {
                let mut buf = [0u8; 128];
                while !stop_clone.load(Ordering::Acquire) {
                    let Ok((n, src)) = socket.recv_from(&mut buf) else {
                        continue;
                    };
                    if buf[..n].starts_with(DISCOVER_MAGIC) {
                        let _ = socket.send_to(reply_payload.as_bytes(), src);
                    }
                }
            })
            .map_err(|e| format!("Failed to spawn LAN discovery thread: {e}"))?;

        Ok(Self {
            stop_flag,
            join_handle: Some(handle),
        })
    }

    #[allow(dead_code)]
    pub fn default_audio_port() -> u16 {
        DEFAULT_UDP_PORT
    }
}

impl Drop for DiscoveryResponder {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddrV4;

    #[test]
    fn responds_to_discover_beacon_on_loopback() {
        let probe_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        probe_sock
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        let test_disc_port = 19491;
        let responder = DiscoveryResponder::start_on_port(test_disc_port, DEFAULT_UDP_PORT)
            .expect("discovery responder should bind");

        let target = SocketAddrV4::new(Ipv4Addr::LOCALHOST, test_disc_port);
        probe_sock.send_to(DISCOVER_MAGIC, target).unwrap();

        let mut reply = [0u8; 128];
        let (n, _) = probe_sock.recv_from(&mut reply).unwrap();
        let text = std::str::from_utf8(&reply[..n]).unwrap();
        assert!(text.starts_with("AETHERSORA_PC_V1|"), "unexpected reply: {text}");
        assert!(text.ends_with(&format!("|{DEFAULT_UDP_PORT}")));
        drop(responder);
    }
}
