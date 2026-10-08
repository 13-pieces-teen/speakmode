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
const RAW_CHUNKS: usize = 64;
const FIRST_SAMPLE_TIMEOUT: Duration = Duration::from_secs(10);
/// Initial engineering threshold; real device and CPU-load acceptance is
/// separate from the deterministic supply tests below. Silence has no limit.
const PCM_STALL_TIMEOUT: Duration = Duration::from_secs(5);
pub const PCM_STALL_ERROR: &str = "microphone PCM callback stalled";
pub const FIRST_SAMPLE_TIMEOUT_ERROR: &str = "microphone first-sample deadline exceeded";
pub const OUTPUT_QUEUE_FULL_ERROR: &str = "ASR bounded audio output queue full";
pub const OUTPUT_CONSUMER_DISCONNECTED_ERROR: &str = "ASR audio output consumer disconnected";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFailureReason { PcmStalled, FirstSampleTimeout, DeviceError, QueueOverflow, MalformedFrame }

/// Classifies native summary/event errors without depending on localized UI
/// text. Unknown resampler/stream-creation errors retain the caller's fallback.
pub fn classify_capture_error(error: Option<&str>) -> Option<CaptureFailureReason> {
    match error? {
        PCM_STALL_ERROR => Some(CaptureFailureReason::PcmStalled),
        FIRST_SAMPLE_TIMEOUT_ERROR => Some(CaptureFailureReason::FirstSampleTimeout),
        "microphone stream error/disconnected" | "microphone input disconnected" => Some(CaptureFailureReason::DeviceError),
        "audio input queue overflow" | OUTPUT_QUEUE_FULL_ERROR => Some(CaptureFailureReason::QueueOverflow),
        "malformed microphone frame" => Some(CaptureFailureReason::MalformedFrame),
        _ => None,
    }
}

/// Callback writes only one atomic timestamp after a nonempty PCM submission
/// has succeeded. Zero means no PCM yet; other values are monotonic nanoseconds
/// since this capture's epoch plus one. No locks, channel sends or allocations
/// are added to the callback. Worker time/RMS/provider progress are unrelated.
struct PcmHeartbeat { epoch: Instant, last_accepted_ns: AtomicU64 }
impl PcmHeartbeat {
    fn new_at(epoch: Instant) -> Self { Self { epoch, last_accepted_ns: AtomicU64::new(0) } }
    fn mark_at(&self, accepted_at: Instant) {
        let offset = accepted_at.saturating_duration_since(self.epoch).as_nanos().min(u128::from(u64::MAX - 1)) as u64;
        self.last_accepted_ns.fetch_max(offset + 1, Ordering::SeqCst);
    }
    fn last_accepted_at(&self) -> Option<Instant> {
        let value = self.last_accepted_ns.load(Ordering::SeqCst);
        (value != 0).then(|| self.epoch + Duration::from_nanos(value - 1))
    }
    fn failure_at(&self, started_at: Instant, now: Instant, accepting: bool) -> Option<&'static str> {
        // User cutoff wins over liveness; stop/cancel drains already accepted
        // samples without judging the deliberately stopped PCM supply.
        if !accepting { return None; }
        match self.last_accepted_at() {
            None if now.saturating_duration_since(started_at) >= FIRST_SAMPLE_TIMEOUT => Some(FIRST_SAMPLE_TIMEOUT_ERROR),
            Some(last) if now.saturating_duration_since(last) >= PCM_STALL_TIMEOUT => Some(PCM_STALL_ERROR),
            _ => None,
        }
    }
}

fn source_fault_error(code: usize) -> &'static str {
    match code { 1 => "audio input queue overflow", 2 => "microphone stream error/disconnected", _ => "malformed microphone frame" }
}
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

/// The audio callback borrows from a fixed pool instead of allocating one Vec
/// per hardware callback. Pool and channel allocation happen before play().
/// Callback operations remain nonblocking; an exhausted pool fails capture
/// explicitly, just like the existing bounded source queue.
struct CallbackIngress {
    sender: Sender<RawChunk>,
    available: Receiver<Vec<f32>>,
    recycle: Sender<Vec<f32>>,
    accepting: Arc<AtomicBool>,
    queued: Arc<AtomicUsize>,
    fault: Arc<AtomicUsize>,
    heartbeat: Arc<PcmHeartbeat>,
    channels: usize,
    budget: usize,
    chunk_capacity: usize,
}
impl CallbackIngress {
    fn new(channels: usize, budget: usize, accepting: Arc<AtomicBool>, queued: Arc<AtomicUsize>,
        fault: Arc<AtomicUsize>) -> (Self, Receiver<RawChunk>, Sender<Vec<f32>>) {
        let (sender, receiver) = bounded(RAW_CHUNKS);
        let (recycle, available) = bounded(RAW_CHUNKS);
        let chunk_capacity = budget.div_ceil(RAW_CHUNKS).max(1);
        for _ in 0..RAW_CHUNKS {
            // Startup-only allocation, outside the native callback thread.
            recycle.try_send(Vec::with_capacity(chunk_capacity)).expect("new pool has capacity");
        }
        let worker_recycle = recycle.clone();
        (Self { sender, available, recycle, accepting, queued, fault, heartbeat: Arc::new(PcmHeartbeat::new_at(Instant::now())),
            channels, budget, chunk_capacity }, receiver, worker_recycle)
    }

