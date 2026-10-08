//! Opt-in, bounded acoustic capture experiment using production CaptureHandle.
//! Plays only explicitly supplied fixed WAV files. Microphone PCM remains in
//! memory long enough to compute aggregates and is never saved or uploaded.
//! This measures a speaker-to-microphone acoustic path, not deterministic WAV
//! injection, recognition accuracy, or the desktop application's full session.
#[cfg(windows)]
mod lab {
    use crossbeam_channel::{Receiver, RecvTimeoutError};
    use serde::Serialize;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use speakmode_lib::platform::audio::{AudioEvent, AudioFrame, CaptureHandle, CaptureSummary, capture_resources_released};
    use std::{fs::{self, File, OpenOptions}, io::Write, path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicBool, AtomicU8, Ordering}, mpsc}, thread, time::{Duration, Instant}};
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "winmm")]
    unsafe extern "system" { fn PlaySoundW(sound: *const u16, module: *mut std::ffi::c_void, flags: u32) -> i32; }
    const ASYNC_FILENAME: u32 = 0x0001 | 0x0002 | 0x0002_0000; // ASYNC | NODEFAULT | FILENAME
    const HARD_LIMIT: Duration = Duration::from_secs(180);
    const FIRST_SAMPLE_LIMIT: Duration = Duration::from_secs(10);
    const DRAIN_LIMIT: Duration = Duration::from_secs(5);

    struct Playback;
    impl Playback {
        fn start(path: &Path) -> Result<Self, String> {
            // canonicalize() emits a verbatim prefix; legacy WinMM filename
            // handling is more portable with the equivalent ordinary path.
            let ordinary = path.to_string_lossy();
            let ordinary = ordinary.strip_prefix("\\\\?\\").unwrap_or(&ordinary);
            let wide: Vec<u16> = std::ffi::OsStr::new(ordinary).encode_wide().chain(Some(0)).collect();
            if unsafe { PlaySoundW(wide.as_ptr(), std::ptr::null_mut(), ASYNC_FILENAME) } == 0 { return Err("fixed_wav_playback_failed".into()); }
            Ok(Self)
        }
    }
    impl Drop for Playback { fn drop(&mut self) { unsafe { PlaySoundW(std::ptr::null(), std::ptr::null_mut(), 0); } } }

    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Fixture { path: PathBuf, duration_ms: u64, sample_rate: u32, channels: u16, sha256: String }
    fn fixture(path: PathBuf) -> Result<Fixture, String> {
        if !path.is_absolute() || !path.is_file() { return Err("fixture_must_be_an_existing_absolute_wav".into()); }
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        let reader = hound::WavReader::open(&path).map_err(|e| format!("invalid_fixed_wav: {e}"))?;
        let spec = reader.spec();
        if spec.sample_rate == 0 || spec.channels == 0 { return Err("invalid_fixed_wav_format".into()); }
        let duration_ms = u64::from(reader.duration()).saturating_mul(1_000).div_ceil(u64::from(spec.sample_rate));
        if !(1_000..=50_000).contains(&duration_ms) { return Err("fixed_wav_duration_must_be_1_to_50_seconds".into()); }
        let sha256 = format!("{:x}", Sha256::digest(fs::read(&path).map_err(|e| e.to_string())?));
        Ok(Fixture { path, duration_ms, sample_rate: spec.sample_rate, channels: spec.channels, sha256 })
    }

    #[derive(Default, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Samples { samples: u64, sum_squares: f64, peak: f64, clipped_samples: u64 }
    impl Samples {
        fn accept(&mut self, values: &[i16]) {
            for &sample in values {
                let value = f64::from(sample) / 32_768.0;
                self.samples += 1; self.sum_squares += value * value; self.peak = self.peak.max(value.abs());
                if sample == i16::MIN || sample == i16::MAX { self.clipped_samples += 1; }
            }
        }
        fn rms(&self) -> f64 { if self.samples == 0 { 0.0 } else { (self.sum_squares / self.samples as f64).sqrt() } }
        fn report(&self) -> Value { json!({"samples":self.samples,"rms":self.rms(),"peak":self.peak,"clippedSamples":self.clipped_samples}) }
    }
    #[derive(Default)]
    struct Metrics {
        frames: u64, pcm_samples: u64, padding_samples: u64, sequence_errors: u64, malformed_frames: u64,
        first_samples_at_ms: Option<f64>, first_frame_at_ms: Option<f64>, last_frame_at_ms: Option<f64>,
        first_sequence: Option<u64>, last_sequence: Option<u64>, events_errors: Vec<String>,
        level_events: u64, level_max: f32, drained_to_disconnect: bool,
        all: Samples, baseline: Samples, playback: Samples, post: Samples, drain: Samples,
    }
    impl Metrics {
        fn report(&self) -> Value { json!({"frames":self.frames,"pcmSamples":self.pcm_samples,"paddingSamples":self.padding_samples,
            "sequenceErrors":self.sequence_errors,"malformedFrames":self.malformed_frames,"firstSequence":self.first_sequence,"lastSequence":self.last_sequence,
            "firstSamplesAtMs":self.first_samples_at_ms,"firstFrameAtMs":self.first_frame_at_ms,"lastFrameAtMs":self.last_frame_at_ms,
            "levelEvents":self.level_events,"levelMax":self.level_max,"captureErrors":self.events_errors,"drainedToDisconnect":self.drained_to_disconnect,
            "all":self.all.report(),"baseline":self.baseline.report(),"playback":self.playback.report(),"postPlayback":self.post.report(),"stopDrain":self.drain.report(),
            "phaseAttribution":"consumer dequeue phase; frame buffering and acoustic latency can cross phase boundaries"}) }
    }
    fn consume(frames: Receiver<AudioFrame>, events: Receiver<AudioEvent>, started: Instant, phase: Arc<AtomicU8>, stop: Arc<AtomicBool>, first: mpsc::SyncSender<()>) -> Metrics {
        let mut metrics = Metrics::default();
        loop {
            for event in events.try_iter() {
                match event {
                    AudioEvent::FirstSamples { at } => { if metrics.first_samples_at_ms.is_none() { metrics.first_samples_at_ms = Some(at.saturating_duration_since(started).as_secs_f64() * 1_000.0); let _ = first.try_send(()); } },
                    AudioEvent::Level(value) => { metrics.level_events += 1; metrics.level_max = metrics.level_max.max(value); },
                    AudioEvent::Error(error) => metrics.events_errors.push(error),
                }
            }
            if stop.load(Ordering::SeqCst) { break; }
            let frame = match frames.recv_timeout(Duration::from_millis(5)) {
                Ok(frame) => frame,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => { metrics.drained_to_disconnect = true; break; },
            };
            let at_ms = started.elapsed().as_secs_f64() * 1_000.0;
            if metrics.first_frame_at_ms.is_none() { metrics.first_frame_at_ms = Some(at_ms); metrics.first_sequence = Some(frame.sequence); }
            if frame.sequence != metrics.frames { metrics.sequence_errors += 1; }
            metrics.frames += 1; metrics.last_sequence = Some(frame.sequence); metrics.last_frame_at_ms = Some(at_ms);
            metrics.pcm_samples += frame.pcm16.len() as u64; metrics.padding_samples += frame.padding_samples as u64;
            if frame.pcm16.len() != 1_600 || frame.padding_samples > frame.pcm16.len() { metrics.malformed_frames += 1; }
            let meaningful = frame.pcm16.len().saturating_sub(frame.padding_samples);
            let samples = &frame.pcm16[..meaningful];
            metrics.all.accept(samples);
            match phase.load(Ordering::SeqCst) { 1 => metrics.baseline.accept(samples), 2 => metrics.playback.accept(samples), 3 => metrics.post.accept(samples), 4 => metrics.drain.accept(samples), _ => {} }
        }
        // Worker sends terminal error metadata before dropping its senders.
        for event in events.try_iter() { if let AudioEvent::Error(error) = event { metrics.events_errors.push(error); } }
        metrics
    }
    fn wait_for(duration: Duration, deadline: Instant, consumer: &thread::JoinHandle<Metrics>) -> Result<(), String> {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            if Instant::now() >= deadline { return Err("experiment_deadline".into()); }
            if consumer.is_finished() { return Err("capture_consumer_ended_early".into()); }
            thread::sleep(Duration::from_millis(10).min(until.saturating_duration_since(Instant::now())));
        }
        Ok(())
    }
    fn summary_report(summary: &CaptureSummary) -> Value { json!({"complete":summary.complete,"cancelled":summary.cancelled,
        "capturedSamples":summary.captured_samples,"inputSampleRate":summary.input_sample_rate,"durationMs":summary.duration_ms(),
        "emittedFrames":summary.emitted_frames,"firstSamplesReceived":summary.first_samples_received,"error":summary.error}) }

    fn round(index: usize, kind: &str, fixture: &Fixture, deadline: Instant) -> Value {
        let started = Instant::now();
        let start_resources_released = capture_resources_released();
        if !start_resources_released { return json!({"round":index,"fixture":kind,"passed":false,"failure":"previous_capture_resources_not_released"}); }
        // Reserve the known WAV duration plus startup, baseline/post, and drain;
        // do not admit a new native start close to the global hard deadline.
        if deadline.saturating_duration_since(started) < Duration::from_millis(fixture.duration_ms) + Duration::from_secs(22) {
            return json!({"round":index,"fixture":kind,"passed":false,"failure":"insufficient_experiment_time_budget"});
        }
        let mut capture = match CaptureHandle::start(None) {
            Ok(capture) => capture,
            Err(error) => return json!({"round":index,"fixture":kind,"passed":false,"failure":"capture_start_failed","error":error.to_string(),"resourcesReleased":capture_resources_released(),"elapsedMs":started.elapsed().as_secs_f64()*1_000.0}),
        };
        let start_returned_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let phase = Arc::new(AtomicU8::new(0)); let stop = Arc::new(AtomicBool::new(false));
        let (first_tx, first_rx) = mpsc::sync_channel(1);
        let (frames, events) = (capture.frames.clone(), capture.events.clone());
        let (consumer_phase, consumer_stop) = (Arc::clone(&phase), Arc::clone(&stop));
        let consumer = thread::spawn(move || consume(frames, events, started, consumer_phase, consumer_stop, first_tx));
        let mut playback: Option<Playback> = None;
        let mut playback_started_ms = None; let mut playback_finished_ms = None;
        let run = (|| -> Result<(), String> {
            first_rx.recv_timeout(FIRST_SAMPLE_LIMIT.min(deadline.saturating_duration_since(Instant::now()))).map_err(|_| "first_sample_not_acknowledged".to_owned())?;
            phase.store(1, Ordering::SeqCst);
            wait_for(Duration::from_millis(500), deadline, &consumer)?;
            phase.store(2, Ordering::SeqCst);
            playback = Some(Playback::start(&fixture.path)?);
            playback_started_ms = Some(started.elapsed().as_secs_f64() * 1_000.0);
            wait_for(Duration::from_millis(fixture.duration_ms), deadline, &consumer)?;
            playback_finished_ms = Some(started.elapsed().as_secs_f64() * 1_000.0);
            phase.store(3, Ordering::SeqCst);
            // Keep the asynchronous player alive through the post interval, to
            // avoid truncating sound if render-device latency exceeds WAV time.
            wait_for(Duration::from_millis(500), deadline, &consumer)?;
            Ok(())
        })();
        drop(playback.take());
        phase.store(4, Ordering::SeqCst);
        let stop_requested_ms = started.elapsed().as_secs_f64() * 1_000.0;
        capture.request_stop();
        let cleanup_started = Instant::now();
        let summary = capture.wait_drained(DRAIN_LIMIT.min(deadline.saturating_duration_since(Instant::now())));
        let cleanup_ms = cleanup_started.elapsed().as_secs_f64() * 1_000.0;
        if summary.is_err() { capture.cancel(); }
        // After successful drain, sender disconnection lets the consumer finish
        // after every queued frame. Bound this join even on cleanup failure.
        let consumer_deadline = Instant::now() + Duration::from_secs(1);
        while !consumer.is_finished() && Instant::now() < consumer_deadline { thread::sleep(Duration::from_millis(5)); }
        stop.store(true, Ordering::SeqCst);
        let metrics = consumer.join();
        let resources_released = capture_resources_released();
        let mut result = json!({"round":index,"fixture":kind,"fixtureSha256":fixture.sha256,"fixtureDurationMs":fixture.duration_ms,
            "startReturnedMs":start_returned_ms,"playbackStartedMs":playback_started_ms,"playbackExpectedFinishedMs":playback_finished_ms,
            "stopRequestedMs":stop_requested_ms,"cleanupMs":cleanup_ms,"resourcesReleased":resources_released,
            "elapsedMs":started.elapsed().as_secs_f64()*1_000.0,"passed":false});
        if let Err(error) = &run { result["failure"] = json!(error); }
        match (&summary, &metrics) {
            (Ok(summary), Ok(metrics)) => {
                result["capture"] = summary_report(summary); result["metrics"] = metrics.report();
                result["frameCountMatchesSummary"] = json!(metrics.frames == summary.emitted_frames);
                let signal_increased = metrics.playback.rms() > 0.0025 && metrics.playback.rms() > metrics.baseline.rms() * 1.5;
                result["playbackSignalIncreaseObserved"] = json!(signal_increased);
                result["signalInterpretation"] = json!("Aggregate acoustic signal heuristic only; does not establish fixture-word recognition or absence of room/background sound");
                let passed = run.is_ok() && summary.complete && !summary.cancelled && summary.first_samples_received && summary.error.is_none()
                    && metrics.frames > 0 && metrics.frames == summary.emitted_frames && metrics.sequence_errors == 0
                    && metrics.malformed_frames == 0 && metrics.events_errors.is_empty() && metrics.drained_to_disconnect && resources_released;
                result["passed"] = json!(passed);
                if !passed && result.get("failure").is_none() { result["failure"] = json!("capture_invariant_failed"); }
            },
            (Err(error), _) => { result["failure"] = json!("capture_drain_failed"); result["error"] = json!(error.to_string()); },
            (_, Err(_)) => { result["failure"] = json!("capture_consumer_panicked"); },
        }
        drop(capture);
        result
    }
    fn write_new(path: &Path, value: &Value) -> Result<(), String> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| e.to_string())?;
        serde_json::to_writer_pretty(&mut file, value).map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| e.to_string())?; file.sync_all().map_err(|e| e.to_string())
    }
    pub fn run() -> Result<bool, String> {
        let mut args = std::env::args().skip(1); let mut out = None; let mut short = None; let mut long = None;
        while let Some(arg) = args.next() {
            let value = PathBuf::from(args.next().ok_or_else(|| "each_argument_requires_a_path".to_owned())?);
            let slot = match arg.as_str() { "--out" => &mut out, "--fixture-short" => &mut short, "--fixture-long" => &mut long, _ => return Err("usage: capture_lab --out ABS_NEW_DIR --fixture-short ABS_WAV --fixture-long ABS_WAV".into()) };
            if slot.replace(value).is_some() { return Err("duplicate_argument".into()); }
        }
        let out = out.ok_or("missing_out")?;
        if !out.is_absolute() || out.exists() || !out.parent().is_some_and(Path::is_dir) { return Err("out_must_be_a_new_absolute_directory_with_existing_parent".into()); }
        let short = fixture(short.ok_or("missing_fixture_short")?)?; let long = fixture(long.ok_or("missing_fixture_long")?)?;
        let expected_playback_ms = short.duration_ms * 3 + long.duration_ms * 2;
        if expected_playback_ms > 140_000 { return Err("five_round_fixture_total_exceeds_140_seconds".into()); }
        fs::create_dir(&out).map_err(|e| e.to_string())?;
        let mut journal: File = OpenOptions::new().write(true).create_new(true).open(out.join("capture-rounds.jsonl")).map_err(|e| e.to_string())?;
        let started = Instant::now();
        let deadline = started + HARD_LIMIT - Duration::from_secs(3);
        eprintln!("Capture lab: five bounded default-microphone rounds; fixed WAV speaker playback; microphone PCM is not saved or uploaded.");
        let mut rounds = Vec::new(); let mut failed = false;
        for (number, (kind, fixture)) in [("short", &short), ("long", &long), ("short", &short), ("long", &long), ("short", &short)].iter().enumerate() {
            if failed { break; }
            eprintln!("Capture lab round {} / 5: {}", number + 1, kind);
            let receipt = round(number + 1, kind, fixture, deadline);
            failed = receipt["passed"] != true;
            serde_json::to_writer(&mut journal, &receipt).map_err(|e| e.to_string())?;
            journal.write_all(b"\n").map_err(|e| e.to_string())?; journal.flush().map_err(|e| e.to_string())?; journal.sync_all().map_err(|e| e.to_string())?;
            rounds.push(receipt);
        }
        let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let completed = rounds.len();
        let passed = !failed && completed == 5 && elapsed_ms < HARD_LIMIT.as_secs_f64() * 1_000.0 && capture_resources_released();
        let microphone_activated = rounds.iter().any(|round| round.get("startReturnedMs").is_some());
        let report = json!({"scope":"REAL_WASAPI_DEFAULT_MICROPHONE_FIXED_WAV_ACOUSTIC_EXPERIMENT","version":env!("CARGO_PKG_VERSION"),
            "microphoneActivated":microphone_activated,"microphonePcmSaved":false,"cloudUsed":false,"keysRead":false,"systemDeviceSettingsChanged":false,
            "audioProcessing":"production CaptureHandle, callback buffer pool, mono downmix, Rubato resampling, 100 ms PCM16 frames, stop/drain",
            "recognitionAndRewrite":"NOT_TESTED","desktopUiAndHotkey":"NOT_TESTED","fixtures":{"short":short,"long":long},
            "roundsPlanned":5,"roundsCompleted":completed,"remainingRoundsStoppedAfterFailure":failed,"elapsedMs":elapsed_ms,
            "hardLimitSeconds":180,"resourcesReleasedAtEnd":capture_resources_released(),"passed":passed,"rounds":rounds});
        write_new(&out.join("capture-summary.json"), &report)?;
        println!("{}", serde_json::to_string(&json!({"passed":passed,"roundsCompleted":completed,"elapsedMs":elapsed_ms,"report":out.join("capture-summary.json")})).unwrap());
        Ok(passed)
    }
}
#[cfg(windows)]
fn main() { match lab::run() { Ok(true) => {}, Ok(false) => std::process::exit(1), Err(error) => { eprintln!("capture_lab: {error}"); std::process::exit(2); } } }
#[cfg(not(windows))]
fn main() { eprintln!("capture_lab requires Windows and explicit fixed WAV fixtures"); std::process::exit(2); }
