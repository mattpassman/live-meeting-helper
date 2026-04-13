use super::{SessionCommand, SessionConfig, SessionEvent, SessionState};
use crate::audio::capture::AudioCaptureHandle;
use crate::audio::AudioChunk;
use crate::notes::generator::NoteGenerator;
use crate::notes::{MeetingNotes, NotesUpdate};
use crate::persistence::{PersistenceService, SessionData};
use crate::transcription::service::TranscriptionService;
use crate::transcription::whisper_service::WhisperService;
use crate::types::{TranscriptSegment, TranscriptionEvent};
use tokio::sync::mpsc;
use std::sync::Arc;
use tokio::sync::Mutex;
use tauri::ipc::Channel;

pub struct SessionManager {
    cmd_rx: mpsc::Receiver<SessionCommand>,
    instruction_rx: mpsc::Receiver<String>,
    state: SessionState,
    persistence: PersistenceService,
    active_session: Option<ActiveSession>,
    event_callback: Box<dyn Fn(SessionEvent) + Send>,
    instruction_fwd_tx: Option<mpsc::Sender<String>>,
    shared_title: Arc<Mutex<String>>,
    shared_notes: Arc<Mutex<Option<MeetingNotes>>>,
    level_channel: Channel<f32>,
}

struct ActiveSession {
    session_id: String,
    config: SessionConfig,
    audio_handle: AudioCaptureHandle,
    update_rx: mpsc::Receiver<NotesUpdate>,
    latest_notes: Option<MeetingNotes>,
    latest_transcript: Vec<TranscriptSegment>,
}

impl SessionManager {
    pub fn new(
        cmd_rx: mpsc::Receiver<SessionCommand>,
        instruction_rx: mpsc::Receiver<String>,
        shared_title: Arc<Mutex<String>>,
        shared_notes: Arc<Mutex<Option<MeetingNotes>>>,
        level_channel: Channel<f32>,
        event_callback: impl Fn(SessionEvent) + Send + 'static,
    ) -> Self {
        Self {
            cmd_rx,
            instruction_rx,
            state: SessionState::Idle,
            persistence: PersistenceService::new(),
            active_session: None,
            event_callback: Box::new(event_callback),
            instruction_fwd_tx: None,
            shared_title,
            shared_notes,
            level_channel,
        }
    }

