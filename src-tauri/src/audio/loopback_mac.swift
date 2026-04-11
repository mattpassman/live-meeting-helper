// macOS system audio capture via ScreenCaptureKit.
// SCStreamOutputType.audio requires macOS 13.0+.
// Weak-linked so the app still loads on older macOS; runtime guard ensures graceful fallback.

import Foundation
import CoreMedia
import ScreenCaptureKit  // weak-linked via -weak_framework in build.rs

// Must be public so the @_cdecl exports can use it without visibility errors.
public typealias AudioDataCallback = @convention(c) (UnsafePointer<Int16>?, Int32, UInt64) -> Void

// ── Capture delegate (macOS 13.0+) ──────────────────────────────────────────

@available(macOS 13.0, *)
private final class SCLoopbackCapture: NSObject, SCStreamOutput, SCStreamDelegate {
    private let cb: AudioDataCallback

    init(_ cb: AudioDataCallback) { self.cb = cb; super.init() }

    func stream(
        _ stream: SCStream,
        didOutputSampleBuffer buf: CMSampleBuffer,
        of type: SCStreamOutputType
    ) {
        guard type == .audio,
              let blockBuf = CMSampleBufferGetDataBuffer(buf)
        else { return }

        var totalLen = 0
        var rawPtr: UnsafeMutablePointer<Int8>? = nil
        guard CMBlockBufferGetDataPointer(
            blockBuf, atOffset: 0,
            lengthAtOffsetOut: nil, totalLengthOut: &totalLen,
            dataPointerOut: &rawPtr
        ) == kCMBlockBufferNoErr,
              let ptr = rawPtr, totalLen > 0
        else { return }

        let pts = CMSampleBufferGetPresentationTimeStamp(buf)
        let ms  = UInt64(max(0.0, CMTimeGetSeconds(pts)) * 1000.0)

        // SCStream delivers audio as Float32 PCM; convert to Int16
        let n      = totalLen / MemoryLayout<Float32>.size
        let floats = UnsafeRawPointer(ptr).bindMemory(to: Float32.self, capacity: n)
        var pcm    = [Int16](repeating: 0, count: n)
        for i in 0..<n {
            pcm[i] = Int16(max(-1.0, min(1.0, floats[i])) * Float(Int16.max))
        }
        pcm.withUnsafeBufferPointer { buf in
            cb(buf.baseAddress, Int32(n), ms)
        }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        NSLog("[LiveMeeting] SCStream stopped: %@", error.localizedDescription)
    }
}

// ── Global capture state (one session at a time) ─────────────────────────────

@available(macOS 13.0, *)
private var gStream:  SCStream?          = nil
@available(macOS 13.0, *)
private var gCapture: SCLoopbackCapture? = nil

// ── Internal start/stop (macOS 13.0+) ────────────────────────────────────────

@available(macOS 13.0, *)
private func doStart(_ cb: AudioDataCallback) -> Bool {
    var ok = false
    let outer = DispatchSemaphore(value: 0)

    SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: false) { content, err in
        defer { outer.signal() }
        guard let content, err == nil, let display = content.displays.first else {
            NSLog("[LiveMeeting] SCShareableContent error: %@", err?.localizedDescription ?? "no display")
            return
        }

        let filter = SCContentFilter(
            display: display,
            excludingApplications: [],
            exceptingWindows: []
        )
        let cfg = SCStreamConfiguration()
        cfg.capturesAudio = true
        cfg.sampleRate    = 16000
        cfg.channelCount  = 1
        // Minimal video footprint so we're effectively audio-only
        cfg.width  = 2
        cfg.height = 2
        cfg.minimumFrameInterval = CMTime(value: 1, timescale: 1) // 1 fps

        let capture = SCLoopbackCapture(cb)
        do {
            let stream = SCStream(filter: filter, configuration: cfg, delegate: capture)
            try stream.addStreamOutput(
                capture, type: .audio,
                sampleHandlerQueue: .global(qos: .userInteractive)
            )
            let inner = DispatchSemaphore(value: 0)
            stream.startCapture { startErr in
                if let startErr {
                    NSLog("[LiveMeeting] startCapture error: %@", startErr.localizedDescription)
                } else {
                    gStream  = stream
                    gCapture = capture
                    ok       = true
                }
                inner.signal()
            }
            inner.wait()
        } catch {
            NSLog("[LiveMeeting] SCStream setup error: %@", error.localizedDescription)
        }
    }
    outer.wait()
    return ok
}

@available(macOS 13.0, *)
private func doStop() {
    guard let stream = gStream else { return }
    let sem = DispatchSemaphore(value: 0)
    stream.stopCapture { _ in sem.signal() }
    sem.wait()
    gStream  = nil
    gCapture = nil
}

// ── C-callable exports ────────────────────────────────────────────────────────

@_cdecl("mac_loopback_start")
public func macLoopbackStart(callback: AudioDataCallback) -> Bool {
    if #available(macOS 13.0, *) {
        return doStart(callback)
    }
    NSLog("[LiveMeeting] System audio capture requires macOS 13.0+")
    return false
}

@_cdecl("mac_loopback_stop")
public func macLoopbackStop() {
    if #available(macOS 13.0, *) { doStop() }
}
