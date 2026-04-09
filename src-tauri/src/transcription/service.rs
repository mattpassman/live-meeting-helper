use super::{TranscriptSegment, TranscriptionError, TranscriptionEvent, SilenceEvent};
use crate::audio::AudioChunk;
use aws_sdk_transcribestreaming::{
    self as transcribe,
    types::{AudioEvent, AudioStream, LanguageCode, MediaEncoding, TranscriptResultStream, error::AudioStreamError},
};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

pub struct TranscriptionService {
    session_start_ms: u64,
    last_speech_ms: u64,
    segment_counter: u64,
}

async fn build_transcribe_client(
    profile: &Option<String>,
    region: &Option<String>,
) -> transcribe::Client {
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
    if let Some(ref p) = profile {
        loader = loader.profile_name(p);
    }
    if let Some(ref r) = region {
        loader = loader.region(aws_config::Region::new(r.clone()));
    } else {
        tracing::warn!("No AWS region configured, defaulting to us-east-1");
        loader = loader.region(aws_config::Region::new("us-east-1"));
    }
    let config = loader.load().await;
    tracing::info!("AWS region resolved: {:?}", config.region());
    tracing::debug!("AWS credentials present: {}", config.credentials_provider().is_some());
    transcribe::Client::new(&config)
}

/// Attempt to start the Transcribe stream. Returns the output handle on success.
async fn start_stream(
    client: &transcribe::Client,
    audio_fwd_rx: mpsc::Receiver<Result<AudioStream, AudioStreamError>>,
) -> Result<
    transcribe::operation::start_stream_transcription::StartStreamTranscriptionOutput,
    String,
> {
    client
        .start_stream_transcription()
        .language_code(LanguageCode::EnUs)
        .media_encoding(MediaEncoding::Pcm)
        .media_sample_rate_hertz(16000)
        .enable_partial_results_stabilization(true)
        .show_speaker_label(true)
        .audio_stream(ReceiverStream(audio_fwd_rx).into())
        .send()
        .await
        .map_err(|e| format!("{e:?}"))
}

impl TranscriptionService {
    pub fn new() -> Self {
        Self {
            session_start_ms: 0,
            last_speech_ms: 0,
            segment_counter: 0,
        }
    }

    pub async fn run(
        &mut self,
        audio_rx: mpsc::Receiver<AudioChunk>,
        event_tx: mpsc::Sender<TranscriptionEvent>,
    ) -> Result<(), TranscriptionError> {
        let app_cfg = crate::config::AppConfig::get();

        let profile = app_cfg
            .aws_profile
            .clone()
            .or_else(|| std::env::var("AWS_PROFILE").ok());
        let region = app_cfg
            .aws_region
            .clone()
            .or_else(|| std::env::var("AWS_REGION").ok())
            .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok());

        tracing::info!("AWS profile={:?}, region={:?}", profile, region);

        self.session_start_ms = now_ms();
        self.last_speech_ms = self.session_start_ms;

        let (audio_fwd_tx, audio_fwd_rx) =
            mpsc::channel::<Result<AudioStream, AudioStreamError>>(64);

        // Forward audio chunks to the Transcribe stream
        let audio_rx = std::sync::Arc::new(tokio::sync::Mutex::new(audio_rx));
        let audio_rx_ref = audio_rx.clone();
        tokio::spawn(async move {
            let mut rx = audio_rx_ref.lock().await;
            while let Some(chunk) = rx.recv().await {
                let bytes: Vec<u8> = chunk.data.iter().flat_map(|s| s.to_le_bytes()).collect();
                let audio_event = AudioStream::AudioEvent(
                    AudioEvent::builder()
                        .audio_chunk(aws_sdk_transcribestreaming::primitives::Blob::new(bytes))
                        .build(),
                );
                if audio_fwd_tx.send(Ok(audio_event)).await.is_err() {
                    break;
                }
            }
        });

        let client = build_transcribe_client(&profile, &region).await;

        tracing::info!("Starting Transcribe streaming session...");
        let mut output = start_stream(&client, audio_fwd_rx).await
            .map_err(|msg| TranscriptionError::StreamStart(msg))?;

        tracing::info!("Transcribe stream connected, listening for results...");

        loop {
            match output.transcript_result_stream.recv().await {
                Ok(Some(TranscriptResultStream::TranscriptEvent(event))) => {
                    if let Some(transcript) = event.transcript {
                        for result in transcript.results() {
                            if result.is_partial() {
                                continue;
                            }
                            for alt in result.alternatives() {
                                let text = alt.transcript().unwrap_or_default().to_string();
                                if text.is_empty() {
                                    continue;
                                }
                                let speaker = alt
                                    .items()
                                    .first()
                                    .and_then(|item| item.speaker().map(|s| s.to_string()));

                                self.segment_counter += 1;
                                let segment = TranscriptSegment {
                                    id: format!("seg-{}", self.segment_counter),
                                    text,
                                    speaker,
                                    start_time_ms: (result.start_time() * 1000.0) as u64,
                                    end_time_ms: (result.end_time() * 1000.0) as u64,
                                    confidence: 0.95,
                                    is_final: true,
                                };
                                self.last_speech_ms = now_ms();
                                let _ = event_tx.send(TranscriptionEvent::Segment(segment)).await;
                            }
                        }
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => {
                    let msg = format!("{e:?}");
                    tracing::error!("Transcription stream error: {msg}");
                    break;
                }
            }

            // Check for silence
            let now = now_ms();
            if now - self.last_speech_ms > 10_000 {
                let _ = event_tx
                    .send(TranscriptionEvent::Silence(SilenceEvent {
                        start_time_ms: self.last_speech_ms,
                        duration_ms: now - self.last_speech_ms,
                    }))
                    .await;
                self.last_speech_ms = now;
            }
        }

        tracing::info!("Transcription service ended");
        Ok(())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

struct ReceiverStream(mpsc::Receiver<Result<AudioStream, AudioStreamError>>);

impl futures_core::Stream for ReceiverStream {
    type Item = Result<AudioStream, AudioStreamError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }
}