    pub async fn run(&mut self) {
        tracing::info!("Session manager started");
        let mut autosave_interval = tokio::time::interval(tokio::time::Duration::from_secs(10));

        loop {
            tokio::select! {
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(SessionCommand::Start(config)) => self.start_meeting(config).await,
                        Some(SessionCommand::Pause) => self.pause_meeting(),
                        Some(SessionCommand::Resume) => self.resume_meeting(),
                        Some(SessionCommand::Stop) => self.stop_meeting().await,
                        None => break,
                    }
                }
                _ = autosave_interval.tick() => {
                    self.autosave();
                }
                instr = self.instruction_rx.recv() => {
                    if let Some(text) = instr {
                        if let Some(ref tx) = self.instruction_fwd_tx {
                            let _ = tx.send(text).await;
                        }
                    }
                }
                update = async {
                    if let Some(ref mut s) = self.active_session {
                        s.update_rx.recv().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    if let Some(update) = update {
                        self.on_notes_update(update);
                    }
                }
            }
        }
    }

    async fn start_meeting(&mut self, config: SessionConfig) {
        if self.state == SessionState::Active || self.state == SessionState::Paused {
            tracing::warn!("Cannot start: session already active");
            return;
        }

        let session_id = uuid::Uuid::new_v4().to_string();
        let title = config.title.clone().unwrap_or_else(|| "Meeting".to_string());
        tracing::info!("Starting meeting: {session_id} - {title}");

        let (audio_tx, audio_rx) = mpsc::channel::<AudioChunk>(256);
        let (transcript_tx, transcript_rx) = mpsc::channel::<TranscriptionEvent>(128);
        let (notes_tx, notes_rx) = mpsc::channel::<NotesUpdate>(32);

        // Build a level callback that emits to the frontend channel
        let level_ch = self.level_channel.clone();
        let level_cb: Arc<dyn Fn(f32) + Send + Sync> = Arc::new(move |level| {
            let _ = level_ch.send(level);
        });

        let audio_handle = match AudioCaptureHandle::start(
            config.audio_source,
            config.mic_device.clone(),
            audio_tx,
            Some(level_cb),
        ) {
            Ok(h) => h,
            Err(e) => {
                tracing::error!("Failed to start audio: {e}");
                self.state = SessionState::Error;
                (self.event_callback)(SessionEvent::StateChanged(SessionState::Error));
                return;
            }
        };

        // Transcription task — dispatch based on configured provider
        let transcription_provider = crate::config::AppConfig::get().transcription_provider;
        match transcription_provider.as_str() {
            "whisper" => {
                let model_path = crate::config::AppConfig::get()
                    .whisper_model_path
                    .unwrap_or_default();
                let mut transcriber = WhisperService::new(model_path);
                tokio::spawn(async move {
                    if let Err(e) = transcriber.run(audio_rx, transcript_tx).await {
                        tracing::error!("Whisper transcription error: {e}");
                    }
                });
            }
            _ => {
                // Default: AWS Transcribe
                let mut transcriber = TranscriptionService::new();
                tokio::spawn(async move {
                    if let Err(e) = transcriber.run(audio_rx, transcript_tx).await {
                        tracing::error!("AWS transcription error: {e}");
                    }
                });
            }
        }

        // Note generator task
        let (fwd_instr_tx, fwd_instr_rx) = mpsc::channel::<String>(64);
        let mut generator = NoteGenerator::new();
        let initial_notes = MeetingNotes::new(&session_id, &title, config.audio_source);
        generator.initialize(config.profile.clone(), Some(initial_notes.clone()));
        generator.set_shared_notes(self.shared_notes.clone());
        // Seed shared notes with initial state
        if let Ok(mut guard) = self.shared_notes.try_lock() {
            *guard = Some(initial_notes);
        }
        tokio::spawn(async move {
            if let Err(e) = generator.run(transcript_rx, notes_tx, fwd_instr_rx).await {
                tracing::error!("Note generator error: {e}");
            }
        });

        self.instruction_fwd_tx = Some(fwd_instr_tx);
        self.active_session = Some(ActiveSession {
            session_id,
            config,
            audio_handle,
            update_rx: notes_rx,
            latest_notes: None,
            latest_transcript: Vec::new(),
        });
        self.state = SessionState::Active;
        (self.event_callback)(SessionEvent::StateChanged(SessionState::Active));
        tracing::info!("Meeting started");
    }

    fn pause_meeting(&mut self) {
        if self.state != SessionState::Active {
            return;
        }
        if let Some(ref s) = self.active_session {
            s.audio_handle.pause();
            self.state = SessionState::Paused;
            (self.event_callback)(SessionEvent::StateChanged(SessionState::Paused));
            tracing::info!("Meeting paused");
        }
    }

    fn resume_meeting(&mut self) {
        if self.state != SessionState::Paused {
            return;
        }
        if let Some(ref s) = self.active_session {
            s.audio_handle.resume();
            self.state = SessionState::Active;
            (self.event_callback)(SessionEvent::StateChanged(SessionState::Active));
            tracing::info!("Meeting resumed");
        }
    }

    async fn stop_meeting(&mut self) {
        if self.state != SessionState::Active && self.state != SessionState::Paused {
            return;
        }

        if let Some(mut session) = self.active_session.take() {
            // Trigger final note generation before stopping audio
            if let Some(ref tx) = self.instruction_fwd_tx {
                let _ = tx.send("__finalize__".to_string()).await;
                // Wait for the final notes to come through
                if let Some(update) = tokio::time::timeout(
                    tokio::time::Duration::from_secs(120),
                    session.update_rx.recv(),
                ).await.ok().flatten() {
                    (self.event_callback)(SessionEvent::NotesUpdated(update.notes.clone()));
                    if !update.transcript.is_empty() {
                        (self.event_callback)(SessionEvent::TranscriptUpdated(update.transcript.clone()));
                    }
                    session.latest_notes = Some(update.notes);
                    session.latest_transcript = update.transcript;
                }
            }

            session.audio_handle.stop();
            self.instruction_fwd_tx = None;

            if let Some(ref notes) = session.latest_notes {
                let now = chrono::Utc::now().timestamp_millis() as u64;
                let mut final_notes = notes.clone();
                final_notes.metadata.end_time = Some(now);
                final_notes.metadata.duration_ms = Some(now - final_notes.metadata.start_time);

                let title = self.shared_title.lock().await.clone();
                let data = SessionData {
                    session_id: session.session_id.clone(),
                    title,
                    state: SessionState::Completed,
                    start_time: final_notes.metadata.start_time,
                    end_time: Some(now),
                    profile: session.config.profile.clone(),
                    notes: final_notes,
                    transcript: session.latest_transcript.clone(),
                };
                if let Err(e) = self.persistence.save_session(&data) {
                    tracing::error!("Failed to save session: {e}");
                }
            }

            self.state = SessionState::Completed;
            (self.event_callback)(SessionEvent::StateChanged(SessionState::Completed));
            tracing::info!("Meeting ended");
        }
        self.state = SessionState::Idle;
        (self.event_callback)(SessionEvent::StateChanged(SessionState::Idle));
    }

    fn on_notes_update(&mut self, update: NotesUpdate) {
        tracing::info!("Notes update received, emitting to frontend");
        (self.event_callback)(SessionEvent::NotesUpdated(update.notes.clone()));
        if !update.transcript.is_empty() {
            (self.event_callback)(SessionEvent::TranscriptUpdated(update.transcript.clone()));
        }
        if let Some(ref mut s) = self.active_session {
            s.latest_notes = Some(update.notes);
            s.latest_transcript = update.transcript;
        }
    }

    fn autosave(&self) {
        if let Some(ref s) = self.active_session {
            // Prefer shared notes (has user edits), fall back to latest from generator
            let notes = self.shared_notes.try_lock().ok()
                .and_then(|g| g.clone())
                .or_else(|| s.latest_notes.clone());
            if let Some(ref notes) = notes {
                let title = self.shared_title.try_lock()
                    .map(|t| t.clone())
                    .unwrap_or_else(|_| s.config.title.clone().unwrap_or_default());
                let data = SessionData {
                    session_id: s.session_id.clone(),
                    title,
                    state: self.state,
                    start_time: notes.metadata.start_time,
                    end_time: None,
                    profile: s.config.profile.clone(),
                    notes: notes.clone(),
                    transcript: s.latest_transcript.clone(),
                };
                if let Err(e) = self.persistence.save_session(&data) {
                    tracing::error!("Autosave failed: {e}");
                }
            }
        }
    }
}
