use super::{
    ActionItemSection, Author, BlockState, DecisionSection, MeetingNotes, NoteGenError,
    NoteSection, TopicSection,
};
use crate::document::DocContext;
use crate::profile::MeetingProfile;
use crate::types::{TranscriptSegment, TranscriptionEvent};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

/// Payload sent from the generator back to the session manager.
#[derive(Clone)]
pub struct NotesUpdate {
    pub notes: MeetingNotes,
    pub transcript: Vec<TranscriptSegment>,
}

pub struct NoteGenerator {
    profile: Option<MeetingProfile>,
    notes: Option<MeetingNotes>,
    shared_notes: Option<Arc<Mutex<Option<MeetingNotes>>>>,
    transcript_buffer: Vec<TranscriptSegment>,
    all_segments: Vec<TranscriptSegment>,
    ad_hoc_instructions: Vec<String>,
    reference_doc: Option<DocContext>,
    generation_in_progress: Arc<Mutex<bool>>,
    last_full_refresh: std::time::Instant,
    pending_instruction: bool,
}

impl NoteGenerator {
    pub fn new() -> Self {
        Self {
            profile: None,
            notes: None,
            shared_notes: None,
            transcript_buffer: Vec::new(),
            all_segments: Vec::new(),
            ad_hoc_instructions: Vec::new(),
            reference_doc: None,
            generation_in_progress: Arc::new(Mutex::new(false)),
            last_full_refresh: std::time::Instant::now(),
            pending_instruction: false,
        }
    }

    pub fn initialize(&mut self, profile: MeetingProfile, existing_notes: Option<MeetingNotes>) {
        self.profile = Some(profile);
        self.notes = existing_notes;
    }

    pub fn set_shared_notes(&mut self, shared: Arc<Mutex<Option<MeetingNotes>>>) {
        self.shared_notes = Some(shared);
    }

    /// Sync from shared notes (picks up user edits) before generation.
    async fn sync_from_shared(&mut self) {
        if let Some(ref shared) = self.shared_notes {
            if let Ok(guard) = shared.try_lock() {
                if let Some(ref shared_notes) = *guard {
                    // Merge user edits into our local copy
                    if let Some(ref mut local) = self.notes {
                        local.corrections = shared_notes.corrections.clone();
                        // Sync block states from shared into local
                        if shared_notes.summary.block_state != BlockState::AiManaged {
                            local.summary = shared_notes.summary.clone();
                        }
                        sync_section_states(&mut local.discussion_topics, &shared_notes.discussion_topics);
                        sync_section_states(&mut local.decisions, &shared_notes.decisions);
                        sync_section_states(&mut local.action_items, &shared_notes.action_items);
                    }
                }
            }
        }
    }

    /// Push our notes to the shared state after generation, preserving any
    /// user edits that arrived while generation was in progress.
    /// Returns the merged notes so the caller can keep self.notes in sync.
    async fn sync_to_shared(&self) -> Option<MeetingNotes> {
        if let Some(ref shared) = self.shared_notes {
            if let Ok(mut guard) = shared.try_lock() {
                let mut updated = match self.notes.clone() {
                    Some(n) => n,
                    None => return None,
                };

                // Merge user edits that may have occurred during generation
                if let Some(ref current_shared) = *guard {
                    // Preserve corrections added by the user during generation
                    for c in &current_shared.corrections {
                        if !updated.corrections.iter().any(|u| u.original == c.original && u.corrected == c.corrected) {
                            updated.corrections.push(c.clone());
                        }
                    }
                    // Preserve user-edited/added block states from shared
                    if current_shared.summary.block_state != BlockState::AiManaged {
                        updated.summary = current_shared.summary.clone();
                    }
                    sync_section_states(&mut updated.discussion_topics, &current_shared.discussion_topics);
                    sync_section_states(&mut updated.decisions, &current_shared.decisions);
                    sync_section_states(&mut updated.action_items, &current_shared.action_items);
                }

                *guard = Some(updated.clone());
                return Some(updated);
            }
        }
        None
    }

