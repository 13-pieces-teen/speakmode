// Extracted from cjpais/Handy, MIT, commit
// 73ab851c2b6242283759a4c101b60f0ece132f08,
// src-tauri/src/audio_toolkit/audio/resampler.rs.
// SpeakMode compensates FFT delay and drains the last source samples before
// framing. Filter latency and zero padding never extend the source duration.
use rubato::{FftFixedIn, Resampler};
use std::time::Duration;

// Make this a constant you can tweak
const RESAMPLER_CHUNK_SIZE: usize = 1024;

pub struct FrameResampler {
    resampler: Option<FftFixedIn<f32>>,
    chunk_in: usize,
    in_buf: Vec<f32>,
    frame_samples: usize,
    pending: Vec<f32>,
    in_hz: usize,
    out_hz: usize,
    /// Original source samples, excluding any zeros fed during tail draining.
    in_count: usize,
    delay_remaining: usize,
    real_output_samples: usize,
    finished: bool,
    fault: Option<String>,
}

impl FrameResampler {
    pub fn new(in_hz: usize, out_hz: usize, frame_dur: Duration) -> Self {
        let frame_samples = ((out_hz as f64 * frame_dur.as_secs_f64()).round()) as usize;
        assert!(frame_samples > 0, "frame duration too short");

        // Use fixed chunk size instead of GCD-based
        let chunk_in = RESAMPLER_CHUNK_SIZE;

        let resampler = (in_hz != out_hz).then(|| {
            FftFixedIn::<f32>::new(in_hz, out_hz, chunk_in, 1, 1)
                .expect("Failed to create resampler")
        });

        let delay_remaining = resampler.as_ref().map_or(0, |r| r.output_delay());
        Self {
            resampler,
            chunk_in,
            in_buf: Vec::with_capacity(chunk_in),
            frame_samples,
            pending: Vec::with_capacity(frame_samples),
            in_hz,
            out_hz,
            in_count: 0,
            delay_remaining,
            real_output_samples: 0,
            finished: false,
            fault: None,
        }
    }

    pub fn push(&mut self, mut src: &[f32], mut emit: impl FnMut(&[f32])) {
        // finish is a terminal boundary; reset is required before reuse.
        if self.finished || self.fault.is_some() { return; }
        self.in_count += src.len();
        if self.resampler.is_none() {
            self.real_output_samples += src.len();
            self.emit_frames(src, &mut emit);
            return;
        }

        while !src.is_empty() {
            let space = self.chunk_in - self.in_buf.len();
            let take = space.min(src.len());
            self.in_buf.extend_from_slice(&src[..take]);
            src = &src[take..];

            if self.in_buf.len() == self.chunk_in {
                // let start = std::time::Instant::now();
                match self
                    .resampler
                    .as_mut()
                    .unwrap()
                    .process(&[&self.in_buf[..]], None)
                {
                  Ok(out) => {
                    // let duration = start.elapsed();
                    // log::debug!("Resampler took: {:?}", duration);
                    self.accept_resampled(&out[0], &mut emit);
                  },
                  Err(error) => self.fault = Some(error.to_string()),
                }
                self.in_buf.clear();
            }
        }
    }

