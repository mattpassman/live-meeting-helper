use crate::types::AudioSource;
use crate::config::AppConfig;
use crate::notes::{BlockState, Correction, MeetingNotes};
use crate::notes::corrections::extract_corrections;
use crate::notes::{ActionItemSection, Author, DecisionSection, NoteSection, TopicSection};
use crate::persistence::{ExportFormat, PersistenceService, SessionSummary};
use crate::profile::{MeetingProfile, ProfileService};
use crate::session::{SessionCommand, SessionConfig, SessionEvent, SessionManager, SessionState};
use std::sync::Arc;
use tauri::State;
use tokio::sync::{mpsc, Mutex};

#[derive(Clone)]
pub struct AppState {
    pub session_manager: Arc<Mutex<Option<SessionManagerHandle>>>,
    pub persistence: Arc<PersistenceService>,
    pub profiles: Arc<ProfileService>,
    pub cmd_tx: Arc<Mutex<Option<mpsc::Sender<SessionCommand>>>>,
    pub meeting_title: Arc<Mutex<String>>,
    pub pending_doc: Arc<Mutex<Option<String>>>,
    pub live_notes: Arc<Mutex<Option<MeetingNotes>>>,
}

pub struct SessionManagerHandle {
    pub state: SessionState,
    pub instruction_tx: Option<mpsc::Sender<String>>,
}

#[tauri::command]
pub async fn start_meeting(
    state: State<'_, AppState>,
    audio_source: String,
    title: Option<String>,
    profile_id: Option<String>,
    mic_device: Option<String>,
    on_notes: tauri::ipc::Channel<MeetingNotes>,
    on_state: tauri::ipc::Channel<String>,
) -> Result<(), String> {
    let mut mgr = state.session_manager.lock().await;
    if let Some(ref h) = *mgr {
        if h.state == SessionState::Active || h.state == SessionState::Paused {
            return Err("Session already active".into());
        }
    }

    let source = match audio_source.as_str() {
        "microphone" => AudioSource::Microphone,
        "system" => AudioSource::SystemAudio,
        "both" => AudioSource::Both,
        _ => AudioSource::Microphone,
    };

    let profile = profile_id
        .and_then(|name| state.profiles.get_profile(&name))
        .unwrap_or_else(ProfileService::default_profile);

    tracing::info!("start_meeting called with title={:?}", title);
    let resolved_title = title.unwrap_or_else(|| "Meeting".into());
    *state.meeting_title.lock().await = resolved_title.clone();
    let config = SessionConfig {
        audio_source: source,
        mic_device,
        title: Some(resolved_title),
        profile,
    };

    let (cmd_tx, cmd_rx) = mpsc::channel::<SessionCommand>(16);
    let (instruction_tx, instruction_rx) = mpsc::channel::<String>(64);

    let mut manager = SessionManager::new(cmd_rx, instruction_rx, state.meeting_title.clone(), state.live_notes.clone(), move |event| {
        match event {
            SessionEvent::NotesUpdated(notes) => {
                tracing::info!("Sending notes via channel");
                // shared live_notes is already updated by sync_to_shared in the
                // generator — do NOT overwrite here or we lose user edits that
                // arrived during generation.
                if let Err(e) = on_notes.send(notes) {
                    tracing::error!("Channel send failed: {e}");
                }
            }
            SessionEvent::StateChanged(s) => {
                let _ = on_state.send(s.to_string());
            }
        }
    });

    // Send start command
    cmd_tx.send(SessionCommand::Start(config)).await.map_err(|e| e.to_string())?;

    // Send any pre-attached document
    if let Some(doc_text) = state.pending_doc.lock().await.take() {
        let _ = instruction_tx.send(format!("__doc__:{doc_text}")).await;
    }

    *state.cmd_tx.lock().await = Some(cmd_tx);
    *mgr = Some(SessionManagerHandle {
        state: SessionState::Active,
        instruction_tx: Some(instruction_tx),
    });

    tokio::spawn(async move {
        manager.run().await;
    });

    Ok(())
}

