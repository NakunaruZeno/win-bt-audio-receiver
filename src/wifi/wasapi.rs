use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};

use windows::core::{w, Result};
use windows::Win32::Foundation::{CloseHandle, FALSE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioClient, IAudioClockAdjustment, IAudioRenderClient,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_RATEADJUST, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
    WAVE_FORMAT_PCM,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW,
    WaitForSingleObject,
};

use crate::wifi::protocol::{DEFAULT_CHANNELS, DEFAULT_SAMPLE_RATE};
use crate::wifi::ringbuf::SpscPcmRingBuffer;

/// Dedicated WASAPI Event-Driven renderer running on a real-time MMCSS "Pro Audio" thread.
/// Uses `IAudioClockAdjustment` to dynamically eliminate clock drift between the phone
/// and the PC's audio DAC with ~0% CPU overhead.
pub struct WasapiRenderer {
    stop_flag: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl WasapiRenderer {
    pub fn start(ring: Arc<SpscPcmRingBuffer>, target_buffer_ms: u32) -> Result<Self> {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop_flag);
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<()>>(1);

        let handle = thread::Builder::new()
            .name("aether-wasapi-render".to_owned())
            .spawn(move || {
                render_thread_main(ring, target_buffer_ms.clamp(10, 80), stop_clone, init_tx);
            })
            .expect("failed to spawn WASAPI render thread");

        match init_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop_flag,
                join_handle: Some(handle),
            }),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => {
                let _ = handle.join();
                Err(windows::core::Error::from_win32())
            }
        }
    }
}

impl Drop for WasapiRenderer {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

fn render_thread_main(
    ring: Arc<SpscPcmRingBuffer>,
    target_buffer_ms: u32,
    stop_flag: Arc<AtomicBool>,
    init_tx: mpsc::SyncSender<Result<()>>,
) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let result = unsafe { run_wasapi_loop(&ring, target_buffer_ms, &stop_flag, &init_tx) };
    if let Err(err) = result {
        let _ = init_tx.try_send(Err(err));
    }

    unsafe {
        CoUninitialize();
    }
}

unsafe fn run_wasapi_loop(
    ring: &Arc<SpscPcmRingBuffer>,
    target_buffer_ms: u32,
    stop_flag: &Arc<AtomicBool>,
    init_tx: &mpsc::SyncSender<Result<()>>,
) -> Result<()> {
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
    let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

    let block_align = (DEFAULT_CHANNELS as u16) * 2;
    let wave_format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: DEFAULT_CHANNELS as u16,
        nSamplesPerSec: DEFAULT_SAMPLE_RATE,
        nAvgBytesPerSec: DEFAULT_SAMPLE_RATE * (block_align as u32),
        nBlockAlign: block_align,
        wBitsPerSample: 16,
        cbSize: 0,
    };

    let stream_flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        | AUDCLNT_STREAMFLAGS_RATEADJUST
        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;

    let hns_buffer_duration = (target_buffer_ms as i64) * 10_000;
    audio_client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        stream_flags,
        hns_buffer_duration,
        0,
        &wave_format,
        None,
    )?;

    let event_handle = CreateEventW(None, FALSE, FALSE, None)?;
    if let Err(e) = audio_client.SetEventHandle(event_handle) {
        let _ = CloseHandle(event_handle);
        return Err(e);
    }

    let render_client: IAudioRenderClient = match audio_client.GetService() {
        Ok(rc) => rc,
        Err(e) => {
            let _ = CloseHandle(event_handle);
            return Err(e);
        }
    };
    let clock_adjustment: Option<IAudioClockAdjustment> = audio_client.GetService().ok();
    let buffer_size_frames = audio_client.GetBufferSize()?;

    if let Ok(data_ptr) = render_client.GetBuffer(buffer_size_frames) {
        slice::from_raw_parts_mut(data_ptr, (buffer_size_frames as usize) * 4).fill(0);
        let _ = render_client.ReleaseBuffer(buffer_size_frames, 0);
    }

    if let Err(e) = audio_client.Start() {
        let _ = CloseHandle(event_handle);
        return Err(e);
    }

    let _ = init_tx.send(Ok(()));

    let mut task_index = 0u32;
    let mmcss_handle = AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_index).ok();

    let target_fill_frames = ((DEFAULT_SAMPLE_RATE / 1000) * target_buffer_ms) as f32;
    let mut smoothed_fill = target_fill_frames;
    let mut prebuffering = true;
    let mut last_sample = (0i16, 0i16);
    let mut tick_counter = 0u32;

    while !stop_flag.load(Ordering::Acquire) {
        let wait_res = WaitForSingleObject(event_handle, 40);
        if wait_res != WAIT_OBJECT_0 {
            continue;
        }

        let padding = match audio_client.GetCurrentPadding() {
            Ok(p) => p,
            Err(_) => break,
        };
        let frames_to_write = buffer_size_frames.saturating_sub(padding);
        if frames_to_write == 0 {
            continue;
        }

        let out_ptr = match render_client.GetBuffer(frames_to_write) {
            Ok(p) => p,
            Err(_) => break,
        };
        let out_slice = slice::from_raw_parts_mut(out_ptr, (frames_to_write as usize) * 4);

        let avail = ring.available_frames();
        if prebuffering {
            if (avail as f32) >= target_fill_frames {
                prebuffering = false;
                smoothed_fill = avail as f32;
            } else {
                out_slice.fill(0);
                let _ = render_client.ReleaseBuffer(frames_to_write, 0);
                continue;
            }
        }

        let read_frames = ring.pop_into_le_bytes(out_slice, &mut last_sample);
        let _ = render_client.ReleaseBuffer(frames_to_write, 0);

        if read_frames == 0 && avail == 0 {
            prebuffering = true;
            continue;
        }

        smoothed_fill = smoothed_fill * 0.98 + (avail as f32) * 0.02;
        tick_counter = tick_counter.wrapping_add(1);
        if tick_counter.is_multiple_of(50) {
            if let Some(ref clock) = clock_adjustment {
                let error_frames = smoothed_fill - target_fill_frames;
                let ratio = (1.0 + (error_frames / 48_000.0) * 0.08).clamp(0.9982, 1.0018);
                let _ = clock.SetSampleRate((DEFAULT_SAMPLE_RATE as f32) * ratio);
            }
        }
    }

    let _ = audio_client.Stop();
    if let Some(h) = mmcss_handle {
        let _ = AvRevertMmThreadCharacteristics(h);
    }
    let _ = CloseHandle(event_handle);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn wasapi_renderer_drains_ring_buffer_and_drops_cleanly() {
        let ring = Arc::new(SpscPcmRingBuffer::new(4096));
        let mut frame_bytes = vec![0u8; 48 * 40 * 4];
        for i in 0..(48 * 40) {
            let sample: i16 = if i % 100 < 50 { 2 } else { -2 };
            let b = sample.to_le_bytes();
            frame_bytes[i * 4] = b[0];
            frame_bytes[i * 4 + 1] = b[1];
            frame_bytes[i * 4 + 2] = b[0];
            frame_bytes[i * 4 + 3] = b[1];
        }
        ring.push_pcm_le_bytes(&frame_bytes);
        let before = ring.available_frames();
        assert_eq!(before, 48 * 40);

        let renderer = WasapiRenderer::start(Arc::clone(&ring), 20)
            .expect("WASAPI default audio render endpoint should initialize");
        std::thread::sleep(Duration::from_millis(80));
        let after = ring.available_frames();
        assert!(
            after < before,
            "WASAPI render thread should drain frames from ring buffer (before={before}, after={after})"
        );
        drop(renderer);
    }
}
