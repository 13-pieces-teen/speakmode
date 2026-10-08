//! CPAL/WASAPI owner thread. Control, metadata, and PCM have separate queues.
//! Importing/enumerating does not acquire microphone input.
use super::{PlatformError, Result};
use super::resampler::FrameResampler;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Receiver, Sender, bounded};
use serde::{Serialize, Deserialize};
use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, AtomicU64, Ordering}};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const OUTPUT_HZ: usize = 16_000;
const FRAME_SAMPLES: usize = 1_600;
static ACTIVE_CAPTURE: AtomicBool = AtomicBool::new(false);
struct CaptureLease;
impl Drop for CaptureLease { fn drop(&mut self) { ACTIVE_CAPTURE.store(false, Ordering::SeqCst); } }
/// True only after the native owner actually exits, including startup errors.
pub fn capture_resources_released() -> bool { !ACTIVE_CAPTURE.load(Ordering::SeqCst) }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDevice {
    /// CPAL 0.16 does not expose a stable WASAPI endpoint ID. Unique device
    /// names are accepted; duplicate names are not silently index-selected.
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: String,
}

pub fn enumerate_inputs() -> Result<Vec<InputDevice>> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().and_then(|d| d.name().ok());
    let mut output = Vec::new();
    for device in host.input_devices().map_err(unavailable)? {
        let name = device.name().map_err(unavailable)?;
        let config = device.default_input_config().map_err(unavailable)?;
        output.push(InputDevice { id: format!("wasapi:{name}"), is_default: default_name.as_ref() == Some(&name),
            name, sample_rate: config.sample_rate().0, channels: config.channels(), sample_format: config.sample_format().to_string() });
    }
    Ok(output)
}

#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub sequence: u64,
    pub pcm16: Vec<i16>,
    pub padding_samples: usize,
}
impl AudioFrame {
    pub fn to_le_bytes(&self) -> Vec<u8> { self.pcm16.iter().flat_map(|v| v.to_le_bytes()).collect() }
}
#[derive(Debug, Clone)]
pub enum AudioEvent {
    FirstSamples { at: Instant },
    Level(f32),
    Error(String),
}
#[derive(Debug, Clone)]
pub struct CaptureSummary {
    pub complete: bool,
    pub cancelled: bool,
    pub captured_samples: u64,
    pub input_sample_rate: u32,
    pub emitted_frames: u64,
    pub first_samples_received: bool,
    pub error: Option<String>,
}
impl CaptureSummary {
    pub fn duration_ms(&self) -> u64 { self.captured_samples.saturating_mul(1_000) / u64::from(self.input_sample_rate.max(1)) }
}