    pub async fn run(
        &mut self,
        mut segment_rx: mpsc::Receiver<TranscriptionEvent>,
        notes_tx: mpsc::Sender<NotesUpdate>,
        mut instruction_rx: mpsc::Receiver<String>,
    ) -> Result<(), NoteGenError> {
        let debounce = tokio::time::Duration::from_secs(3);
        let mut debounce_timer = tokio::time::interval(debounce);
        debounce_timer.tick().await; // consume first immediate tick
        let mut pending = false;

        loop {
            tokio::select! {
                event = segment_rx.recv() => {
                    match event {
                        Some(TranscriptionEvent::Segment(seg)) => {
                            if let Some(ref speaker) = seg.speaker {
                                if let Some(ref mut notes) = self.notes {
                                    if !notes.metadata.speakers.contains(speaker) {
                                        notes.metadata.speakers.push(speaker.clone());
                                    }
                                }
                            }
                            tracing::debug!("Buffered segment #{} ({} in buffer)", self.all_segments.len() + 1, self.transcript_buffer.len() + 1);
                            self.all_segments.push(seg.clone());
                            self.transcript_buffer.push(seg);
                            pending = true;
                            debounce_timer.reset();
                        }
                        Some(TranscriptionEvent::Silence(_)) => {
                            if !self.transcript_buffer.is_empty() {
                                if let Err(e) = self.generate_notes(&notes_tx).await {
                                    tracing::error!("Note generation failed: {e}");
                                }
                                pending = false;
                            }
                        }
                        None => break,
                    }
                }
                instruction = instruction_rx.recv() => {
                    match instruction {
                        Some(ref instr) if instr == "__finalize__" => {
                            tracing::info!("Running final note generation pass");
                            self.transcript_buffer = self.all_segments.clone();
                            if let Err(e) = self.generate_final_notes(&notes_tx).await {
                                tracing::error!("Final note generation failed: {e}");
                            }
                            break;
                        }
                        Some(ref instr) if instr.starts_with("__doc__:") => {
                            let doc_text = instr.strip_prefix("__doc__:").unwrap().to_string();
                            tracing::info!("Reference document attached ({} chars)", doc_text.len());
                            let sections = crate::document::chunk_into_sections(&doc_text);
                            tracing::info!("Document chunked into {} sections", sections.len());
                            let summary = self.summarize_document(&doc_text).await;
                            self.reference_doc = Some(DocContext {
                                full_text: doc_text,
                                summary,
                                sections,
                            });
                            self.pending_instruction = true;
                            self.transcript_buffer = self.all_segments.clone();
                            if let Err(e) = self.generate_notes(&notes_tx).await {
                                tracing::error!("Doc-triggered generation failed: {e}");
                            }
                            pending = false;
                        }
                        Some(instr) => {
                            tracing::info!("Received ad-hoc instruction: {instr}");
                            self.ad_hoc_instructions.push(instr);
                            // Force full-context refresh so the LLM sees the entire transcript
                            self.pending_instruction = true;
                            self.transcript_buffer = self.all_segments.clone();
                            if let Err(e) = self.generate_notes(&notes_tx).await {
                                tracing::error!("Instruction-triggered generation failed: {e}");
                            }
                            pending = false;
                        }
                        None => {}
                    }
                }
                _ = debounce_timer.tick(), if pending => {
                    tracing::info!("Debounce fired, generating notes from {} segments", self.transcript_buffer.len());
                    if let Err(e) = self.generate_notes(&notes_tx).await {
                        tracing::error!("Note generation failed: {e}");
                    }
                    pending = false;
                }
            }
        }

        Ok(())
    }