    pub fn finish(&mut self, mut emit: impl FnMut(&[f32])) {
        if self.finished { return; }
        self.finished = true;
        if self.resampler.is_some() {
            // Process any remaining input samples (padded internally).
            if !self.in_buf.is_empty() {
                let result = self
                    .resampler
                    .as_mut()
                    .unwrap()
                    .process_partial(Some(&[&self.in_buf[..]]), None);
                match result {
                  Ok(out) => {
                    self.accept_resampled(&out[0], &mut emit);
                  },
                  Err(error) => self.fault = Some(error.to_string()),
                }
                // Drop the consumed input: a full in_buf would satisfy the
                // next push()'s chunk check immediately, re-processing this
                // padded tail into the following recording.
                self.in_buf.clear();
            }

            // Drain the delay line after removing its leading latency. Merely
            // clipping the old output would discard the last spoken samples.
            // An awkward sample-rate ratio may need many input chunks before
            // one FFT block is ready, so derive the bound from its delay.
            if self.in_count > 0 && self.fault.is_none() {
                let delay = self.resampler.as_ref().unwrap().output_delay();
                let expected = self.source_output_samples();
                // Up to two FFT blocks may be needed: one to complete buffered
                // input, and another to recover the delayed end of that block.
                let max_rounds = ((delay * 4 + 4) * self.in_hz)
                    .div_ceil(self.out_hz).div_ceil(self.chunk_in) + 2;
                let mut rounds = 0;
                while self.real_output_samples < expected && rounds < max_rounds {
                    rounds += 1;
                    let result = self
                        .resampler
                        .as_mut()
                        .unwrap()
                        .process_partial::<&[f32]>(None, None);
                    match result {
                        Ok(out) => {
                            self.accept_resampled(&out[0], &mut emit);
                        }
                        Err(error) => { self.fault = Some(error.to_string()); break; },
                    }
                }
                if self.fault.is_none() && self.real_output_samples != self.source_output_samples() {
                    self.fault = Some("resampler tail did not reach source duration".into());
                }
            }
        }

        // Emit any remaining pending frame (padded with zeros)
        if !self.pending.is_empty() {
            self.pending.resize(self.frame_samples, 0.0);
            emit(&self.pending);
            self.pending.clear();
        }
    }

    /// Clear all internal buffers so the next `push()` starts from a clean state.
    ///
    /// Call this between recordings to prevent stale audio from the previous
    /// session leaking into the start of the next one via the FFT overlap buffers.
    #[allow(dead_code)] // Retained reusable upstream API, exercised by source tests.
    pub fn reset(&mut self) {
        self.in_buf.clear();
        self.pending.clear();
        self.in_count = 0;
        self.delay_remaining = self.resampler.as_ref().map_or(0, |r| r.output_delay());
        self.real_output_samples = 0;
        self.finished = false;
        self.fault = None;
        if let Some(ref mut resampler) = self.resampler {
            resampler.reset();
        }
    }

    pub fn error(&self) -> Option<&str> { self.fault.as_deref() }
    /// Exact source duration at the output rate, excluding FFT latency and
    /// frame padding. Flooring loses less than one output sample, never a frame.
    pub fn source_output_samples(&self) -> usize {
        self.in_count * self.out_hz / self.in_hz
    }

    fn accept_resampled(&mut self, data: &[f32], emit: &mut impl FnMut(&[f32])) {
        let skip = self.delay_remaining.min(data.len());
        self.delay_remaining -= skip;
        let data = &data[skip..];
        let take = self.source_output_samples().saturating_sub(self.real_output_samples).min(data.len());
        self.real_output_samples += take;
        self.emit_frames(&data[..take], emit);
    }