enum Control { Stop, Cancel }
struct RawChunk { samples: Vec<f32>, accepted_at: Instant }
pub struct CaptureHandle {
    control: Sender<Control>,
    accepting: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    duration_ms: Arc<AtomicU64>,
    summary: Receiver<CaptureSummary>,
    pub frames: Receiver<AudioFrame>,
    pub events: Receiver<AudioEvent>,
    worker: Option<JoinHandle<()>>,
}
impl CaptureHandle {
    /// Explicit user start only. Caller must concurrently drain `frames` to
    /// its provider. Native 44 + bridge 5 + provider in-flight 1 = 50 frames,
    /// a combined 5-second budget. Overflow fails instead of discarding audio.
    pub fn start(device_id: Option<&str>) -> Result<Self> {
        if ACTIVE_CAPTURE.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return Err(PlatformError::Unavailable("previous native capture owner has not released resources".into()));
        }
        let lease = CaptureLease;
        let device_id = device_id.map(str::to_owned);
        let (control_tx, control_rx) = bounded(1);
        let (frame_tx, frame_rx) = bounded(44);
        let (event_tx, event_rx) = bounded(32);
        let (summary_tx, summary_rx) = bounded(1);
        let (ready_tx, ready_rx) = bounded(1);
        let accepting = Arc::new(AtomicBool::new(true));
        let cancelled = Arc::new(AtomicBool::new(false));
        let duration_ms = Arc::new(AtomicU64::new(0));
        let duration_progress = Arc::clone(&duration_ms);
        let accept = Arc::clone(&accepting);
        let cancel = Arc::clone(&cancelled);
        let worker = thread::Builder::new().name("speakmode-capture".into()).spawn(move || {
            let _lease = lease;
            match capture_worker(device_id, control_rx, frame_tx, &event_tx, Arc::clone(&accept), cancel, &ready_tx, duration_progress) {
                Ok(summary) => { let _ = summary_tx.send(summary); },
                Err(error) => { accept.store(false, Ordering::SeqCst); let _ = ready_tx.try_send(Err(error.to_string()));
                    let _ = event_tx.try_send(AudioEvent::Error(error.to_string())); },
            }
        }).map_err(unavailable)?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(Self { control: control_tx, accepting, cancelled, duration_ms, summary: summary_rx, frames: frame_rx, events: event_rx, worker: Some(worker) }),
            Ok(Err(error)) => { accepting.store(false, Ordering::SeqCst); Err(PlatformError::Unavailable(error)) },
            Err(_) => { accepting.store(false, Ordering::SeqCst); cancelled.store(true, Ordering::SeqCst); let _ = control_tx.try_send(Control::Cancel); Err(PlatformError::Timeout("audio stream creation")) },
        }
    }
    /// Fast cutoff; it never waits behind raw sample messages. Worker drop of
    /// CPAL stream confirms callback quiescence before it drains accepted data.
    pub fn request_stop(&self) { self.accepting.store(false, Ordering::SeqCst); let _ = self.control.try_send(Control::Stop); }
    pub fn cancel(&self) { self.cancelled.store(true, Ordering::SeqCst); self.accepting.store(false, Ordering::SeqCst); let _ = self.control.try_send(Control::Cancel); }
    /// Real mono source samples, including silence, excluding resampler padding
    /// and startup/network time. The owner updates this outside the callback.
    pub fn duration_ms(&self) -> u64 { self.duration_ms.load(Ordering::SeqCst) }
    pub fn wait_drained(&mut self, deadline: Duration) -> Result<CaptureSummary> {
        let summary = self.summary.recv_timeout(deadline).map_err(|e| match e {
            crossbeam_channel::RecvTimeoutError::Timeout => PlatformError::Timeout("audio cleanup/drain"),
            crossbeam_channel::RecvTimeoutError::Disconnected => PlatformError::Disconnected,
        })?;
        // A summary is sent only after stream destruction + full finish. Join
        // may wait briefly for that send to return, never for capture itself.
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
        Ok(summary)
    }
}
impl Drop for CaptureHandle { fn drop(&mut self) { self.cancel(); } }

fn unavailable(error: impl std::fmt::Display) -> PlatformError { PlatformError::Unavailable(error.to_string()) }

fn select_device(id: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Some(id) = id {
        let mut matching = host.input_devices().map_err(unavailable)?.filter(|d| d.name().ok().is_some_and(|n| format!("wasapi:{n}") == id));
        let device = matching.next().ok_or_else(|| PlatformError::Unavailable("selected microphone no longer exists".into()))?;
        if matching.next().is_some() { return Err(PlatformError::Unavailable("microphone names are ambiguous; use system default".into())); }
        Ok(device)
    } else { host.default_input_device().ok_or_else(|| PlatformError::Unavailable("no input microphone".into())) }
}