    fn fail(&self, code: usize) {
        let _ = self.fault.compare_exchange(0, code, Ordering::SeqCst, Ordering::SeqCst);
        self.accepting.store(false, Ordering::SeqCst);
    }

    /// One hardware callback may be larger than a pool slot. Split only at
    /// complete mono frames, retaining its timestamp and original sample order.
    #[allow(deprecated)]
    fn submit_at<T: cpal::Sample>(&self, data: &[T], accepted_at: Instant) where f32: cpal::FromSample<T> {
        if !self.accepting.load(Ordering::SeqCst) || data.is_empty() { return; }
        if self.channels == 0 || data.len() % self.channels != 0 { self.fail(3); return; }
        let count = data.len() / self.channels;
        if self.queued.fetch_update(Ordering::SeqCst, Ordering::SeqCst,
            |n| n.checked_add(count).filter(|v| *v <= self.budget)).is_err() {
            self.fail(1); return;
        }
        let mut remaining = count;
        while remaining != 0 {
            let mut buffer = match self.available.try_recv() {
                Ok(buffer) => buffer,
                Err(_) => { self.queued.fetch_sub(remaining, Ordering::SeqCst); self.fail(1); return; },
            };
            debug_assert!(buffer.is_empty());
            debug_assert!(buffer.capacity() >= self.chunk_capacity);
            let chunk_count = remaining.min(self.chunk_capacity);
            let offset = (count - remaining) * self.channels;
            downmix_into(&data[offset..offset + chunk_count * self.channels], self.channels, &mut buffer);
            match self.sender.try_send(RawChunk { samples: buffer, accepted_at }) {
                Ok(()) => remaining -= chunk_count,
                Err(error) => {
                    // Any already accepted chunks retain their accounting. The
                    // rest was reserved but never handed to the worker.
                    let mut buffer = error.into_inner().samples;
                    buffer.clear();
                    let _ = self.recycle.try_send(buffer);
                    self.queued.fetch_sub(remaining, Ordering::SeqCst);
                    self.fail(1); return;
                },
            }
        }
        self.heartbeat.mark_at(accepted_at);
    }
}

fn recycle_chunk(mut chunk: RawChunk, recycle: &Sender<Vec<f32>>) {
    chunk.samples.clear();
    // The callback removed exactly this slot before enqueueing it. A stopped
    // or disconnected pool is harmless; never block the worker on recycling.
    let _ = recycle.try_send(chunk.samples);
}
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
    let queued_samples = Arc::new(AtomicUsize::new(0));
    let fault = Arc::new(AtomicUsize::new(0)); // 1=source queue, 2=device, 3=malformed frame
    let budget = config.sample_rate.0 as usize * 2;
    let (ingress, raw_rx, recycle) = CallbackIngress::new(usize::from(config.channels), budget,
        Arc::clone(&accepting), Arc::clone(&queued_samples), Arc::clone(&fault));
    let heartbeat = Arc::clone(&ingress.heartbeat);
    let stream = build_stream(&device, &config, supported.sample_format(), ingress)?;
    let mut resampler = FrameResampler::new(config.sample_rate.0 as usize, OUTPUT_HZ, Duration::from_millis(100));
    stream.play().map_err(unavailable)?;
    let started = Instant::now();
    let _ = ready.send(Ok(()));
    let mut captured: u64 = 0;
    let mut sequence = 0;
    let mut error: Option<String> = None;
    let mut cancelled = false;
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
        if code != 0 { error = Some(source_fault_error(code).into()); break; }
        if let Some(reason) = heartbeat.failure_at(started, Instant::now(), accepting.load(Ordering::SeqCst)) {
            error = Some(reason.into()); break;
        }
        match raw_rx.recv_timeout(Duration::from_millis(10)) {
            Ok(chunk) => {
                account_received_chunk(&chunk, &queued_samples, &mut captured, events);
                duration_progress.store(captured.saturating_mul(1000) / u64::from(config.sample_rate.0), Ordering::SeqCst);
                if level_at.elapsed() >= Duration::from_millis(50) {
                    let _ = events.try_send(AudioEvent::Level(rms(&chunk.samples))); level_at = Instant::now();
                }
                resampler.push(&chunk.samples, |frame| emit_frame(frame, 0, &frames, &mut sequence, &mut error));
                recycle_chunk(chunk, &recycle);
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
            recycle_chunk(chunk, &recycle);
        }
        let actual_output = captured as usize * OUTPUT_HZ / config.sample_rate.0 as usize + resampler.output_delay_samples();
        // Preserve ALL finish emissions; expose only actual synthetic tail.
        resampler.finish(|frame| {
            let meaningful = actual_output.saturating_sub(sequence as usize * FRAME_SAMPLES).min(FRAME_SAMPLES);
            emit_frame(frame, FRAME_SAMPLES - meaningful, &frames, &mut sequence, &mut error);
        });
        if let Some(value) = resampler.error() { error = Some(value.into()); }
    }
    let final_fault = fault.load(Ordering::SeqCst);
    if final_fault != 0 && error.is_none() { error = Some(source_fault_error(final_fault).into()); }
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
    match output.try_send(AudioFrame { sequence: *sequence, pcm16, padding_samples }) {
        Ok(()) => *sequence += 1,
        Err(crossbeam_channel::TrySendError::Full(_)) => *error = Some(OUTPUT_QUEUE_FULL_ERROR.into()),
        // A downstream receiver may close because of its own cloud failure.
        // Keep the original ASR error authoritative rather than misclassifying
        // that consequence as a proven native queue overflow.
        Err(crossbeam_channel::TrySendError::Disconnected(_)) => *error = Some(OUTPUT_CONSUMER_DISCONNECTED_ERROR.into()),
    }
}

