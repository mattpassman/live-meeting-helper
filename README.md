# Live Meeting Helper

Live Meeting Helper is a desktop app that listens to your meetings and writes your notes for you. It captures both your microphone and system audio in real time, transcribes everyone speaking, and uses an AI model to produce structured, readable notes — all while you stay focused on the conversation itself. It's built for anyone who has ever missed something important because they were too busy writing it down.

## Download

v0.1.0 is the first release. Installers are available on the Releases page. macOS builds are uploaded manually by the maintainer.

| Platform | Download |
|----------|----------|
| macOS (Apple Silicon) | See Releases page |
| macOS (Intel) | See Releases page |
| Windows 10/11 | See Releases page |
| Linux (Ubuntu 20.04+ / Debian) | See Releases page |
| Linux (AppImage) | See Releases page |

## Quick Start (No build required)

1. Download the installer for your platform from the Releases page
2. Open the app — a setup wizard will guide you through configuration
3. Enter your Claude or OpenAI API key when prompted
4. Click **Start** on the Meeting tab and begin speaking

## Features

- **Live, structured notes generated automatically as you speak** — no manual summarizing after the fact
- **Captures everyone on the call** — records both your microphone and system audio, so remote participants are included too
- **Speaker-aware transcripts** — the notes reflect who said what, not just a wall of text
- **Works with the AI you already have** — bring your own Claude or OpenAI API key; no subscription to a new service required
- **Full session history** — past meetings are saved automatically, and nothing is lost if the app closes unexpectedly
- **Stays out of your way** — lives in the system tray during meetings and only surfaces when you need it
- **Private-by-default option** — enable local Whisper transcription in Settings to keep all audio on your machine
- **Profile support** — save different configurations for standups, client calls, interviews, and more

## Supported Platforms

macOS 13+, Windows 10/11, Linux (Ubuntu 20.04+)

## Getting an API Key

**Claude (default and recommended)**
Go to [console.anthropic.com](https://console.anthropic.com), create an account, and generate an API key. Claude is the default because it produces especially clear and well-structured meeting notes. A paid account is recommended for regular use — the free tier has rate limits that may interrupt longer meetings.

**OpenAI**
Go to [platform.openai.com](https://platform.openai.com), create an account, and generate an API key under the API section. Select OpenAI as your provider in Settings after launching the app. A paid account is likewise recommended for heavy use.

## Privacy & Audio

By default, audio is sent to AWS Transcribe for transcription — the connection is encrypted in transit and AWS does not store your audio after transcription. Only the resulting transcript text is then sent to your chosen AI provider (Anthropic or OpenAI) to generate notes; raw audio never leaves your device for that step.

If you want everything to stay on your machine, enable the **local Whisper** option in Settings. This runs transcription locally using an on-device model and nothing leaves your device at all.

Your API keys are stored locally in a config file on your own machine and are never transmitted anywhere other than directly to Anthropic or OpenAI.

---

## For Developers & Contributors

### Prerequisites

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

### Build

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

### Development

```bash
cargo tauri dev
```

### Configuration

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

#### Local Whisper — model downloads

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

### Project Structure

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

### Architecture

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
