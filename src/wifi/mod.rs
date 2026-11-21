pub mod adb;
pub mod auto_watch;
pub mod discovery;
pub mod protocol;
pub mod ringbuf;
pub mod wasapi;

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::log;
use crate::signal::{set_streaming_keep_awake, AppSignal};
use crate::wifi::adb::{try_launch_android_agent, AdbAgentProcess};
use crate::wifi::discovery::DiscoveryResponder;
use crate::wifi::protocol::{
    PacketHeader, ACK_PACKET, DEFAULT_UDP_PORT, FRAMES_PER_PACKET, HEADER_SIZE,
    MAX_UDP_PACKET_SIZE, PCM_BYTES_PER_PACKET,
};
use crate::wifi::ringbuf::SpscPcmRingBuffer;
use crate::wifi::wasapi::WasapiRenderer;

const STREAM_SILENCE_TIMEOUT_MS: u64 = 2500;
const COMPANION_IDLE_TIMEOUT_MS: u64 = 3500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiExitReason {
    UserExitOrSwitch,
    StreamLost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiAppEvent {
    Connected(IpAddr),
    Disconnected,
}

/// Tier 3: Strictly isolated Wi-Fi 5 GHz LAN audio session using ADB Wireless + embedded 5.3 KB DEX.
/// Zero WinRT Bluetooth handles exist while `WifiAdbSession` is active.
pub struct WifiAdbSession {
    stop_flag: Arc<AtomicBool>,
    last_packet_ms: Arc<AtomicU64>,
    start_instant: Instant,
    udp_thread: Option<JoinHandle<()>>,
    _wasapi: WasapiRenderer,
    _adb_agent: AdbAgentProcess,
    adb_host_hint: Option<String>,
    buffer_ms: u32,
}

impl WifiAdbSession {
    pub fn try_start(
        adb_host_hint: Option<String>,
        buffer_ms: u32,
    ) -> std::result::Result<Self, String> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DEFAULT_UDP_PORT))
            .map_err(|e| format!("Failed to bind UDP port {DEFAULT_UDP_PORT}: {e}"))?;
        let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));

        let adb_agent = try_launch_android_agent(adb_host_hint.as_deref(), DEFAULT_UDP_PORT)?;

        let ring = Arc::new(SpscPcmRingBuffer::new(16_384));
        let wasapi = WasapiRenderer::start(Arc::clone(&ring), buffer_ms)
            .map_err(|e| format!("Failed to initialize WASAPI renderer: {e}"))?;

        let start_instant = Instant::now();
        let last_packet_ms = Arc::new(AtomicU64::new(0));
        let stop_flag = Arc::new(AtomicBool::new(false));

        let udp_thread = spawn_adb_udp_receiver(
            socket,
            ring,
            Arc::clone(&stop_flag),
            Arc::clone(&last_packet_ms),
            start_instant,
        )?;

        set_streaming_keep_awake(true);

        Ok(Self {
            stop_flag,
            last_packet_ms,
            start_instant,
            udp_thread: Some(udp_thread),
            _wasapi: wasapi,
            _adb_agent: adb_agent,
            adb_host_hint,
            buffer_ms,
        })
    }

    pub fn run_until_exit_or_lost(&mut self, signal: &Arc<AppSignal>) -> WifiExitReason {
        log::info(format!(
            "[Mode: Wi-Fi 5GHz ADB] Streaming 48 kHz 16-bit Stereo Lossless PCM (buffer: {} ms, 0 BT radio load).",
            self.buffer_ms
        ));

        loop {
            if signal.sleep_interruptible(Duration::from_millis(400)) {
                return WifiExitReason::UserExitOrSwitch;
            }

            let now_ms = self.start_instant.elapsed().as_millis() as u64;
            let last_ms = self.last_packet_ms.load(Ordering::Acquire);

            if now_ms.saturating_sub(last_ms) > STREAM_SILENCE_TIMEOUT_MS {
                log::info(format!(
                    "[Wi-Fi ADB] No UDP packets for {:.1}s. Attempting fast recovery...",
                    (now_ms.saturating_sub(last_ms)) as f64 / 1000.0
                ));

                match try_launch_android_agent(self.adb_host_hint.as_deref(), DEFAULT_UDP_PORT) {
                    Ok(new_agent) => {
                        self._adb_agent = new_agent;
                        let refreshed = self.start_instant.elapsed().as_millis() as u64;
                        self.last_packet_ms.store(refreshed, Ordering::Release);
                        log::info("[Wi-Fi ADB] Re-established Android Wi-Fi stream.");
                    }
                    Err(err) => {
                        log::info(format!("[Wi-Fi ADB] Wi-Fi recovery failed ({err})."));
                        return WifiExitReason::StreamLost;
                    }
                }
            }
        }
    }
}

