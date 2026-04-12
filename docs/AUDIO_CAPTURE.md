# Audio Capture

Live Meeting Helper can capture two audio sources: the system's microphone input, and the system's audio output (so it can transcribe the remote side of a call). Each platform uses a different mechanism for system audio capture. Microphone capture is cross-platform and uses the same code path everywhere.

---

## Microphone capture (all platforms)

Microphone input is captured via [cpal](https://github.com/RustAudio/cpal), a cross-platform audio I/O library. On start, the app opens the user-selected device (or the system default) using whatever sample rate and channel count the device prefers, then converts the raw samples to 16 kHz mono i16 — the format expected by Whisper for transcription.

The conversion happens in two steps in `src-tauri/src/audio/capture.rs`:

1. **Downmix** — multi-channel frames are averaged to mono.
2. **Resample** — linear interpolation maps from the device sample rate to 16 kHz.

The capture runs on a dedicated OS thread. It writes `AudioChunk` values into a `tokio::mpsc` channel, where the transcription service reads them.

---

## System audio capture — macOS

Capturing the speaker output on macOS is significantly more involved than microphone capture, for two reasons:

- macOS doesn't expose a simple loopback device the way Windows does (WASAPI loopback).
- The correct API has changed several times across OS versions and has its own TCC (Transparency, Consent, Control) permission category.

### Why not ScreenCaptureKit?

ScreenCaptureKit can capture system audio and is the approach taken by OBS and some other apps. It requires the **Screen Recording** TCC permission (`com.apple.TCC.kTCCServiceScreenCapture`). This permission is tied to the app's code signing identity, which means every development build that isn't signed with a stable identity causes macOS to treat the app as new, and the user is asked to re-grant access every time. This made ScreenCaptureKit unusable in development and unreliable for unsigned distribution builds.

### CoreAudio process tap (macOS 14.4+)

The app uses the **CoreAudio process tap** API introduced in macOS 14.4. This API only requires the **Microphone** TCC permission (`NSAudioCaptureUsageDescription`), which is stable across builds and doesn't have the code-signing identity problem.

The relevant files are:

| File | Role |
|---|---|
| `src-tauri/src/audio/loopback_mac.swift` | Swift implementation of the tap |
| `src-tauri/src/audio/loopback_mac.rs` | Rust FFI glue; owns the channel sender and callback |
| `src-tauri/build.rs` | Compiles the Swift file to a static `.a` library at build time |
| `src-tauri/Info.plist` | Declares `NSAudioCaptureUsageDescription` and `NSMicrophoneUsageDescription` |

### How the tap works

The setup sequence in `doStart()` inside `loopback_mac.swift`:

**1. Create a global process tap**

```swift
let tapDesc = CATapDescription(monoGlobalTapButExcludeProcesses: [])
tapDesc.uuid = UUID()
tapDesc.muteBehavior = .unmuted
AudioHardwareCreateProcessTap(tapDesc, &tapID)
```

`monoGlobalTapButExcludeProcesses([])` creates a system-wide tap that captures **all** audio output — from every process, including processes that launch after the tap is created — and excludes nobody. This is the key difference from `stereoMixdownOfProcesses(_:)`, which only captures the specific PIDs listed at creation time and misses apps that start later.

The tap does not silence or alter what plays through the speakers (`muteBehavior = .unmuted`).

**2. Find the default output device**

`AudioObjectGetPropertyData` with `kAudioHardwarePropertyDefaultSystemOutputDevice` returns the device ID, and a second call with `kAudioDevicePropertyDeviceUID` returns its persistent UID string (e.g. `"AppleHDAEngineOutput:1,0,1,2:0:{...}"`).

**3. Create a private aggregate device**

CoreAudio doesn't allow attaching an IO proc directly to a tap. Instead, the tap is wrapped in a private aggregate device that joins the real output device with the tap as a virtual sub-tap:

```swift
let aggDesc: [String: Any] = [
    kAudioAggregateDeviceNameKey:          "LMH-SysAudio",
    kAudioAggregateDeviceUIDKey:           UUID().uuidString,
    kAudioAggregateDeviceIsPrivateKey:     true,   // hidden from other apps
    kAudioAggregateDeviceTapAutoStartKey:  true,
    kAudioAggregateDeviceSubDeviceListKey: [[kAudioSubDeviceUIDKey: outputUID]],
    kAudioAggregateDeviceTapListKey: [[
        kAudioSubTapDriftCompensationKey: true,
        kAudioSubTapUIDKey: tapDesc.uuid.uuidString,
    ]],
]
AudioHardwareCreateAggregateDevice(aggDesc as CFDictionary, &deviceID)
```

`kAudioAggregateDeviceIsPrivateKey: true` keeps the device invisible to Audio MIDI Setup and other apps. `kAudioSubTapDriftCompensationKey: true` prevents clock drift between the tap and the output device.

**4. Read the tap's native format**

`kAudioTapPropertyFormat` returns an `AudioStreamBasicDescription` for the tap. If it comes back empty (can happen if the aggregate device hasn't settled yet), the code falls back to 44100 Hz stereo Float32, which matches what most macOS output devices deliver.

**5. Install an IO proc**

```swift
AudioDeviceCreateIOProcIDWithBlock(&procID, deviceID, nil) { _, inInputData, _, _, _ in
    // inInputData is the AudioBufferList from the tap
}
```

The IO proc fires on a real-time audio thread. No allocation, no locks, no Objective-C runtime calls are safe here.

**6. Convert to 16 kHz mono i16**

Inside the IO proc, samples are read directly from the `AudioBufferList` using `UnsafeMutableAudioBufferListPointer`. The tap delivers Float32 PCM; the layout is non-interleaved (one `AudioBuffer` per channel) or mono:

- **Non-interleaved (stereo/multi-channel):** each buffer is one channel; channels are averaged.
- **Interleaved:** all channels are packed in a single buffer with a stride of `nChannels`; same averaging logic.

After downmixing to mono Float32, linear interpolation resamples from the native tap rate to 16 kHz, and each sample is scaled to `Int16`.

**7. Send to Rust via callback**

The Swift `@_cdecl("mac_loopback_start")` export takes a C function pointer (`AudioDataCallback`) and an error buffer. The IO proc calls this function pointer with `(UnsafePointer<Int16>, count, timestamp_ms)` on every audio buffer.

On the Rust side (`loopback_mac.rs`), a static `extern "C" fn audio_callback` receives the pointer, copies the samples into a `Vec<i16>`, wraps it in an `AudioChunk`, and sends it into the same `mpsc` channel used by microphone capture.

### Build-time Swift compilation

`build.rs` compiles `loopback_mac.swift` at Cargo build time:

```
swiftc -sdk <macos-sdk> -target arm64-apple-macos14.4 -O -parse-as-library -emit-object -o loopback_mac.o loopback_mac.swift
ar rcs libloopback_mac.a loopback_mac.o
```

The resulting static library is linked into the Rust binary. Cargo links `CoreAudio`, `AudioToolbox`, and `Foundation` frameworks. **`AVFoundation` is deliberately excluded** — importing it causes macOS to probe the user's Music and Photos libraries on first use, triggering unwanted TCC permission dialogs.

The Swift target is `macos14.4` because `CATapDescription` and `AudioHardwareCreateProcessTap` were introduced in that release. The `@_cdecl` exports are guarded at runtime with `if #available(macOS 14.4, *)`, so the binary loads and returns a clear error message on older macOS instead of crashing.

### macOS 14.3 and earlier

On macOS 14.3 and earlier, `mac_loopback_start` writes `"System audio capture requires macOS 14.4+"` into the error buffer and returns `false`. The Rust caller logs this and, if the user requested system-audio-only mode, surfaces it as an error. In "Both" mode (mic + system audio), capture continues with microphone only.

### Cleanup

`doStop()` tears down in reverse order: stop the IO proc, destroy the aggregate device, then destroy the tap. The Rust `MacLoopbackHandle` wraps this in a `Drop` impl, so teardown is guaranteed when the meeting ends regardless of how `stop()` is called.

---

## System audio capture — Windows

Windows uses WASAPI loopback via `src-tauri/src/audio/loopback_win.rs`. WASAPI exposes the default render (output) device as a capture device in loopback mode — no special permission is needed beyond normal audio access. The samples are converted to 16 kHz mono i16 using the same `convert_and_resample` helpers in `capture.rs`.

---

## Data flow summary

```
macOS system audio
  loopback_mac.swift (IO proc, real-time thread)
    → audio_callback (extern "C", loopback_mac.rs)
      → mpsc::Sender<AudioChunk>

Microphone (all platforms)
  cpal input stream callback (capture.rs)
    → convert_and_resample / convert_and_resample_i16
      → mpsc::Sender<AudioChunk>

Windows system audio
  WASAPI loopback (loopback_win.rs)
    → convert_and_resample
      → mpsc::Sender<AudioChunk>

mpsc::Receiver<AudioChunk>
  → transcription service (Whisper)
  → notes generator (Claude)
```

All three sources write `AudioChunk` values into the same channel. The transcription service reads from that channel without needing to know which source produced a given chunk.
