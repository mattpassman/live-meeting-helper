use std::path::PathBuf;

const APP_NAME_WIN: &str = "Live Meeting Helper";
const APP_NAME_UNIX: &str = "live-meeting-helper";

/// Config root — roaming on Windows, ~/.config on Linux, ~/Library/Application Support on macOS.
/// Windows: C:\Users\<user>\AppData\Roaming\Live Meeting Helper
/// macOS:   ~/Library/Application Support/live-meeting-helper
/// Linux:   ~/.config/live-meeting-helper
pub fn config_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        dirs::data_dir() // AppData\Roaming
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_NAME_WIN)
    }
    #[cfg(not(target_os = "windows"))]
    {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_NAME_UNIX)
    }
}

/// Local data root — for logs, sessions, cache (machine-specific, not roamed).
/// Windows: C:\Users\<user>\AppData\Local\Live Meeting Helper
/// macOS:   ~/Library/Application Support/live-meeting-helper
/// Linux:   ~/.local/share/live-meeting-helper
pub fn data_local_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        dirs::data_local_dir() // AppData\Local
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_NAME_WIN)
    }
    #[cfg(not(target_os = "windows"))]
    {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_NAME_UNIX)
    }
}

pub fn log_dir() -> PathBuf {
    data_local_dir().join("logs")
}

pub fn sessions_dir() -> PathBuf {
    data_local_dir().join("sessions")
}

pub fn profiles_dir() -> PathBuf {
    config_dir().join("profiles")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.json")
}