#[tauri::command]
pub async fn pause_meeting(state: State<'_, AppState>) -> Result<(), String> {
    let tx = state.cmd_tx.lock().await;
    let tx = tx.as_ref().ok_or("No active session")?;
    tx.send(SessionCommand::Pause).await.map_err(|e| e.to_string())?;
    if let Some(ref mut h) = *state.session_manager.lock().await {
        h.state = SessionState::Paused;
    }
    Ok(())
}

#[tauri::command]
pub async fn resume_meeting(state: State<'_, AppState>) -> Result<(), String> {
    let tx = state.cmd_tx.lock().await;
    let tx = tx.as_ref().ok_or("No active session")?;
    tx.send(SessionCommand::Resume).await.map_err(|e| e.to_string())?;
    if let Some(ref mut h) = *state.session_manager.lock().await {
        h.state = SessionState::Active;
    }
    Ok(())
}

#[tauri::command]
pub async fn stop_meeting(state: State<'_, AppState>) -> Result<(), String> {
    let tx = state.cmd_tx.lock().await;
    let tx = tx.as_ref().ok_or("No active session")?;
    tx.send(SessionCommand::Stop).await.map_err(|e| e.to_string())?;
    // Clean up after a brief delay to let the manager process
    let mgr = state.session_manager.clone();
    let cmd = state.cmd_tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        *mgr.lock().await = None;
        *cmd.lock().await = None;
    });
    Ok(())
}

#[tauri::command]
pub async fn get_session_state(state: State<'_, AppState>) -> Result<String, String> {
    let mgr = state.session_manager.lock().await;
    Ok(match &*mgr {
        Some(h) => h.state.to_string(),
        None => "Idle".into(),
    })
}

