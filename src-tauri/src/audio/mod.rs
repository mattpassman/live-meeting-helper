pub mod capture;
#[cfg(target_os = "windows")]
pub mod loopback_win;
#[cfg(target_os = "macos")]
pub mod loopback_mac;

pub use crate::types::AudioSource;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AudioChunk {
    pub data: Vec<i16>,
    pub timestamp_ms: u64,
    pub source: AudioSource,
    pub duration_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum CaptureState {
    Idle,
    Capturing,
    Paused,
    Error,
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum AudioCaptureError {
    #[error("audio source unavailable")]
    SourceUnavailable,
    #[error("permission denied for audio capture")]
    PermissionDenied,
    #[error("audio device lost during capture")]
    DeviceLost,
    #[error("capture error: {0}")]
    Other(String),
}