fn capture_worker(device_id: Option<String>, control: Receiver<Control>, frames: Sender<AudioFrame>,
    events: &Sender<AudioEvent>, accepting: Arc<AtomicBool>, cancellation: Arc<AtomicBool>, ready: &Sender<std::result::Result<(), String>>, duration_progress: Arc<AtomicU64>) -> Result<CaptureSummary> {
    let device = select_device(device_id.as_deref())?;
    let supported = device.default_input_config().map_err(unavailable)?;
    let config: cpal::StreamConfig = supported.clone().into();
    if config.sample_rate.0 < 8_000 || config.sample_rate.0 > 384_000 || config.channels == 0 {
        return Err(PlatformError::Invalid("unsupported microphone configuration".into()));
    }
    let (raw_tx, raw_rx) = bounded::<RawChunk>(64);
    let queued_samples = Arc::new(AtomicUsize::new(0));
    let fault = Arc::new(AtomicUsize::new(0)); // 1=source queue, 2=device, 3=malformed frame
    let budget = config.sample_rate.0 as usize * 2;
    let stream = build_stream(&device, &config, supported.sample_format(), raw_tx,
        Arc::clone(&accepting), Arc::clone(&queued_samples), Arc::clone(&fault), budget)?;
    let mut resampler = FrameResampler::new(config.sample_rate.0 as usize, OUTPUT_HZ, Duration::from_millis(100));
    stream.play().map_err(unavailable)?;
    let _ = ready.send(Ok(()));
    let mut captured: u64 = 0;
    let mut sequence = 0;
    let mut error: Option<String> = None;
    let mut cancelled = false;
    let started = Instant::now();
    let mut level_at = Instant::now() - Duration::from_secs(1);
    loop {
        if cancellation.load(Ordering::SeqCst) { cancelled = true; break; }
        match control.try_recv() {
            Ok(Control::Cancel) => { cancelled = true; break; },
            Ok(Control::Stop) => break,
            Err(crossbeam_channel::TryRecvError::Disconnected) => { cancelled = true; break; },
            Err(crossbeam_channel::TryRecvError::Empty) => {},
        }
        let code = fault.load(Ordering::SeqCst);
        if code != 0 { error = Some(match code { 1 => "audio input queue overflow", 2 => "microphone stream error/disconnected", _ => "malformed microphone frame" }.into()); break; }
        if captured == 0 && started.elapsed() >= Duration::from_secs(10) { error = Some("microphone first-sample deadline exceeded".into()); break; }
        match raw_rx.recv_timeout(Duration::from_millis(10)) {
            Ok(chunk) => {
                account_received_chunk(&chunk, &queued_samples, &mut captured, events);
                duration_progress.store(captured.saturating_mul(1000) / u64::from(config.sample_rate.0), Ordering::SeqCst);
                if level_at.elapsed() >= Duration::from_millis(50) {
                    let _ = events.try_send(AudioEvent::Level(rms(&chunk.samples))); level_at = Instant::now();
                }
                resampler.push(&chunk.samples, |frame| emit_frame(frame, 0, &frames, &mut sequence, &mut error));
                if let Some(value) = resampler.error() { error = Some(value.into()); }
                if error.is_some() { break; }
            },
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => { error = Some("microphone input disconnected".into()); break; },
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {},
        }
    }
    accepting.store(false, Ordering::SeqCst);
    // On WASAPI CPAL Drop commands termination and joins its callback owner.
    // Caller wait_drained deadline keeps admission closed if native cleanup stalls.
    drop(stream);
    cancelled |= cancellation.load(Ordering::SeqCst);
    if !cancelled && error.is_none() {
        while let Ok(chunk) = raw_rx.try_recv() {
            account_received_chunk(&chunk, &queued_samples, &mut captured, events);
            duration_progress.store(captured.saturating_mul(1000) / u64::from(config.sample_rate.0), Ordering::SeqCst);
            resampler.push(&chunk.samples, |frame| emit_frame(frame, 0, &frames, &mut sequence, &mut error));
        }
        let actual_output = captured as usize * OUTPUT_HZ / config.sample_rate.0 as usize + resampler.output_delay_samples();
        // Preserve ALL finish emissions; expose only actual synthetic tail.
        resampler.finish(|frame| {
            let meaningful = actual_output.saturating_sub(sequence as usize * FRAME_SAMPLES).min(FRAME_SAMPLES);
            emit_frame(frame, FRAME_SAMPLES - meaningful, &frames, &mut sequence, &mut error);
        });
        if let Some(value) = resampler.error() { error = Some(value.into()); }
    }
    if fault.load(Ordering::SeqCst) != 0 && error.is_none() { error = Some("audio source failed before cutoff completed".into()); }
    cancelled |= cancellation.load(Ordering::SeqCst);
    if let Some(value) = &error { let _ = events.try_send(AudioEvent::Error(value.clone())); }
    Ok(CaptureSummary { complete: !cancelled && error.is_none(), cancelled, captured_samples: captured,
        input_sample_rate: config.sample_rate.0, emitted_frames: sequence, first_samples_received: captured > 0, error })
}

fn account_received_chunk(chunk: &RawChunk, queued: &AtomicUsize, captured: &mut u64, events: &Sender<AudioEvent>) {
    queued.fetch_sub(chunk.samples.len(), Ordering::SeqCst);
    if *captured == 0 { let _ = events.try_send(AudioEvent::FirstSamples { at: chunk.accepted_at }); }
    *captured += chunk.samples.len() as u64;
}