impl Drop for WifiAdbSession {
    fn drop(&mut self) {
        set_streaming_keep_awake(false);
        self.stop_flag.store(true, Ordering::Release);
        if let Some(handle) = self.udp_thread.take() {
            let _ = handle.join();
        }
    }
}

/// Tier 2: Strictly isolated Wi-Fi 5 GHz LAN passive session for the Android Companion APK
/// (No Developer Mode, No ADB required).
/// Uses lazy WASAPI activation: keeps WASAPI closed (0% CPU) until the phone taps Connect.
pub struct WifiAppSession {
    stop_flag: Arc<AtomicBool>,
    udp_thread: Option<JoinHandle<()>>,
    _discovery: DiscoveryResponder,
    active_peer: Arc<Mutex<Option<SocketAddr>>>,
    initial_peer: Option<SocketAddr>,
    buffer_ms: u32,
}

impl WifiAppSession {
    pub fn start(buffer_ms: u32) -> std::result::Result<Self, String> {
        Self::start_with_peer(buffer_ms, None)
    }

    pub fn start_with_peer(
        buffer_ms: u32,
        initial_peer: Option<SocketAddr>,
    ) -> std::result::Result<Self, String> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DEFAULT_UDP_PORT))
            .map_err(|e| format!("Failed to bind UDP port {DEFAULT_UDP_PORT}: {e}"))?;
        let _ = socket.set_read_timeout(Some(Duration::from_millis(200)));

        let discovery = DiscoveryResponder::start(DEFAULT_UDP_PORT)?;
        let stop_flag = Arc::new(AtomicBool::new(false));
        let active_peer = Arc::new(Mutex::new(initial_peer));

        let udp_thread = spawn_companion_udp_receiver(
            socket,
            buffer_ms,
            Arc::clone(&stop_flag),
            Arc::clone(&active_peer),
            initial_peer.is_some(),
        )?;

        Ok(Self {
            stop_flag,
            udp_thread: Some(udp_thread),
            _discovery: discovery,
            active_peer,
            initial_peer,
            buffer_ms,
        })
    }

    pub fn run_until_exit(
        &mut self,
        signal: &Arc<AppSignal>,
        exit_on_disconnect: bool,
        on_event: impl Fn(WifiAppEvent),
    ) {
        log::info(format!(
            "[Mode: Wi-Fi 5GHz Companion App] Listening on UDP {DEFAULT_UDP_PORT} + LAN discovery 19481 (buffer: {} ms, No Dev Mode).",
            self.buffer_ms
        ));

        let mut last_peer: Option<SocketAddr> = self.initial_peer;
        loop {
            if signal.sleep_interruptible(Duration::from_millis(300)) {
                break;
            }
            let current_peer = self.active_peer.lock().ok().and_then(|g| *g);
            if current_peer != last_peer {
                last_peer = current_peer;
                match current_peer {
                    Some(addr) => on_event(WifiAppEvent::Connected(addr.ip())),
                    None => {
                        on_event(WifiAppEvent::Disconnected);
                        if exit_on_disconnect {
                            break;
                        }
                    }
                }
            }
        }
    }
}

impl Drop for WifiAppSession {
    fn drop(&mut self) {
        set_streaming_keep_awake(false);
        self.stop_flag.store(true, Ordering::Release);
        if let Some(handle) = self.udp_thread.take() {
            let _ = handle.join();
        }
    }
}

