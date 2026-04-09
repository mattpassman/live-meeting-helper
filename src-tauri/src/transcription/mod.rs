pub mod service;
pub mod whisper_service;

pub use crate::types::{TranscriptSegment, SilenceEvent, TranscriptionEvent};

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum TranscriptionError {
    #[error("failed to start transcription stream: {0}")]
    StreamStart(String),
    #[error("transcription stream error: {0}")]
    StreamError(String),
    #[error("AWS credentials error: {0}")]
    Credentials(String),
    #[error("transcription backend not available: {0}")]
    NotAvailable(String),
}
