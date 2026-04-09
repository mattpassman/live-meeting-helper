use super::{TranscriptionError, TranscriptionEvent};
#[cfg(feature = "whisper")]
use super::{SilenceEvent, TranscriptSegment};
use crate::audio::AudioChunk;
use tokio::sync::mpsc;

#[cfg(feature = "whisper")]
/// How many samples to accumulate before running Whisper inference.
/// At 16 kHz this is 30 seconds of audio — a safe upper bound before forcing inference.
const MAX_BUFFER_SAMPLES: usize = 16_000 * 30;

#[cfg(feature = "whisper")]
/// Minimum number of samples required before we'll run inference (0.5 s).
/// Prevents running Whisper on tiny spurious audio blips.
const MIN_SPEECH_SAMPLES: usize = 16_000 / 2;

#[cfg(feature = "whisper")]
/// Number of trailing samples to check for silence (last 0.8 s).
const SILENCE_WINDOW: usize = 16_000 * 8 / 10;

#[cfg(feature = "whisper")]
/// RMS amplitude threshold below which audio is considered silent.
const SILENCE_THRESHOLD: f32 = 0.015;

#[cfg(feature = "whisper")]
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

#[allow(dead_code)]
pub struct WhisperService {
    pub model_path: String,
    session_start_ms: u64,
    last_speech_ms: u64,
    segment_counter: u64,
}

impl WhisperService {
    pub fn new(model_path: String) -> Self {
        let now = now_ms();
        Self {
            model_path,
            session_start_ms: now,
            last_speech_ms: now,
            segment_counter: 0,
        }
    }

    pub async fn run(
        &mut self,
        audio_rx: mpsc::Receiver<AudioChunk>,
        event_tx: mpsc::Sender<TranscriptionEvent>,
    ) -> Result<(), TranscriptionError> {
        #[cfg(not(feature = "whisper"))]
        {
            drop(audio_rx);
            drop(event_tx);
            return Err(TranscriptionError::NotAvailable(
                "Local Whisper transcription is not compiled in. \
                 Rebuild with `--features whisper` (requires cmake and a C++ compiler)."
                    .into(),
            ));
        }

        #[cfg(feature = "whisper")]
        self.run_whisper(audio_rx, event_tx).await
    }

    #[cfg(feature = "whisper")]
    async fn run_whisper(
        &mut self,
        mut audio_rx: mpsc::Receiver<AudioChunk>,
        event_tx: mpsc::Sender<TranscriptionEvent>,
    ) -> Result<(), TranscriptionError> {
        let model_path = self.model_path.clone();

        tracing::info!("Loading Whisper model from {model_path}");
        // Load model in a blocking thread — it's a large file read
        let ctx = tokio::task::spawn_blocking(move || {
            WhisperContext::new_with_params(&model_path, WhisperContextParameters::default())
        })
        .await
        .map_err(|e| TranscriptionError::StreamStart(format!("Whisper load task failed: {e}")))?
        .map_err(|e| {
            TranscriptionError::StreamStart(format!("Failed to load Whisper model: {e}"))
        })?;

        let ctx = std::sync::Arc::new(ctx);
        tracing::info!("Whisper model loaded");

        let mut audio_buffer: Vec<f32> = Vec::with_capacity(MAX_BUFFER_SAMPLES);
        // Offset into the session timeline for this buffer's start (in ms)
        let mut buffer_start_ms = self.session_start_ms;

        loop {
            // Collect up to 50 ms worth of audio non-blockingly before deciding
            let chunk = tokio::time::timeout(
                tokio::time::Duration::from_millis(50),
                audio_rx.recv(),
            )
            .await;

            match chunk {
                Ok(Some(c)) => {
                    // Convert i16 PCM → f32 in [-1.0, 1.0]
                    audio_buffer.extend(c.data.iter().map(|&s| s as f32 / 32_768.0));
                }
                Ok(None) => {
                    // Channel closed — process any remaining audio and exit
                    if audio_buffer.len() >= MIN_SPEECH_SAMPLES {
                        let segments = self
                            .transcribe(&ctx, &audio_buffer, buffer_start_ms)
                            .await;
                        for seg in segments {
                            self.last_speech_ms = seg.end_time_ms;
                            let _ = event_tx.send(TranscriptionEvent::Segment(seg)).await;
                        }
                    }
                    break;
                }
                Err(_) => {
                    // Timeout — no new audio; check silence / max buffer conditions below
                }
            }

            if audio_buffer.is_empty() {
                continue;
            }

            let should_process = audio_buffer.len() >= MAX_BUFFER_SAMPLES
                || (audio_buffer.len() >= MIN_SPEECH_SAMPLES && is_trailing_silent(&audio_buffer));

            if should_process {
                let segments = self
                    .transcribe(&ctx, &audio_buffer, buffer_start_ms)
                    .await;

                let emitted_any = !segments.is_empty();
                for seg in segments {
                    self.last_speech_ms = seg.end_time_ms;
                    let _ = event_tx.send(TranscriptionEvent::Segment(seg)).await;
                }

                // After processing emit a silence event so the note generator can trigger
                if emitted_any {
                    let now = now_ms();
                    let _ = event_tx
                        .send(TranscriptionEvent::Silence(SilenceEvent {
                            start_time_ms: self.last_speech_ms,
                            duration_ms: now.saturating_sub(self.last_speech_ms),
                        }))
                        .await;
                }

                // Advance buffer start to now and clear
                buffer_start_ms = now_ms();
                audio_buffer.clear();
            }

            // If we've been silent a long time without any buffered speech, just update clock
            let now = now_ms();
            if now - self.last_speech_ms > 10_000 && audio_buffer.is_empty() {
                let _ = event_tx
                    .send(TranscriptionEvent::Silence(SilenceEvent {
                        start_time_ms: self.last_speech_ms,
                        duration_ms: now - self.last_speech_ms,
                    }))
                    .await;
                self.last_speech_ms = now;
            }
        }

        tracing::info!("Whisper transcription service ended");
        Ok(())
    }

