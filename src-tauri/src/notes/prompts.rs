/// Prompt templates for note generation.
///
/// Each LLM API call is stateless — the FULL prompt (SYSTEM_PROMPT with
/// an action + all context) is sent every time. There is no conversation history
/// between calls. The prompt includes:
///   - {action}             — one of ACTION_INCREMENTAL, ACTION_FULL_REFRESH, or ACTION_FINAL
///   - {profile_name}       — the meeting profile name (e.g. "Default", "Sprint Retro")
///   - {instructions}       — profile instructions + any ad-hoc instructions from the user
///   - {current_notes_json} — the current notes as a JSON object (or "null" if first run)
///   - {new_transcript}     — formatted transcript lines: "[MM:SS] speaker: text"

/// Action line for incremental updates (new segments only, every ~3s pause in speech).
/// Receives: only the NEW transcript segments since last generation + current notes.
pub const ACTION_INCREMENTAL: &str =
    "Update the meeting notes with the new transcript content. \
     Refine existing sections and add new topics/decisions/action items as needed. \
     IMPORTANT: Rewrite the summary from scratch each time — it must stay 1-2 sentences max.";

/// Action line for full context refresh (every 5 minutes).
/// Receives: the COMPLETE transcript from the start of the meeting + current notes.
pub const ACTION_FULL_REFRESH: &str =
    "FULL CONTEXT REFRESH: Regenerate the meeting notes using the COMPLETE transcript below. \
     Use the current notes as a starting point — they may contain errors or drift from \
     incremental updates. Correct any inaccuracies and ensure all topics, decisions, \
     and action items are captured.";

/// Action line for the final pass (when user clicks Stop).
/// Receives: the COMPLETE transcript from the start of the meeting + current notes.
pub const ACTION_FINAL: &str =
    "FINAL PASS: Generate comprehensive final meeting notes using the COMPLETE transcript below. \
     Use the current notes as a starting point — they may contain errors or drift from \
     incremental updates. Correct any inaccuracies, fill in gaps, and produce polished final notes.";

/// The main system prompt template. All placeholders are filled at runtime.
pub const SYSTEM_PROMPT: &str = r#"You are a meeting notes assistant producing well-structured, readable notes. {action}

Meeting Profile: {profile_name}
Instructions: {instructions}

SPEAKER IDENTIFICATION:
The transcript uses generic speaker labels (spk_0, spk_1, etc.) from automatic transcription.
You SHOULD infer real names when possible:
- If someone is addressed by name ("Hey Matt, what do you think?"), the next speaker is likely that person.
- If someone introduces themselves ("This is Sarah from the platform team"), map that label to the name.
- Once you identify a speaker, use their real name consistently in all sections.
- If you cannot infer a name, keep the generic label (spk_0).
Include a "speaker_map" in your response mapping generic labels to inferred names.

FORMATTING RULES:
- summary: A concise 1-2 sentence overview of the meeting so far. NEVER more than 2 sentences — rewrite, don't append.
- action_items: Each item should include description, owner (real name if known), and due date if mentioned. Format dates as relative (tomorrow) or MM/DD. e.g. "Submit request to owning team. Matt (05/25)"
- decisions: Short, clear statement of what was decided.
- discussion_topics: Each topic gets a clear title. The content should synthesize the conversation into complete thoughts and key points — not just single-sentence bullets. Use markdown formatting with bullet points (- ) for structure, but each point should be a full, meaningful statement that captures the substance of what was discussed.
- Prefer bullet points over prose. Keep notes concise and scannable.
- Do NOT repeat the same information across sections.
- The transcription may have mistakes or mis-transcriptions. Make sensible assumptions where appropriate.

Current notes state:
{current_notes_json}
{user_constraints}
New transcript content:
{new_transcript}

Respond with ONLY a valid JSON object matching this structure (no markdown fences, no explanation):
{{
  "summary": "concise overview",
  "speaker_map": {{"spk_0": "Matt", "spk_1": "Sarah"}},
  "action_items": [{{"description": "task with owner and date if known", "assignee": "person or null"}}],
  "decisions": [{{"decision_text": "what was decided"}}],
  "discussion_topics": [{{"topic_title": "title", "content": "- complete thought synthesizing the discussion\n- another key point"}}]
}}"#;

/// Build the final prompt string with all placeholders filled.
pub fn build_prompt(
    action: &str,
    profile_name: &str,
    instructions: &str,
    current_notes_json: &str,
    new_transcript: &str,
) -> String {
    build_prompt_with_constraints(action, profile_name, instructions, current_notes_json, new_transcript, "")
}

/// Build prompt with user edit constraints injected.
pub fn build_prompt_with_constraints(
    action: &str,
    profile_name: &str,
    instructions: &str,
    current_notes_json: &str,
    new_transcript: &str,
    user_constraints: &str,
) -> String {
    SYSTEM_PROMPT
        .replace("{action}", action)
        .replace("{profile_name}", profile_name)
        .replace("{instructions}", instructions)
        .replace("{current_notes_json}", current_notes_json)
        .replace("{user_constraints}", user_constraints)
        .replace("{new_transcript}", new_transcript)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prompt_fills_all_placeholders() {
        let prompt = build_prompt("ACT", "Prof", "Instr", "{}", "[00:00] hi");
        assert!(prompt.contains("ACT"));
        assert!(prompt.contains("Prof"));
        assert!(prompt.contains("Instr"));
        assert!(!prompt.contains("{action}"));
        assert!(!prompt.contains("{profile_name}"));
        assert!(!prompt.contains("{user_constraints}"));
    }

    #[test]
    fn build_prompt_with_constraints_injects_user_content() {
        let constraints = "USER-LOCKED CONTENT:\n- [Decision] \"Use Postgres\"";
        let prompt = build_prompt_with_constraints("ACT", "P", "I", "{}", "t", constraints);
        assert!(prompt.contains("USER-LOCKED CONTENT"));
        assert!(prompt.contains("Use Postgres"));
    }
}
