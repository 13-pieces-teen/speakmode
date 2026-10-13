//! Experimental, causal level conditioning for 16 kHz mono floating-point audio.
//!
//! This is not a denoiser or a speech detector. A conservative energy/noise
//! estimate only controls gain; it never discards a sample or closes a gate.
//! It cannot improve SNR, recover audio removed by a driver, or establish ASR
//! quality. Those require paired recordings and transcription evaluation.
//!
//! Call on the audio worker after resampling and before PCM16 quantization.
//! Pass real samples only, excluding network-frame padding. No audio is retained
//! beyond scalar observation state, and there is no lookahead or buffered tail.

use serde::Serialize;
use std::time::Instant;

const SAMPLE_RATE: u32 = 16_000;
const OBSERVATION_SAMPLES: u32 = 160; // 10 ms; independent of caller chunking.
const MAX_GAIN: f64 = 3.981_071_705_534_972_2; // +12 dB.
const FIXED_GAIN: f64 = 1.995_262_314_968_879_5; // +6 dB, experiment only.
const TARGET_RMS: f64 = 0.056_234_132_519_034_91; // -25 dBFS.
const MIN_ELIGIBLE_RMS: f64 = 0.001; // -60 dBFS; quieter audio still passes.
const INITIAL_NOISE_RMS: f64 = 0.000_316_227_766_016_837_94; // -70 dBFS.
const MIN_NOISE_RMS: f64 = 0.000_031_622_776_601_683_795; // -90 dBFS.
const MAX_NOISE_RMS: f64 = 0.017_782_794_100_389_23; // -35 dBFS.
const NOISE_OUTPUT_LIMIT: f64 = 0.005_623_413_251_903_491; // -45 dBFS.
const NOISE_MARGIN_RATIO: f64 = 2.818_382_931_264_453_7; // 9 dB.
const PEAK_CEILING: f64 = 0.891_250_938_133_745_6; // -1 dBFS headroom.
const GAIN_HOLD_SAMPLES: u32 = SAMPLE_RATE * 120 / 1000;
const SILENCE_DBFS: f64 = -120.0;
const PROFILE_VERSION: &str = "soft-voice-v1";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConditioningMode {
    #[default]
    Bypass,
    SoftVoice,
    /// Offline comparison arm. Not a user-facing input preference.
    FixedGain6Db,
}

impl ConditioningMode {
    pub fn profile(self) -> &'static str {
        match self {
            Self::Bypass => "bypass",
            Self::SoftVoice => "softVoice",
            Self::FixedGain6Db => "fixedGain6Db",
        }
    }
}

/// Content-free, finite, serializable session aggregates. Peak/RMS values use
/// -120 dBFS for digital silence. Mean gain is the arithmetic mean of effective
/// linear per-sample gain, converted to dB (not a speech-only loudness metric).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConditioningSummary {
    pub profile: &'static str,
    pub version: &'static str,
    pub sample_rate_hz: u32,
    pub real_samples: u64,
    pub input_rms_dbfs: f64,
    pub output_rms_dbfs: f64,
    pub input_peak_dbfs: f64,
    pub output_peak_dbfs: f64,
    pub mean_gain_db: f64,
    pub max_gain_db: f64,
    pub input_clipped_samples: u64,
    pub output_clipped_samples: u64,
    pub peak_limited_samples: u64,
    pub nonfinite_samples: u64,
    pub processing_us: u64,
}

pub struct AudioConditioner {
    mode: ConditioningMode,
    gain: f64,
    target_gain: f64,
    gain_rise_coefficient: f64,
    gain_fall_coefficient: f64,
    peak_release_coefficient: f64,
    peak_envelope: f64,
    noise_rms: f64,
    hold_remaining: u32,
    observed: u32,
    observed_energy: f64,
    observed_peak: f64,
    observed_crossings: u32,
    previous_sign: i8,
    real_samples: u64,
    input_energy: f64,
    output_energy: f64,
    input_peak: f64,
    output_peak: f64,
    gain_sum: f64,
    max_gain: f64,
    input_clipped_samples: u64,
    output_clipped_samples: u64,
    peak_limited_samples: u64,
    nonfinite_samples: u64,
    processing_us: u64,
}