    #[cfg(feature = "whisper")]
    async fn transcribe(
        &mut self,
        ctx: &std::sync::Arc<WhisperContext>,
        samples: &[f32],
        buffer_start_ms: u64,
    ) -> Vec<TranscriptSegment> {
        let ctx = std::sync::Arc::clone(ctx);
        let samples = samples.to_vec();
        let base_ms = buffer_start_ms;
        let mut counter = self.segment_counter;

        let result = tokio::task::spawn_blocking(move || {
            let mut state = ctx.create_state().map_err(|e| format!("{e}"))?;
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_language(Some("en"));
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(true);
            params.set_token_timestamps(false);
            // Suppress blank and no-speech tokens that whisper sometimes emits
            params.set_suppress_blank(true);
            params.set_no_speech_thold(0.6);

            state.full(params, &samples).map_err(|e| format!("{e}"))?;

            // whisper-rs 0.16 API: full_n_segments() returns c_int directly (no Result)
            let n = state.full_n_segments();
            let mut segments = Vec::with_capacity(n as usize);
            for i in 0..n {
                let seg = match state.get_segment(i) {
                    Some(s) => s,
                    None => continue,
                };
                let text = match seg.to_str_lossy() {
                    Ok(t) => t.trim().to_string(),
                    Err(_) => continue,
                };
                if text.is_empty() || text == "[BLANK_AUDIO]" || text == "(silence)" {
                    continue;
                }
                // Whisper timestamps are in 10 ms increments
                let t0 = seg.start_timestamp() as u64 * 10;
                let t1 = seg.end_timestamp() as u64 * 10;
                counter += 1;
                segments.push(TranscriptSegment {
                    id: format!("seg-{counter}"),
                    text,
                    speaker: None, // Whisper doesn't provide speaker diarization
                    start_time_ms: base_ms + t0,
                    end_time_ms: base_ms + t1,
                    confidence: 0.9,
                    is_final: true,
                });
            }
            Ok::<_, String>(segments)
        })
        .await;

        match result {
            Ok(Ok(segs)) => {
                self.segment_counter = counter.max(
                    segs.last()
                        .map(|s| s.id.trim_start_matches("seg-").parse::<u64>().unwrap_or(0))
                        .unwrap_or(self.segment_counter),
                );
                segs
            }
            Ok(Err(e)) => {
                tracing::error!("Whisper inference error: {e}");
                vec![]
            }
            Err(e) => {
                tracing::error!("Whisper task panicked: {e}");
                vec![]
            }
        }
    }
}

/// Returns true if the trailing SILENCE_WINDOW samples are below the silence threshold.
#[cfg(feature = "whisper")]
fn is_trailing_silent(samples: &[f32]) -> bool {
    if samples.len() < SILENCE_WINDOW {
        return false;
    }
    let window = &samples[samples.len() - SILENCE_WINDOW..];
    let rms = (window.iter().map(|&s| s * s).sum::<f32>() / window.len() as f32).sqrt();
    rms < SILENCE_THRESHOLD
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
