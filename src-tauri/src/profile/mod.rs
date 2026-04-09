use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingProfile {
    pub id: String,
    pub name: String,
    pub sections: Vec<SectionConfig>,
    pub instructions: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionConfig {
    pub section_type: SectionType,
    pub enabled: bool,
    pub custom_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SectionType {
    Summary,
    DiscussionTopics,
    Decisions,
    ActionItems,
    Custom(String),
}

pub struct ProfileService {
    data_dir: std::path::PathBuf,
}

impl ProfileService {
    pub fn new() -> Self {
        let data_dir = crate::paths::profiles_dir();
        std::fs::create_dir_all(&data_dir).ok();
        Self { data_dir }
    }

    pub fn default_profile() -> MeetingProfile {
        MeetingProfile {
            id: "default".to_string(),
            name: "Default".to_string(),
            sections: vec![
                SectionConfig {
                    section_type: SectionType::Summary,
                    enabled: true,
                    custom_name: None,
                },
                SectionConfig {
                    section_type: SectionType::DiscussionTopics,
                    enabled: true,
                    custom_name: None,
                },
                SectionConfig {
                    section_type: SectionType::Decisions,
                    enabled: true,
                    custom_name: None,
                },
                SectionConfig {
                    section_type: SectionType::ActionItems,
                    enabled: true,
                    custom_name: None,
                },
            ],
            instructions: String::new(),
        }
    }

    pub fn list_profiles(&self) -> Vec<MeetingProfile> {
        let mut profiles = vec![Self::default_profile()];
        if let Ok(entries) = std::fs::read_dir(&self.data_dir) {
            for entry in entries.flatten() {
                if entry.path().extension().map_or(false, |e| e == "json") {
                    if let Ok(data) = std::fs::read_to_string(entry.path()) {
                        if let Ok(profile) = serde_json::from_str::<MeetingProfile>(&data) {
                            profiles.push(profile);
                        }
                    }
                }
            }
        }
        profiles
    }

    pub fn save_profile(&self, profile: &MeetingProfile) -> std::io::Result<()> {
        let path = self.data_dir.join(format!("{}.json", profile.id));
        let data = serde_json::to_string_pretty(profile)?;
        atomic_write(&path, data.as_bytes())
    }

    pub fn get_profile(&self, id: &str) -> Option<MeetingProfile> {
        if id == "default" {
            return Some(Self::default_profile());
        }
        let path = self.data_dir.join(format!("{id}.json"));
        let data = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }

    pub fn delete_profile(&self, id: &str) -> std::io::Result<()> {
        let path = self.data_dir.join(format!("{id}.json"));
        std::fs::remove_file(path)
    }
}

fn dirs_path() -> std::path::PathBuf {
    crate::paths::config_dir()
}

fn atomic_write(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}