    fn emit_frames(&mut self, mut data: &[f32], emit: &mut impl FnMut(&[f32])) {
        while !data.is_empty() {
            let space = self.frame_samples - self.pending.len();
            let take = space.min(data.len());
            self.pending.extend_from_slice(&data[..take]);
            data = &data[take..];

            if self.pending.len() == self.frame_samples {
                emit(&self.pending);
                self.pending.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a 1kHz sine wave at the given sample rate and duration.
    fn sine_wave(sample_rate: usize, freq: f64, duration_secs: f64) -> Vec<f32> {
        let n = (sample_rate as f64 * duration_secs) as usize;
        (0..n)
            .map(|i| {
                (2.0 * std::f64::consts::PI * freq * i as f64 / sample_rate as f64).sin() as f32
            })
            .collect()
    }

    fn collect_output(resampler: &mut FrameResampler, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::new();
        resampler.push(input, |frame| out.extend_from_slice(frame));
        out
    }

    #[test]
    fn reset_clears_in_buf_and_pending() {
        let mut r = FrameResampler::new(48000, 16000, Duration::from_millis(30));

        // Push less than one chunk (1024 samples) to leave data in in_buf
        let partial = vec![0.5f32; 500];
        let _ = collect_output(&mut r, &partial);

        r.reset();

        // Now push silence — should get only silence out, no remnants of 0.5
        let silence = vec![0.0f32; 4096];
        let out = collect_output(&mut r, &silence);

        let max_abs = out.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max_abs < 0.01,
            "After reset, silence input should produce near-silence output, got max_abs={}",
            max_abs
        );
    }

    #[test]
    fn reset_clears_fft_overlap_buffers() {
        let mut r = FrameResampler::new(48000, 16000, Duration::from_millis(30));

        // Push a loud 1kHz sine wave through the resampler (simulates recording 1)
        let sine = sine_wave(48000, 1000.0, 0.5); // 500ms of audio
        let _ = collect_output(&mut r, &sine);
        r.finish(|_| {});

        // Reset (simulates new recording starting)
        r.reset();

        // Push silence (simulates recording 2 starting with no speech)
        let silence = vec![0.0f32; 4096];
        let out = collect_output(&mut r, &silence);

        // The output should be near-zero. If the FFT overlap buffers weren't
        // cleared, the sine wave's tail would leak into this output.
        let max_abs = out.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max_abs < 0.01,
            "FFT overlap should not leak after reset; got max_abs={} (expected near-zero)",
            max_abs
        );
    }

    #[test]
    fn reset_between_recordings_no_crosstalk() {
        let mut r = FrameResampler::new(48000, 16000, Duration::from_millis(30));

        // Recording 1: ascending ramp (distinctive pattern)
        let ramp: Vec<f32> = (0..48000).map(|i| i as f32 / 48000.0).collect(); // 1 second
        let out1 = collect_output(&mut r, &ramp);
        r.finish(|_| {});
        assert!(!out1.is_empty(), "Recording 1 should produce output");

        // Reset between recordings
        r.reset();

        // Recording 2: constant DC signal of -0.5
        let dc = vec![-0.5f32; 48000]; // 1 second
        let out2 = collect_output(&mut r, &dc);

        // After the resampler settles (skip first frame which may have transient),
        // all samples should be near -0.5, not contaminated by the ascending ramp.
        if out2.len() > 480 {
            // Skip first frame (480 samples at 16kHz/30ms), check the rest
            let tail = &out2[480..];
            for (i, &s) in tail.iter().enumerate() {
                assert!(
                    (s - (-0.5)).abs() < 0.05,
                    "Recording 2 sample {} = {} (expected ~-0.5); ramp leaked through",
                    i + 480,
                    s
                );
            }
        }
    }

    #[test]
    fn reset_passthrough_mode_clears_pending() {
        // When in_hz == out_hz, no rubato resampler is created (passthrough mode).
        // Reset should still clear the pending frame buffer.
        let mut r = FrameResampler::new(16000, 16000, Duration::from_millis(30));

        // Push partial frame (less than 480 samples) to leave data in pending
        let partial = vec![1.0f32; 200];
        let _ = collect_output(&mut r, &partial);

        r.reset();

        // Push silence
        let silence = vec![0.0f32; 960];
        let out = collect_output(&mut r, &silence);

        // First complete frame should be all zeros, not contain the 1.0 values
        if !out.is_empty() {
            let max_abs = out.iter().take(480).map(|s| s.abs()).fold(0.0f32, f32::max);
            assert!(
                max_abs < 0.001,
                "Passthrough mode: pending buffer should be cleared after reset, got max_abs={}",
                max_abs
            );
        }
    }

    /// Push silence ending in a 200-sample 0.5 burst, then assert finish()
    /// recovers the burst and emits floor(input*ratio) source samples,
    /// padded to whole 480-sample frames.
    fn assert_tail_burst_flushed(in_hz: usize, input_len: usize, expected_out: usize) {
        let mut rs = FrameResampler::new(in_hz, 16000, Duration::from_millis(30));
        let mut input = vec![0.0f32; input_len];
        input[input_len - 200..].fill(0.5);

        let mut out = Vec::new();
        rs.push(&input, |frame| out.extend_from_slice(frame));
        rs.finish(|frame| out.extend_from_slice(frame));

        let max_abs = out.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max_abs > 0.3,
            "tail burst was lost in the resampler, max_abs={max_abs}"
        );
        assert_eq!(out.len(), expected_out);
    }

    #[test]
    fn finish_flushes_resampler_delay() {
        // Exact chunks keep in_buf empty, so the burst survives only via the
        // delay-line drain. 4096 in -> 1365 real out -> 1440 framed.
        assert_tail_burst_flushed(48000, 4 * RESAMPLER_CHUNK_SIZE, 1440);
    }