    async fn generate_notes(
        &mut self,
        notes_tx: &mpsc::Sender<NotesUpdate>,
    ) -> Result<(), NoteGenError> {
        let mut in_progress = self.generation_in_progress.lock().await;
        if *in_progress {
            return Ok(());
        }
        *in_progress = true;
        drop(in_progress);

        // Pick up user edits before generating
        self.sync_from_shared().await;

        // Full refresh on: periodic 5-min cycle OR pending ad-hoc instruction
        let full_refresh = self.pending_instruction
            || self.last_full_refresh.elapsed() >= std::time::Duration::from_secs(300);

        let segments = std::mem::take(&mut self.transcript_buffer);
        let invoke_segments = if full_refresh {
            tracing::info!("Full-context refresh ({} total segments, instruction={})", self.all_segments.len(), self.pending_instruction);
            self.all_segments.clone()
        } else {
            segments.clone()
        };
        self.pending_instruction = false;
        let result = self.invoke_llm(&invoke_segments, false).await;

        let mut in_progress = self.generation_in_progress.lock().await;
        *in_progress = false;

        match result {
            Ok(updated_notes) => {
                if full_refresh {
                    self.last_full_refresh = std::time::Instant::now();
                }
                self.notes = Some(updated_notes.clone());
                // Merge with shared state and keep self.notes in sync
                let merged = self.sync_to_shared().await;
                let emit_notes = merged.unwrap_or(updated_notes);
                self.notes = Some(emit_notes.clone());
                let _ = notes_tx.send(NotesUpdate {
                    notes: emit_notes,
                    transcript: self.all_segments.clone(),
                }).await;
            }
            Err(e) => {
                tracing::error!("Note generation failed: {e}");
                self.transcript_buffer.splice(0..0, segments);
            }
        }

        Ok(())
    }

