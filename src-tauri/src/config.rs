use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::RwLock;

static APP_CONFIG: RwLock<Option<AppConfig>> = RwLock::new(None);

/// App-level configuration stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// AI provider to use for note generation: "claude" (default), "openai", or "claude-cli"
    #[serde(default = "default_ai_provider")]
    pub ai_provider: String,

    /// Anthropic API key for Claude note generation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_api_key: Option<String>,

    /// Claude model to use (defaults to claude-sonnet-4-6)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_model: Option<String>,

    /// Path to the claude CLI binary (defaults to "claude" in PATH).
    /// Used when ai_provider is "claude-cli".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_cli_path: Option<String>,

    /// OpenAI API key for GPT note generation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_api_key: Option<String>,

    /// OpenAI model to use (defaults to gpt-4o)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_model: Option<String>,

    /// Transcription backend: "whisper" (default, local) or "aws"
    #[serde(default = "default_transcription_provider")]
    pub transcription_provider: String,

    /// Path to a Whisper GGML model file for local transcription
    /// Download from https://huggingface.co/ggerganov/whisper.cpp
    /// Recommended: ggml-base.en.bin (~142 MB) for English
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whisper_model_path: Option<String>,

    /// AWS profile name to use for Transcribe (e.g. "default")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_profile: Option<String>,

    /// AWS region override (defaults to us-east-1 if unset everywhere)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_region: Option<String>,

    /// Audio input device name (substring match). If unset, uses system default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_device: Option<String>,

    /// Enable verbose (debug-level) logging
    #[serde(default)]
    pub verbose_logging: bool,

    /// Whether the first-run onboarding wizard has been completed or skipped
    #[serde(default)]
    pub setup_complete: bool,
}

fn default_ai_provider() -> String {
    "claude".to_string()
}

fn default_transcription_provider() -> String {
    "whisper".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ai_provider: default_ai_provider(),
            claude_api_key: None,
            claude_model: None,
            claude_cli_path: None,
            openai_api_key: None,
            openai_model: None,
            transcription_provider: default_transcription_provider(), // "whisper"
            whisper_model_path: None,
            aws_profile: None,
            aws_region: None,
            audio_device: None,
            verbose_logging: false,
            setup_complete: false,
        }
    }
}

impl AppConfig {
    /// Load config from disk and store globally. Returns a clone.
    pub fn init() -> Self {
        let cfg = Self::load_from_disk();
        *APP_CONFIG.write().unwrap() = Some(cfg.clone());
        cfg
    }

    /// Get the global config (must call init() first).
    pub fn get() -> Self {
        APP_CONFIG.read().unwrap().clone().expect("AppConfig::init() not called")
    }

    fn load_from_disk() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_else(|e| {
                tracing::warn!("Failed to parse config at {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => {
                tracing::info!("No config file at {}, using defaults", path.display());
                Self::default()
            }
        }
    }

    /// Save config to disk and update the global instance.
    pub fn save(&self) -> std::io::Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&path, data)?;
        *APP_CONFIG.write().unwrap() = Some(self.clone());
        tracing::info!("Config saved to {}", path.display());
        Ok(())
    }
}

/// Returns the config file path:
/// - Windows: C:\Users\<user>\AppData\Roaming\Live Meeting Helper\config.json
/// - macOS:   ~/Library/Application Support/live-meeting-helper/config.json
/// - Linux:   ~/.config/live-meeting-helper/config.json
fn config_path() -> PathBuf {
    crate::paths::config_file()
}