    #[test]
    fn finish_flushes_resampler_delay_44100() {
        // fft_size_in (1323) exceeds the 1024 chunk, so the drain must survive
        // a zero-output round. 4096 in -> 1486 real out -> 1920 framed.
        assert_tail_burst_flushed(44100, 4 * RESAMPLER_CHUNK_SIZE, 1920);
    }

    #[test]
    fn finish_flushes_unaligned_tail() {
        // Ends mid-chunk: partial-chunk path plus delay drain together.
        // 4396 in -> 1465 real out -> 1920 framed.
        assert_tail_burst_flushed(48000, 4 * RESAMPLER_CHUNK_SIZE + 300, 1920);
    }

    #[test]
    fn finish_does_not_leak_tail_into_next_session() {
        // 48kHz -> 16kHz, 30ms frames (480 output samples per frame).
        let mut rs = FrameResampler::new(48000, 16000, Duration::from_millis(30));

        // Leave a partial chunk buffered, then end the session.
        rs.push(&[0.5f32; 100], |_| {});
        rs.finish(|_| {});

        // A finished stream cannot be appended to without reset. In particular,
        // leftover zero padding cannot leak into an accidental later push.
        let mut emitted = 0usize;
        rs.push(&[0.25f32; RESAMPLER_CHUNK_SIZE], |frame| {
            emitted += frame.len()
        });
        assert_eq!(
            emitted, 0,
            "stale resampler tail from finish() leaked into the next session"
        );
    }

    #[test]
    fn compensated_output_matches_source_duration_for_short_and_unaligned_inputs() {
        for hz in [8_000, 16_000, 32_000, 44_100, 48_000, 96_001] {
            for n in [0, 1, 2, 10, 100, 1023, 1024, 1025, 1599, 1600, 1601, 4096, 4396, 50_333] {
                let mut r = FrameResampler::new(hz, 16_000, Duration::from_millis(100));
                let mut out = Vec::new();
                // Hardware callback boundaries must not change the result.
                for chunk in vec![0.2; n].chunks(137) {
                    r.push(chunk, |frame| out.extend_from_slice(frame));
                }
                r.finish(|frame| out.extend_from_slice(frame));
                let real = n * 16_000 / hz;
                assert!(r.error().is_none(), "hz={hz}, n={n}: {:?}", r.error());
                assert_eq!(r.real_output_samples, real, "hz={hz}, n={n}");
                assert_eq!(out.len(), real.div_ceil(1600) * 1600, "hz={hz}, n={n}");
                assert!(out[real..].iter().all(|&v| v == 0.0), "padding must be zero: hz={hz}, n={n}");
                let before = out.len();
                r.finish(|frame| out.extend_from_slice(frame));
                assert_eq!(out.len(), before, "finish must be idempotent");
            }
        }
    }

    #[test]
    fn delay_compensation_keeps_first_and_last_speech_at_their_source_positions() {
        for hz in [8_000, 16_000, 32_000, 44_100, 48_000, 96_001] {
            let n = hz / 2 + 333;
            let burst = hz / 50; // 20 ms of speech at both ends, silence between.
            let mut input = vec![0.0; n];
            input[..burst].fill(0.5);
            input[n - burst..].fill(-0.5);
            let mut r = FrameResampler::new(hz, 16_000, Duration::from_millis(100));
            let mut out = Vec::new();
            r.push(&input, |frame| out.extend_from_slice(frame));
            r.finish(|frame| out.extend_from_slice(frame));
            let real = n * 16_000 / hz;
            assert!(r.error().is_none(), "hz={hz}: {:?}", r.error());
            // After compensation these must be at the beginning/end of real
            // audio, not in leading latency or trimmed synthetic output.
            assert!(out[..160].iter().any(|&v| v > 0.4), "first speech lost or delayed: hz={hz}");
            assert!(out[real - 160..real].iter().any(|&v| v < -0.4), "last speech lost: hz={hz}");
            assert!(out[real..].iter().all(|&v| v == 0.0));
        }
    }
}
