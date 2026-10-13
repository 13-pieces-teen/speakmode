//! Offline engineering probe only. Deterministic synthetic signals are not speech
//! recordings and cannot establish recognition quality, SNR gains, or device behavior.
//! Uses no microphone, app profile, filesystem writes, credentials, or network.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use speakmode_lib::platform::audio_conditioner::{AudioConditioner, ConditioningMode};
use std::{f64::consts::TAU, time::Instant};

const SAMPLE_RATE: usize = 16_000;
const FRAME_SAMPLES: usize = 1600;

struct Signal {
    id: &'static str,
    description: &'static str,
    samples: Vec<f32>,
}

fn harmonic(index: usize, amplitude: f64) -> f32 {
    let t = index as f64 / SAMPLE_RATE as f64;
    // A voiced-like harmonic envelope, deliberately neither recorded speech nor TTS.
    let envelope = 0.25 + 0.75 * (TAU * 1.7 * t).sin().powi(2);
    let carrier = 0.7 * (TAU * 170.0 * t).sin()
        + 0.2 * (TAU * 340.0 * t).sin()
        + 0.1 * (TAU * 510.0 * t).sin();
    (amplitude * envelope * carrier) as f32
}

fn signals() -> Vec<Signal> {
    let mut seed = 0x4d53_504bu32;
    let noise = (0..2 * SAMPLE_RATE).map(|_| {
        // Xorshift32 has fixed seed/order; never uses machine time or OS randomness.
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        ((f64::from(seed) / f64::from(u32::MAX) * 2.0 - 1.0) * 0.0007) as f32
    }).collect();
    let mut jump = (0..3 * SAMPLE_RATE).map(|index| {
        if index < 2 * SAMPLE_RATE { harmonic(index, 0.006) }
        else { (0.96 * (TAU * 170.0 * index as f64 / SAMPLE_RATE as f64).sin()) as f32 }
    }).collect::<Vec<_>>();
    // Ensure the first loud sample exercises sudden peak protection, independent of phase.
    jump[2 * SAMPLE_RATE] = 0.98;
    vec![
        Signal { id: "low-voiced-harmonic", description: "Quiet harmonic carrier with amplitude envelope; not speech",
            samples: (0..3 * SAMPLE_RATE).map(|i| harmonic(i, 0.006)).collect() },
        Signal { id: "normal-voice-surrogate", description: "Higher-level version of the same harmonic envelope; not speech",
            samples: (0..3 * SAMPLE_RATE).map(|i| harmonic(i, 0.18)).collect() },
        Signal { id: "pure-silence", description: "Exact digital zero; does not model a microphone noise floor",
            samples: vec![0.0; 2 * SAMPLE_RATE] },
        Signal { id: "stationary-low-broadband-noise", description: "Deterministic low-level uniform broadband noise",
            samples: noise },
        Signal { id: "soft-to-loud-jump", description: "Quiet harmonic envelope followed by a sudden loud sine and 0.98 peak",
            samples: jump },
        Signal { id: "short-tail", description: "1701 real samples: one 100 ms frame followed by 101 samples; no padding processed",
            samples: (0..1701).map(|i| harmonic(i, 0.006)).collect() },
    ]
}

fn sample_hash(samples: &[f32]) -> String {
    let mut hash = Sha256::new();
    for sample in samples { hash.update(sample.to_le_bytes()); }
    format!("{:x}", hash.finalize())
}

fn run(signal: &Signal, mode: ConditioningMode) -> Value {
    // Every arm starts from the same original float signal and fresh DSP state.
    let mut output = signal.samples.clone();
    let mut conditioner = AudioConditioner::new(mode);
    let mut max_chunk_processing_us = 0u64;
    let started = Instant::now();
    for chunk in output.chunks_mut(FRAME_SAMPLES) {
        let chunk_started = Instant::now();
        conditioner.process(chunk);
        max_chunk_processing_us = max_chunk_processing_us.max(chunk_started.elapsed().as_micros() as u64);
    }
    let elapsed_us = started.elapsed().as_micros() as u64;
    let summary = conditioner.summary();
    let all_samples_finite = output.iter().all(|sample| sample.is_finite());
    let sample_count_preserved = output.len() == signal.samples.len()
        && summary.real_samples == signal.samples.len() as u64;
    let bypass_identity = output.iter().zip(&signal.samples).all(|(a, b)| a.to_bits() == b.to_bits());
    let summary_finite = [summary.input_rms_dbfs, summary.output_rms_dbfs, summary.input_peak_dbfs,
        summary.output_peak_dbfs, summary.mean_gain_db, summary.max_gain_db].iter().all(|v| v.is_finite());
    let no_output_clipping = summary.output_clipped_samples == 0 && output.iter().all(|sample| sample.abs() < 1.0);
    let checks_passed = all_samples_finite && sample_count_preserved && summary_finite && no_output_clipping
        && (mode != ConditioningMode::Bypass || bypass_identity);
    json!({
        "profile": mode.profile(), "summary": summary,
        "rmsChangeDb": summary.output_rms_dbfs - summary.input_rms_dbfs,
        "outputFloat32Sha256": sample_hash(&output),
        "checks": {"allSamplesFinite": all_samples_finite, "summaryFinite": summary_finite,
            "sampleCountPreserved": sample_count_preserved, "noOutputClipping": no_output_clipping,
            "baselineBitIdentity": if mode == ConditioningMode::Bypass { Some(bypass_identity) } else { None },
            "passed": checks_passed},
        "timing": {"processingWallUs": elapsed_us, "maxChunkProcessingUs": max_chunk_processing_us,
            "processingOnlyRealtimeFactor": elapsed_us as f64 / (signal.samples.len() as f64 / SAMPLE_RATE as f64 * 1_000_000.0),
            "allocationGenerationAndHashingIncluded": false, "repeats": 1}
    })
}

fn main() {
    let rows = signals().iter().map(|signal| {
        let arms = [ConditioningMode::Bypass, ConditioningMode::FixedGain6Db, ConditioningMode::SoftVoice]
            .into_iter().map(|mode| run(signal, mode)).collect::<Vec<_>>();
        json!({"signalId": signal.id, "description": signal.description,
            "inputFloat32Sha256": sample_hash(&signal.samples), "realSamples": signal.samples.len(),
            "audioDurationMs": signal.samples.len() as f64 * 1000.0 / SAMPLE_RATE as f64,
            "arms": arms})
    }).collect::<Vec<_>>();
    let passed = rows.iter().all(|row| row["arms"].as_array().unwrap().iter().all(|arm| arm["checks"]["passed"] == true));
    let report = json!({"schemaVersion": 1, "tool": "speakmode-soft-voice-probe",
        "appVersion": env!("CARGO_PKG_VERSION"), "evidenceKind": "deterministic-synthetic-engineering",
        "buildProfile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "sampleRateHz": SAMPLE_RATE, "chunkSamples": FRAME_SAMPLES,
        "signalCount": rows.len(), "profileCount": 3, "engineeringChecksPassed": passed,
        "signals": rows,
        "limitations": [
            "These signals are not human speech or recorded quiet speech; no ASR/CER/WER or SNR benefit is measured.",
            "No microphone, device/driver DSP, resampler, PCM16 quantization, cloud, UI, or delivery path is exercised.",
            "Timing is one synthetic processing pass in the declared build profile, not a latency benchmark or production P95.",
            "Original float samples are processed once per arm with fresh state; no audio content is exported."
        ]});
    println!("{}", serde_json::to_string_pretty(&report).expect("finite engineering report serializes"));
    if !passed { std::process::exit(1); }
}
