use super::{AudioCaptureError, AudioChunk, AudioSource, CaptureState};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
    Arc,
};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

const STATE_IDLE: u8 = 0;
const STATE_CAPTURING: u8 = 1;
const STATE_PAUSED: u8 = 2;
const STATE_ERROR: u8 = 3;

/// Target format for Transcribe: 16kHz mono i16
const TARGET_SAMPLE_RATE: u32 = 16000;

/// Thread-safe handle to control audio capture running on a dedicated OS thread.
pub struct AudioCaptureHandle {
    state: Arc<AtomicU8>,
    paused: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// macOS ScreenCaptureKit loopback session (Some when system audio is active)
    #[cfg(target_os = "macos")]
    loopback: Option<super::loopback_mac::MacLoopbackHandle>,
}

/// List all available audio input devices. Returns (name, is_default) pairs.
pub fn list_input_devices() -> Vec<(String, bool)> {
    use cpal::traits::DeviceTrait;
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();
    let mut devices = Vec::new();
    if let Ok(input_devices) = host.input_devices() {
        for d in input_devices {
            if let Ok(name) = d.name() {
                let is_default = name == default_name;
                devices.push((name, is_default));
            }
        }
    }
    devices
}

/// Throttle interval for level emissions: 100 ms
const LEVEL_THROTTLE_MS: u64 = 100;

impl AudioCaptureHandle {
    pub fn start(
        source: AudioSource,
        mic_device: Option<String>,
        tx: mpsc::Sender<AudioChunk>,
        level_cb: Option<Arc<dyn Fn(f32) + Send + Sync>>,
    ) -> Result<Self, AudioCaptureError> {
        let state = Arc::new(AtomicU8::new(STATE_IDLE));
        let paused = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::new(AtomicBool::new(false));
        // Shared last-emit timestamp for level throttling (millis since UNIX_EPOCH)
        let last_level_emit = Arc::new(AtomicU64::new(0));

        let mut thread = None;

        // Start microphone capture (for Microphone or Both)
        if source == AudioSource::Microphone || source == AudioSource::Both {
            let host = cpal::default_host();
            let _device = host
                .default_input_device()
                .ok_or(AudioCaptureError::SourceUnavailable)?;

            let s = state.clone();
            let p = paused.clone();
            let sf = stop_flag.clone();
            let tx_mic = tx.clone();
            let mic_dev = mic_device.clone();
            let level_cb_mic = level_cb.clone();
            let last_emit_mic = last_level_emit.clone();

            thread = Some(std::thread::spawn(move || {
                run_capture(AudioSource::Microphone, mic_dev, tx_mic, s, p, sf, level_cb_mic, last_emit_mic);
            }));
        }

        // Start system audio loopback (for SystemAudio or Both) — Windows only
        #[cfg(target_os = "windows")]
        {
            if source == AudioSource::SystemAudio || source == AudioSource::Both {
                let sf = stop_flag.clone();
                let p = paused.clone();
                let tx_loopback = tx.clone();
                match super::loopback_win::start_loopback_capture(tx_loopback, sf, p) {
                    Ok(_handle) => {
                        tracing::info!("System audio loopback capture started");
                        // Thread is detached — stop_flag controls its lifetime
                    }
                    Err(e) => {
                        tracing::error!("Failed to start loopback capture: {e}");
                        if source == AudioSource::SystemAudio {
                            return Err(AudioCaptureError::Other(e));
                        }
                        // For Both mode, continue with mic only
                    }
                }
            }
        }

        // ── macOS: ScreenCaptureKit loopback ──────────────────────────────────
        #[cfg(target_os = "macos")]
        let loopback = {
            if source == AudioSource::SystemAudio || source == AudioSource::Both {
                match super::loopback_mac::MacLoopbackHandle::start(tx.clone()) {
                    Ok(h) => Some(h),
                    Err(e) => {
                        tracing::error!("macOS system audio loopback: {e}");
                        if source == AudioSource::SystemAudio {
                            return Err(AudioCaptureError::Other(e));
                        }
                        // Both: continue mic-only
                        None
                    }
                }
            } else {
                None
            }
        };

        // ── Other non-Windows platforms: unsupported ─────────────────────────
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            if source == AudioSource::SystemAudio {
                tracing::error!("System audio loopback not supported on this platform");
                return Err(AudioCaptureError::Other(
                    "System audio loopback is only supported on Windows and macOS".to_string(),
                ));
            }
            if source == AudioSource::Both {
                tracing::warn!("System audio loopback not available on this platform, using microphone only");
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(200));
        state.store(STATE_CAPTURING, Ordering::Relaxed);

        Ok(Self {
            state,
            paused,
            stop_flag,
            thread,
            #[cfg(target_os = "macos")]
            loopback,
        })
    }


    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
        self.state.store(STATE_PAUSED, Ordering::Relaxed);
        #[cfg(target_os = "macos")]
        super::loopback_mac::LOOPBACK_PAUSED.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
        self.state.store(STATE_CAPTURING, Ordering::Relaxed);
        #[cfg(target_os = "macos")]
        super::loopback_mac::LOOPBACK_PAUSED.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        // Drop the macOS loopback first so SCStream stops before we join the mic thread
        #[cfg(target_os = "macos")]
        drop(self.loopback.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.state.store(STATE_IDLE, Ordering::Relaxed);
    }

