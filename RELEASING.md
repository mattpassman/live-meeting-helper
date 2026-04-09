# Release Process

macOS builds are created locally by the maintainer and uploaded manually to the GitHub Release draft. GitHub Actions handles Windows and Linux automatically.

## Steps

### 1. Bump the version

Update the version number in both files:

- `src-tauri/tauri.conf.json` — `"version"` field
- `src-tauri/Cargo.toml` — `version` field under `[package]`

Commit the change:
```bash
git add src-tauri/tauri.conf.json src-tauri/Cargo.toml
git commit -m "chore: bump version to v0.x.x"
```

### 2. Tag and push — triggers GH Actions

```bash
git tag v0.x.x
git push origin main
git push --tags
```

Pushing the tag triggers the `release.yml` workflow, which:
- Builds Windows installers (MSI + NSIS) — no Whisper/cmake required
- Builds Linux packages (.deb + AppImage) — with Whisper via cmake/clang
- Creates a **draft** GitHub Release containing the Windows and Linux artifacts

The draft will appear under Releases on GitHub. Do not publish it yet.

### 3. Build macOS locally

Run the build script on a Mac:
```bash
./build.sh macos
```

The `.dmg` files will be in `src-tauri/target/release/bundle/dmg/` (or `dist/` if the script copies them there). Build both architectures if needed:
- Apple Silicon: `cargo tauri build --target aarch64-apple-darwin`
- Intel: `cargo tauri build --target x86_64-apple-darwin`

### 4. Upload macOS .dmg to the draft release

1. Go to the GitHub Releases page for this repository
2. Open the draft release created by the Actions workflow
3. Drag in the `.dmg` file(s) from step 3
4. Review the auto-generated release notes and adjust if needed
5. Click **Publish release**

The release is now live with installers for all three platforms.