fn build_stream(device: &cpal::Device, config: &cpal::StreamConfig, format: cpal::SampleFormat,
    ingress: CallbackIngress) -> Result<cpal::Stream> {
    macro_rules! sample { ($ty:ty) => { build_typed::<$ty>(device, config, ingress) }; }
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
fn build_typed<T>(device: &cpal::Device, config: &cpal::StreamConfig, ingress: CallbackIngress) -> Result<cpal::Stream>
where T: cpal::SizedSample, f32: cpal::FromSample<T> {
    let error_fault = Arc::clone(&ingress.fault);
    let error_accept = Arc::clone(&ingress.accepting);
    device.build_input_stream(config, move |data: &[T], _| {
        ingress.submit_at(data, Instant::now());
    }, move |_| { error_fault.store(2, Ordering::SeqCst); error_accept.store(false, Ordering::SeqCst); }, None).map_err(unavailable)
}
pub fn downmix<T: cpal::Sample>(samples: &[T], channels: usize) -> Vec<f32> where f32: cpal::FromSample<T> {
    if channels == 0 { return Vec::new(); }
    let mut output = Vec::with_capacity(samples.len() / channels);
    downmix_into(samples, channels, &mut output);
    output
}
fn downmix_into<T: cpal::Sample>(samples: &[T], channels: usize, output: &mut Vec<f32>) where f32: cpal::FromSample<T> {
    for chunk in samples.chunks_exact(channels) {
        let sample = chunk.iter().map(|sample| sample.to_sample::<f32>())
            .map(|v| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 }).sum::<f32>() / channels as f32;
        output.push(sample);
    }
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

    #[test] fn pcm_supply_stall_is_detected_after_first_callback_at_the_five_second_boundary() {
        let started_at = Instant::now();
        let (mut ingress, receiver, recycle) = test_ingress(1, 96_000);
        ingress.heartbeat = Arc::new(PcmHeartbeat::new_at(started_at));
        let accepted_at = started_at + Duration::from_millis(20);
        ingress.submit_at(&[0.0_f32; 480], accepted_at);
        assert_eq!(collect_ingress(&ingress, &receiver, &recycle), vec![0.0; 480]);
        assert_eq!(ingress.heartbeat.failure_at(started_at, accepted_at + PCM_STALL_TIMEOUT - Duration::from_nanos(1), true), None);
        assert_eq!(ingress.heartbeat.failure_at(started_at, accepted_at + PCM_STALL_TIMEOUT, true), Some(PCM_STALL_ERROR));
    }

    #[test] fn first_pcm_deadline_is_ten_seconds_and_empty_callbacks_do_not_extend_it() {
        let started_at = Instant::now();
        let (mut ingress, receiver, _recycle) = test_ingress(1, 96_000);
        ingress.heartbeat = Arc::new(PcmHeartbeat::new_at(started_at));
        ingress.submit_at::<f32>(&[], started_at + Duration::from_secs(9));
        assert!(receiver.is_empty());
        assert_eq!(ingress.heartbeat.last_accepted_at(), None);
        assert_eq!(ingress.heartbeat.failure_at(started_at, started_at + FIRST_SAMPLE_TIMEOUT - Duration::from_nanos(1), true), None);
        assert_eq!(ingress.heartbeat.failure_at(started_at, started_at + FIRST_SAMPLE_TIMEOUT, true), Some(FIRST_SAMPLE_TIMEOUT_ERROR));
    }

    #[test] fn another_successful_pcm_callback_restarts_the_supply_deadline() {
        let started_at = Instant::now();
        let (mut ingress, receiver, recycle) = test_ingress(1, 96_000);
        ingress.heartbeat = Arc::new(PcmHeartbeat::new_at(started_at));
        ingress.submit_at(&[0.0_f32; 480], started_at);
        collect_ingress(&ingress, &receiver, &recycle);
        let next_at = started_at + Duration::from_millis(4_900);
        ingress.submit_at(&[0.0_f32; 480], next_at);
        assert_eq!(ingress.heartbeat.failure_at(started_at, started_at + PCM_STALL_TIMEOUT, true), None);
        assert_eq!(ingress.heartbeat.failure_at(started_at, next_at + PCM_STALL_TIMEOUT, true), Some(PCM_STALL_ERROR));
        assert!(ingress.accepting.load(Ordering::SeqCst));
        assert_eq!(ingress.fault.load(Ordering::SeqCst), 0);
    }

    #[test] fn consuming_old_queued_pcm_does_not_refresh_the_callback_heartbeat() {
        let started_at = Instant::now();
        let (mut ingress, receiver, recycle) = test_ingress(1, 96_000);
        ingress.heartbeat = Arc::new(PcmHeartbeat::new_at(started_at));
        ingress.submit_at(&[0.0_f32; 480], started_at);
        // Even with a source chunk waiting, the callback timestamp is stale.
        assert_eq!(receiver.len(), 1);
        assert_eq!(ingress.heartbeat.failure_at(started_at, started_at + PCM_STALL_TIMEOUT, true), Some(PCM_STALL_ERROR));
        let (events, _received_events) = bounded(1);
        let mut captured = 0;
        let chunk = receiver.try_recv().unwrap();
        account_received_chunk(&chunk, &ingress.queued, &mut captured, &events);
        recycle_chunk(chunk, &recycle);
        assert_eq!(captured, 480);
        assert_eq!(ingress.heartbeat.last_accepted_at(), Some(started_at));
        assert_eq!(ingress.heartbeat.failure_at(started_at, started_at + Duration::from_secs(6), true), Some(PCM_STALL_ERROR));
    }

    #[test] fn failed_and_closed_pcm_callbacks_do_not_refresh_the_heartbeat() {
        let started_at = Instant::now();
        let (mut ingress, receiver, _recycle) = test_ingress(1, 96_000);
        ingress.heartbeat = Arc::new(PcmHeartbeat::new_at(started_at));
        for _ in 0..RAW_CHUNKS { ingress.submit_at(&[0.0_f32], started_at); }
        assert_eq!(receiver.len(), RAW_CHUNKS);
        ingress.submit_at(&[0.0_f32], started_at + Duration::from_secs(1));
        assert_eq!(ingress.fault.load(Ordering::SeqCst), 1);
        assert_eq!(ingress.heartbeat.last_accepted_at(), Some(started_at));
        ingress.submit_at(&[0.0_f32], started_at + Duration::from_secs(2));
        assert_eq!(ingress.heartbeat.last_accepted_at(), Some(started_at));
    }

    #[test] fn pcm_heartbeat_never_moves_backward_and_user_cutoff_disables_deadlines() {
        let started_at = Instant::now();
        let heartbeat = PcmHeartbeat::new_at(started_at);
        assert_eq!(heartbeat.failure_at(started_at, started_at + Duration::from_secs(60), false), None);
        let latest_at = started_at + Duration::from_secs(2);
        heartbeat.mark_at(latest_at);
        heartbeat.mark_at(started_at + Duration::from_secs(1));
        assert_eq!(heartbeat.last_accepted_at(), Some(latest_at));
        assert_eq!(heartbeat.failure_at(started_at, latest_at + PCM_STALL_TIMEOUT, true), Some(PCM_STALL_ERROR));
        // request_stop/cancel both close admission before sending control.
        assert_eq!(heartbeat.failure_at(started_at, started_at + Duration::from_secs(60), false), None);
    }

    #[test] fn capture_errors_have_stable_distinct_classifications() {
        for (reason, expected) in [
            (PCM_STALL_ERROR, CaptureFailureReason::PcmStalled),
            (FIRST_SAMPLE_TIMEOUT_ERROR, CaptureFailureReason::FirstSampleTimeout),
            (source_fault_error(1), CaptureFailureReason::QueueOverflow),
            (source_fault_error(2), CaptureFailureReason::DeviceError),
            ("microphone input disconnected", CaptureFailureReason::DeviceError),
            (OUTPUT_QUEUE_FULL_ERROR, CaptureFailureReason::QueueOverflow),
            (source_fault_error(3), CaptureFailureReason::MalformedFrame),
        ] { assert_eq!(classify_capture_error(Some(reason)), Some(expected)); }
        assert_eq!(classify_capture_error(None), None);
        for reason in [OUTPUT_CONSUMER_DISCONNECTED_ERROR, "ASR bounded audio queue full or consumer disconnected", "unknown future native failure"] {
            assert_eq!(classify_capture_error(Some(reason)), None);
        }
    }

    fn test_ingress(channels: usize, budget: usize) -> (CallbackIngress, Receiver<RawChunk>, Sender<Vec<f32>>) {
        CallbackIngress::new(channels, budget, Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)))
    }
    fn collect_ingress(ingress: &CallbackIngress, receiver: &Receiver<RawChunk>, recycle: &Sender<Vec<f32>>) -> Vec<f32> {
        let mut output = Vec::new();
        while let Ok(chunk) = receiver.try_recv() {
            ingress.queued.fetch_sub(chunk.samples.len(), Ordering::SeqCst);
            output.extend_from_slice(&chunk.samples);
            recycle_chunk(chunk, recycle);
        }
        output
    }
    fn assert_pooled_matches<T: cpal::Sample>(data: &[T], channels: usize) where f32: cpal::FromSample<T> {
        let (ingress, receiver, recycle) = test_ingress(channels, 96_000);
        ingress.submit_at(data, Instant::now());
        assert_eq!(collect_ingress(&ingress, &receiver, &recycle), downmix(data, channels));
        assert_eq!(ingress.queued.load(Ordering::SeqCst), 0);
        assert_eq!(ingress.available.len(), RAW_CHUNKS);
        assert_eq!(ingress.fault.load(Ordering::SeqCst), 0);
    }
    #[test] fn pooled_callback_preserves_supported_format_and_channel_conversion() {
        assert_pooled_matches(&[i8::MIN, 0, i8::MAX], 1);
        assert_pooled_matches(&[i16::MIN, i16::MAX, 0, 42], 2);
        assert_pooled_matches(&[cpal::I24::new(-8_388_608).unwrap(), cpal::I24::new(0).unwrap(), cpal::I24::new(8_388_607).unwrap()], 1);
        assert_pooled_matches(&[i32::MIN, 0, i32::MAX], 1);
        assert_pooled_matches(&[i64::MIN, 0, i64::MAX], 1);
        assert_pooled_matches(&[0_u8, 128, u8::MAX], 1);
        assert_pooled_matches(&[0_u16, 32_768, u16::MAX], 1);
        assert_pooled_matches(&[0_u32, 2_147_483_648, u32::MAX], 1);
        assert_pooled_matches(&[0_u64, 9_223_372_036_854_775_808, u64::MAX], 1);
        assert_pooled_matches(&[f32::NAN, f32::INFINITY, -2.0, 2.0, 0.25, -0.5], 2);
        assert_pooled_matches(&[f64::NAN, f64::NEG_INFINITY, -1.0, 1.0], 2);
        assert_pooled_matches(&[0.3_f32, -0.1, 0.4, -0.3, 0.1, -0.4], 3);
    }
    #[test] fn large_hardware_callback_splits_in_order_without_changing_timestamp() {
        let (ingress, receiver, recycle) = test_ingress(2, 1_024);
        let data: Vec<f32> = (0..101).flat_map(|i| [i as f32 / 128.0, -(i as f32) / 256.0]).collect();
        let accepted_at = Instant::now() - Duration::from_millis(123);
        ingress.submit_at(&data, accepted_at);
        assert_eq!(receiver.len(), 7);
        let mut output = Vec::new();
        while let Ok(chunk) = receiver.try_recv() {
            assert_eq!(chunk.accepted_at, accepted_at);
            assert!(chunk.samples.len() <= 16);
            ingress.queued.fetch_sub(chunk.samples.len(), Ordering::SeqCst);
            output.extend_from_slice(&chunk.samples);
            recycle_chunk(chunk, &recycle);
        }
        assert_eq!(output, downmix(&data, 2));
        assert_eq!(ingress.queued.load(Ordering::SeqCst), 0);
        assert_eq!(ingress.available.len(), RAW_CHUNKS);
    }
    #[test] fn full_pool_is_a_fault_even_when_raw_sample_budget_has_room() {
        let (ingress, receiver, recycle) = test_ingress(1, 96_000);
        for _ in 0..RAW_CHUNKS { ingress.submit_at(&[0.25_f32], Instant::now()); }
        assert_eq!(ingress.queued.load(Ordering::SeqCst), RAW_CHUNKS);
        ingress.submit_at(&[0.5_f32], Instant::now());
        assert_eq!(ingress.fault.load(Ordering::SeqCst), 1);
        assert!(!ingress.accepting.load(Ordering::SeqCst));
        assert_eq!(ingress.queued.load(Ordering::SeqCst), RAW_CHUNKS);
        assert_eq!(collect_ingress(&ingress, &receiver, &recycle), vec![0.25; RAW_CHUNKS]);
        assert_eq!(ingress.available.len(), RAW_CHUNKS);
        assert_eq!(ingress.queued.load(Ordering::SeqCst), 0);
    }
    #[test] fn sample_budget_is_enforced_before_splitting_and_pool_rounding() {
        let (ingress, receiver, recycle) = test_ingress(1, 101);
        ingress.submit_at(&[0.25_f32; 101], Instant::now());
        assert_eq!(ingress.queued.load(Ordering::SeqCst), 101);
        assert_eq!(receiver.len(), 51);
        ingress.submit_at(&[0.5_f32], Instant::now());
        assert_eq!(ingress.fault.load(Ordering::SeqCst), 1);
        assert_eq!(collect_ingress(&ingress, &receiver, &recycle), vec![0.25; 101]);
        assert_eq!(ingress.available.len(), RAW_CHUNKS);
    }
    #[test] fn native_rate_boundaries_keep_exact_sample_budget_and_64_slot_limit() {
        for sample_rate in [8_000_usize, 44_100, 384_000] {
            let budget = sample_rate * 2;
            let (ingress, receiver, recycle) = test_ingress(1, budget);
            assert_eq!(ingress.available.len(), RAW_CHUNKS);
            // ceil() adds at most 63 f32s of startup capacity slack. Accepted
            // samples still use the exact two-second budget, not that slack.
            let buffers: Vec<_> = ingress.available.try_iter().collect();
            let capacity: usize = buffers.iter().map(Vec::capacity).sum();
            assert!(capacity >= budget && capacity < budget + RAW_CHUNKS);
            for buffer in buffers { recycle.try_send(buffer).unwrap(); }
            let data = vec![0.0_f32; budget];
            ingress.submit_at(&data, Instant::now());
            assert_eq!(receiver.len(), RAW_CHUNKS);
            assert_eq!(ingress.queued.load(Ordering::SeqCst), budget);
            assert!(ingress.accepting.load(Ordering::SeqCst));
            assert_eq!(ingress.fault.load(Ordering::SeqCst), 0);
            ingress.submit_at(&[0.0_f32], Instant::now());
            assert_eq!(ingress.fault.load(Ordering::SeqCst), 1);
            assert_eq!(ingress.queued.load(Ordering::SeqCst), budget);
            assert_eq!(collect_ingress(&ingress, &receiver, &recycle), data);
            assert_eq!(ingress.queued.load(Ordering::SeqCst), 0);
            assert_eq!(ingress.available.len(), RAW_CHUNKS);
        }
    }
    #[test] fn sixty_seconds_of_silent_callbacks_do_not_stop_or_drop_audio_or_report_a_stall() {
        let (mut ingress, receiver, recycle) = test_ingress(1, 96_000);
        let (events, received_events) = bounded(4);
        let started_at = Instant::now();
        ingress.heartbeat = Arc::new(PcmHeartbeat::new_at(started_at));
        let mut captured = 0;
        for callback in 0..6_000 {
            let accepted_at = started_at + Duration::from_millis(callback * 10);
            ingress.submit_at(&[0.0_f32; 480], accepted_at);
            let chunk = receiver.try_recv().unwrap();
            assert_eq!(chunk.samples, vec![0.0; 480]);
            assert_eq!(chunk.accepted_at, accepted_at);
            account_received_chunk(&chunk, &ingress.queued, &mut captured, &events);
            recycle_chunk(chunk, &recycle);
            assert_eq!(ingress.heartbeat.last_accepted_at(), Some(accepted_at));
            assert_eq!(ingress.heartbeat.failure_at(started_at, accepted_at + Duration::from_millis(10), true), None);
        }
        assert_eq!(captured, 60 * 48_000);
        assert!(ingress.accepting.load(Ordering::SeqCst));
        assert_eq!(ingress.fault.load(Ordering::SeqCst), 0);
        assert_eq!(ingress.queued.load(Ordering::SeqCst), 0);
        assert_eq!(ingress.available.len(), RAW_CHUNKS);
        assert!(matches!(received_events.try_recv(), Ok(AudioEvent::FirstSamples { at }) if at == started_at));
        assert!(received_events.try_recv().is_err());
    }
    #[test] fn malformed_source_does_not_reserve_samples_or_borrow_a_slot() {
        for channels in [0, 2] {
            let (ingress, receiver, _recycle) = test_ingress(channels, 96_000);
            ingress.submit_at(&[0.25_f32; 3], Instant::now());
            assert_eq!(ingress.fault.load(Ordering::SeqCst), 3);
            assert!(!ingress.accepting.load(Ordering::SeqCst));
            assert_eq!(ingress.queued.load(Ordering::SeqCst), 0);
            assert_eq!(ingress.available.len(), RAW_CHUNKS);
            assert!(receiver.is_empty());
            assert_eq!(ingress.heartbeat.last_accepted_at(), None);
        }
    }
    #[test] fn disconnected_raw_consumer_releases_reserved_samples_and_buffer() {
        let (ingress, receiver, _recycle) = test_ingress(1, 96_000);
        drop(receiver);
        ingress.submit_at(&[0.5_f32; 480], Instant::now());
        assert_eq!(ingress.fault.load(Ordering::SeqCst), 1);
        assert!(!ingress.accepting.load(Ordering::SeqCst));
        assert_eq!(ingress.queued.load(Ordering::SeqCst), 0);
        assert_eq!(ingress.available.len(), RAW_CHUNKS);
        assert_eq!(ingress.heartbeat.last_accepted_at(), None);
    }
    #[test] fn partial_large_callback_failure_keeps_only_accepted_samples_accounted() {
        let (mut ingress, _receiver, recycle) = test_ingress(1, 96_000);
        // An independently narrowed queue exercises rollback after an accepted
        // split, which can also occur if its consumer disconnects mid-callback.
        let (narrow_sender, narrow_receiver) = bounded(1);
        ingress.sender = narrow_sender;
        ingress.submit_at(&[0.5_f32; 2_000], Instant::now());
        assert_eq!(ingress.fault.load(Ordering::SeqCst), 1);
        assert_eq!(ingress.queued.load(Ordering::SeqCst), 1_500);
        assert_eq!(ingress.available.len(), RAW_CHUNKS - 1);
        assert_eq!(collect_ingress(&ingress, &narrow_receiver, &recycle), vec![0.5; 1_500]);
        assert_eq!(ingress.queued.load(Ordering::SeqCst), 0);
        assert_eq!(ingress.available.len(), RAW_CHUNKS);
        assert_eq!(ingress.heartbeat.last_accepted_at(), None);
    }
    #[test] fn pooled_stop_cutoff_drains_accepted_samples_and_preserves_resampler_tail() {
        let (ingress, receiver, recycle) = test_ingress(1, 96_000);
        let data: Vec<f32> = (0..50_333).map(|i| (i as f32 / 31.0).sin() * 0.2).collect();
        let accepted_at = Instant::now() - Duration::from_millis(100);
        ingress.submit_at(&data, accepted_at);
        ingress.accepting.store(false, Ordering::SeqCst);
        ingress.submit_at(&[1.0_f32; 4_800], Instant::now());
        let mut pooled = FrameResampler::new(48_000, 16_000, Duration::from_millis(100));
        let mut baseline = FrameResampler::new(48_000, 16_000, Duration::from_millis(100));
        let (events, received_events) = bounded(4);
        let mut captured = 0;
        let mut pooled_output = Vec::new();
        while let Ok(chunk) = receiver.try_recv() {
            account_received_chunk(&chunk, &ingress.queued, &mut captured, &events);
            pooled.push(&chunk.samples, |frame| pooled_output.push(frame.to_vec()));
            recycle_chunk(chunk, &recycle);
        }
        pooled.finish(|frame| pooled_output.push(frame.to_vec()));
        let mut baseline_output = Vec::new();
        baseline.push(&data, |frame| baseline_output.push(frame.to_vec()));
        baseline.finish(|frame| baseline_output.push(frame.to_vec()));
        assert_eq!(captured, data.len() as u64);
        assert_eq!(pooled_output, baseline_output);
        assert!(pooled.error().is_none());
        assert_eq!(ingress.queued.load(Ordering::SeqCst), 0);
        assert_eq!(ingress.available.len(), RAW_CHUNKS);
        assert!(matches!(received_events.try_recv(), Ok(AudioEvent::FirstSamples { at }) if at == accepted_at));
        assert!(received_events.try_recv().is_err());
    }
    #[test] fn empty_and_closed_callbacks_do_not_change_admission_or_accounting() {
        let (ingress, receiver, _recycle) = test_ingress(1, 96_000);
        ingress.submit_at::<f32>(&[], Instant::now());
        ingress.accepting.store(false, Ordering::SeqCst);
        ingress.submit_at(&[0.5_f32; 480], Instant::now());
        assert!(receiver.is_empty());
        assert_eq!(ingress.fault.load(Ordering::SeqCst), 0);
        assert_eq!(ingress.queued.load(Ordering::SeqCst), 0);
        assert_eq!(ingress.available.len(), RAW_CHUNKS);
    }

    // Allocation instrumentation exists only in the test binary. Delegation to
    // System requires the standard allocator's unsafe interface; capture code
    // above remains entirely safe Rust. Other test threads are never counted.
    mod allocations {
        use std::alloc::{GlobalAlloc, Layout, System};
        use std::cell::Cell;
        thread_local! {
            static ENABLED: Cell<bool> = const { Cell::new(false) };
            static COUNT: Cell<usize> = const { Cell::new(0) };
        }
        struct CountingAllocator;
        fn record() {
            let _ = ENABLED.try_with(|enabled| {
                if enabled.get() { let _ = COUNT.try_with(|count| count.set(count.get() + 1)); }
            });
        }
        // SAFETY: Every operation forwards the unchanged pointer/Layout to
        // System. Tracking does not inspect, retain or modify allocated memory.
        unsafe impl GlobalAlloc for CountingAllocator {
            unsafe fn alloc(&self, layout: Layout) -> *mut u8 { record(); unsafe { System.alloc(layout) } }
            unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 { record(); unsafe { System.alloc_zeroed(layout) } }
            unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 { record(); unsafe { System.realloc(ptr, layout, size) } }
            unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) { unsafe { System.dealloc(ptr, layout) } }
        }
        #[global_allocator]
        static ALLOCATOR: CountingAllocator = CountingAllocator;
        struct Scope;
        impl Drop for Scope { fn drop(&mut self) { ENABLED.with(|enabled| enabled.set(false)); } }
        pub(super) fn measure<T>(f: impl FnOnce() -> T) -> (T, usize) {
            COUNT.with(|count| count.set(0));
            ENABLED.with(|enabled| enabled.set(true));
            let scope = Scope;
            let result = f();
            drop(scope);
            (result, COUNT.with(Cell::get))
        }
    }
    #[test] fn pooled_callback_owns_no_allocations_including_large_first_callback() {
        let (ingress, receiver, recycle) = test_ingress(2, 96_000);
        let data: Vec<i16> = (0..8_192).map(|i| (i % 32_768) as i16).collect();
        let at = Instant::now();
        for _ in 0..3 {
            let (_, allocation_count) = allocations::measure(|| ingress.submit_at(&data, at));
            assert_eq!(allocation_count, 0);
            assert_eq!(collect_ingress(&ingress, &receiver, &recycle), downmix(&data, 2));
        }
    }
    #[test] fn pooled_callback_fault_and_cutoff_paths_own_no_allocations() {
        let (ingress, receiver, _recycle) = test_ingress(1, 96_000);
        let at = Instant::now();
        for _ in 0..RAW_CHUNKS { ingress.submit_at(&[0.5_f32], at); }
        let (_, count) = allocations::measure(|| ingress.submit_at(&[0.5_f32; 480], at));
        assert_eq!(count, 0);
        let (_, count) = allocations::measure(|| ingress.submit_at(&[0.5_f32; 480], at));
        assert_eq!(count, 0);
        assert_eq!(receiver.len(), RAW_CHUNKS);
        let (ingress, receiver, _recycle) = test_ingress(1, 96_000);
        drop(receiver);
        let (_, count) = allocations::measure(|| ingress.submit_at(&[0.5_f32; 480], at));
        assert_eq!(count, 0);
    }

    /// Reproduction of the original allocation-owning callback, retained only
    /// for a like-for-like synthetic benchmark of admission + downmix + enqueue.
    #[allow(deprecated)]
    fn baseline_callback<T: cpal::Sample>(data: &[T], channels: usize, sender: &Sender<RawChunk>,
        queued: &AtomicUsize, accepting: &AtomicBool, fault: &AtomicUsize, budget: usize, at: Instant)
        where f32: cpal::FromSample<T> {
        if !accepting.load(Ordering::SeqCst) || data.is_empty() { return; }
        if data.len() % channels != 0 { fault.store(3, Ordering::SeqCst); accepting.store(false, Ordering::SeqCst); return; }
        let count = data.len() / channels;
        if queued.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_add(count).filter(|v| *v <= budget)).is_err() {
            fault.store(1, Ordering::SeqCst); accepting.store(false, Ordering::SeqCst); return;
        }
        let samples = data.chunks_exact(channels).map(|chunk| {
            chunk.iter().map(|sample| sample.to_sample::<f32>()).map(|v| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 }).sum::<f32>() / channels as f32
        }).collect();
        if sender.try_send(RawChunk { samples, accepted_at: at }).is_err() {
            queued.fetch_sub(count, Ordering::SeqCst); fault.store(1, Ordering::SeqCst); accepting.store(false, Ordering::SeqCst);
        }
    }
    fn benchmark_case<T: cpal::Sample>(label: &str, data: &[T], channels: usize) where f32: cpal::FromSample<T> {
        const WARMUP: usize = 1_000;
        const ITERATIONS: usize = 20_000;
        let (baseline_sender, baseline_receiver) = bounded(RAW_CHUNKS);
        let baseline_queued = AtomicUsize::new(0);
        let baseline_accepting = AtomicBool::new(true);
        let baseline_fault = AtomicUsize::new(0);
        let (ingress, receiver, recycle) = test_ingress(channels, 96_000);
        let mut old_ns = Vec::with_capacity(ITERATIONS);
        let mut new_ns = Vec::with_capacity(ITERATIONS);
        let mut old_allocations = 0;
        let mut new_allocations = 0;
        for iteration in 0..WARMUP + ITERATIONS {
            // Alternate the measurement order to reduce warm CPU/order bias.
            for pooled in if iteration % 2 == 0 { [false, true] } else { [true, false] } {
                let at = Instant::now();
                let (elapsed, allocations) = allocations::measure(|| {
                    let begin = Instant::now();
                    if pooled { ingress.submit_at(std::hint::black_box(data), at); }
                    else {
                        baseline_callback(std::hint::black_box(data), channels, &baseline_sender, &baseline_queued,
                            &baseline_accepting, &baseline_fault, 96_000, at);
                    }
                    begin.elapsed().as_nanos()
                });
                // Worker processing/deallocation/recycling is deliberately
                // outside the callback allocation and timing measurement.
                if pooled {
                    while let Ok(chunk) = receiver.try_recv() {
                        ingress.queued.fetch_sub(chunk.samples.len(), Ordering::SeqCst);
                        std::hint::black_box(&chunk.samples);
                        recycle_chunk(chunk, &recycle);
                    }
                } else {
                    let chunk = baseline_receiver.try_recv().unwrap();
                    baseline_queued.fetch_sub(chunk.samples.len(), Ordering::SeqCst);
                    std::hint::black_box(&chunk.samples);
                }
                if iteration >= WARMUP {
                    if pooled { new_ns.push(elapsed); new_allocations += allocations; }
                    else { old_ns.push(elapsed); old_allocations += allocations; }
                }
            }
        }
        assert_eq!(old_allocations, ITERATIONS);
        assert_eq!(new_allocations, 0);
        assert_eq!(baseline_fault.load(Ordering::SeqCst), 0);
        assert_eq!(ingress.fault.load(Ordering::SeqCst), 0);
        old_ns.sort_unstable(); new_ns.sort_unstable();
        println!("AUDIO_INGRESS_BENCH {{\"case\":\"{label}\",\"iterations\":{ITERATIONS},\"warmup\":{WARMUP},\"monoFrames\":{},\"channels\":{channels},\"sampleRate\":48000,\"baselineAllocations\":{old_allocations},\"pooledAllocations\":{new_allocations},\"baselineP50Ns\":{},\"baselineP95Ns\":{},\"pooledP50Ns\":{},\"pooledP95Ns\":{},\"poolBytes\":384000}}",
            data.len() / channels, old_ns[ITERATIONS / 2], old_ns[ITERATIONS * 95 / 100], new_ns[ITERATIONS / 2], new_ns[ITERATIONS * 95 / 100]);
    }
    #[test]
    #[ignore = "manual synthetic release benchmark; no microphone or provider is used"]
    fn callback_allocation_and_latency_benchmark() {
        println!("AUDIO_INGRESS_BENCH_SCOPE callback-owned allocations; synthetic no-contention ingress; excludes CPAL, worker/resampler, OS scheduling, model and network latency");
        benchmark_case("f32_mono_10ms", &vec![0.2_f32; 480], 1);
        benchmark_case("i16_stereo_10ms", &vec![6_553_i16; 960], 2);
        benchmark_case("i16_stereo_large_4096frames", &vec![6_553_i16; 8_192], 2);
    }
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
        assert_eq!(seq, 1); assert_eq!(error.as_deref(), Some(OUTPUT_QUEUE_FULL_ERROR));
        assert_eq!(classify_capture_error(error.as_deref()), Some(CaptureFailureReason::QueueOverflow));
    }
    #[test] fn disconnected_output_consumer_is_not_misclassified_as_queue_overflow() {
        let (tx, rx) = bounded(1); let mut seq = 0; let mut error = None;
        drop(rx);
        emit_frame(&[0.1], 0, &tx, &mut seq, &mut error);
        assert_eq!(seq, 0); assert_eq!(error.as_deref(), Some(OUTPUT_CONSUMER_DISCONNECTED_ERROR));
        assert_eq!(classify_capture_error(error.as_deref()), None);
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
