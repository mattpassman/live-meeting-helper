# Live Meeting Helper (Tauri Desktop App)

A real-time meeting assistant that captures audio (microphone + system audio), transcribes speech, and generates structured meeting notes live — packaged as a native desktop application.

## Features

- **Native Desktop App**: Tauri v2 with system webview — small binary, native performance
- **Audio Capture**: Microphone via cpal, system audio via platform-native APIs
  - macOS: ScreenCaptureKit (macOS 13+)
  - Windows: WASAPI loopback capture
  - Linux: PulseAudio/PipeWire monitor sources
- **Real-Time Transcription**: Amazon Transcribe Streaming with speaker identification
- **Live Note Generation**: LLM-powered structured notes via Claude (Anthropic) or OpenAI
- **In-App UI**: Meeting controls, live notes, session history, profile management, settings
- **System Tray**: Runs unobtrusively during meetings with quick controls
- **Persistence**: Auto-save sessions to disk with crash recovery

## Prerequisites

- Rust 1.70+
- **cmake** and a C++ compiler (required for Whisper, which builds by default):
  - **macOS**: `xcode-select --install` + `brew install cmake`
  - **Linux**: `sudo apt install cmake clang`
  - **Windows**: [cmake installer](https://cmake.org/download/) + Visual Studio Build Tools with the C++ workload
- System UI dependencies:
  - **Linux**: `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libappindicator3-dev libasound2-dev`
  - **Windows**: WebView2 (pre-installed on Windows 10/11)
- An **AI API key** — either:
  - [Anthropic Claude](https://console.anthropic.com) (default, `claude-sonnet-4-6`)
  - [OpenAI](https://platform.openai.com) (`gpt-4o`)
- **Transcription backend** — choose one:
  - **AWS Transcribe** (default): requires an AWS account with `transcribe:StartStreamTranscription` permission. Credentials via standard chain (`~/.aws/credentials`, env vars, IAM role).
  - **Local Whisper** (offline, private): build with `--features whisper` (needs cmake and a C++ compiler), then download a GGML model file and point to it in Settings. No cloud account needed.

## Quick Start

1. Build and run the app (see Build section below)
2. Open **Settings** and enter your Claude or OpenAI API key
3. Configure your AWS profile/region for transcription
4. Click **Start** on the Meeting tab and begin speaking

## Build

Cross-platform build script (Linux/macOS):
```bash
./build.sh            # Build both platforms (macOS skipped if not on a Mac)
./build.sh linux      # Linux only (uses Docker)
./build.sh macos      # macOS only (must run on a Mac)
```

Windows:
```bat
build-windows.bat         # Release build
dev-build-windows.bat     # Debug build (faster compile)
```

Binaries are output to `dist/`.

## Development

```bash
cargo tauri dev
```

## Configuration

Settings are stored in `~/.config/live-meeting-helper/config.json` (macOS/Linux) or `%APPDATA%\Live Meeting Helper\config.json` (Windows).

Key settings (configurable via the Settings UI):

| Setting | Description |
|---------|-------------|
| AI Provider | `claude` (default) or `openai` |
| Claude API Key | Your Anthropic API key |
| Claude Model | Model to use (default: `claude-sonnet-4-6`) |
| OpenAI API Key | Your OpenAI API key |
| OpenAI Model | Model to use (default: `gpt-4o`) |
| Transcription Provider | `aws` (default) or `whisper` (local) |
| Whisper Model Path | Path to a GGML model file (e.g. `~/models/ggml-base.en.bin`) |
| AWS Profile | Named AWS credentials profile (default: `default`) |
| AWS Region | AWS region for Transcribe (default: `us-east-1`) |
| Audio Device | Specific audio device to capture from |

### Local Whisper — model downloads

Download GGML models from [huggingface.co/ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp):

| Model | Size | Notes |
|-------|------|-------|
| `ggml-tiny.en.bin` | 75 MB | Fastest, lower accuracy |
| `ggml-base.en.bin` | 142 MB | Good balance (recommended) |
| `ggml-small.en.bin` | 466 MB | Better accuracy |
| `ggml-medium.en.bin` | 1.5 GB | High accuracy |

Whisper support is compiled in by default. To build **without** it (e.g. on a machine without cmake):
```bash
cargo tauri build --no-default-features --features app
```

## Project Structure

```
├── build.sh                # Cross-platform build script (Linux/macOS)
├── build-windows.bat       # Windows release build
├── dev-build-windows.bat   # Windows debug build
├── package.json
├── src/                    # Frontend (HTML/CSS/JS)
│   ├── index.html
│   ├── css/style.css
│   └── js/app.js
├── src-tauri/              # Rust backend
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── src/
│       ├── main.rs         # Tauri app entry point + tray setup
│       ├── lib.rs          # Library root
│       ├── commands.rs     # Tauri IPC command handlers
│       ├── config.rs       # App configuration
│       ├── types.rs        # Shared types
│       ├── paths.rs        # Path utilities
│       ├── document.rs     # Document handling
│       ├── audio/          # Audio capture (mic + system audio)
│       ├── transcription/  # AWS Transcribe streaming
│       ├── notes/          # Note generation via Claude/OpenAI API
│       ├── session/        # Session lifecycle management
│       ├── persistence/    # Session save/load/export
│       └── profile/        # Meeting profile management
```

## Architecture

```
┌─────────────────────────────────────────────┐
│              Tauri Webview (UI)              │
│  Meeting Controls · Live Notes · History    │
└──────────────┬──────────────────────────────┘
               │ Tauri IPC (invoke / events)
┌──────────────┴──────────────────────────────┐
│              Rust Backend                    │
│  Audio Capture → Transcription → Notes      │
│       │               │             │       │
│       ▼               ▼             ▼       │
│   cpal/WASAPI   AWS Transcribe   Claude or  │
│                  Streaming       OpenAI API  │
└─────────────────────────────────────────────┘
```
