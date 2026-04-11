use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use tokio::sync::mpsc;

use super::{AudioChunk, AudioSource};

// ── FFI declarations (compiled from loopback_mac.swift via build.rs) ────────

extern "C" {
    fn mac_loopback_start(callback: extern "C" fn(*const i16, i32, u64)) -> bool;
    fn mac_loopback_stop();
}

// ── Global channel sender (set on start, cleared on stop) ───────────────────

static LOOPBACK_TX: OnceLock<Mutex<Option<mpsc::Sender<AudioChunk>>>> = OnceLock::new();
pub static LOOPBACK_PAUSED: AtomicBool = AtomicBool::new(false);

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

// ── Public handle ────────────────────────────────────────────────────────────

/// Holds the ScreenCaptureKit session. Dropped when the meeting ends.
pub struct MacLoopbackHandle;

impl MacLoopbackHandle {
    pub fn start(tx: mpsc::Sender<AudioChunk>) -> Result<Self, String> {
        let global = LOOPBACK_TX.get_or_init(|| Mutex::new(None));
        *global.lock().unwrap() = Some(tx);
        LOOPBACK_PAUSED.store(false, Ordering::Relaxed);

        let ok = unsafe { mac_loopback_start(audio_callback) };
        if ok {
            tracing::info!("macOS system audio loopback started via ScreenCaptureKit");
            Ok(Self)
        } else {
            *global.lock().unwrap() = None;
            Err(
                "System audio capture failed. \
                 Grant Screen Recording permission in System Settings → Privacy & Security → Screen Recording, \
                 then restart the app."
                    .into(),
            )
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
        tracing::info!("macOS system audio loopback stopped");
    }
}