    async fn generate_final_notes(
        &mut self,
        notes_tx: &mpsc::Sender<NotesUpdate>,
    ) -> Result<(), NoteGenError> {
        tracing::info!("Final generation with {} total segments", self.all_segments.len());
        self.sync_from_shared().await;
        let segments = self.all_segments.clone();
        let result = self.invoke_llm(&segments, true).await;

        match result {
            Ok(updated_notes) => {
                self.notes = Some(updated_notes.clone());
                let merged = self.sync_to_shared().await;
                let emit_notes = merged.unwrap_or(updated_notes);
                self.notes = Some(emit_notes.clone());
                let _ = notes_tx.send(NotesUpdate {
                    notes: emit_notes,
                    transcript: self.all_segments.clone(),
                }).await;
            }
            Err(e) => {
                tracing::error!("Final note generation failed: {e}");
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn finalize(&mut self) -> Result<MeetingNotes, NoteGenError> {
        // Wait for any in-progress generation
        loop {
            let in_progress = self.generation_in_progress.lock().await;
            if !*in_progress {
                break;
            }
            drop(in_progress);
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        // Final generation with all segments
        let segments = std::mem::take(&mut self.transcript_buffer);
        if !segments.is_empty() {
            self.all_segments.extend(segments);
        }

        let all_segs: Vec<TranscriptSegment> = self.all_segments.clone();
        let result = self.invoke_llm(&all_segs, true).await?;
        self.notes = Some(result.clone());
        Ok(result)
    }

    async fn summarize_document(&self, text: &str) -> String {
        let prompt = format!(
            "Summarize the following document in 2-4 sentences. Focus on the main topics, \
             structure, and key points. Respond with ONLY the summary text, no JSON.\n\n{text}"
        );
        match call_llm_api(&prompt).await {
            Ok(summary) => summary.trim().to_string(),
            Err(e) => {
                tracing::warn!("Document summarization failed: {e}, using first 500 chars as summary");
                let end = text.floor_char_boundary(500.min(text.len()));
                format!("{}...", &text[..end])
            }
        }
    }

    fn build_user_constraints(&self) -> String {
        let notes = match self.notes.as_ref() {
            Some(n) => n,
            None => return String::new(),
        };

        let mut parts = Vec::new();

        // Locked content
        let mut locked = Vec::new();
        if notes.summary.block_state == BlockState::UserEdited {
            locked.push(format!("- [Summary] {:?} (user-edited)", notes.summary.content));
        }
        for t in &notes.discussion_topics {
            match t.base.block_state {
                BlockState::UserEdited | BlockState::UserAdded => {
                    locked.push(format!("- [Topic: {}] {:?} ({})", t.topic_title, t.base.content,
                        if t.base.block_state == BlockState::UserAdded { "user-added" } else { "user-edited" }));
                }
                _ => {}
            }
        }
        for d in &notes.decisions {
            match d.base.block_state {
                BlockState::UserEdited | BlockState::UserAdded => {
                    locked.push(format!("- [Decision] {:?} ({})", d.decision_text,
                        if d.base.block_state == BlockState::UserAdded { "user-added" } else { "user-edited" }));
                }
                _ => {}
            }
        }
        for a in &notes.action_items {
            match a.base.block_state {
                BlockState::UserEdited | BlockState::UserAdded => {
                    locked.push(format!("- [Action Item] {:?} ({})", a.description,
                        if a.base.block_state == BlockState::UserAdded { "user-added" } else { "user-edited" }));
                }
                _ => {}
            }
        }
        if !locked.is_empty() {
            parts.push(format!("USER-LOCKED CONTENT (do not modify, do not duplicate):\n{}", locked.join("\n")));
        }

        // Suppressed content
        let mut suppressed = Vec::new();
        for t in &notes.discussion_topics {
            if t.base.block_state == BlockState::UserDeleted {
                suppressed.push(format!("- [Topic] {:?}", t.topic_title));
            }
        }
        for d in &notes.decisions {
            if d.base.block_state == BlockState::UserDeleted {
                suppressed.push(format!("- [Decision] {:?}", d.decision_text));
            }
        }
        for a in &notes.action_items {
            if a.base.block_state == BlockState::UserDeleted {
                suppressed.push(format!("- [Action Item] {:?}", a.description));
            }
        }
        if !suppressed.is_empty() {
            parts.push(format!("SUPPRESSED CONTENT (do not regenerate):\n{}", suppressed.join("\n")));
        }

        // Corrections
        if !notes.corrections.is_empty() {
            let corr: Vec<_> = notes.corrections.iter()
                .map(|c| format!("- Use {:?} instead of {:?}", c.corrected, c.original))
                .collect();
            parts.push(format!("USER CORRECTIONS (apply globally):\n{}", corr.join("\n")));
        }

        if parts.is_empty() {
            String::new()
        } else {
            format!("\n{}\n", parts.join("\n\n"))
        }
    }

    async fn invoke_llm(
        &mut self,
        new_segments: &[TranscriptSegment],
        is_final: bool,
    ) -> Result<MeetingNotes, NoteGenError> {
        let profile = self
            .profile
            .as_ref()
            .cloned()
            .unwrap_or_else(crate::profile::ProfileService::default_profile);

        let current_notes_json = self
            .notes
            .as_ref()
            .map(|n| serde_json::to_string_pretty(n).unwrap_or_default())
            .unwrap_or_else(|| "null".to_string());

        let new_transcript: String = new_segments
            .iter()
            .map(|s| {
                let speaker = s.speaker.as_deref().unwrap_or("Unknown");
                format!("[{}] {}: {}", format_ms(s.start_time_ms), speaker, s.text)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let instructions = if self.ad_hoc_instructions.is_empty() {
            profile.instructions.clone()
        } else {
            let latest = self.ad_hoc_instructions.last().unwrap();
            let previous: Vec<_> = self.ad_hoc_instructions[..self.ad_hoc_instructions.len() - 1].to_vec();
            if previous.is_empty() {
                format!(
                    "{}\n\nUser's latest instruction (address this with the FULL transcript context):\n{}",
                    profile.instructions, latest
                )
            } else {
                format!(
                    "{}\n\nPrevious instructions:\n{}\n\nUser's latest instruction (address this with the FULL transcript context):\n{}",
                    profile.instructions, previous.join("\n"), latest
                )
            }
        };

        let is_full_context = !is_final && new_segments.len() == self.all_segments.len() && !self.all_segments.is_empty();

        // Append reference document context (tiered by call type)
        let instructions = if let Some(ref doc) = self.reference_doc {
            let mode = if is_final || is_full_context {
                crate::document::DocIncludeMode::Full
            } else {
                crate::document::DocIncludeMode::Relevant
            };
            let doc_prompt = crate::document::build_doc_prompt(doc, mode, &new_transcript);
            format!("{instructions}\n\n{doc_prompt}")
        } else {
            instructions
        };

        let action = if is_final {
            super::prompts::ACTION_FINAL
        } else if is_full_context {
            super::prompts::ACTION_FULL_REFRESH
        } else {
            super::prompts::ACTION_INCREMENTAL
        };

        // Build user constraints from block states and corrections
        let user_constraints = self.build_user_constraints();

        let prompt = super::prompts::build_prompt_with_constraints(
            action,
            &profile.name,
            &instructions,
            &current_notes_json,
            &new_transcript,
            &user_constraints,
        );

        let response = call_llm_api(&prompt).await?;
        self.parse_llm_response(&response)
    }

    fn parse_llm_response(&self, response: &str) -> Result<MeetingNotes, NoteGenError> {
        // Try to extract JSON from the response
        let json_str = extract_json(response).ok_or_else(|| {
            NoteGenError::OutputParseError(format!(
                "No JSON found in response: {}",
                &response[..response.len().min(200)]
            ))
        })?;

        let parsed: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| NoteGenError::OutputParseError(e.to_string()))?;

        let now = chrono::Utc::now().timestamp_millis() as u64;
        let mut notes = self
            .notes
            .clone()
            .unwrap_or_else(|| MeetingNotes::new("unknown", "Meeting", crate::types::AudioSource::Microphone));

        // Update speaker map — replace generic labels with inferred names
        if let Some(map) = parsed.get("speaker_map").and_then(|v| v.as_object()) {
            for (generic, real_name) in map {
                if let Some(name) = real_name.as_str() {
                    if let Some(pos) = notes.metadata.speakers.iter().position(|s| s == generic) {
                        notes.metadata.speakers[pos] = name.to_string();
                    } else if !notes.metadata.speakers.iter().any(|s| s == name) {
                        notes.metadata.speakers.push(name.to_string());
                    }
                }
            }
        }

        // Update summary — only if AI-managed
        if let Some(summary) = parsed.get("summary").and_then(|v| v.as_str()) {
            if notes.summary.block_state == BlockState::AiManaged {
                notes.summary.content = summary.to_string();
                notes.summary.last_updated_at = now;
            }
        }

        // Update discussion topics — preserve user-touched blocks
        if let Some(topics) = parsed.get("discussion_topics").and_then(|v| v.as_array()) {
            let user_blocks: Vec<_> = notes.discussion_topics.drain(..)
                .filter(|t| t.base.block_state != BlockState::AiManaged)
                .collect();
            let user_ids: std::collections::HashSet<_> = user_blocks.iter().map(|t| t.base.id.clone()).collect();
            let mut ai_idx = 0;
            notes.discussion_topics = topics
                .iter()
                .filter_map(|t| {
                    let title = t.get("topic_title")?.as_str()?.to_string();
                    let content = t.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    // Find a non-colliding ID
                    while user_ids.contains(&format!("topic-{ai_idx}")) { ai_idx += 1; }
                    let id = format!("topic-{ai_idx}");
                    ai_idx += 1;
                    Some(TopicSection {
                        base: NoteSection {
                            id,
                            content,
                            last_updated_by: Author::Ai,
                            last_updated_at: now,
                            block_state: BlockState::AiManaged,
                            original_ai_content: None,
                        },
                        topic_title: title,
                    })
                })
                .collect();
            notes.discussion_topics.extend(user_blocks);
        }

        // Update decisions — preserve user-touched blocks
        if let Some(decisions) = parsed.get("decisions").and_then(|v| v.as_array()) {
            let user_blocks: Vec<_> = notes.decisions.drain(..)
                .filter(|d| d.base.block_state != BlockState::AiManaged)
                .collect();
            let user_ids: std::collections::HashSet<_> = user_blocks.iter().map(|d| d.base.id.clone()).collect();
            let mut ai_idx = 0;
            notes.decisions = decisions
                .iter()
                .filter_map(|d| {
                    let text = d.get("decision_text")?.as_str()?.to_string();
                    while user_ids.contains(&format!("decision-{ai_idx}")) { ai_idx += 1; }
                    let id = format!("decision-{ai_idx}");
                    ai_idx += 1;
                    Some(DecisionSection {
                        base: NoteSection {
                            id,
                            content: text.clone(),
                            last_updated_by: Author::Ai,
                            last_updated_at: now,
                            block_state: BlockState::AiManaged,
                            original_ai_content: None,
                        },
                        decision_text: text,
                    })
                })
                .collect();
            notes.decisions.extend(user_blocks);
        }

        // Update action items — preserve user-touched blocks
        if let Some(items) = parsed.get("action_items").and_then(|v| v.as_array()) {
            let user_blocks: Vec<_> = notes.action_items.drain(..)
                .filter(|a| a.base.block_state != BlockState::AiManaged)
                .collect();
            let user_ids: std::collections::HashSet<_> = user_blocks.iter().map(|a| a.base.id.clone()).collect();
            let mut ai_idx = 0;
            notes.action_items = items
                .iter()
                .filter_map(|a| {
                    let desc = a.get("description")?.as_str()?.to_string();
                    let assignee = a.get("assignee").and_then(|v| v.as_str()).map(|s| s.to_string());
                    while user_ids.contains(&format!("action-{ai_idx}")) { ai_idx += 1; }
                    let id = format!("action-{ai_idx}");
                    ai_idx += 1;
                    Some(ActionItemSection {
                        base: NoteSection {
                            id,
                            content: desc.clone(),
                            last_updated_by: Author::Ai,
                            last_updated_at: now,
                            block_state: BlockState::AiManaged,
                            original_ai_content: None,
                        },
                        description: desc,
                        assignee,
                    })
                })
                .collect();
            notes.action_items.extend(user_blocks);
        }

        Ok(notes)
    }
}

fn extract_json(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut search_from = 0;
    while let Some(offset) = text[search_from..].find('{') {
        let start = search_from + offset;
        let mut depth = 0;
        for i in start..bytes.len() {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        let candidate = &text[start..=i];
                        if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                            return Some(candidate);
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
        search_from = start + 1;
    }
    None
}

/// Regenerate notes for a saved session with a user instruction.
/// Used by the history view to update notes rather than just answering questions.
pub async fn regenerate_with_instruction(
    notes: &MeetingNotes,
    transcript: &[TranscriptSegment],
    profile: &MeetingProfile,
    instruction: &str,
) -> Result<MeetingNotes, NoteGenError> {
    let current_notes_json = serde_json::to_string_pretty(notes).unwrap_or_default();
    let new_transcript: String = transcript
        .iter()
        .map(|s| {
            let speaker = s.speaker.as_deref().unwrap_or("Unknown");
            let ms = s.start_time_ms;
            format!("[{:02}:{:02}] {}: {}", ms / 60_000, (ms / 1000) % 60, speaker, s.text)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let instructions = format!(
        "{}\n\nUser's instruction (address this with the FULL transcript context):\n{}",
        profile.instructions, instruction
    );
    let prompt = super::prompts::build_prompt(
        super::prompts::ACTION_FULL_REFRESH,
        &profile.name,
        &instructions,
        &current_notes_json,
        &new_transcript,
    );

    let response = call_llm_api(&prompt).await?;
    let json_str = extract_json(&response)
        .ok_or_else(|| NoteGenError::OutputParseError("No JSON found in response".into()))?;
    let parsed: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| NoteGenError::OutputParseError(e.to_string()))?;

    let now = chrono::Utc::now().timestamp_millis() as u64;
    let mut updated = notes.clone();

    if let Some(map) = parsed.get("speaker_map").and_then(|v| v.as_object()) {
        for (generic, real_name) in map {
            if let Some(name) = real_name.as_str() {
                if let Some(pos) = updated.metadata.speakers.iter().position(|s| s == generic) {
                    updated.metadata.speakers[pos] = name.to_string();
                } else if !updated.metadata.speakers.iter().any(|s| s == name) {
                    updated.metadata.speakers.push(name.to_string());
                }
            }
        }
    }
    if let Some(s) = parsed.get("summary").and_then(|v| v.as_str()) {
        updated.summary.content = s.to_string();
        updated.summary.last_updated_at = now;
    }
    if let Some(topics) = parsed.get("discussion_topics").and_then(|v| v.as_array()) {
        updated.discussion_topics = topics.iter().enumerate().filter_map(|(i, t)| {
            let title = t.get("topic_title")?.as_str()?.to_string();
            let content = t.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            Some(TopicSection { base: NoteSection { id: format!("topic-{i}"), content, last_updated_by: Author::Ai, last_updated_at: now, block_state: BlockState::AiManaged, original_ai_content: None }, topic_title: title })
        }).collect();
    }
    if let Some(decisions) = parsed.get("decisions").and_then(|v| v.as_array()) {
        updated.decisions = decisions.iter().enumerate().filter_map(|(i, d)| {
            let text = d.get("decision_text")?.as_str()?.to_string();
            Some(DecisionSection { base: NoteSection { id: format!("decision-{i}"), content: text.clone(), last_updated_by: Author::Ai, last_updated_at: now, block_state: BlockState::AiManaged, original_ai_content: None }, decision_text: text })
        }).collect();
    }
    if let Some(items) = parsed.get("action_items").and_then(|v| v.as_array()) {
        updated.action_items = items.iter().enumerate().filter_map(|(i, a)| {
            let desc = a.get("description")?.as_str()?.to_string();
            let assignee = a.get("assignee").and_then(|v| v.as_str()).map(|s| s.to_string());
            Some(ActionItemSection { base: NoteSection { id: format!("action-{i}"), content: desc.clone(), last_updated_by: Author::Ai, last_updated_at: now, block_state: BlockState::AiManaged, original_ai_content: None }, description: desc, assignee })
        }).collect();
    }

    Ok(updated)
}

/// Call the configured LLM API (Claude or OpenAI) with a prompt and return the text response.
async fn call_llm_api(prompt: &str) -> Result<String, NoteGenError> {
    let cfg = crate::config::AppConfig::get();

    match cfg.ai_provider.as_str() {
        "claude-cli" => {
            use tokio::io::AsyncWriteExt;

            let cli_path = cfg.claude_cli_path.as_deref().unwrap_or("claude").to_string();
            tracing::debug!("Calling Claude CLI (path={cli_path})");

            let mut child = tokio::process::Command::new(&cli_path)
                .arg("-p")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| NoteGenError::ApiError(
                    format!("Failed to launch claude CLI at '{cli_path}': {e}. Make sure the Claude CLI is installed and in your PATH.")
                ))?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(prompt.as_bytes()).await
                    .map_err(|e| NoteGenError::ApiError(format!("Failed to write prompt to claude CLI: {e}")))?;
            }

            let output = child.wait_with_output().await
                .map_err(|e| NoteGenError::ApiError(format!("Claude CLI process error: {e}")))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(NoteGenError::ApiError(format!("Claude CLI exited with error: {stderr}")));
            }

            let text = String::from_utf8(output.stdout)
                .map_err(|e| NoteGenError::ApiError(format!("Invalid UTF-8 in claude CLI output: {e}")))?;

            Ok(text.trim().to_string())
        }
        "openai" => {
            let api_key = cfg.openai_api_key.as_ref()
                .ok_or_else(|| NoteGenError::ApiError(
                    "OpenAI API key not configured. Please add it in Settings.".into()
                ))?;
            let model = cfg.openai_model.as_deref().unwrap_or("gpt-4o");

            let client = reqwest::Client::new();
            let body = serde_json::json!({
                "model": model,
                "messages": [{"role": "user", "content": prompt}],
                "max_tokens": 8096,
            });

            tracing::debug!("Calling OpenAI API (model={model})");
            let resp = client
                .post("https://api.openai.com/v1/chat/completions")
                .header("Authorization", format!("Bearer {api_key}"))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| NoteGenError::ApiError(format!("OpenAI request failed: {e}")))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                return Err(NoteGenError::ApiError(format!("OpenAI API error {status}: {text}")));
            }

            let json: serde_json::Value = resp.json().await
                .map_err(|e| NoteGenError::ApiError(format!("Failed to parse OpenAI response: {e}")))?;

            json["choices"][0]["message"]["content"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| NoteGenError::ApiError("Unexpected OpenAI response format".into()))
        }
        _ => {
            // Default: Claude
            let api_key = cfg.claude_api_key.as_ref()
                .ok_or_else(|| NoteGenError::ApiError(
                    "Claude API key not configured. Please add it in Settings.".into()
                ))?;
            let model = cfg.claude_model.as_deref().unwrap_or("claude-sonnet-4-6");

            let client = reqwest::Client::new();
            let body = serde_json::json!({
                "model": model,
                "max_tokens": 8096,
                "messages": [{"role": "user", "content": prompt}],
            });

            tracing::debug!("Calling Claude API (model={model})");
            let resp = client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", api_key.as_str())
                .header("anthropic-version", "2023-06-01")
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| NoteGenError::ApiError(format!("Claude request failed: {e}")))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                return Err(NoteGenError::ApiError(format!("Claude API error {status}: {text}")));
            }

            let json: serde_json::Value = resp.json().await
                .map_err(|e| NoteGenError::ApiError(format!("Failed to parse Claude response: {e}")))?;

            json["content"][0]["text"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| NoteGenError::ApiError("Unexpected Claude response format".into()))
        }
    }
}