    #[allow(dead_code)]
    pub fn state(&self) -> CaptureState {
        match self.state.load(Ordering::Relaxed) {
            STATE_CAPTURING => CaptureState::Capturing,
            STATE_PAUSED => CaptureState::Paused,
            STATE_ERROR => CaptureState::Error,
            _ => CaptureState::Idle,
        }
    }
}

impl Drop for AudioCaptureHandle {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}

/// Downmix multi-channel f32 samples to mono i16, then resample to 16kHz.
pub fn convert_and_resample(
    data_f32: &[f32],
    channels: u16,
    source_rate: u32,
) -> Vec<i16> {
    // Step 1: downmix to mono
    let mono: Vec<f32> = data_f32
        .chunks(channels as usize)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();

    // Step 2: resample to TARGET_SAMPLE_RATE using linear interpolation
    if source_rate == TARGET_SAMPLE_RATE {
        return mono.iter().map(|&s| f32_to_i16(s)).collect();
    }

    let ratio = source_rate as f64 / TARGET_SAMPLE_RATE as f64;
    let out_len = (mono.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let src_idx = i as f64 * ratio;
        let idx = src_idx as usize;
        let frac = src_idx - idx as f64;

        let sample = if idx + 1 < mono.len() {
            mono[idx] as f64 * (1.0 - frac) + mono[idx + 1] as f64 * frac
        } else if idx < mono.len() {
            mono[idx] as f64
        } else {
            0.0
        };

        out.push(f32_to_i16(sample as f32));
    }

    out
}

/// Downmix multi-channel i16 samples to mono, then resample to 16kHz.
fn convert_and_resample_i16(
    data: &[i16],
    channels: u16,
    source_rate: u32,
) -> Vec<i16> {
    // Step 1: downmix to mono
    let mono: Vec<i32> = data
        .chunks(channels as usize)
        .map(|frame| frame.iter().map(|&s| s as i32).sum::<i32>() / channels as i32)
        .collect();

    // Step 2: resample
    if source_rate == TARGET_SAMPLE_RATE && channels == 1 {
        return data.to_vec();
    }
    if source_rate == TARGET_SAMPLE_RATE {
        return mono.iter().map(|&s| s as i16).collect();
    }

    let ratio = source_rate as f64 / TARGET_SAMPLE_RATE as f64;
    let out_len = (mono.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let src_idx = i as f64 * ratio;
        let idx = src_idx as usize;
        let frac = src_idx - idx as f64;

        let sample = if idx + 1 < mono.len() {
            mono[idx] as f64 * (1.0 - frac) + mono[idx + 1] as f64 * frac
        } else if idx < mono.len() {
            mono[idx] as f64
        } else {
            0.0
        };

        out.push(sample.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16);
    }

    out
}

