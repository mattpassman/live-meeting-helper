use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

use super::{AudioChunk, AudioSource};

// ── FFI declarations (compiled from loopback_mac.swift via build.rs) ────────

extern "C" {
    /// Returns true on success. On failure writes a UTF-8 error into err_buf.
    fn mac_loopback_start(
        callback: extern "C" fn(*const i16, i32, u64),
        err_buf: *mut std::os::raw::c_char,
        err_len: i32,
    ) -> bool;
    fn mac_loopback_stop();
}

// ── Global channel sender (set on start, cleared on stop) ───────────────────

static LOOPBACK_TX: OnceLock<Mutex<Option<mpsc::Sender<AudioChunk>>>> = OnceLock::new();
pub static LOOPBACK_PAUSED: AtomicBool = AtomicBool::new(false);

// ── Global level callback for the VU meter ───────────────────────────────────

type LevelCb = Arc<dyn Fn(f32) + Send + Sync>;
static LOOPBACK_LEVEL_CB: OnceLock<Mutex<Option<LevelCb>>> = OnceLock::new();
// Last level-emit timestamp (ms since epoch) for throttling
static LOOPBACK_LAST_LEVEL_EMIT: AtomicU64 = AtomicU64::new(0);
const LEVEL_THROTTLE_MS: u64 = 100;

use std::sync::Arc;

extern "C" fn audio_callback(data: *const i16, count: i32, timestamp_ms: u64) {
    if LOOPBACK_PAUSED.load(Ordering::Relaxed) {
        return;
    }
    if data.is_null() || count <= 0 {
        return;
    }

    let samples: Vec<i16> =
        unsafe { std::slice::from_raw_parts(data, count as usize).to_vec() };
    let duration_ms = (samples.len() as u32 * 1000) / 16_000;

    // Emit level to the VU meter callback, throttled to LEVEL_THROTTLE_MS
    if let Some(global) = LOOPBACK_LEVEL_CB.get() {
        if let Ok(guard) = global.lock() {
            if let Some(ref cb) = *guard {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let last = LOOPBACK_LAST_LEVEL_EMIT.load(Ordering::Relaxed);
                if now.saturating_sub(last) >= LEVEL_THROTTLE_MS {
                    LOOPBACK_LAST_LEVEL_EMIT.store(now, Ordering::Relaxed);
                    cb(rms_level(&samples));
                }
            }
        }
    }

    let chunk = AudioChunk {
        data: samples,
        timestamp_ms,
        source: AudioSource::SystemAudio,
        duration_ms,
    };

    // LOOPBACK_TX.get() is a fast pointer load after initialisation
    if let Some(global) = LOOPBACK_TX.get() {
        if let Ok(guard) = global.lock() {
            if let Some(ref tx) = *guard {
                let _ = tx.try_send(chunk);
            }
        }
    }
}

/// Compute RMS of i16 samples as a 0.0–1.0 float.
fn rms_level(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64 / 32768.0).powi(2)).sum();
    ((sum_sq / samples.len() as f64).sqrt() as f32).min(1.0)
}

// ── Public handle ────────────────────────────────────────────────────────────

/// Holds the ScreenCaptureKit session. Dropped when the meeting ends.
pub struct MacLoopbackHandle;

impl MacLoopbackHandle {
    pub fn start(
        tx: mpsc::Sender<AudioChunk>,
        level_cb: Option<Arc<dyn Fn(f32) + Send + Sync>>,
    ) -> Result<Self, String> {
        let global = LOOPBACK_TX.get_or_init(|| Mutex::new(None));
        *global.lock().unwrap() = Some(tx);
        LOOPBACK_PAUSED.store(false, Ordering::Relaxed);

        // Store the level callback for use in audio_callback
        let level_global = LOOPBACK_LEVEL_CB.get_or_init(|| Mutex::new(None));
        *level_global.lock().unwrap() = level_cb;
        LOOPBACK_LAST_LEVEL_EMIT.store(0, Ordering::Relaxed);

        let mut err_buf = [0i8; 512];
        let ok = unsafe {
            mac_loopback_start(audio_callback, err_buf.as_mut_ptr(), err_buf.len() as i32)
        };

        if ok {
            tracing::info!("macOS system audio loopback started via ScreenCaptureKit");
            Ok(Self)
        } else {
            *global.lock().unwrap() = None;
            *level_global.lock().unwrap() = None;
            // Surface the actual Swift error into the Rust log
            let msg = unsafe { std::ffi::CStr::from_ptr(err_buf.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            let msg = if msg.is_empty() {
                "Unknown error from ScreenCaptureKit".to_string()
            } else {
                msg
            };
            tracing::error!("mac_loopback_start Swift error: {msg}");
            Err(msg)
        }
    }
}

impl Drop for MacLoopbackHandle {
    fn drop(&mut self) {
        unsafe { mac_loopback_stop() };
        if let Some(global) = LOOPBACK_TX.get() {
            if let Ok(mut g) = global.lock() {
                *g = None;
            }
        }
        // Clear the level callback on stop
        if let Some(global) = LOOPBACK_LEVEL_CB.get() {
            if let Ok(mut g) = global.lock() {
                *g = None;
            }
        }
        tracing::info!("macOS system audio loopback stopped");
    }
}
