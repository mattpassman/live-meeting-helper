# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

Live Meeting Helper is a Tauri v2 desktop application that captures meeting audio (microphone + system audio), transcribes it in real time, and uses AI (Claude or OpenAI) to generate structured meeting notes while the user speaks.

## Build & Development Commands

```bash
# Run in development mode (hot-reload frontend, auto-restart backend)
cargo tauri dev

# Fast CI check (skips Whisper/cmake dependency)
cargo check --no-default-features --features app

# Full production build (macOS + Linux via Docker)
./build.sh          # builds both
./build.sh macos    # macOS only
./build.sh linux    # Linux only (uses Docker)

# Windows (run on Windows host)
build-windows.bat       # release
dev-build-windows.bat   # debug (faster)
```

## Testing

```bash
# Run all tests
cd src-tauri && cargo test

# Run tests for a specific module
cargo test audio::
cargo test notes::corrections

# Run a single named test
cargo test build_prompt_fills_all_placeholders

# Run without Whisper feature (faster, matches CI)
cargo test --no-default-features --features app

# Show test output
cargo test -- --nocapture
```

Tests live in 4 files: `audio/capture.rs` (resampling/downmixing), `notes/prompts.rs` (templating), `notes/mod.rs` (data structures), `notes/corrections.rs` (user edit tracking).

## Architecture

```
src/                    # Frontend — vanilla HTML/CSS/JS, no build step
src-tauri/src/
├── main.rs             # Tauri builder, tray setup, invoke_handler registration
├── commands.rs         # All #[tauri::command] IPC handlers (~600 lines)
├── config.rs           # App config (persisted to ~/.config/live-meeting-helper/config.json)
├── types.rs            # Shared types
├── audio/              # Audio capture (mic + system loopback)
├── transcription/      # AWS Transcribe streaming + local Whisper (feature-gated)
├── notes/              # AI note generation, prompt templating, user corrections
├── session/            # Session lifecycle management
├── persistence/        # Save/load/export sessions
└── profile/            # Profile management
```

**Data flow:** Audio (mic/system) → transcription service (AWS Transcribe or Whisper) → AI API (Claude/OpenAI) → structured notes displayed live and persisted to disk.

**Frontend ↔ Backend IPC:**
- Frontend calls backend: `window.__TAURI__.core.invoke('command_name', args)`
- Backend pushes events to frontend: `window.__TAURI__.event.listen('event_name', handler)`

## Key Development Rules

- **New Tauri commands** must be registered in the `invoke_handler![]` macro in `main.rs` or they won't be callable from the frontend.
- **New config fields** must have `#[serde(default)]` to avoid breaking existing user configs on upgrade.
- **Whisper code** is `#[cfg(feature = "whisper")]`-gated — keep that boundary clean. The default feature set omits Whisper to avoid cmake as a build dependency.
- **Frontend changes** take effect immediately in `cargo tauri dev` — no build step needed.
- **Platform-specific audio:** macOS uses Swift FFI (`loopback_mac.swift` + `loopback_mac.rs`), Windows uses WASAPI (`loopback_win.rs`), microphone uses cpal everywhere. See `docs/AUDIO_CAPTURE.md` for details.