fn format_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let mins = secs / 60;
    let secs = secs % 60;
    format!("{mins:02}:{secs:02}")
}

/// Sync user-edited block states from shared into local section vec.
/// Adds any user-added blocks that don't exist locally, and updates block_state for existing ones.
fn sync_section_states<T: HasBase + Clone>(local: &mut Vec<T>, shared: &[T]) {
    for shared_item in shared {
        let sb = shared_item.base_ref();
        if sb.block_state == BlockState::AiManaged {
            continue;
        }
        if let Some(local_item) = local.iter_mut().find(|l| l.base_ref().id == sb.id) {
            *local_item.base_mut() = sb.clone();
        } else {
            // User-added block not in local — append it
            local.push(shared_item.clone());
        }
    }
}

trait HasBase {
    fn base_ref(&self) -> &NoteSection;
    fn base_mut(&mut self) -> &mut NoteSection;
}

impl HasBase for TopicSection {
    fn base_ref(&self) -> &NoteSection { &self.base }
    fn base_mut(&mut self) -> &mut NoteSection { &mut self.base }
}

impl HasBase for DecisionSection {
    fn base_ref(&self) -> &NoteSection { &self.base }
    fn base_mut(&mut self) -> &mut NoteSection { &mut self.base }
}

impl HasBase for ActionItemSection {
    fn base_ref(&self) -> &NoteSection { &self.base }
    fn base_mut(&mut self) -> &mut NoteSection { &mut self.base }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_clean() {
        let input = r#"{"summary": "hello"}"#;
        assert_eq!(extract_json(input), Some(input));
    }

