use crate::notes::MeetingNotes;
use crate::profile::MeetingProfile;
use crate::types::{SessionState, TranscriptSegment};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub session_id: String,
    pub title: String,
    pub state: SessionState,
    pub start_time: u64,
    pub end_time: Option<u64>,
    pub profile: MeetingProfile,
    pub notes: MeetingNotes,
    pub transcript: Vec<TranscriptSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub title: String,
    pub start_time: u64,
    pub end_time: Option<u64>,
    pub state: SessionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    PlainText,
}

pub struct PersistenceService {
    pub(crate) data_dir: std::path::PathBuf,
}

impl PersistenceService {
    pub fn new() -> Self {
        let data_dir = crate::paths::sessions_dir();
        std::fs::create_dir_all(&data_dir).ok();
        Self { data_dir }
    }

    pub fn save_session(&self, session: &SessionData) -> std::io::Result<()> {
        let path = self.data_dir.join(format!("{}.json", session.session_id));
        let data = serde_json::to_string_pretty(session)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        atomic_write(&path, data.as_bytes())
    }

    pub fn load_session(&self, session_id: &str) -> Option<SessionData> {
        let path = self.data_dir.join(format!("{session_id}.json"));
        let data = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }

    pub fn list_sessions(&self) -> Vec<SessionSummary> {
        let mut sessions = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.data_dir) {
            for entry in entries.flatten() {
                if entry.path().extension().map_or(false, |e| e == "json") {
                    if let Ok(data) = std::fs::read_to_string(entry.path()) {
                        if let Ok(session) = serde_json::from_str::<SessionData>(&data) {
                            sessions.push(SessionSummary {
                                session_id: session.session_id,
                                title: session.title,
                                start_time: session.start_time,
                                end_time: session.end_time,
                                state: session.state,
                            });
                        }
                    }
                }
            }
        }
        sessions
    }

    pub fn export_notes(&self, session_id: &str, format: ExportFormat) -> Option<String> {
        let session = self.load_session(session_id)?;
        Some(match format {
            ExportFormat::Markdown => crate::notes::render_notes_as_markdown(&session.notes),
            ExportFormat::PlainText => render_plain_text(&session.notes),
        })
    }

    pub fn delete_session(&self, session_id: &str) -> std::io::Result<()> {
        let path = self.data_dir.join(format!("{session_id}.json"));
        std::fs::remove_file(path)
    }
}

fn render_plain_text(notes: &MeetingNotes) -> String {
    let md = crate::notes::render_notes_as_markdown(notes);
    md.replace("# ", "")
        .replace("## ", "")
        .replace("### ", "")
        .replace("**", "")
        .replace("- [ ] ", "  * ")
        .replace("- ", "  * ")
}

fn atomic_write(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AudioSource, SessionState, TranscriptSegment};
    use crate::profile::ProfileService;

    fn test_persistence() -> (PersistenceService, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let svc = PersistenceService {
            data_dir: dir.path().to_path_buf(),
        };
        (svc, dir)
    }

    fn sample_session(id: &str) -> SessionData {
        SessionData {
            session_id: id.to_string(),
            title: "Test Meeting".into(),
            state: SessionState::Completed,
            start_time: 1700000000000,
            end_time: Some(1700003600000),
            profile: ProfileService::default_profile(),
            notes: MeetingNotes::new(id, "Test Meeting", AudioSource::Microphone),
            transcript: vec![TranscriptSegment {
                id: "seg-0".into(),
                text: "Hello everyone".into(),
                start_time_ms: 0,
                end_time_ms: 2000,
                speaker: Some("spk_0".into()),
                confidence: 0.95,
                is_final: false,
            }],
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let (svc, _dir) = test_persistence();
        let session = sample_session("sess-1");
        svc.save_session(&session).unwrap();
        let loaded = svc.load_session("sess-1").unwrap();
        assert_eq!(loaded.session_id, "sess-1");
        assert_eq!(loaded.title, "Test Meeting");
        assert_eq!(loaded.transcript.len(), 1);
        assert_eq!(loaded.transcript[0].text, "Hello everyone");
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let (svc, _dir) = test_persistence();
        assert!(svc.load_session("does-not-exist").is_none());
    }

    #[test]
    fn list_sessions_returns_saved() {
        let (svc, _dir) = test_persistence();
        svc.save_session(&sample_session("a")).unwrap();
        svc.save_session(&sample_session("b")).unwrap();
        let list = svc.list_sessions();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn delete_session_removes_file() {
        let (svc, _dir) = test_persistence();
        svc.save_session(&sample_session("del-me")).unwrap();
        assert!(svc.load_session("del-me").is_some());
        svc.delete_session("del-me").unwrap();
        assert!(svc.load_session("del-me").is_none());
    }

    #[test]
    fn export_markdown() {
        let (svc, _dir) = test_persistence();
        let mut session = sample_session("export-1");
        session.notes.summary.content = "A productive meeting".into();
        svc.save_session(&session).unwrap();
        let md = svc.export_notes("export-1", ExportFormat::Markdown).unwrap();
        assert!(md.contains("# Meeting:"));
        assert!(md.contains("A productive meeting"));
    }
}
