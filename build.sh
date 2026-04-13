#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"
TAURI_DIR="$PROJECT_DIR/src-tauri"
OUT_DIR="$PROJECT_DIR/dist"
IMAGE_NAME="lmh-builder"

mkdir -p "$OUT_DIR"

# --- Linux (via Docker on any host) ---
build_linux() {
    echo "=== Building Linux x86_64 ==="

    if ! command -v docker &>/dev/null; then
        echo "  ⚠ Skipping Linux build — Docker not found"
        return 0
    fi

    # Build the Docker image once with all deps cached
    docker build -t "$IMAGE_NAME" -f - "$PROJECT_DIR" <<'DOCKERFILE'
FROM ubuntu:22.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update -qq && apt-get install -y -qq \
    curl build-essential pkg-config cmake clang \
    libwebkit2gtk-4.1-dev libgtk-3-dev libappindicator3-dev \
    libasound2-dev libssl-dev && \
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y -q
ENV PATH="/root/.cargo/bin:${PATH}"
DOCKERFILE

    docker run --rm -v "$PROJECT_DIR":/app -w /app/src-tauri "$IMAGE_NAME" \
        cargo build --release

    cp "$TAURI_DIR/target/release/live-meeting-helper" "$OUT_DIR/live-meeting-helper-linux-x86_64"
    chmod +x "$OUT_DIR/live-meeting-helper-linux-x86_64"
    echo "  → $OUT_DIR/live-meeting-helper-linux-x86_64"
}

# --- macOS (native, must run on a Mac) ---
build_macos() {
    echo "=== Building macOS ($(uname -m)) ==="

    if [[ "$(uname)" != "Darwin" ]]; then
        echo "  ⚠ Skipping macOS build — must run on a Mac"
        return 0
    fi

    # Ensure tauri-cli is available
    if ! cargo tauri --version &>/dev/null 2>&1; then
        echo "  Installing tauri-cli..."
        cargo install tauri-cli --version "^2" --locked
    fi

    cd "$TAURI_DIR"

    if [[ "$DEV_BUILD" == "true" ]]; then
        echo "  Building .app (debug, no DMG)..."
        cargo tauri build --debug --bundles app

        APP=$(find target/debug/bundle/macos -name "*.app" -maxdepth 1 | head -1)
        if [[ -z "$APP" ]]; then
            echo "  ERROR: No .app found after debug build"
            exit 1
        fi

        DEST="$OUT_DIR/$(basename "$APP")"
        rm -rf "$DEST"
        cp -r "$APP" "$DEST"
        echo "  → $DEST"
        echo "  Tip: open \"$DEST\" to launch"
    else
        echo "  Building .dmg..."
        cargo tauri build --bundles dmg

        DMG=$(find target/release/bundle/dmg -name "*.dmg" | head -1)
        if [[ -z "$DMG" ]]; then
            echo "  ERROR: No .dmg found after build"
            exit 1
        fi

        # Tauri's DMG bundler mounts the image during creation, which triggers
        # macOS to open a Finder window. Eject it immediately after the build.
        hdiutil info | awk '/\/Volumes\/Live Meeting Helper/{print $1}' | xargs -I{} hdiutil detach {} 2>/dev/null || true

        cp "$DMG" "$OUT_DIR/"
        echo "  → $OUT_DIR/$(basename "$DMG")"
    fi
}

# --- Main ---
usage() {
    echo "Usage: $0 [linux|macos|all] [--dev]"
    echo "  linux     - Build Linux binary (uses Docker)"
    echo "  macos     - Build macOS .dmg (must run on Mac)"
    echo "  all       - Build all platforms (skips unavailable)"
    echo "  --dev     - macOS only: debug build, outputs .app instead of .dmg (much faster)"
    echo ""
    echo "Windows: use build-windows.bat on a Windows machine"
}

DEV_BUILD=false
ARGS=()
for arg in "$@"; do
    case "$arg" in
        --dev) DEV_BUILD=true ;;
        *)     ARGS+=("$arg") ;;
    esac
done

TARGET="${ARGS[0]:-all}"

case "$TARGET" in
    linux)   build_linux ;;
    macos)   build_macos ;;
    all)
        build_linux
        build_macos
        echo ""
        echo "=== Build complete ==="
        ls -lh "$OUT_DIR"/ 2>/dev/null || echo "No artifacts found"
        ;;
    -h|--help) usage ;;
    *) echo "Unknown target: $TARGET"; usage; exit 1 ;;
esac