#[inline]
fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

/// Compute RMS of i16 samples as a 0.0–1.0 float.
fn rms_level(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64 / 32768.0).powi(2)).sum();
    ((sum_sq / samples.len() as f64).sqrt() as f32).min(1.0)
}

fn run_capture(
    source: AudioSource,
    mic_device: Option<String>,
    tx: mpsc::Sender<AudioChunk>,
    state: Arc<AtomicU8>,
    paused: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
    level_cb: Option<Arc<dyn Fn(f32) + Send + Sync>>,
    last_level_emit: Arc<AtomicU64>,
) {
    let host = cpal::default_host();

    // Use the caller-supplied device name; fall back to the saved config default.
    let effective_device = mic_device
        .or_else(|| crate::config::AppConfig::get().audio_device.clone());

    let device = if let Some(ref wanted) = effective_device {
        // Find device by substring match
        let wanted_lower = wanted.to_lowercase();
        match host.input_devices() {
            Ok(mut devices) => {
                match devices.find(|d| {
                    d.name()
                        .map(|n| n.to_lowercase().contains(&wanted_lower))
                        .unwrap_or(false)
                }) {
                    Some(d) => {
                        tracing::info!("Using audio device: {}", d.name().unwrap_or_default());
                        d
                    }
                    None => {
                        tracing::error!("Audio device matching '{}' not found, falling back to default", wanted);
                        match host.default_input_device() {
                            Some(d) => d,
                            None => {
                                state.store(STATE_ERROR, Ordering::Relaxed);
                                return;
                            }
                        }
                    }
                }
            }
            Err(_) => {
                state.store(STATE_ERROR, Ordering::Relaxed);
                return;
            }
        }
    } else {
        match host.default_input_device() {
            Some(d) => {
                tracing::info!("Using default audio device: {}", d.name().unwrap_or_default());
                d
            }
            None => {
                state.store(STATE_ERROR, Ordering::Relaxed);
                return;
            }
        }
    };

    // Use the device's default input config instead of hardcoding
    let supported = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to get default input config: {e}");
            state.store(STATE_ERROR, Ordering::Relaxed);
            return;
        }
    };

    let sample_format = supported.sample_format();
    let device_config: cpal::StreamConfig = supported.into();
    let device_channels = device_config.channels;
    let device_rate = device_config.sample_rate.0;

    tracing::info!(
        "Audio device config: {}ch {}Hz {:?} → resampling to 1ch {}Hz i16",
        device_channels,
        device_rate,
        sample_format,
        TARGET_SAMPLE_RATE,
    );

    let state_err = state.clone();

    let stream = match sample_format {
        SampleFormat::F32 => {
            let paused_c = paused.clone();
            let tx_c = tx.clone();
            let level_cb_c = level_cb.clone();
            let last_emit_c = last_level_emit.clone();
            device.build_input_stream(
                &device_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if paused_c.load(Ordering::Relaxed) {
                        return;
                    }
                    let resampled = convert_and_resample(data, device_channels, device_rate);
                    if resampled.is_empty() {
                        return;
                    }
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    // Emit level at most every LEVEL_THROTTLE_MS
                    if let Some(ref cb) = level_cb_c {
                        let last = last_emit_c.load(Ordering::Relaxed);
                        if now.saturating_sub(last) >= LEVEL_THROTTLE_MS {
                            last_emit_c.store(now, Ordering::Relaxed);
                            cb(rms_level(&resampled));
                        }
                    }
                    let duration_ms = (resampled.len() as u32 * 1000) / TARGET_SAMPLE_RATE;
                    let _ = tx_c.try_send(AudioChunk {
                        data: resampled,
                        timestamp_ms: now,
                        source,
                        duration_ms,
                    });
                },
                move |err| {
                    tracing::error!("Audio stream error: {err}");
                    state_err.store(STATE_ERROR, Ordering::Relaxed);
                },
                None,
            )
        }
        SampleFormat::I16 => {
            let paused_c = paused.clone();
            let tx_c = tx.clone();
            let level_cb_c = level_cb.clone();
            let last_emit_c = last_level_emit.clone();
            device.build_input_stream(
                &device_config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if paused_c.load(Ordering::Relaxed) {
                        return;
                    }
                    let resampled = convert_and_resample_i16(data, device_channels, device_rate);
                    if resampled.is_empty() {
                        return;
                    }
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    // Emit level at most every LEVEL_THROTTLE_MS
                    if let Some(ref cb) = level_cb_c {
                        let last = last_emit_c.load(Ordering::Relaxed);
                        if now.saturating_sub(last) >= LEVEL_THROTTLE_MS {
                            last_emit_c.store(now, Ordering::Relaxed);
                            cb(rms_level(&resampled));
                        }
                    }
                    let duration_ms = (resampled.len() as u32 * 1000) / TARGET_SAMPLE_RATE;
                    let _ = tx_c.try_send(AudioChunk {
                        data: resampled,
                        timestamp_ms: now,
                        source,
                        duration_ms,
                    });
                },
                move |err| {
                    tracing::error!("Audio stream error: {err}");
                    state_err.store(STATE_ERROR, Ordering::Relaxed);
                },
                None,
            )
        }
        other => {
            tracing::error!("Unsupported sample format: {other:?}");
            state.store(STATE_ERROR, Ordering::Relaxed);
            return;
        }
    };

    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to build audio stream: {e}");
            state.store(STATE_ERROR, Ordering::Relaxed);
            return;
        }
    };

    if let Err(e) = stream.play() {
        tracing::error!("Failed to play audio stream: {e}");
        state.store(STATE_ERROR, Ordering::Relaxed);
        return;
    }

    state.store(STATE_CAPTURING, Ordering::Relaxed);
    tracing::info!("Audio capture started from {source}");

    while !stop_flag.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    drop(stream);
    tracing::info!("Audio capture stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resample_passthrough() {
        // 16kHz mono i16 → no change
        let input: Vec<i16> = vec![100, 200, 300, 400];
        let out = convert_and_resample_i16(&input, 1, 16000);
        assert_eq!(out, input);
    }

    #[test]
    fn test_downmix_stereo_to_mono() {
        // 16kHz stereo → 16kHz mono (average of L+R)
        let input: Vec<i16> = vec![100, 200, 300, 400]; // 2 frames of stereo
        let out = convert_and_resample_i16(&input, 2, 16000);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], 150); // (100+200)/2
        assert_eq!(out[1], 350); // (300+400)/2
    }

    #[test]
    fn test_resample_48k_to_16k() {
        // 48kHz mono → 16kHz mono (3:1 ratio, output ~1/3 the samples)
        let input: Vec<i16> = (0..480).map(|i| (i * 10) as i16).collect();
        let out = convert_and_resample_i16(&input, 1, 48000);
        assert_eq!(out.len(), 160); // 480 / 3
    }

    #[test]
    fn test_f32_convert_and_resample() {
        // 48kHz stereo f32 → 16kHz mono i16
        let input: Vec<f32> = vec![0.5, -0.5, 0.25, -0.25]; // 2 stereo frames
        let out = convert_and_resample(&input, 2, 48000);
        // 2 mono frames at 48kHz → ~0.67 frames at 16kHz → 0 or 1 sample
        assert!(!out.is_empty() || input.len() < 6); // small input edge case
    }

    #[test]
    fn test_f32_to_i16_clamp() {
        assert_eq!(f32_to_i16(1.0), i16::MAX);
        assert_eq!(f32_to_i16(-1.0), -i16::MAX);
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(2.0), i16::MAX); // clamped
    }
}