fn spawn_adb_udp_receiver(
    socket: UdpSocket,
    ring: Arc<SpscPcmRingBuffer>,
    stop_flag: Arc<AtomicBool>,
    last_packet_ms: Arc<AtomicU64>,
    start_instant: Instant,
) -> std::result::Result<JoinHandle<()>, String> {
    thread::Builder::new()
        .name("aether-udp-recv".to_owned())
        .spawn(move || {
            let mut buf = [0u8; MAX_UDP_PACKET_SIZE + 64];
            let mut expected_seq: Option<u32> = None;
            let mut last_lr = (0i16, 0i16);

            while !stop_flag.load(Ordering::Acquire) {
                let Ok((n, _src)) = socket.recv_from(&mut buf) else {
                    continue;
                };
                let Some(hdr) = PacketHeader::decode(&buf[..n]) else {
                    continue;
                };

                let elapsed_ms = start_instant.elapsed().as_millis() as u64;
                last_packet_ms.store(elapsed_ms, Ordering::Release);

                if hdr.is_keepalive() || hdr.is_hello() || hdr.is_bye() {
                    continue;
                }

                push_audio_payload(
                    &ring,
                    &buf[HEADER_SIZE..n],
                    hdr.seq,
                    &mut expected_seq,
                    &mut last_lr,
                );
            }
        })
        .map_err(|e| format!("Failed to spawn UDP receiver thread: {e}"))
}

fn spawn_companion_udp_receiver(
    socket: UdpSocket,
    buffer_ms: u32,
    stop_flag: Arc<AtomicBool>,
    active_peer: Arc<Mutex<Option<SocketAddr>>>,
    start_active: bool,
) -> std::result::Result<JoinHandle<()>, String> {
    thread::Builder::new()
        .name("aether-app-udp".to_owned())
        .spawn(move || {
            let mut buf = [0u8; MAX_UDP_PACKET_SIZE + 64];
            let mut ring: Option<Arc<SpscPcmRingBuffer>> = None;
            let mut wasapi: Option<WasapiRenderer> = None;
            let mut last_activity: Option<Instant> = start_active.then(Instant::now);
            let mut expected_seq: Option<u32> = None;
            let mut last_lr = (0i16, 0i16);

            while !stop_flag.load(Ordering::Acquire) {
                match socket.recv_from(&mut buf) {
                    Ok((n, src)) => {
                        let Some(hdr) = PacketHeader::decode(&buf[..n]) else {
                            continue;
                        };

                        if hdr.is_hello() {
                            if let Ok(mut p) = active_peer.lock() {
                                *p = Some(src);
                            }
                            last_activity = Some(Instant::now());
                            let _ = socket.send_to(&ACK_PACKET, src);
                            log::info(format!(
                                "[Wi-Fi Companion] Handshake HELLO from {src} -> ACK sent."
                            ));
                            continue;
                        }

                        if hdr.is_bye() {
                            log::info(format!(
                                "[Wi-Fi Companion] Client {src} disconnected (BYE)."
                            ));
                            teardown_lazy_wasapi(
                                &mut wasapi,
                                &mut ring,
                                &mut last_activity,
                                &mut expected_seq,
                                &active_peer,
                            );
                            continue;
                        }

                        last_activity = Some(Instant::now());
                        if let Ok(mut p) = active_peer.lock() {
                            if *p != Some(src) {
                                *p = Some(src);
                            }
                        }

                        if hdr.is_keepalive() {
                            continue;
                        }

                        if wasapi.is_none() {
                            let new_ring = Arc::new(SpscPcmRingBuffer::new(16_384));
                            match WasapiRenderer::start(Arc::clone(&new_ring), buffer_ms) {
                                Ok(renderer) => {
                                    log::info(format!(
                                        "[Wi-Fi Companion] Streaming 48 kHz Lossless PCM from {src}!"
                                    ));
                                    ring = Some(new_ring);
                                    wasapi = Some(renderer);
                                    set_streaming_keep_awake(true);
                                }
                                Err(err) => {
                                    log::error(format!(
                                        "[Wi-Fi Companion] Failed to open WASAPI: {err}"
                                    ));
                                    continue;
                                }
                            }
                        }

                        if let Some(ref active_ring) = ring {
                            push_audio_payload(
                                active_ring,
                                &buf[HEADER_SIZE..n],
                                hdr.seq,
                                &mut expected_seq,
                                &mut last_lr,
                            );
                        }
                    }
                    Err(_) => {
                        if let Some(last) = last_activity {
                            if last.elapsed() > Duration::from_millis(COMPANION_IDLE_TIMEOUT_MS) {
                                log::info(
                                    "[Wi-Fi Companion] Client idle timeout; releasing WASAPI (0% CPU standby).",
                                );
                                teardown_lazy_wasapi(
                                    &mut wasapi,
                                    &mut ring,
                                    &mut last_activity,
                                    &mut expected_seq,
                                    &active_peer,
                                );
                            }
                        }
                    }
                }
            }

            teardown_lazy_wasapi(
                &mut wasapi,
                &mut ring,
                &mut last_activity,
                &mut expected_seq,
                &active_peer,
            );
        })
        .map_err(|e| format!("Failed to spawn Companion UDP thread: {e}"))
}

