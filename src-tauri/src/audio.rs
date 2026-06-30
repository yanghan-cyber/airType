use std::sync::{Arc, Mutex};

/// Circular PCM buffer for collecting audio samples during recording.
pub struct AudioBuffer {
    data: Vec<i16>,
    sample_rate: u32,
    capturing: bool,
    /// Cursor for realtime incremental reads (non-destructive; batch reads still see full buffer).
    realtime_cursor: usize,
}

impl AudioBuffer {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            data: Vec::new(),
            sample_rate,
            capturing: false,
            realtime_cursor: 0,
        }
    }

    pub fn push_f32(&mut self, samples: &[f32]) {
        for &s in samples {
            let clamped = s.clamp(-1.0, 1.0);
            let val = (clamped * 32767.0) as i16;
            self.data.push(val);
        }
    }

    pub fn push_i16(&mut self, samples: &[i16]) {
        self.data.extend_from_slice(samples);
    }

    pub fn take_pcm_bytes(&mut self) -> Vec<u8> {
        let bytes: Vec<u8> = self.data.iter()
            .flat_map(|&s| s.to_le_bytes())
            .collect();
        self.data.clear();
        bytes
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn duration_secs(&self) -> f32 {
        self.data.len() as f32 / self.sample_rate as f32
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn clear(&mut self) {
        self.data.clear();
        self.realtime_cursor = 0;
    }

    pub fn start_capture(&mut self) {
        self.clear();
        self.capturing = true;
    }

    pub fn stop_capture(&mut self) {
        self.capturing = false;
    }

    pub fn is_capturing(&self) -> bool {
        self.capturing
    }

    /// Return a slice of the raw i16 audio data (for RMS, no copy).
    pub fn as_i16_slice(&self) -> &[i16] {
        &self.data
    }

    /// Take samples accumulated since the last realtime read (non-destructive:
    /// the full buffer is retained for HTTP-batch fallback). Advances the
    /// realtime cursor.
    pub fn take_recent_samples(&mut self) -> Vec<i16> {
        let recent = self.data[self.realtime_cursor..].to_vec();
        self.realtime_cursor = self.data.len();
        recent
    }
}

/// Calculate RMS (Root Mean Square) of i16 PCM data.
/// Returns value in [0.0, 1.0].
pub fn calculate_rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter()
        .map(|&s| {
            let f = s as f64 / 32768.0;
            f * f
        })
        .sum();
    (sum_sq / samples.len() as f64).sqrt() as f32
}

/// Compute per-bar heights from RMS level.
/// `weights` is [0.5, 0.8, 1.0, 0.75, 0.55].
/// Returns bar heights in [0.0, 1.0].
pub fn compute_bar_heights(rms: f32, weights: &[f32], smooth: &mut [f32]) -> Vec<f32> {
    const ATTACK: f32 = 0.4;
    const RELEASE: f32 = 0.15;

    weights.iter().enumerate().map(|(i, &w)| {
        let raw = rms * w;
        let jitter = (rand_val(i) - 0.5) * 0.08;
        let target = raw + jitter;
        let coeff = if target > smooth[i] { ATTACK } else { RELEASE };
        smooth[i] += (target - smooth[i]) * coeff;
        smooth[i].clamp(0.0, 1.0)
    }).collect()
}

fn rand_val(seed: usize) -> f32 {
    let x = (seed as u64).wrapping_mul(6364136223846793005).wrapping_add(1);
    (x >> 33) as f32 / (1u64 << 31) as f32
}

