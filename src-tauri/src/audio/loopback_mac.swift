// macOS system audio capture via CoreAudio process tap.
// Requires macOS 14.4+ (CATapDescription + AudioHardwareCreateProcessTap).
// Uses NSAudioCaptureUsageDescription — NOT "Screen Recording" — so no more -3801.
// Falls back gracefully on older macOS with a clear error message.

import Foundation
import CoreAudio
import AudioToolbox
import AVFoundation

// Must be public so the @_cdecl exports can use it without visibility errors.
public typealias AudioDataCallback = @convention(c) (UnsafePointer<Int16>?, Int32, UInt64) -> Void

// ── Helpers ───────────────────────────────────────────────────────────────────

private func writeErr(_ msg: String, to buf: UnsafeMutablePointer<Int8>?, len: Int32) {
    guard let buf = buf, len > 1 else { return }
    let bytes = Array(msg.utf8.prefix(Int(len) - 1))
    for (i, b) in bytes.enumerated() { buf[i] = Int8(bitPattern: b) }
    buf[bytes.count] = 0
}

private func propAddr(_ selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress(
        mSelector: selector,
        mScope:    kAudioObjectPropertyScopeGlobal,
        mElement:  kAudioObjectPropertyElementMain
    )
}

// ── Global capture state ──────────────────────────────────────────────────────

private var gTapID:    AudioObjectID        = kAudioObjectUnknown
private var gDeviceID: AudioObjectID        = kAudioObjectUnknown
private var gProcID:   AudioDeviceIOProcID? = nil

// ── CoreAudio query helpers ───────────────────────────────────────────────────

/// Returns the UID string of the current default system output device.
private func defaultSystemOutputUID() -> String? {
    let sys = AudioObjectID(kAudioObjectSystemObject)
    var a = propAddr(kAudioHardwarePropertyDefaultSystemOutputDevice)
    var devID = AudioDeviceID(kAudioObjectUnknown)
    var size  = UInt32(MemoryLayout<AudioDeviceID>.size)
    guard AudioObjectGetPropertyData(sys, &a, 0, nil, &size, &devID) == noErr,
          devID != kAudioObjectUnknown else { return nil }

    var ua = propAddr(kAudioDevicePropertyDeviceUID)
    var uid: CFString = "" as CFString
    size = UInt32(MemoryLayout<CFString>.size)
    withUnsafeMutablePointer(to: &uid) { ptr in
        _ = AudioObjectGetPropertyData(devID, &ua, 0, nil, &size, ptr)
    }
    let s = uid as String
    return s.isEmpty ? nil : s
}

// ── Internal start/stop (macOS 14.4+) ────────────────────────────────────────