fn emit_frame(frame: &[f32], padding_samples: usize, output: &Sender<AudioFrame>, sequence: &mut u64, error: &mut Option<String>) {
    if error.is_some() { return; }
    let pcm16 = frame.iter().map(|value| quantize(*value)).collect();
    if output.try_send(AudioFrame { sequence: *sequence, pcm16, padding_samples }).is_err() {
        *error = Some("ASR bounded audio queue full or consumer disconnected".into());
    } else { *sequence += 1; }
}

fn build_stream(device: &cpal::Device, config: &cpal::StreamConfig, format: cpal::SampleFormat,
    sender: Sender<RawChunk>, accepting: Arc<AtomicBool>, queued: Arc<AtomicUsize>, fault: Arc<AtomicUsize>, budget: usize) -> Result<cpal::Stream> {
    macro_rules! sample { ($ty:ty) => { build_typed::<$ty>(device, config, sender, accepting, queued, fault, budget) }; }
    match format {
        cpal::SampleFormat::I8 => sample!(i8), cpal::SampleFormat::I16 => sample!(i16),
        cpal::SampleFormat::I24 => sample!(cpal::I24), cpal::SampleFormat::I32 => sample!(i32),
        cpal::SampleFormat::I64 => sample!(i64), cpal::SampleFormat::U8 => sample!(u8),
        cpal::SampleFormat::U16 => sample!(u16), cpal::SampleFormat::U32 => sample!(u32),
        cpal::SampleFormat::U64 => sample!(u64), cpal::SampleFormat::F32 => sample!(f32),
        cpal::SampleFormat::F64 => sample!(f64),
        _ => Err(PlatformError::Unavailable(format!("unsupported audio sample format: {format}"))),
    }
}
#[allow(deprecated)] // fetch_update retains compatibility with earlier stable MSVC Rust.
fn build_typed<T>(device: &cpal::Device, config: &cpal::StreamConfig, sender: Sender<RawChunk>,
    accepting: Arc<AtomicBool>, queued: Arc<AtomicUsize>, fault: Arc<AtomicUsize>, budget: usize) -> Result<cpal::Stream>