/// Resample f32 audio from orig_sr to target_sr using linear interpolation.
pub fn resample(samples: &[f32], orig_sr: u32, target_sr: u32) -> Vec<f32> {
    if orig_sr == target_sr || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = target_sr as f64 / orig_sr as f64;
    let new_len = (samples.len() as f64 * ratio) as usize;
    let mut out = Vec::with_capacity(new_len);
    for i in 0..new_len {
        let src_pos = i as f64 / ratio;
        let idx = src_pos as usize;
        let frac = src_pos - idx as f64;
        let s0 = samples.get(idx).copied().unwrap_or(0.0);
        let s1 = samples.get(idx + 1).copied().unwrap_or(0.0);
        out.push(s0 + (s1 - s0) * frac as f32);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_push_i16_and_take() {
        let mut buf = AudioBuffer::new(16000);
        buf.push_i16(&[100, 200, 300]);
        assert_eq!(buf.len(), 3);
        let bytes = buf.take_pcm_bytes();
        assert_eq!(bytes.len(), 6);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_buffer_push_f32() {
        let mut buf = AudioBuffer::new(16000);
        buf.push_f32(&[0.0, 1.0, -1.0, 0.5]);
        assert_eq!(buf.len(), 4);
        let bytes = buf.take_pcm_bytes();
        assert_eq!(bytes.len(), 8);
    }

    #[test]
    fn test_buffer_duration() {
        let mut buf = AudioBuffer::new(16000);
        buf.push_i16(&[0i16; 16000]);
        assert!((buf.duration_secs() - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_rms_silence() {
        let samples = [0i16; 1600];
        assert_eq!(calculate_rms(&samples), 0.0);
    }

    #[test]
    fn test_rms_full_scale() {
        let samples = [32767i16; 100];
        let rms = calculate_rms(&samples);
        assert!((rms - 0.9999).abs() < 0.01);
    }

    #[test]
    fn test_rms_known_sine() {
        let samples: Vec<i16> = (0..16000)
            .map(|i| ((i as f32 / 16000.0 * 2.0 * std::f32::consts::PI).sin() * 32767.0) as i16)
            .collect();
        let rms = calculate_rms(&samples);
        assert!((rms - 0.707).abs() < 0.05);
    }

    #[test]
    fn test_rms_empty() {
        assert_eq!(calculate_rms(&[]), 0.0);
    }

    #[test]
    fn test_bar_heights_with_zero_rms() {
        let mut smooth = [0.0; 5];
        let weights = [0.5, 0.8, 1.0, 0.75, 0.55];
        let bars = compute_bar_heights(0.0, &weights, &mut smooth);
        assert_eq!(bars.len(), 5);
        for b in &bars {
            assert!(b.abs() < 0.1);
        }
    }

    #[test]
    fn test_bar_heights_weight_proportions() {
        let mut smooth = [0.0; 5];
        let weights = [0.5, 0.8, 1.0, 0.75, 0.55];
        let bars = compute_bar_heights(0.8, &weights, &mut smooth);
        assert!(bars[2] >= bars[0]);
        assert!(bars[2] >= bars[4]);
    }

    #[test]
    fn test_resample_same_rate() {
        let samples = vec![1.0f32, 2.0, 3.0];
        let result = resample(&samples, 16000, 16000);
        assert_eq!(result, samples);
    }

    #[test]
    fn test_resample_48k_to_16k() {
        let samples = vec![0.0f32; 48000];
        let result = resample(&samples, 48000, 16000);
        assert_eq!(result.len(), 16000);
    }

    #[test]
    fn test_resample_44100_to_16k() {
        let samples = vec![0.5f32; 44100];
        let result = resample(&samples, 44100, 16000);
        assert_eq!(result.len(), 16000);
    }

    #[test]
    fn test_resample_empty() {
        let result = resample(&[], 48000, 16000);
        assert!(result.is_empty());
    }

    #[test]
    fn test_buffer_take_clears() {
        let mut buf = AudioBuffer::new(16000);
        buf.push_i16(&[1, 2, 3]);
        let _ = buf.take_pcm_bytes();
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_take_recent_samples_incremental() {
        let mut buf = AudioBuffer::new(16000);
        buf.push_i16(&[1, 2, 3]);
        assert_eq!(buf.take_recent_samples(), vec![1, 2, 3]);
        // buffer not cleared by realtime read
        assert_eq!(buf.len(), 3);
        buf.push_i16(&[4, 5]);
        // only the new samples since last read
        assert_eq!(buf.take_recent_samples(), vec![4, 5]);
        // full buffer still available for batch fallback
        assert_eq!(buf.as_i16_slice(), &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_clear_resets_realtime_cursor() {
        let mut buf = AudioBuffer::new(16000);
        buf.push_i16(&[1, 2, 3]);
        let _ = buf.take_recent_samples();
        buf.clear();
        buf.push_i16(&[9]);
        assert_eq!(buf.take_recent_samples(), vec![9]);
    }
}
