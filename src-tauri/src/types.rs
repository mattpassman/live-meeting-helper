//! Shared lightweight types used across modules.
//! These have no heavy dependencies (no Tauri, cpal, AWS SDK).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioSource {
    Microphone,
    SystemAudio,
    Both,
}

impl std::fmt::Display for AudioSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Microphone => write!(f, "Microphone"),
            Self::SystemAudio => write!(f, "SystemAudio"),
            Self::Both => write!(f, "Both"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Idle,
    Active,
    Paused,
    Completed,
    Error,
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "Idle"),
            Self::Active => write!(f, "Active"),
            Self::Paused => write!(f, "Paused"),
            Self::Completed => write!(f, "Completed"),
            Self::Error => write!(f, "Error"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub text: String,
    pub speaker: Option<String>,
    pub start_time_ms: u64,
    pub end_time_ms: u64,
    pub confidence: f32,
    pub is_final: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SilenceEvent {
    pub start_time_ms: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
pub enum TranscriptionEvent {
    Segment(TranscriptSegment),
    Silence(SilenceEvent),
}