@available(macOS 14.4, *)
private func doStart(
    _ cb: AudioDataCallback,
    errBuf: UnsafeMutablePointer<Int8>?,
    errLen: Int32
) -> Bool {

    // ── 1. Create global system-wide tap ─────────────────────────────────────
    // monoGlobalTapButExcludeProcesses([]) = capture ALL system audio output,
    // present and future, excluding nobody. This is a true system-wide tap and
    // does not require enumerating per-process IDs at start time.
    let tapDesc = CATapDescription(monoGlobalTapButExcludeProcesses: [])
    tapDesc.uuid         = UUID()
    tapDesc.muteBehavior = .unmuted

    var tapID = AudioObjectID(kAudioObjectUnknown)
    let tapErr = AudioHardwareCreateProcessTap(tapDesc, &tapID)
    guard tapErr == noErr else {
        let msg = "AudioHardwareCreateProcessTap failed (err=\(tapErr)). " +
                  "Grant microphone/audio access in System Settings → Privacy & Security → Microphone."
        writeErr(msg, to: errBuf, len: errLen)
        NSLog("[LiveMeeting] %@", msg)
        return false
    }
    gTapID = tapID
    NSLog("[LiveMeeting] Process tap created: id=%u uuid=%@", tapID, tapDesc.uuid.uuidString)

    // ── 3. Find the default system output device ──────────────────────────────
    guard let outputUID = defaultSystemOutputUID() else {
        AudioHardwareDestroyProcessTap(tapID); gTapID = kAudioObjectUnknown
        writeErr("No system output device found", to: errBuf, len: errLen)
        return false
    }
    NSLog("[LiveMeeting] System output UID: %@", outputUID)

    // ── 4. Create aggregate device wrapping the tap ───────────────────────────
    let aggDesc: [String: Any] = [
        kAudioAggregateDeviceNameKey:          "LMH-SysAudio",
        kAudioAggregateDeviceUIDKey:           UUID().uuidString,
        kAudioAggregateDeviceMainSubDeviceKey: outputUID,
        kAudioAggregateDeviceIsPrivateKey:     true,
        kAudioAggregateDeviceIsStackedKey:     false,
        kAudioAggregateDeviceTapAutoStartKey:  true,
        kAudioAggregateDeviceSubDeviceListKey: [[kAudioSubDeviceUIDKey: outputUID]],
        kAudioAggregateDeviceTapListKey: [[
            kAudioSubTapDriftCompensationKey: true,
            kAudioSubTapUIDKey:               tapDesc.uuid.uuidString,
        ]],
    ]

    var deviceID = AudioObjectID(kAudioObjectUnknown)
    let aggErr = AudioHardwareCreateAggregateDevice(aggDesc as CFDictionary, &deviceID)
    guard aggErr == noErr else {
        AudioHardwareDestroyProcessTap(tapID); gTapID = kAudioObjectUnknown
        writeErr("AudioHardwareCreateAggregateDevice failed (err=\(aggErr))", to: errBuf, len: errLen)
        return false
    }
    gDeviceID = deviceID
    NSLog("[LiveMeeting] Aggregate device created: id=%u", deviceID)

    // Brief settle time for the aggregate device to become ready.
    Thread.sleep(forTimeInterval: 0.1)

    // ── 5. Read tap stream description ────────────────────────────────────────
    var tapFmtAddr = propAddr(kAudioTapPropertyFormat)
    var tapFmt     = AudioStreamBasicDescription()
    var fmtSize    = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
    AudioObjectGetPropertyData(tapID, &tapFmtAddr, 0, nil, &fmtSize, &tapFmt)

    // Fall back to 44100 Hz stereo Float32 if the format wasn't populated yet.
    if tapFmt.mSampleRate == 0 {
        tapFmt.mSampleRate       = 44100
        tapFmt.mFormatID         = kAudioFormatLinearPCM
        tapFmt.mFormatFlags      = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked
        tapFmt.mChannelsPerFrame = 2
        tapFmt.mBitsPerChannel   = 32
        tapFmt.mBytesPerFrame    = 8
        tapFmt.mFramesPerPacket  = 1
        tapFmt.mBytesPerPacket   = 8
    }

    let srcRate = tapFmt.mSampleRate
    let nCh     = Int(max(1, tapFmt.mChannelsPerFrame))
    NSLog("[LiveMeeting] Tap format: %.0f Hz %d ch", srcRate, nCh)

    // Build AVAudioFormat for bufferListNoCopy wrapping in the IO proc.
    var mutableFmt = tapFmt
    guard let srcAVFormat = AVAudioFormat(streamDescription: &mutableFmt) else {
        AudioHardwareDestroyAggregateDevice(deviceID); gDeviceID = kAudioObjectUnknown
        AudioHardwareDestroyProcessTap(tapID);         gTapID    = kAudioObjectUnknown
        writeErr("Failed to create AVAudioFormat from tap description", to: errBuf, len: errLen)
        return false
    }

    // ── 6. Install IO proc ────────────────────────────────────────────────────
    var procID: AudioDeviceIOProcID? = nil
    let ioErr = AudioDeviceCreateIOProcIDWithBlock(&procID, deviceID, nil) {
        _, inInputData, _, _, _ in

        // Wrap the CoreAudio buffer list in an AVAudioPCMBuffer (no copy).
        guard let avBuf = AVAudioPCMBuffer(
            pcmFormat: srcAVFormat,
            bufferListNoCopy: inInputData,
            deallocator: nil
        ),
        let floatData = avBuf.floatChannelData,
        avBuf.frameLength > 0 else { return }

        let nFrames = Int(avBuf.frameLength)
        let chCount = Int(avBuf.format.channelCount)

        // Downmix to mono Float32.
        // floatChannelData is always channel-deinterleaved regardless of the
        // source format, so floatData[c][i] is safe for both interleaved and
        // non-interleaved tap formats.
        var mono = [Float32](repeating: 0, count: nFrames)
        for i in 0..<nFrames {
            var s: Float32 = 0
            for c in 0..<chCount { s += floatData[c][i] }
            mono[i] = s / Float32(chCount)
        }

        // Resample from native tap rate → 16 kHz via linear interpolation.
        let ratio  = srcRate / 16_000.0
        let outLen = max(1, Int(Double(nFrames) / ratio))
        var pcm    = [Int16](repeating: 0, count: outLen)
        for i in 0..<outLen {
            let srcF = Double(i) * ratio
            let j    = Int(srcF)
            let frac = Float32(srcF - Double(j))
            let s: Float32
            if j + 1 < nFrames {
                s = mono[j] * (1 - frac) + mono[j + 1] * frac
            } else if j < nFrames {
                s = mono[j]
            } else {
                s = 0
            }
            pcm[i] = Int16(max(-1, min(1, s)) * Float32(Int16.max))
        }

        let ms = UInt64(Date().timeIntervalSince1970 * 1_000)
        pcm.withUnsafeBufferPointer { b in
            cb(b.baseAddress, Int32(outLen), ms)
        }
    }

    guard ioErr == noErr else {
        AudioHardwareDestroyAggregateDevice(deviceID); gDeviceID = kAudioObjectUnknown
        AudioHardwareDestroyProcessTap(tapID);         gTapID    = kAudioObjectUnknown
        writeErr("AudioDeviceCreateIOProcIDWithBlock failed (err=\(ioErr))", to: errBuf, len: errLen)
        return false
    }
    gProcID = procID

    // ── 7. Start capture ──────────────────────────────────────────────────────
    let startErr = AudioDeviceStart(deviceID, procID)
    guard startErr == noErr else {
        AudioDeviceDestroyIOProcID(deviceID, procID!);  gProcID   = nil
        AudioHardwareDestroyAggregateDevice(deviceID); gDeviceID = kAudioObjectUnknown
        AudioHardwareDestroyProcessTap(tapID);         gTapID    = kAudioObjectUnknown
        writeErr("AudioDeviceStart failed (err=\(startErr))", to: errBuf, len: errLen)
        return false
    }

    NSLog("[LiveMeeting] CoreAudio system audio tap started — %.0f Hz → 16 kHz mono", srcRate)
    return true
}