where T: cpal::SizedSample, f32: cpal::FromSample<T> {
    let channels = usize::from(config.channels);
    let error_fault = Arc::clone(&fault);
    let error_accept = Arc::clone(&accepting);
    device.build_input_stream(config, move |data: &[T], _| {
        if !accepting.load(Ordering::SeqCst) || data.is_empty() { return; }
        if data.len() % channels != 0 { fault.store(3, Ordering::SeqCst); accepting.store(false, Ordering::SeqCst); return; }
        let count = data.len() / channels;
        if queued.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_add(count).filter(|v| *v <= budget)).is_err() {
            fault.store(1, Ordering::SeqCst); accepting.store(false, Ordering::SeqCst); return;
        }
        // Metadata only: retain the accepted callback time through queue wait.
        // No logging, locking or I/O is added to the real-time callback.
        let accepted_at = Instant::now();
        let mono = downmix(data, channels);
        if sender.try_send(RawChunk { samples: mono, accepted_at }).is_err() { queued.fetch_sub(count, Ordering::SeqCst); fault.store(1, Ordering::SeqCst); accepting.store(false, Ordering::SeqCst); }
    }, move |_| { error_fault.store(2, Ordering::SeqCst); error_accept.store(false, Ordering::SeqCst); }, None).map_err(unavailable)
}
pub fn downmix<T: cpal::Sample>(samples: &[T], channels: usize) -> Vec<f32> where f32: cpal::FromSample<T> {
    if channels == 0 { return Vec::new(); }
    samples.chunks_exact(channels).map(|chunk| {
        chunk.iter().map(|sample| sample.to_sample::<f32>()).map(|v| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 }).sum::<f32>() / channels as f32
    }).collect()
}
pub fn quantize(value: f32) -> i16 {
    if !value.is_finite() { return 0; }
    if value <= -1.0 { i16::MIN } else { (value.min(1.0) * f32::from(i16::MAX)).round() as i16 }
}
fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() { return 0.0; }
    (samples.iter().map(|v| v * v).sum::<f32>() / samples.len() as f32).sqrt().min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn stereo_downmix_preserves_time_not_channel_count() {
        assert_eq!(downmix(&[1.0_f32, -1.0, 0.5, 0.5], 2), vec![0.0, 0.5]);
    }
    #[test] fn integer_center_and_pcm_endpoints_are_correct() {
        assert_eq!(downmix(&[0_i16, i16::MAX, i16::MIN], 1)[0], 0.0);
        assert_eq!(downmix(&[128_u8], 1), vec![0.0]);
        assert_eq!(quantize(-1.0), i16::MIN); assert_eq!(quantize(1.0), i16::MAX);
        assert_eq!(quantize(f32::NAN), 0);
    }
    #[test] fn full_provider_queue_is_an_error_not_a_drop() {
        let (tx, _rx) = bounded(1); let mut seq = 0; let mut error = None;
        emit_frame(&[0.1], 0, &tx, &mut seq, &mut error);
        emit_frame(&[0.2], 0, &tx, &mut seq, &mut error);
        assert_eq!(seq, 1); assert!(error.is_some());
    }
    #[test] fn source_duration_excludes_resampler_padding() {
        let s = CaptureSummary { complete: true, cancelled: false, captured_samples: 48_001,
            input_sample_rate: 48_000, emitted_frames: 11, first_samples_received: true, error: None };
        assert_eq!(s.duration_ms(), 1_000);
    }
    #[test] fn finish_preserves_multiple_frames_including_last_padding() {
        let mut r = FrameResampler::new(48_000, 16_000, Duration::from_millis(100));
        let mut output = Vec::new();
        r.push(&vec![0.2; 50_333], |frame| output.push(frame.to_vec()));
        let before = output.len(); r.finish(|frame| output.push(frame.to_vec()));
        assert!(output.len() > before); assert!(output.iter().all(|frame| frame.len() == 1_600));
        assert!(r.error().is_none());
    }
    #[test] fn controller_is_send_without_moving_a_cpal_stream() {
        fn assert_send<T: Send>() {}
        assert_send::<CaptureHandle>();
    }
    #[test] fn first_sample_time_survives_normal_queue_wait_and_stop_drain() {
        let accepted_at = Instant::now() - Duration::from_millis(250);
        for drain_only in [false, true] {
            let (raw_tx, raw_rx) = bounded(2);
            let (events, received_events) = bounded(4);
            let queued = AtomicUsize::new(5);
            let mut captured = 0;
            let first_samples = vec![0.2, -0.2, 0.0];
            raw_tx.send(RawChunk { samples: first_samples.clone(), accepted_at }).unwrap();
            raw_tx.send(RawChunk { samples: vec![0.3, 0.4], accepted_at: accepted_at + Duration::from_millis(10) }).unwrap();
            let first = if drain_only { raw_rx.try_recv().unwrap() } else { raw_rx.recv_timeout(Duration::from_millis(10)).unwrap() };
            let dequeued_at = Instant::now();
            account_received_chunk(&first, &queued, &mut captured, &events);
            assert_eq!(first.samples, first_samples);
            assert_eq!(queued.load(Ordering::SeqCst), 2);assert_eq!(captured, 3);
            match received_events.try_recv().unwrap() {
                AudioEvent::FirstSamples { at } => { assert_eq!(at, accepted_at);assert!(at < dequeued_at); },
                _ => panic!("expected accepted callback timestamp"),
            }
            let second = raw_rx.try_recv().unwrap();
            account_received_chunk(&second, &queued, &mut captured, &events);
            assert_eq!(queued.load(Ordering::SeqCst), 0);assert_eq!(captured, 5);
            assert!(received_events.try_recv().is_err());
        }
    }
    #[test] fn raw_chunk_metadata_does_not_change_pcm_samples_or_duration() {
        let samples = downmix(&[0.2_f32, 0.4, -0.2, -0.4], 2);
        let accepted_at = Instant::now() - Duration::from_secs(1);
        let chunk = RawChunk { samples: samples.clone(), accepted_at };
        let queued = AtomicUsize::new(chunk.samples.len());let mut captured = 0;
        let (events, _received_events) = bounded(1);
        account_received_chunk(&chunk, &queued, &mut captured, &events);
        let original_pcm: Vec<_> = samples.iter().map(|value| quantize(*value)).collect();
        let chunk_pcm: Vec<_> = chunk.samples.iter().map(|value| quantize(*value)).collect();
        assert_eq!(chunk_pcm, original_pcm);assert_eq!(captured, 2);
        let summary = CaptureSummary { complete: true, cancelled: false, captured_samples: captured,
            input_sample_rate: 16_000, emitted_frames: 0, first_samples_received: true, error: None };
        assert_eq!(summary.duration_ms(), 0); // callback/queue elapsed time is not audio duration.
    }
}