    #[test]
    fn extract_json_with_preamble() {
        let input = r#"Here is the JSON:
{"summary": "test", "items": [{"a": 1}]}"#;
        let result = extract_json(input).unwrap();
        assert!(result.starts_with('{'));
        assert!(result.ends_with('}'));
        assert!(serde_json::from_str::<serde_json::Value>(result).is_ok());
    }

    #[test]
    fn extract_json_skips_invalid_braces() {
        let input = r#"error {0} occurred, here is result: {"summary": "ok"}"#;
        let result = extract_json(input).unwrap();
        assert_eq!(result, r#"{"summary": "ok"}"#);
    }

    #[test]
    fn extract_json_nested_braces() {
        let input = r#"{"outer": {"inner": "value"}}"#;
        assert_eq!(extract_json(input), Some(input));
    }

    #[test]
    fn extract_json_no_json() {
        assert_eq!(extract_json("no json here"), None);
    }

    #[test]
    fn extract_json_unclosed() {
        assert_eq!(extract_json("{ unclosed"), None);
    }

    #[test]
    fn format_ms_zero() {
        assert_eq!(format_ms(0), "00:00");
    }

    #[test]
    fn format_ms_minutes_and_seconds() {
        assert_eq!(format_ms(125_000), "02:05");
    }

    #[test]
    fn format_ms_over_an_hour() {
        assert_eq!(format_ms(3_661_000), "61:01");
    }
}
