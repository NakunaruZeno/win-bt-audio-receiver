pub mod adb;
pub mod protocol;
pub mod ringbuf;
pub mod wasapi;

use std::net::{Ipv4Addr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::signal::{set_streaming_keep_awake, AppSignal};
use crate::wifi::adb::{try_launch_android_agent, AdbAgentProcess};
use crate::wifi::protocol::{
    PacketHeader, DEFAULT_UDP_PORT, FRAMES_PER_PACKET, HEADER_SIZE, MAX_UDP_PACKET_SIZE,
    PCM_BYTES_PER_PACKET,
};
use crate::wifi::ringbuf::SpscPcmRingBuffer;
use crate::wifi::wasapi::WasapiRenderer;

const STREAM_SILENCE_TIMEOUT_MS: u64 = 2500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiExitReason {
    UserExit,
    StreamLost,
}

/// Strictly isolated Wi-Fi 5 GHz LAN audio session.
/// Owns the UDP receiver thread, lock-free SPSC ring buffer, WASAPI Pro Audio renderer,
/// and the ADB lifeline pipe. Zero WinRT Bluetooth resources exist while `WifiSession` is active.
pub struct WifiSession {
    stop_flag: Arc<AtomicBool>,
    last_packet_ms: Arc<AtomicU64>,
    start_instant: Instant,
    udp_thread: Option<JoinHandle<()>>,
    _wasapi: WasapiRenderer,
    _adb_agent: AdbAgentProcess,
    adb_host_hint: Option<String>,
    buffer_ms: u32,
}

impl WifiSession {
    pub fn try_start(
        adb_host_hint: Option<String>,
        buffer_ms: u32,
    ) -> std::result::Result<Self, String> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DEFAULT_UDP_PORT))
            .map_err(|e| format!("Failed to bind UDP port {DEFAULT_UDP_PORT}: {e}"))?;
        let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));

        // Start remote Android capture agent over ADB Wireless/USB first; if unreachable,
        // return Err immediately BEFORE initializing WASAPI or spawning audio threads.
        let adb_agent = try_launch_android_agent(adb_host_hint.as_deref(), DEFAULT_UDP_PORT)?;

        let ring = Arc::new(SpscPcmRingBuffer::new(16_384));
        let wasapi = WasapiRenderer::start(Arc::clone(&ring), buffer_ms)
            .map_err(|e| format!("Failed to initialize WASAPI renderer: {e}"))?;

        let start_instant = Instant::now();
        let last_packet_ms = Arc::new(AtomicU64::new(0));
        let stop_flag = Arc::new(AtomicBool::new(false));

        let udp_thread = spawn_udp_receiver(
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
        println!(
            "[Mode: Wi-Fi 5GHz LAN] Streaming 48 kHz 16-bit Stereo Lossless PCM (buffer: {} ms, 0 BT radio load).",
            self.buffer_ms
        );

        loop {
            if signal.sleep_interruptible(Duration::from_millis(400)) {
                return WifiExitReason::UserExit;
            }

            let now_ms = self.start_instant.elapsed().as_millis() as u64;
            let last_ms = self.last_packet_ms.load(Ordering::Acquire);

            if now_ms.saturating_sub(last_ms) > STREAM_SILENCE_TIMEOUT_MS {
                println!(
                    "[Wi-Fi LAN] No UDP audio/keepalive packets for {:.1}s. Attempting fast Wi-Fi recovery...",
                    (now_ms.saturating_sub(last_ms)) as f64 / 1000.0
                );

                // Try one quick ADB agent relaunch in case of transient Wi-Fi roaming
                match try_launch_android_agent(self.adb_host_hint.as_deref(), DEFAULT_UDP_PORT) {
                    Ok(new_agent) => {
                        self._adb_agent = new_agent;
                        let refreshed = self.start_instant.elapsed().as_millis() as u64;
                        self.last_packet_ms.store(refreshed, Ordering::Release);
                        println!("[Wi-Fi LAN] Re-established Android Wi-Fi stream.");
                    }
                    Err(err) => {
                        println!("[Wi-Fi LAN] Wi-Fi recovery failed ({err}).");
                        return WifiExitReason::StreamLost;
                    }
                }
            }
        }
    }
}

impl Drop for WifiSession {
    fn drop(&mut self) {
        set_streaming_keep_awake(false);
        self.stop_flag.store(true, Ordering::Release);
        if let Some(handle) = self.udp_thread.take() {
            let _ = handle.join();
        }
        // `_wasapi` and `_adb_agent` drop automatically in declaration order,
        // stopping WASAPI audio client and closing the ADB lifeline pipe.
    }
}

fn spawn_udp_receiver(
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

                if hdr.is_keepalive() {
                    continue;
                }

                let payload = &buf[HEADER_SIZE..n];
                if payload.len() < 4 {
                    continue;
                }

                if let Some(exp) = expected_seq {
                    let diff = hdr.seq.wrapping_sub(exp);
                    if diff > 0 && diff <= 8 {
                        // Conceal up to 8 lost packets (<= 40 ms) with smooth fade-out
                        ring.push_concealment_frames((diff as usize) * FRAMES_PER_PACKET, last_lr);
                    } else if diff > 0x8000_0000 {
                        // Out-of-order late packet: drop it
                        continue;
                    }
                }
                expected_seq = Some(hdr.seq.wrapping_add(1));

                let valid_len = payload.len().min(PCM_BYTES_PER_PACKET) & !3;
                if valid_len >= 4 {
                    let tail = valid_len - 4;
                    let l = i16::from_le_bytes([payload[tail], payload[tail + 1]]);
                    let r = i16::from_le_bytes([payload[tail + 2], payload[tail + 3]]);
                    last_lr = (l, r);
                    ring.push_pcm_le_bytes(&payload[..valid_len]);
                }
            }
        })
        .map_err(|e| format!("Failed to spawn UDP receiver thread: {e}"))
}
