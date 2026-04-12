pub mod corrections;
pub mod generator;
pub mod prompts;
#[cfg(target_os = "macos")]
pub mod spawn_mac;

pub use corrections::Correction;
pub use generator::NotesUpdate;

use crate::types::AudioSource;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockState {
    AiManaged,
    UserEdited,
    UserAdded,
    UserDeleted,
}

impl Default for BlockState {
    fn default() -> Self {
        Self::AiManaged
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingNotes {
    pub session_id: String,
    pub title: String,
    pub summary: NoteSection,
    pub discussion_topics: Vec<TopicSection>,
    pub decisions: Vec<DecisionSection>,
    pub action_items: Vec<ActionItemSection>,
    pub custom_sections: Vec<CustomSection>,
    pub metadata: MeetingMetadata,
    #[serde(default)]
    pub corrections: Vec<Correction>,
}

impl MeetingNotes {
    pub fn new(session_id: &str, title: &str, audio_source: AudioSource) -> Self {
        let now = chrono::Utc::now().timestamp_millis() as u64;
        Self {
            session_id: session_id.to_string(),
            title: title.to_string(),
            summary: NoteSection {
                id: "section-summary".to_string(),
                content: String::new(),
                last_updated_by: Author::Ai,
                last_updated_at: now,
                block_state: BlockState::AiManaged,
                original_ai_content: None,
            },
            discussion_topics: Vec::new(),
            decisions: Vec::new(),
            action_items: Vec::new(),
            custom_sections: Vec::new(),
            metadata: MeetingMetadata {
                start_time: now,
                end_time: None,
                duration_ms: None,
                speakers: Vec::new(),
                audio_source,
            },
            corrections: Vec::new(),
        }
    }

    /// Find a mutable reference to any NoteSection by its ID.
    pub fn find_block_mut(&mut self, block_id: &str) -> Option<&mut NoteSection> {
        if self.summary.id == block_id {
            return Some(&mut self.summary);
        }
        for t in &mut self.discussion_topics {
            if t.base.id == block_id {
                return Some(&mut t.base);
            }
        }
        for d in &mut self.decisions {
            if d.base.id == block_id {
                return Some(&mut d.base);
            }
        }
        for a in &mut self.action_items {
            if a.base.id == block_id {
                return Some(&mut a.base);
            }
        }
        for c in &mut self.custom_sections {
            if c.base.id == block_id {
                return Some(&mut c.base);
            }
        }
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteSection {
    pub id: String,
    pub content: String,
    pub last_updated_by: Author,
    pub last_updated_at: u64,
    #[serde(default)]
    pub block_state: BlockState,
    #[serde(default)]
    pub original_ai_content: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Author {
    Ai,
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicSection {
    #[serde(flatten)]
    pub base: NoteSection,
    pub topic_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionSection {
    #[serde(flatten)]
    pub base: NoteSection,
    pub decision_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionItemSection {
    #[serde(flatten)]
    pub base: NoteSection,
    pub description: String,
    pub assignee: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomSection {
    #[serde(flatten)]
    pub base: NoteSection,
    pub section_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingMetadata {
    pub start_time: u64,
    pub end_time: Option<u64>,
    pub duration_ms: Option<u64>,
    pub speakers: Vec<String>,
    pub audio_source: AudioSource,
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum NoteGenError {
    #[error("LLM API error: {0}")]
    ApiError(String),
    #[error("failed to parse LLM output: {0}")]
    OutputParseError(String),
    #[error("note generation error: {0}")]
    Other(String),
}

/// Render notes as Markdown for export/clipboard
pub fn render_notes_as_markdown(notes: &MeetingNotes) -> String {
    let mut md = String::new();
    md.push_str(&format!("# Meeting: {}\n", notes.title));

    let date = chrono::DateTime::from_timestamp_millis(notes.metadata.start_time as i64)
        .map(|dt| dt.format("%B %d, %Y %H:%M UTC").to_string())
        .unwrap_or_default();
    md.push_str(&format!("**Date:** {date}\n"));

    if let Some(dur) = notes.metadata.duration_ms {
        md.push_str(&format!("**Duration:** {} minutes\n", dur / 60_000));
    }
    if !notes.metadata.speakers.is_empty() {
        md.push_str(&format!(
            "**Speakers:** {}\n",
            notes.metadata.speakers.join(", ")
        ));
    }
    md.push('\n');

    if !notes.summary.content.is_empty() && notes.summary.block_state != BlockState::UserDeleted {
        md.push_str("## Summary\n");
        md.push_str(&notes.summary.content);
        md.push_str("\n\n");
    }

    let visible_topics: Vec<_> = notes.discussion_topics.iter()
        .filter(|t| t.base.block_state != BlockState::UserDeleted)
        .collect();
    if !visible_topics.is_empty() {
        md.push_str("## Discussion Topics\n");
        for topic in visible_topics {
            md.push_str(&format!("### {}\n", topic.topic_title));
            md.push_str(&topic.base.content);
            md.push_str("\n\n");
        }
    }

    let visible_decisions: Vec<_> = notes.decisions.iter()
        .filter(|d| d.base.block_state != BlockState::UserDeleted)
        .collect();
    if !visible_decisions.is_empty() {
        md.push_str("## Decisions\n");
        for d in visible_decisions {
            md.push_str(&format!("- {}\n", d.decision_text));
        }
        md.push('\n');
    }

    let visible_actions: Vec<_> = notes.action_items.iter()
        .filter(|a| a.base.block_state != BlockState::UserDeleted)
        .collect();
    if !visible_actions.is_empty() {
        md.push_str("## Action Items\n");
        for ai in visible_actions {
            let assignee = ai.assignee.as_deref().unwrap_or("Unassigned");
            md.push_str(&format!("- [ ] @{}: {}\n", assignee, ai.description));
        }
        md.push('\n');
    }

    for cs in &notes.custom_sections {
        if cs.base.block_state == BlockState::UserDeleted {
            continue;
        }
        md.push_str(&format!("## {}\n", cs.section_name));
        md.push_str(&cs.base.content);
        md.push_str("\n\n");
    }

    md
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_notes() -> MeetingNotes {
        let mut notes = MeetingNotes::new("test-1", "Sprint Planning", AudioSource::Microphone);
        notes.summary.content = "Discussed sprint goals and priorities.".into();
        notes.discussion_topics.push(TopicSection {
            base: NoteSection {
                id: "topic-0".into(),
                content: "- Reviewed backlog items\n- Estimated story points".into(),
                last_updated_by: Author::Ai,
                last_updated_at: 0,
                block_state: BlockState::AiManaged,
                original_ai_content: None,
            },
            topic_title: "Backlog Review".into(),
        });
        notes.decisions.push(DecisionSection {
            base: NoteSection {
                id: "decision-0".into(),
                content: "Focus on API work this sprint".into(),
                last_updated_by: Author::Ai,
                last_updated_at: 0,
                block_state: BlockState::AiManaged,
                original_ai_content: None,
            },
            decision_text: "Focus on API work this sprint".into(),
        });
        notes.action_items.push(ActionItemSection {
            base: NoteSection {
                id: "action-0".into(),
                content: "Write design doc".into(),
                last_updated_by: Author::Ai,
                last_updated_at: 0,
                block_state: BlockState::AiManaged,
                original_ai_content: None,
            },
            description: "Write design doc".into(),
            assignee: Some("Matt".into()),
        });
        notes.metadata.speakers = vec!["Matt".into(), "Sarah".into()];
        notes
    }

    #[test]
    fn render_markdown_includes_all_sections() {
        let notes = sample_notes();
        let md = render_notes_as_markdown(&notes);
        assert!(md.contains("# Meeting: Sprint Planning"));
        assert!(md.contains("## Summary"));
        assert!(md.contains("sprint goals"));
        assert!(md.contains("## Discussion Topics"));
        assert!(md.contains("### Backlog Review"));
        assert!(md.contains("## Decisions"));
        assert!(md.contains("Focus on API work"));
        assert!(md.contains("## Action Items"));
        assert!(md.contains("@Matt"));
        assert!(md.contains("Write design doc"));
        assert!(md.contains("Matt, Sarah"));
    }

    #[test]
    fn render_markdown_empty_notes() {
        let notes = MeetingNotes::new("test-2", "Empty", AudioSource::Microphone);
        let md = render_notes_as_markdown(&notes);
        assert!(md.contains("# Meeting: Empty"));
        assert!(!md.contains("## Summary"));
        assert!(!md.contains("## Decisions"));
    }

    #[test]
    fn render_markdown_action_item_no_assignee() {
        let mut notes = MeetingNotes::new("test-3", "Test", AudioSource::Microphone);
        notes.action_items.push(ActionItemSection {
            base: NoteSection {
                id: "action-0".into(),
                content: "Do something".into(),
                last_updated_by: Author::Ai,
                last_updated_at: 0,
                block_state: BlockState::AiManaged,
                original_ai_content: None,
            },
            description: "Do something".into(),
            assignee: None,
        });
        let md = render_notes_as_markdown(&notes);
        assert!(md.contains("@Unassigned"));
    }

    #[test]
    fn render_markdown_skips_deleted_blocks() {
        let mut notes = sample_notes();
        notes.decisions[0].base.block_state = BlockState::UserDeleted;
        notes.action_items[0].base.block_state = BlockState::UserDeleted;
        let md = render_notes_as_markdown(&notes);
        assert!(!md.contains("## Decisions"));
        assert!(!md.contains("## Action Items"));
        assert!(md.contains("## Discussion Topics"));
    }

    #[test]
    fn render_markdown_includes_user_added_blocks() {
        let mut notes = sample_notes();
        notes.decisions.push(DecisionSection {
            base: NoteSection {
                id: "user-dec-1".into(),
                content: "User added decision".into(),
                last_updated_by: Author::User,
                last_updated_at: 0,
                block_state: BlockState::UserAdded,
                original_ai_content: None,
            },
            decision_text: "User added decision".into(),
        });
        let md = render_notes_as_markdown(&notes);
        assert!(md.contains("User added decision"));
    }

    #[test]
    fn find_block_mut_across_sections() {
        let mut notes = sample_notes();
        assert!(notes.find_block_mut("section-summary").is_some());
        assert!(notes.find_block_mut("topic-0").is_some());
        assert!(notes.find_block_mut("decision-0").is_some());
        assert!(notes.find_block_mut("action-0").is_some());
        assert!(notes.find_block_mut("nonexistent").is_none());
    }

    #[test]
    fn find_block_mut_allows_edit() {
        let mut notes = sample_notes();
        let block = notes.find_block_mut("decision-0").unwrap();
        block.content = "Updated by user".into();
        block.block_state = BlockState::UserEdited;
        assert_eq!(notes.decisions[0].base.content, "Updated by user");
        assert_eq!(notes.decisions[0].base.block_state, BlockState::UserEdited);
    }
}
