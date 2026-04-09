/// WASAPI loopback capture for system audio on Windows.
/// Captures what's playing through the default output device (speakers/headphones).
use super::{AudioChunk, AudioSource};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use wasapi::*;

const TARGET_SAMPLE_RATE: u32 = 16000;

pub fn start_loopback_capture(
    tx: mpsc::Sender<AudioChunk>,
    stop_flag: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<()>, String> {
    let handle = std::thread::spawn(move || {
        if let Err(e) = run_loopback(tx, stop_flag, paused) {
            tracing::error!("Loopback capture error: {e}");
        }
    });
    Ok(handle)
}

fn run_loopback(
    tx: mpsc::Sender<AudioChunk>,
    stop_flag: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error>> {
    initialize_mta().ok()?;

    let enumerator = DeviceEnumerator::new()?;
    // Direction::Render gets the default OUTPUT device — loopback captures from it
    let device = enumerator.get_default_device(&Direction::Render)?;
    let device_name = device.get_friendlyname()?;
    tracing::info!("Loopback capturing from output device: {device_name}");

    let mut audio_client = device.get_iaudioclient()?;

    // Request f32 format; autoconvert will handle any mismatch
    let desired_format = WaveFormat::new(32, 32, &SampleType::Float, 44100, 2, None);
    let blockalign = desired_format.get_blockalign() as usize;
    let channels = 2u16;
    let source_rate = 44100u32;

    let (_def_time, min_time) = audio_client.get_device_period()?;
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: min_time,
    };

    // Initialize in loopback mode (Capture from a Render device)
    audio_client.initialize_client(&desired_format, &Direction::Capture, &mode)?;

    let h_event = audio_client.set_get_eventhandle()?;
    let buffer_frame_count = audio_client.get_buffer_size()?;
    let capture_client = audio_client.get_audiocaptureclient()?;

    let mut sample_queue: VecDeque<u8> =
        VecDeque::with_capacity(blockalign * (1024 + 2 * buffer_frame_count as usize));

    let chunk_frames = 4096usize;
    let chunk_bytes = blockalign * chunk_frames;

    audio_client.start_stream()?;
    tracing::info!("WASAPI loopback capture started");

    while !stop_flag.load(Ordering::Relaxed) {
        // Read available data
        capture_client.read_from_device_to_deque(&mut sample_queue)?;

        // Send chunks when we have enough
        while sample_queue.len() >= chunk_bytes {
            if paused.load(Ordering::Relaxed) {
                // Drain but don't send
                for _ in 0..chunk_bytes {
                    sample_queue.pop_front();
                }
                continue;
            }

            let mut raw = vec![0u8; chunk_bytes];
            for byte in raw.iter_mut() {
                *byte = sample_queue.pop_front().unwrap();
            }

            // Convert f32 bytes to i16 samples, downmix stereo to mono, resample
            let f32_samples: Vec<f32> = raw
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect();

            let resampled = super::capture::convert_and_resample(
                &f32_samples,
                channels,
                source_rate,
            );

            if resampled.is_empty() {
                continue;
            }

            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let duration_ms = (resampled.len() as u32 * 1000) / TARGET_SAMPLE_RATE;

            let _ = tx.try_send(AudioChunk {
                data: resampled,
                timestamp_ms: now,
                source: AudioSource::SystemAudio,
                duration_ms,
            });
        }

        // Wait for next buffer event (timeout 100ms)
        if h_event.wait_for_event(100).is_err() {
            // Timeout is fine, just loop and check stop_flag
        }
    }

    audio_client.stop_stream()?;
    tracing::info!("WASAPI loopback capture stopped");
    Ok(())
}