fn teardown_lazy_wasapi(
    wasapi: &mut Option<WasapiRenderer>,
    ring: &mut Option<Arc<SpscPcmRingBuffer>>,
    last_activity: &mut Option<Instant>,
    expected_seq: &mut Option<u32>,
    active_peer: &Arc<Mutex<Option<SocketAddr>>>,
) {
    if wasapi.take().is_some() {
        set_streaming_keep_awake(false);
    }
    *ring = None;
    *last_activity = None;
    *expected_seq = None;
    if let Ok(mut p) = active_peer.lock() {
        *p = None;
    }
}

fn push_audio_payload(
    ring: &SpscPcmRingBuffer,
    payload: &[u8],
    seq: u32,
    expected_seq: &mut Option<u32>,
    last_lr: &mut (i16, i16),
) {
    if payload.len() < 4 {
        return;
    }

    if let Some(exp) = *expected_seq {
        let diff = seq.wrapping_sub(exp);
        if diff > 0 && diff <= 8 {
            ring.push_concealment_frames((diff as usize) * FRAMES_PER_PACKET, *last_lr);
        } else if diff > 0x8000_0000 {
            return;
        }
    }
    *expected_seq = Some(seq.wrapping_add(1));

    let valid_len = payload.len().min(PCM_BYTES_PER_PACKET) & !3;
    if valid_len >= 4 {
        let tail = valid_len - 4;
        let l = i16::from_le_bytes([payload[tail], payload[tail + 1]]);
        let r = i16::from_le_bytes([payload[tail + 2], payload[tail + 3]]);
        *last_lr = (l, r);
        ring.push_pcm_le_bytes(&payload[..valid_len]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi::protocol::{
        DEFAULT_CHANNELS, DEFAULT_SAMPLE_RATE, FLAG_BYE, FLAG_HELLO,
    };
    use std::net::SocketAddrV4;

    #[test]
    fn companion_udp_handles_hello_ack_and_bye_cleanly() {
        let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = sock.local_addr().unwrap().port();
        sock.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let peer = Arc::new(Mutex::new(None));
        let handle = spawn_companion_udp_receiver(
            sock,
            20,
            Arc::clone(&stop),
            Arc::clone(&peer),
            false,
        )
        .unwrap();

        let client = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let dest = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);

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
        assert!(peer.lock().unwrap().is_some());

        let mut bye = [0u8; HEADER_SIZE];
        PacketHeader {
            seq: 1,
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: DEFAULT_CHANNELS,
            flags: FLAG_BYE,
        }
        .encode(&mut bye);
        client.send_to(&bye, dest).unwrap();

        thread::sleep(Duration::from_millis(80));
        assert!(peer.lock().unwrap().is_none());

        stop.store(true, Ordering::Release);
        handle.join().unwrap();
    }
}