private func doStop() {
    if gDeviceID != kAudioObjectUnknown {
        if let p = gProcID {
            AudioDeviceStop(gDeviceID, p)
            AudioDeviceDestroyIOProcID(gDeviceID, p)
            gProcID = nil
        }
        AudioHardwareDestroyAggregateDevice(gDeviceID)
        gDeviceID = kAudioObjectUnknown
    }
    if gTapID != kAudioObjectUnknown {
        AudioHardwareDestroyProcessTap(gTapID)
        gTapID = kAudioObjectUnknown
    }
    NSLog("[LiveMeeting] CoreAudio system audio tap stopped")
}

// ── C-callable exports ────────────────────────────────────────────────────────

/// Returns true on success. On failure, writes a UTF-8 error message into errBuf.
@_cdecl("mac_loopback_start")
public func macLoopbackStart(
    callback: AudioDataCallback,
    errBuf: UnsafeMutablePointer<Int8>?,
    errLen: Int32
) -> Bool {
    if #available(macOS 14.4, *) {
        return doStart(callback, errBuf: errBuf, errLen: errLen)
    }
    writeErr("System audio capture requires macOS 14.4+", to: errBuf, len: errLen)
    return false
}

@_cdecl("mac_loopback_stop")
public func macLoopbackStop() {
    doStop()
}
