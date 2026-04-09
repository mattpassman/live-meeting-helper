#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use live_meeting_helper::commands::AppState;
use live_meeting_helper::{commands, config, paths, persistence, profile, session};
use std::sync::Arc;
use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Manager,
};
use tokio::sync::Mutex;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() {
    // Load config early so we can use verbose_logging for the log filter
    let cfg = config::AppConfig::init();

    // Logging — one file per day, auto-delete > 7 days old
    let log_dir = paths::log_dir();
    std::fs::create_dir_all(&log_dir).ok();
    cleanup_old_logs(&log_dir, 7);

    let today = chrono::Local::now().format("%Y-%m-%d");
    let log_path = log_dir.join(format!("app-{today}.log"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();

    let level = if cfg.verbose_logging { "debug" } else { "info" };
    let filter = EnvFilter::from_default_env()
        .add_directive(format!("live_meeting_helper={level}").parse().unwrap());

    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stdout));

    if let Some(file) = file {
        registry
            .with(fmt::layer().with_ansi(false).with_writer(std::sync::Mutex::new(file)))
            .init();
    } else {
        registry.init();
    }

    tracing::info!("Live Meeting Helper (Tauri) starting...");

    let state = AppState {
        session_manager: Arc::new(Mutex::new(None)),
        persistence: Arc::new(persistence::PersistenceService::new()),
        profiles: Arc::new(profile::ProfileService::new()),
        cmd_tx: Arc::new(Mutex::new(None)),
        meeting_title: Arc::new(Mutex::new("Meeting".into())),
        pending_doc: Arc::new(Mutex::new(None)),
        live_notes: Arc::new(Mutex::new(None)),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::start_meeting,
            commands::pause_meeting,
            commands::resume_meeting,
            commands::stop_meeting,
            commands::get_session_state,
            commands::list_sessions,
            commands::get_session,
            commands::export_session,
            commands::delete_session,
            commands::list_profiles,
            commands::get_profile,
            commands::save_profile,
            commands::delete_profile,
            commands::get_config,
            commands::save_config,
            commands::list_audio_devices,
            commands::send_instruction,
            commands::update_meeting_title,
            commands::query_session,
            commands::attach_document_text,
            commands::attach_document_file,
            commands::edit_note_block,
            commands::add_note_block,
            commands::delete_note_block,
            commands::restore_note_block,
            commands::get_corrections,
            commands::remove_correction,
            commands::save_session_file,
            commands::test_ai_connection,
            commands::mark_setup_complete,
            #[cfg(feature = "whisper")]
            commands::download_whisper_model,
        ])
        .setup(|app| {
            // System tray
            let show = MenuItemBuilder::with_id("show", "Show Window").build(app)?;
            let pause = MenuItemBuilder::with_id("pause", "Pause").build(app)?;
            let stop = MenuItemBuilder::with_id("stop", "Stop Meeting").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&show, &pause, &stop, &quit])
                .build()?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Live Meeting Helper")
                .icon(app.default_window_icon().cloned().unwrap_or_else(|| Image::new_owned(vec![0; 4], 1, 1)))
                .on_menu_event(move |app, event| {
                    match event.id().as_ref() {
                        "show" => {
                            if let Some(w) = app.get_webview_window("main") {
                                w.show().ok();
                                w.set_focus().ok();
                            }
                        }
                        "pause" => {
                            let state = app.state::<AppState>();
                            let tx = state.cmd_tx.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Some(tx) = tx.lock().await.as_ref() {
                                    let _ = tx.send(session::SessionCommand::Pause).await;
                                }
                            });
                        }
                        "stop" => {
                            let state = app.state::<AppState>();
                            let tx = state.cmd_tx.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Some(tx) = tx.lock().await.as_ref() {
                                    let _ = tx.send(session::SessionCommand::Stop).await;
                                }
                            });
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            // Hide instead of close if session is active
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<AppState>();
                let tx = state.cmd_tx.clone();
                let has_session = tauri::async_runtime::block_on(async {
                    tx.lock().await.is_some()
                });
                if has_session {
                    api.prevent_close();
                    window.hide().ok();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn cleanup_old_logs(dir: &std::path::Path, max_age_days: u64) {
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(max_age_days * 86400);
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map_or(true, |e| e != "log") {
            continue;
        }
        if let Ok(meta) = path.metadata() {
            if let Ok(modified) = meta.modified() {
                if modified < cutoff {
                    std::fs::remove_file(&path).ok();
                }
            }
        }
    }
}