impl AudioConditioner {
    pub fn new(mode: ConditioningMode) -> Self {
        Self {
            mode,
            gain: 1.0,
            target_gain: match mode {
                ConditioningMode::FixedGain6Db => FIXED_GAIN,
                _ => 1.0,
            },
            // Reach about 95% of a requested change in 3 time constants.
            gain_rise_coefficient: smoothing_coefficient(0.040),
            gain_fall_coefficient: smoothing_coefficient(0.080),
            peak_release_coefficient: smoothing_coefficient(0.080),
            peak_envelope: 0.0,
            noise_rms: INITIAL_NOISE_RMS,
            hold_remaining: 0,
            observed: 0,
            observed_energy: 0.0,
            observed_peak: 0.0,
            observed_crossings: 0,
            previous_sign: 0,
            real_samples: 0,
            input_energy: 0.0,
            output_energy: 0.0,
            input_peak: 0.0,
            output_peak: 0.0,
            gain_sum: 0.0,
            max_gain: 1.0,
            input_clipped_samples: 0,
            output_clipped_samples: 0,
            peak_limited_samples: 0,
            nonfinite_samples: 0,
            processing_us: 0,
        }
    }

    /// In-place and allocation-free. Every input has one output at the same
    /// position. Finite bypass input remains bit-identical, including -0.0.
    /// Nonfinite samples become zero in every profile to avoid poisoning DSP.
    pub fn process(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }
        let started = Instant::now();
        for sample in samples {
            if !sample.is_finite() {
                self.nonfinite_samples += 1;
                *sample = 0.0;
            }
            let input = f64::from(*sample);
            let magnitude = input.abs();
            self.real_samples += 1;
            self.input_energy += input * input;
            self.input_peak = self.input_peak.max(magnitude);
            self.input_clipped_samples += u64::from(magnitude >= 1.0);

            let mut applied_gain = 1.0;
            if self.mode != ConditioningMode::Bypass {
                // Decaying peak envelope provides instant gain reduction and
                // gradual recovery, rather than hard-clipping waveform peaks.
                self.peak_envelope = magnitude
                    .max(self.peak_envelope * self.peak_release_coefficient);
                let coefficient = if self.target_gain > self.gain {
                    self.gain_rise_coefficient
                } else {
                    self.gain_fall_coefficient
                };
                self.gain = self.target_gain + coefficient * (self.gain - self.target_gain);
                let peak_safe_gain = if self.peak_envelope > 0.0 {
                    PEAK_CEILING / self.peak_envelope
                } else {
                    MAX_GAIN
                };
                applied_gain = self.gain.min(peak_safe_gain);
                if applied_gain < self.gain {
                    self.peak_limited_samples += 1;
                    // Keep the controller in sync with the emergency reduction;
                    // it must not spring back to a hidden high gain next sample.
                    self.gain = applied_gain;
                }
                *sample = (input * applied_gain) as f32;
            }

            let output = f64::from(*sample);
            self.output_energy += output * output;
            self.output_peak = self.output_peak.max(output.abs());
            self.output_clipped_samples += u64::from(output.abs() >= 1.0);
            self.gain_sum += applied_gain;
            self.max_gain = self.max_gain.max(applied_gain);

            if self.mode == ConditioningMode::SoftVoice {
                self.observe(input);
            }
        }
        self.processing_us = self
            .processing_us
            .saturating_add(started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64);
    }

    pub fn summary(&self) -> ConditioningSummary {
        let count = self.real_samples.max(1) as f64;
        ConditioningSummary {
            profile: self.mode.profile(),
            version: PROFILE_VERSION,
            sample_rate_hz: SAMPLE_RATE,
            real_samples: self.real_samples,
            input_rms_dbfs: amplitude_dbfs((self.input_energy / count).sqrt()),
            output_rms_dbfs: amplitude_dbfs((self.output_energy / count).sqrt()),
            input_peak_dbfs: amplitude_dbfs(self.input_peak),
            output_peak_dbfs: amplitude_dbfs(self.output_peak),
            mean_gain_db: if self.real_samples == 0 {
                0.0
            } else {
                amplitude_dbfs(self.gain_sum / count)
            },
            max_gain_db: amplitude_dbfs(self.max_gain),
            input_clipped_samples: self.input_clipped_samples,
            output_clipped_samples: self.output_clipped_samples,
            peak_limited_samples: self.peak_limited_samples,
            nonfinite_samples: self.nonfinite_samples,
            processing_us: self.processing_us,
        }
    }

    fn observe(&mut self, input: f64) {
        self.observed += 1;
        self.observed_energy += input * input;
        self.observed_peak = self.observed_peak.max(input.abs());
        let sign = if input > 0.0 { 1 } else if input < 0.0 { -1 } else { 0 };
        if sign != 0 && self.previous_sign != 0 && sign != self.previous_sign {
            self.observed_crossings += 1;
        }
        if sign != 0 {
            self.previous_sign = sign;
        }
        self.hold_remaining = self.hold_remaining.saturating_sub(1);
        if self.observed < OBSERVATION_SAMPLES {
            return;
        }

        let rms = (self.observed_energy / f64::from(self.observed)).sqrt();
        let crest = if rms > 0.0 { self.observed_peak / rms } else { 0.0 };
        let crossings = f64::from(self.observed_crossings) / f64::from(self.observed);
        // This heuristic is deliberately not labelled speech probability. It
        // avoids chasing tiny levels and obvious broadband stationary noise,
        // but voiced speech and tonal equipment can overlap. A short hold keeps
        // consonants from repeatedly resetting an already established gain.
        let gain_eligible = rms >= MIN_ELIGIBLE_RMS
            && rms >= self.noise_rms * NOISE_MARGIN_RATIO
            && (1.15..=10.0).contains(&crest)
            && (0.005..=0.22).contains(&crossings);

        if gain_eligible {
            self.hold_remaining = GAIN_HOLD_SAMPLES;
            let level_gain = (TARGET_RMS / rms).clamp(1.0, MAX_GAIN);
            let noise_gain = (NOISE_OUTPUT_LIMIT / self.noise_rms).clamp(1.0, MAX_GAIN);
            self.target_gain = level_gain.min(noise_gain);
        } else if rms < MIN_ELIGIBLE_RMS || self.hold_remaining == 0 {
            self.target_gain = 1.0;
        }

        // Only noneligible, unheld, nonimpulsive observations update the noise
        // estimate. No calibration silence is required, and the estimate never
        // controls sample retention. Digital silence does not chase gain up.
        if !gain_eligible && self.hold_remaining == 0 && crest <= 4.5 {
            let rate = if rms < self.noise_rms { 0.15 } else { 0.02 };
            self.noise_rms = (self.noise_rms + rate * (rms - self.noise_rms))
                .clamp(MIN_NOISE_RMS, MAX_NOISE_RMS);
        }

        self.observed = 0;
        self.observed_energy = 0.0;
        self.observed_peak = 0.0;
        self.observed_crossings = 0;
    }
}

