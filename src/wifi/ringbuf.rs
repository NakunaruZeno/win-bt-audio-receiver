use std::sync::atomic::{AtomicI16, AtomicUsize, Ordering};

/// Lock-free Single-Producer Single-Consumer (SPSC) stereo PCM ring buffer.
/// Uses 100% safe Rust (`AtomicI16` + `AtomicUsize`) with zero mutex contention
/// and built-in Packet Loss Concealment (smooth exponential decay on underrun).
pub struct SpscPcmRingBuffer {
    samples: Box<[AtomicI16]>,
    capacity_frames: usize,
    mask_frames: usize,
    write_frames: AtomicUsize,
    read_frames: AtomicUsize,
}

impl SpscPcmRingBuffer {
    /// Creates a ring buffer whose capacity in stereo frames is rounded up to a power of two.
    /// Default 16,384 stereo frames (~341 ms at 48 kHz) uses 64 KB of memory.
    pub fn new(min_capacity_frames: usize) -> Self {
        let cap = min_capacity_frames.next_power_of_two().max(1024);
        let total_samples = cap * 2;
        let mut vec = Vec::with_capacity(total_samples);
        for _ in 0..total_samples {
            vec.push(AtomicI16::new(0));
        }

        Self {
            samples: vec.into_boxed_slice(),
            capacity_frames: cap,
            mask_frames: cap - 1,
            write_frames: AtomicUsize::new(0),
            read_frames: AtomicUsize::new(0),
        }
    }

    pub fn available_frames(&self) -> usize {
        let w = self.write_frames.load(Ordering::Acquire);
        let r = self.read_frames.load(Ordering::Acquire);
        w.wrapping_sub(r)
    }

    /// Producer (UDP receive thread): pushes interleaved little-endian 16-bit stereo PCM bytes.
    /// If the ring buffer is nearly full, drops oldest frames by advancing `read_frames` isn't
    /// safe from producer, so producer clamps to free space to keep SPSC lock-free invariants.
    pub fn push_pcm_le_bytes(&self, pcm_bytes: &[u8]) -> usize {
        let incoming_frames = pcm_bytes.len() / 4;
        if incoming_frames == 0 {
            return 0;
        }

        let w = self.write_frames.load(Ordering::Relaxed);
        let r = self.read_frames.load(Ordering::Acquire);
        let used = w.wrapping_sub(r);
        let free = self.capacity_frames.saturating_sub(used);

        let to_write = incoming_frames.min(free);
        for i in 0..to_write {
            let byte_offset = i * 4;
            let left = i16::from_le_bytes([pcm_bytes[byte_offset], pcm_bytes[byte_offset + 1]]);
            let right =
                i16::from_le_bytes([pcm_bytes[byte_offset + 2], pcm_bytes[byte_offset + 3]]);

            let frame_slot = (w.wrapping_add(i)) & self.mask_frames;
            let sample_idx = frame_slot * 2;
            self.samples[sample_idx].store(left, Ordering::Relaxed);
            self.samples[sample_idx + 1].store(right, Ordering::Relaxed);
        }

        self.write_frames
            .store(w.wrapping_add(to_write), Ordering::Release);
        to_write
    }

    /// Producer helper: inserts `frames` of smooth fade-out silence when a sequence gap (packet loss)
    /// is detected, preventing sudden waveform discontinuities.
    pub fn push_concealment_frames(&self, frames: usize, last_lr: (i16, i16)) {
        let w = self.write_frames.load(Ordering::Relaxed);
        let r = self.read_frames.load(Ordering::Acquire);
        let free = self.capacity_frames.saturating_sub(w.wrapping_sub(r));
        let to_write = frames.min(free);

        let mut l = last_lr.0 as i32;
        let mut r_ch = last_lr.1 as i32;
        for i in 0..to_write {
            l = (l * 31) / 32;
            r_ch = (r_ch * 31) / 32;
            let frame_slot = (w.wrapping_add(i)) & self.mask_frames;
            let sample_idx = frame_slot * 2;
            self.samples[sample_idx].store(l as i16, Ordering::Relaxed);
            self.samples[sample_idx + 1].store(r_ch as i16, Ordering::Relaxed);
        }

        self.write_frames
            .store(w.wrapping_add(to_write), Ordering::Release);
    }