#[tauri::command]
pub async fn send_instruction(state: State<'_, AppState>, text: String) -> Result<(), String> {
    let mgr = state.session_manager.lock().await;
    let h = mgr.as_ref().ok_or("No active session")?;
    let tx = h.instruction_tx.as_ref().ok_or("No instruction channel")?;
    tx.send(text).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_meeting_title(state: State<'_, AppState>, title: String) -> Result<(), String> {
    *state.meeting_title.lock().await = title;
    Ok(())
}

// Session history commands
#[tauri::command]
pub async fn list_sessions(state: State<'_, AppState>) -> Result<Vec<SessionSummary>, String> {
    Ok(state.persistence.list_sessions())
}

#[tauri::command]
pub async fn get_session(state: State<'_, AppState>, session_id: String) -> Result<serde_json::Value, String> {
    state.persistence.load_session(&session_id)
        .map(|s| serde_json::to_value(s).unwrap_or_default())
        .ok_or_else(|| "Session not found".into())
}

#[tauri::command]
pub async fn export_session(state: State<'_, AppState>, session_id: String, format: String) -> Result<String, String> {
    let fmt = match format.as_str() {
        "plaintext" => ExportFormat::PlainText,
        _ => ExportFormat::Markdown,
    };
    state.persistence.export_notes(&session_id, fmt)
        .ok_or_else(|| "Session not found".into())
}

#[tauri::command]
pub async fn delete_session(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    state.persistence.delete_session(&session_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_session_file(
    state: State<'_, AppState>,
    session_id: String,
    format: String,
) -> Result<String, String> {
    let fmt = match format.as_str() {
        "plaintext" => ExportFormat::PlainText,
        _ => ExportFormat::Markdown,
    };

    let text = state
        .persistence
        .export_notes(&session_id, fmt)
        .ok_or_else(|| "Session not found".to_string())?;

    let session = state
        .persistence
        .load_session(&session_id)
        .ok_or_else(|| "Session not found".to_string())?;

    // Build safe filename from session title
    let safe_title: String = session
        .title
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .chars()
        .take(60)
        .collect();
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let extension = match fmt {
        ExportFormat::Markdown => "md",
        ExportFormat::PlainText => "txt",
    };
    let filename = format!("{safe_title}_{timestamp}.{extension}");

    // Choose save directory: Desktop > Documents > current dir
    let save_dir = dirs::desktop_dir()
        .or_else(dirs::document_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let file_path = save_dir.join(&filename);
    std::fs::write(&file_path, text).map_err(|e| e.to_string())?;

    Ok(file_path.to_string_lossy().into_owned())
}

// Profile commands
#[tauri::command]
pub async fn list_profiles(state: State<'_, AppState>) -> Result<Vec<MeetingProfile>, String> {
    Ok(state.profiles.list_profiles())
}

#[tauri::command]
pub async fn get_profile(state: State<'_, AppState>, id: String) -> Result<MeetingProfile, String> {
    state.profiles.get_profile(&id).ok_or_else(|| "Profile not found".into())
}

#[tauri::command]
pub async fn save_profile(state: State<'_, AppState>, profile: MeetingProfile) -> Result<(), String> {
    state.profiles.save_profile(&profile).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_profile(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.profiles.delete_profile(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn query_session(state: State<'_, AppState>, session_id: String, question: String) -> Result<MeetingNotes, String> {
    let mut session = state.persistence.load_session(&session_id)
        .ok_or("Session not found")?;
    if session.transcript.is_empty() {
        return Err("No transcript saved for this session".into());
    }
    let updated = crate::notes::generator::regenerate_with_instruction(
        &session.notes,
        &session.transcript,
        &session.profile,
        &question,
    )
    .await
    .map_err(|e| format!("{e}"))?;

    session.notes = updated.clone();
    state.persistence.save_session(&session).map_err(|e| e.to_string())?;
    Ok(updated)
}

// Config commands
#[tauri::command]
pub async fn get_config() -> Result<AppConfig, String> {
    Ok(AppConfig::get().clone())
}

#[tauri::command]
pub async fn save_config(config: AppConfig) -> Result<(), String> {
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_audio_devices() -> Result<Vec<(String, bool)>, String> {
    Ok(crate::audio::capture::list_input_devices())
}

// Document attachment commands
#[tauri::command]
pub async fn attach_document_text(state: State<'_, AppState>, text: String) -> Result<(), String> {
    let mgr = state.session_manager.lock().await;
    if let Some(h) = mgr.as_ref() {
        if let Some(tx) = h.instruction_tx.as_ref() {
            return tx.send(format!("__doc__:{text}")).await.map_err(|e| e.to_string());
        }
    }
    *state.pending_doc.lock().await = Some(text);
    Ok(())
}

#[tauri::command]
pub async fn attach_document_file(state: State<'_, AppState>, filename: String, data: Vec<u8>) -> Result<String, String> {
    let text = crate::document::extract_text_from_bytes(&filename, &data)?;
    let mgr = state.session_manager.lock().await;
    if let Some(h) = mgr.as_ref() {
        if let Some(tx) = h.instruction_tx.as_ref() {
            tx.send(format!("__doc__:{text}")).await.map_err(|e| e.to_string())?;
            return Ok(filename);
        }
    }
    *state.pending_doc.lock().await = Some(text);
    Ok(filename)
}

// --- User edit commands ---

#[tauri::command]
pub async fn edit_note_block(state: State<'_, AppState>, block_id: String, content: String) -> Result<(), String> {
    let mut notes_guard = state.live_notes.lock().await;
    let notes = notes_guard.as_mut().ok_or("No active notes")?;
    let block = notes.find_block_mut(&block_id).ok_or("Block not found")?;

    // Store original AI content for correction extraction
    if block.original_ai_content.is_none() && block.block_state == BlockState::AiManaged {
        block.original_ai_content = Some(block.content.clone());
    }

    // Extract corrections from the diff
    if let Some(ref original) = block.original_ai_content {
        let new_corrections = extract_corrections(original, &content);
        notes.corrections.extend(new_corrections);
    }

    // Re-borrow after corrections extraction
    let block = notes.find_block_mut(&block_id).ok_or("Block not found")?;
    block.content = content;
    block.block_state = BlockState::UserEdited;
    block.last_updated_by = Author::User;
    block.last_updated_at = chrono::Utc::now().timestamp_millis() as u64;
    Ok(())
}

#[tauri::command]
pub async fn add_note_block(state: State<'_, AppState>, section: String, content: String, position: Option<usize>) -> Result<String, String> {
    let mut notes_guard = state.live_notes.lock().await;
    let notes = notes_guard.as_mut().ok_or("No active notes")?;
    let now = chrono::Utc::now().timestamp_millis() as u64;
    let block_id = format!("user-{}", uuid::Uuid::new_v4());

    let base = NoteSection {
        id: block_id.clone(),
        content: content.clone(),
        last_updated_by: Author::User,
        last_updated_at: now,
        block_state: BlockState::UserAdded,
        original_ai_content: None,
    };

    match section.as_str() {
        "action_items" => {
            let item = ActionItemSection { base, description: content, assignee: None };
            let pos = position.unwrap_or(notes.action_items.len());
            notes.action_items.insert(pos.min(notes.action_items.len()), item);
        }
        "decisions" => {
            let item = DecisionSection { base, decision_text: content };
            let pos = position.unwrap_or(notes.decisions.len());
            notes.decisions.insert(pos.min(notes.decisions.len()), item);
        }
        "discussion_topics" => {
            let item = TopicSection { base, topic_title: content.lines().next().unwrap_or("New Topic").to_string() };
            let pos = position.unwrap_or(notes.discussion_topics.len());
            notes.discussion_topics.insert(pos.min(notes.discussion_topics.len()), item);
        }
        _ => return Err(format!("Unknown section: {section}")),
    }
    Ok(block_id)
}

#[tauri::command]
pub async fn delete_note_block(state: State<'_, AppState>, block_id: String) -> Result<(), String> {
    let mut notes_guard = state.live_notes.lock().await;
    let notes = notes_guard.as_mut().ok_or("No active notes")?;
    let block = notes.find_block_mut(&block_id).ok_or("Block not found")?;
    block.block_state = BlockState::UserDeleted;
    Ok(())
}

#[tauri::command]
pub async fn restore_note_block(state: State<'_, AppState>, block_id: String) -> Result<(), String> {
    let mut notes_guard = state.live_notes.lock().await;
    let notes = notes_guard.as_mut().ok_or("No active notes")?;
    let block = notes.find_block_mut(&block_id).ok_or("Block not found")?;
    block.block_state = BlockState::AiManaged;
    block.original_ai_content = None;
    Ok(())
}

#[tauri::command]
pub async fn get_corrections(state: State<'_, AppState>) -> Result<Vec<Correction>, String> {
    let notes_guard = state.live_notes.lock().await;
    let notes = notes_guard.as_ref().ok_or("No active notes")?;
    Ok(notes.corrections.clone())
}

#[tauri::command]
pub async fn remove_correction(state: State<'_, AppState>, index: usize) -> Result<(), String> {
    let mut notes_guard = state.live_notes.lock().await;
    let notes = notes_guard.as_mut().ok_or("No active notes")?;
    if index >= notes.corrections.len() {
        return Err("Correction index out of bounds".into());
    }
    notes.corrections.remove(index);
    Ok(())
}

// ── Onboarding wizard commands ───────────────────────────────────────────────

#[tauri::command]
pub async fn test_ai_connection() -> Result<String, String> {
    let config = AppConfig::get();
    let client = reqwest::Client::new();
    match config.ai_provider.as_str() {
        "openai" => {
            let key = config.openai_api_key.ok_or("No OpenAI API key set")?;
            let res = client
                .post("https://api.openai.com/v1/chat/completions")
                .header("Authorization", format!("Bearer {}", key))
                .json(&serde_json::json!({
                    "model": "gpt-4o-mini",
                    "max_tokens": 1,
                    "messages": [{"role": "user", "content": "hi"}]
                }))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if res.status().is_success() { Ok("OpenAI".into()) }
            else { Err(format!("API error: {}", res.status())) }
        }
        _ => {
            let key = config.claude_api_key.ok_or("No Claude API key set")?;
            let res = client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &key)
                .header("anthropic-version", "2023-06-01")
                .json(&serde_json::json!({
                    "model": "claude-haiku-4-5-20251001",
                    "max_tokens": 1,
                    "messages": [{"role": "user", "content": "hi"}]
                }))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if res.status().is_success() { Ok("Claude".into()) }
            else { Err(format!("API error: {}", res.status())) }
        }
    }
}

#[tauri::command]
pub async fn mark_setup_complete() -> Result<(), String> {
    let mut config = AppConfig::get();
    config.setup_complete = true;
    config.save().map_err(|e| e.to_string())
}

// ── Whisper model downloader ─────────────────────────────────────────────────

#[cfg(feature = "whisper")]
#[tauri::command]
pub async fn download_whisper_model(
    model_id: String,
    on_progress: tauri::ipc::Channel<f64>,
) -> Result<String, String> {
    use futures_util::StreamExt;
    use std::io::Write;

    // Validate model_id
    let valid_ids = ["tiny.en", "base.en", "small.en", "medium.en"];
    if !valid_ids.contains(&model_id.as_str()) {
        return Err(format!("Unknown model id '{}'. Valid ids: tiny.en, base.en, small.en, medium.en", model_id));
    }

    let filename = format!("ggml-{}.bin", model_id);

    // Resolve download directory
    let models_dir = dirs::data_dir()
        .ok_or_else(|| "Cannot determine data directory".to_string())?
        .join("live-meeting-helper")
        .join("models");

    std::fs::create_dir_all(&models_dir)
        .map_err(|e| format!("Cannot create models directory: {e}"))?;

    let dest_path = models_dir.join(&filename);

    // Already downloaded — return immediately
    if dest_path.exists() {
        let path_str = dest_path.to_string_lossy().into_owned();
        tracing::info!("Model already exists at {}", path_str);
        let _ = on_progress.send(1.0);
        return Ok(path_str);
    }

    let tmp_path = models_dir.join(format!("{}.tmp", filename));

    // Clean up any leftover .tmp from a previous failed attempt
    if tmp_path.exists() {
        let _ = std::fs::remove_file(&tmp_path);
    }

    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
        filename
    );

    tracing::info!("Downloading {} from {}", filename, url);

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("HTTP {}: {}", response.status(), url));
    }

    let content_length = response.content_length();
    tracing::info!(
        "Content-Length: {}",
        content_length.map_or("unknown".to_string(), |n| n.to_string())
    );

    // Open .tmp file for writing
    let mut tmp_file = std::fs::File::create(&tmp_path)
        .map_err(|e| format!("Cannot create temp file: {e}"))?;

    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;
    // Throttle: send a progress update every ~1 MB
    const PROGRESS_INTERVAL: u64 = 1_024 * 1_024; // 1 MB
    let mut next_report_at: u64 = PROGRESS_INTERVAL;
    let mut last_fraction: f64 = -1.0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            // Attempt cleanup on error
            let _ = std::fs::remove_file(&tmp_path);
            format!("Download error: {e}")
        })?;

        tmp_file.write_all(&chunk).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            format!("Write error: {e}")
        })?;

        downloaded += chunk.len() as u64;

        // Throttled progress reporting
        if downloaded >= next_report_at {
            next_report_at = downloaded + PROGRESS_INTERVAL;
            let fraction = if let Some(total) = content_length {
                (downloaded as f64 / total as f64).clamp(0.0, 1.0)
            } else {
                // Unknown length — send bytes as a negative sentinel (won't reach 1.0 until done)
                -1.0_f64.min(downloaded as f64)
            };
            // Only send if it actually changed by at least 1%
            if (fraction - last_fraction).abs() >= 0.01 {
                last_fraction = fraction;
                let _ = on_progress.send(fraction);
            }
        }
    }

    // Flush and close
    tmp_file.flush().map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Flush error: {e}")
    })?;
    drop(tmp_file);

    // Rename .tmp → final
    std::fs::rename(&tmp_path, &dest_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Rename failed: {e}")
    })?;

    let path_str = dest_path.to_string_lossy().into_owned();
    tracing::info!("Download complete: {}", path_str);

    // Final progress ping
    let _ = on_progress.send(1.0);

    // Persist path to config
    let mut config = AppConfig::get();
    config.whisper_model_path = Some(path_str.clone());
    if let Err(e) = config.save() {
        tracing::warn!("Failed to save config after model download: {e}");
    }

    Ok(path_str)
}