fn smoothing_coefficient(seconds: f64) -> f64 {
    (-1.0 / (seconds * f64::from(SAMPLE_RATE))).exp()
}

fn amplitude_dbfs(amplitude: f64) -> f64 {
    if amplitude <= 0.0 {
        SILENCE_DBFS
    } else {
        (20.0 * amplitude.log10()).max(SILENCE_DBFS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(samples: usize, amplitude: f32) -> Vec<f32> {
        (0..samples)
            .map(|index| {
                amplitude
                    * (2.0 * std::f32::consts::PI * 200.0 * index as f32 / SAMPLE_RATE as f32)
                        .sin()
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f64 {
        (samples.iter().map(|value| f64::from(*value).powi(2)).sum::<f64>()
            / samples.len() as f64)
            .sqrt()
    }

    #[test]
    fn bypass_preserves_every_finite_bit_and_reports_only_real_samples() {
        let mut values = vec![-0.0, 0.0, -1.0, 1.0, 0.000_003, -0.25, 1.3];
        let before: Vec<u32> = values.iter().map(|value: &f32| value.to_bits()).collect();
        let mut conditioner = AudioConditioner::new(ConditioningMode::default());
        conditioner.process(&mut values);
        assert_eq!(before, values.iter().map(|value| value.to_bits()).collect::<Vec<_>>());
        let summary = conditioner.summary();
        assert_eq!(summary.real_samples, 7);
        assert_eq!(summary.profile, "bypass");
        assert_eq!(summary.mean_gain_db, 0.0);
        assert_eq!(summary.input_clipped_samples, 3);
        assert_eq!(summary.output_clipped_samples, 3);
    }

    #[test]
    fn weak_voiced_signal_is_preserved_then_raised_without_startup_silence() {
        let original = tone(8_000, 0.003);
        let mut output = original.clone();
        let mut conditioner = AudioConditioner::new(ConditioningMode::SoftVoice);
        conditioner.process(&mut output);
        // Initial observation is passed through instead of withheld for VAD.
        assert_eq!(&output[..160], &original[..160]);
        assert!(rms(&output[320..1600]) > rms(&original[320..1600]) * 2.0);
        assert!(rms(&output[3200..]) > rms(&original[3200..]) * 3.8);
        assert!(conditioner.summary().max_gain_db <= 12.0 + 1e-6);
        assert_eq!(conditioner.summary().real_samples, original.len() as u64);
        assert_eq!(conditioner.summary().output_clipped_samples, 0);
    }

    #[test]
    fn silence_and_stationary_low_noise_do_not_chase_gain() {
        let mut conditioner = AudioConditioner::new(ConditioningMode::SoftVoice);
        let mut silence = vec![0.0; 16_000];
        conditioner.process(&mut silence);
        assert!(silence.iter().all(|value| *value == 0.0));
        // Deterministic stationary noise, well below the eligible energy floor.
        let mut state = 0x1234_5678_u32;
        let mut noise: Vec<f32> = (0..16_000)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((f64::from(state) / f64::from(u32::MAX)) * 2.0 - 1.0) as f32 * 0.0005
            })
            .collect();
        let original = noise.clone();
        conditioner.process(&mut noise);
        assert_eq!(noise, original);
        assert_eq!(conditioner.summary().max_gain_db, 0.0);
        assert_eq!(conditioner.summary().real_samples, 32_000);
    }

    #[test]
    fn stationary_broadband_noise_above_energy_floor_is_not_boosted() {
        let mut state = 47_u32;
        let mut noise: Vec<f32> = (0..16_000)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((f64::from(state) / f64::from(u32::MAX)) * 2.0 - 1.0) as f32 * 0.01
            })
            .collect();
        let original = noise.clone();
        let mut conditioner = AudioConditioner::new(ConditioningMode::SoftVoice);
        conditioner.process(&mut noise);
        assert_eq!(noise, original);
        assert_eq!(conditioner.summary().max_gain_db, 0.0);
    }

    #[test]
    fn abrupt_loud_input_is_peak_protected_without_dropping_samples() {
        for mode in [ConditioningMode::SoftVoice, ConditioningMode::FixedGain6Db] {
            let mut conditioner = AudioConditioner::new(mode);
            let mut soft = tone(8_000, 0.003);
            conditioner.process(&mut soft);
            let mut loud = vec![0.98, -0.9, 0.7, -0.3];
            conditioner.process(&mut loud);
            assert!(loud.iter().all(|value| value.abs() <= PEAK_CEILING as f32 + 1e-7));
            assert!(loud[0] > loud[2]);
            assert!(loud[1] < loud[3]);
            assert_eq!(conditioner.summary().real_samples, 8_004);
            assert_eq!(conditioner.summary().output_clipped_samples, 0);
            assert!(conditioner.summary().peak_limited_samples > 0);
        }
    }

    #[test]
    fn processing_is_partition_independent_and_keeps_short_tail_in_order() {
        let original = tone(3_217, 0.003);
        let mut whole = original.clone();
        let mut fragmented = original.clone();
        let mut first = AudioConditioner::new(ConditioningMode::SoftVoice);
        let mut second = AudioConditioner::new(ConditioningMode::SoftVoice);
        first.process(&mut whole);
        for chunk in fragmented.chunks_mut(137) {
            second.process(chunk);
        }
        second.process(&mut []);
        assert_eq!(whole, fragmented);
        assert_eq!(first.summary().real_samples, 3_217);
        assert_eq!(second.summary().real_samples, 3_217);
        assert_eq!(first.summary().input_rms_dbfs, second.summary().input_rms_dbfs);
        assert_eq!(first.summary().output_rms_dbfs, second.summary().output_rms_dbfs);
        for (before, after) in original.iter().zip(whole.iter()) {
            assert_eq!(before.signum(), after.signum());
        }
        assert_ne!(whole[3_216], 0.0);
    }

    #[test]
    fn invalid_samples_are_sanitized_and_all_summaries_serialize_finitely() {
        for mode in [ConditioningMode::Bypass, ConditioningMode::SoftVoice, ConditioningMode::FixedGain6Db] {
            let mut conditioner = AudioConditioner::new(mode);
            let empty = serde_json::to_value(conditioner.summary()).unwrap();
            assert_eq!(empty["inputRmsDbfs"], SILENCE_DBFS);
            let mut values = vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX, -f32::MAX, f32::MIN_POSITIVE, 0.0];
            conditioner.process(&mut values);
            assert_eq!(&values[..3], &[0.0, 0.0, 0.0]);
            assert!(values.iter().all(|value| value.is_finite()));
            assert_eq!(conditioner.summary().nonfinite_samples, 3);
            assert_eq!(conditioner.summary().real_samples, 7);
            let summary = serde_json::to_value(conditioner.summary()).unwrap();
            assert!(summary.as_object().unwrap().values().all(|value| !value.is_null()));
            if mode != ConditioningMode::Bypass {
                assert!(values.iter().all(|value| value.abs() <= PEAK_CEILING as f32 + 1e-7));
            }
        }
    }

    #[test]
    fn fixed_comparison_arm_settles_near_six_db() {
        let mut output = tone(8_000, 0.003);
        let mut conditioner = AudioConditioner::new(ConditioningMode::FixedGain6Db);
        conditioner.process(&mut output);
        assert!((conditioner.summary().max_gain_db - 6.0).abs() < 0.001);
        assert_eq!(conditioner.summary().profile, "fixedGain6Db");
    }
}