    /// Consumer (WASAPI Pro Audio thread): reads `requested_frames` stereo frames into `out_bytes`
    /// (`requested_frames * 4` bytes). If an underrun occurs, smoothly decays `last_sample`
    /// instead of hard-clipping to zero.
    pub fn pop_into_le_bytes(
        &self,
        out_bytes: &mut [u8],
        last_sample: &mut (i16, i16),
    ) -> usize {
        let requested_frames = out_bytes.len() / 4;
        if requested_frames == 0 {
            return 0;
        }

        let r = self.read_frames.load(Ordering::Relaxed);
        let w = self.write_frames.load(Ordering::Acquire);
        let avail = w.wrapping_sub(r);

        // If buffer overflowed badly (> 120 ms queued), skip stale frames to snap back to low latency.
        let max_allowed_queue = 48 * 120; // 120 ms at 48 kHz = 5760 frames
        let effective_r = if avail > max_allowed_queue {
            w.wrapping_sub(48 * 25) // Snap to 25 ms
        } else {
            r
        };
        let effective_avail = w.wrapping_sub(effective_r);
        let to_read = requested_frames.min(effective_avail);

        for i in 0..to_read {
            let frame_slot = (effective_r.wrapping_add(i)) & self.mask_frames;
            let sample_idx = frame_slot * 2;
            let left = self.samples[sample_idx].load(Ordering::Relaxed);
            let right = self.samples[sample_idx + 1].load(Ordering::Relaxed);
            *last_sample = (left, right);

            let out_idx = i * 4;
            let lb = left.to_le_bytes();
            let rb = right.to_le_bytes();
            out_bytes[out_idx] = lb[0];
            out_bytes[out_idx + 1] = lb[1];
            out_bytes[out_idx + 2] = rb[0];
            out_bytes[out_idx + 3] = rb[1];
        }

        self.read_frames
            .store(effective_r.wrapping_add(to_read), Ordering::Release);

        // Conceal any remaining underrun frames with smooth exponential decay to 0.
        let mut l = last_sample.0 as i32;
        let mut r_ch = last_sample.1 as i32;
        for i in to_read..requested_frames {
            l = (l * 31) / 32;
            r_ch = (r_ch * 31) / 32;
            let out_idx = i * 4;
            let lb = (l as i16).to_le_bytes();
            let rb = (r_ch as i16).to_le_bytes();
            out_bytes[out_idx] = lb[0];
            out_bytes[out_idx + 1] = lb[1];
            out_bytes[out_idx + 2] = rb[0];
            out_bytes[out_idx + 3] = rb[1];
        }
        *last_sample = (l as i16, r_ch as i16);

        to_read
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spsc_push_and_pop_preserves_stereo_samples() {
        let rb = SpscPcmRingBuffer::new(1024);
        let mut in_bytes = [0u8; 16]; // 4 stereo frames
        for f in 0..4 {
            let l = ((f + 1) * 1000) as i16;
            let r = -l;
            in_bytes[f * 4..f * 4 + 2].copy_from_slice(&l.to_le_bytes());
            in_bytes[f * 4 + 2..f * 4 + 4].copy_from_slice(&r.to_le_bytes());
        }

        assert_eq!(rb.push_pcm_le_bytes(&in_bytes), 4);
        assert_eq!(rb.available_frames(), 4);

        let mut out_bytes = [0u8; 16];
        let mut last = (0i16, 0i16);
        assert_eq!(rb.pop_into_le_bytes(&mut out_bytes, &mut last), 4);
        assert_eq!(in_bytes, out_bytes);
        assert_eq!(rb.available_frames(), 0);
    }
}
