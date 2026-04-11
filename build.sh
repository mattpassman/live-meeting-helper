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
    echo "=== Building macOS ==="

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

    if [[ "$(uname -m)" == "arm64" ]]; then
        echo "  Building aarch64 .app..."
        cargo tauri build --target aarch64-apple-darwin --bundles app
        BUNDLE_DIR="target/aarch64-apple-darwin/release/bundle/macos"

        echo "  Building x86_64 .app (cross)..."
        rustup target add x86_64-apple-darwin 2>/dev/null || true
        cargo tauri build --target x86_64-apple-darwin --bundles app

        echo "  Creating universal .app via lipo..."
        ARM_BIN="target/aarch64-apple-darwin/release/live-meeting-helper"
        X86_BIN="target/x86_64-apple-darwin/release/live-meeting-helper"
        rm -rf "$OUT_DIR/Live Meeting Helper.app"
        cp -r "$BUNDLE_DIR/Live Meeting Helper.app" "$OUT_DIR/Live Meeting Helper.app"
        lipo -create "$ARM_BIN" "$X86_BIN" \
            -output "$OUT_DIR/Live Meeting Helper.app/Contents/MacOS/live-meeting-helper"
        echo "  → $OUT_DIR/Live Meeting Helper.app (universal)"

        echo "  Ad-hoc codesigning..."
        codesign --force --deep --sign - "$OUT_DIR/Live Meeting Helper.app"

        echo "  Creating universal .dmg..."
        rm -f "$OUT_DIR/Live Meeting Helper-universal.dmg"
        hdiutil create \
            -volname "Live Meeting Helper" \
            -srcfolder "$OUT_DIR/Live Meeting Helper.app" \
            -ov -format UDZO \
            "$OUT_DIR/Live Meeting Helper-universal.dmg"
        echo "  → $OUT_DIR/Live Meeting Helper-universal.dmg"
    else
        echo "  Building x86_64 .app and .dmg..."
        cargo tauri build --bundles dmg

        rm -rf "$OUT_DIR/Live Meeting Helper.app"
        cp -r "target/release/bundle/macos/Live Meeting Helper.app" "$OUT_DIR/Live Meeting Helper.app"

        DMG=$(ls target/release/bundle/dmg/*.dmg | head -1)
        cp "$DMG" "$OUT_DIR/"
        echo "  → $OUT_DIR/$(basename "$DMG")"

        echo "  Ad-hoc codesigning .app..."
        codesign --force --deep --sign - "$OUT_DIR/Live Meeting Helper.app"
        echo "  Signed."
    fi
}

# --- Main ---
usage() {
    echo "Usage: $0 [linux|macos|all]"
    echo "  linux - Build Linux binary (uses Docker)"
    echo "  macos - Build macOS binary (must run on Mac)"
    echo "  all   - Build all platforms (skips unavailable)"
    echo ""
    echo "Windows: use build-windows.bat on a Windows machine"
}

TARGET="${1:-all}"

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
