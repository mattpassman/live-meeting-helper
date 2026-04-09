pub mod manager;

use crate::types::AudioSource;
use crate::notes::MeetingNotes;
use crate::profile::MeetingProfile;

pub use crate::types::SessionState;
pub use manager::SessionManager;

pub enum SessionEvent {
    NotesUpdated(MeetingNotes),
    StateChanged(SessionState),
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub audio_source: AudioSource,
    pub mic_device: Option<String>,
    pub title: Option<String>,
    pub profile: MeetingProfile,
}

#[derive(Debug, Clone)]
pub enum SessionCommand {
    Start(SessionConfig),
    Pause,
    Resume,
    Stop,
}
