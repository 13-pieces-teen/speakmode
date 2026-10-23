//! Explicit, bounded cloud experiment using only preregistered fixed PCM fixtures.
//! This bypasses the microphone, app database writes, UI, and cross-app delivery.
//! No secret, credential account, request header, workspace, or provider body is logged.
#[path="lab_provider_scope/mod.rs"] mod lab_provider_scope;
use chrono::Utc;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use speakmode_lib::{
    cloud::{self, CloudError, asr::{AsrInput, AsrProgress}},
    core::{BaseMode, DictionaryEntry, DictionarySnapshot, ModeSnapshot, ProviderConfig,
        SourceRevision, DICTIONARY_MAX_CONTEXT_BYTES, DICTIONARY_MAX_ENTRIES,
        DICTIONARY_MAX_TERM_CHARS},
    platform::{audio::quantize, audio_conditioner::{AudioConditioner, ConditioningMode}},
};
use std::{collections::{HashMap, HashSet}, fs::{self, File, OpenOptions}, io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf}, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;
use unicode_casefold::UnicodeCaseFold;
use unicode_categories::UnicodeCategories;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

const MAX_RUNS: usize = 18;
const MAX_TOTAL_SAMPLES: u64 = 16_000 * 600;
const FRAME_SAMPLES: usize = 1600;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest { scope: Value, asr_runs: Vec<RunSpec> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunSpec {
    run_id: String, fixture_id: String, wav_path: PathBuf,
    dictionary_terms: Vec<String>, language: String, base_modes: Vec<BaseMode>,
    #[serde(default)] asr_model: Option<String>,
    #[serde(default)] rewrite_model: Option<String>,
    #[serde(default)] conditioning_profile: ConditioningProfile,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ConditioningProfile { #[default] Bypass, SoftVoice, FixedGain6Db }
impl ConditioningProfile {
    fn mode(self) -> ConditioningMode {
        match self {
            Self::Bypass => ConditioningMode::Bypass,
            Self::SoftVoice => ConditioningMode::SoftVoice,
            Self::FixedGain6Db => ConditioningMode::FixedGain6Db,
        }
    }
}
struct Fixture { pcm: Arc<Vec<i16>>, sha256: String }
struct PreparedRun { spec: RunSpec, fixture: Arc<Fixture>, dictionary: DictionarySnapshot }
struct PreparedManifest { scope: Value, runs: Vec<PreparedRun>, total_samples: u64, manifest_sha256: String }

// All errors used by this tool are static safe messages, rather than OS/provider error bodies.
fn lab_error(code: &str, message: &str) -> CloudError { CloudError::new(code, "experiment", message) }
fn absolute(path: &Path) -> Result<(), CloudError> {
    if !path.is_absolute() { return Err(lab_error("absolute_path_required", "实验输入和输出必须使用绝对路径")); }
    Ok(())
}
fn valid_label(label: &str) -> bool {
    !label.is_empty() && label.len() <= 128 && label.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
fn dictionary(terms: &[String]) -> Result<DictionarySnapshot, CloudError> {
    if terms.len() > DICTIONARY_MAX_ENTRIES { return Err(lab_error("dictionary_entry_limit", "实验词典超过条目限制")); }
    let mut folded = HashSet::new();
    let mut entries = Vec::new();
    for (index, term) in terms.iter().enumerate() {
        if term.is_empty() || term.trim() != term || term.chars().count() > DICTIONARY_MAX_TERM_CHARS
            || term.chars().any(|c| c.is_control() || c.is_other_format() || c == '\u{2028}' || c == '\u{2029}') {
            return Err(lab_error("dictionary_invalid", "实验词典包含无效词条"));
        }
        if !folded.insert(term.nfkc().case_fold().collect::<String>()) {
            return Err(lab_error("dictionary_duplicate", "实验词典包含重复词条"));
        }
        entries.push(DictionaryEntry { id: format!("voice-lab-term-{index}"), term: term.clone(),
            created_at: "2026-10-08T00:00:00Z".into(), updated_at: "2026-10-08T00:00:00Z".into() });
    }
    let snapshot = DictionarySnapshot { version: 1, entries };
    if snapshot.asr_context().is_some_and(|context| context.len() > DICTIONARY_MAX_CONTEXT_BYTES) {
        return Err(lab_error("dictionary_context_limit", "实验词典超过上下文容量"));
    }
    Ok(snapshot)
}
fn load_fixture(path: &Path) -> Result<Fixture, CloudError> {
    absolute(path)?;
    let metadata = fs::metadata(path).map_err(|_| lab_error("fixture_unavailable", "固定音频不可读"))?;
    // One run cannot exceed the entire 600 second budget; bound bytes before allocating.
    if !metadata.is_file() || metadata.len() > MAX_TOTAL_SAMPLES * 2 + 1024 * 1024 {
        return Err(lab_error("fixture_size_limit", "固定音频超过容量限制"));
    }
    let bytes = fs::read(path).map_err(|_| lab_error("fixture_unavailable", "固定音频不可读"))?;
    let mut reader = hound::WavReader::new(std::io::Cursor::new(&bytes))
        .map_err(|_| lab_error("fixture_invalid", "固定音频不是有效 WAV"))?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.sample_rate != 16_000 || spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
        return Err(lab_error("fixture_format", "固定音频必须是单声道 16kHz PCM16 WAV"));
    }
    if reader.duration() == 0 || u64::from(reader.duration()) > MAX_TOTAL_SAMPLES {
        return Err(lab_error("fixture_duration_limit", "固定音频时长无效或超过限制"));
    }
    let pcm = reader.samples::<i16>().collect::<Result<Vec<_>, _>>()
        .map_err(|_| lab_error("fixture_invalid", "固定音频 PCM 数据无效"))?;
    if pcm.is_empty() || pcm.len() as u64 > MAX_TOTAL_SAMPLES {
        return Err(lab_error("fixture_duration_limit", "固定音频时长无效或超过限制"));
    }
    Ok(Fixture { pcm: Arc::new(pcm), sha256: format!("{:x}", Sha256::digest(&bytes)) })
}
fn prepare(path: &Path) -> Result<PreparedManifest, CloudError> {
    absolute(path)?;
    let metadata = fs::metadata(path).map_err(|_| lab_error("manifest_unavailable", "实验清单不可读"))?;
    if metadata.len() > 1024 * 1024 { return Err(lab_error("manifest_limit", "实验清单超过容量限制")); }
    let bytes = fs::read(path).map_err(|_| lab_error("manifest_unavailable", "实验清单不可读"))?;
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|_| lab_error("manifest_invalid", "实验清单格式无效"))?;
    if manifest.scope.is_null() || manifest.scope == json!("") || manifest.asr_runs.is_empty() || manifest.asr_runs.len() > MAX_RUNS {
        return Err(lab_error("manifest_limit", "实验范围不能为空，ASR 请求必须为 1 至 18 次"));
    }
    let mut ids = HashSet::new();
    let mut fixture_ids = HashMap::<String, PathBuf>::new();
    let mut fixtures = HashMap::<PathBuf, Arc<Fixture>>::new();
    let mut total_samples = 0u64;
    let mut runs = Vec::new();
    for spec in manifest.asr_runs {
        if !valid_label(&spec.run_id) || !valid_label(&spec.fixture_id) || !ids.insert(spec.run_id.clone()) {
            return Err(lab_error("manifest_ids", "实验编号必须唯一且为有效标签"));
        }
        if spec.language != "auto" || spec.base_modes.len() > 2 || spec.base_modes.iter().any(|mode| *mode == BaseMode::Auto)
            || (spec.base_modes.len() == 2 && spec.base_modes[0] == spec.base_modes[1]) {
            return Err(lab_error("manifest_modes", "实验只允许 auto 语言和各一次 light、structure 整理"));
        }
        absolute(&spec.wav_path)?;
        if fixture_ids.get(&spec.fixture_id).is_some_and(|old| old != &spec.wav_path) {
            return Err(lab_error("manifest_fixture_id", "同一固定音频编号不能指向不同文件"));
        }
        fixture_ids.insert(spec.fixture_id.clone(), spec.wav_path.clone());
        let fixture = match fixtures.get(&spec.wav_path) {
            Some(fixture) => fixture.clone(),
            None => { let fixture = Arc::new(load_fixture(&spec.wav_path)?); fixtures.insert(spec.wav_path.clone(), fixture.clone()); fixture }
        };
        total_samples = total_samples.checked_add(fixture.pcm.len() as u64)
            .ok_or_else(|| lab_error("manifest_duration_limit", "实验总时长超过限制"))?;
        if total_samples > MAX_TOTAL_SAMPLES { return Err(lab_error("manifest_duration_limit", "全部 ASR 请求的音频总时长不能超过 600 秒")); }
        let dictionary = dictionary(&spec.dictionary_terms)?;
        runs.push(PreparedRun { spec, fixture, dictionary });
    }
    Ok(PreparedManifest { scope: manifest.scope, runs, total_samples, manifest_sha256: format!("{:x}", Sha256::digest(&bytes)) })
}
fn config(path: &Path) -> Result<ProviderConfig, CloudError> {
    absolute(path)?;
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_| lab_error("config_unavailable", "指定的云端配置数据库不可只读访问"))?;
    let text: String = connection.query_row("SELECT data FROM provider_config WHERE singleton=1", [], |row| row.get(0))
        .map_err(|_| lab_error("config_unavailable", "指定数据库中没有有效云端配置"))?;
    if text.len() > 16 * 1024 { return Err(lab_error("config_invalid", "云端配置超过容量限制")); }
    let config: ProviderConfig = serde_json::from_str(&text)
        .map_err(|_| lab_error("config_invalid", "云端配置格式无效"))?;
    lab_provider_scope::paired_voice(&config)?;
    Ok(config)
}
fn config_metadata(config: &ProviderConfig) -> Value {
    json!({"region": config.region, "asrModel": config.asr_model, "rewriteModel": config.rewrite_model,
        "adapterVersion": config.adapter_version})
}
fn config_for_run(base: &ProviderConfig, spec: &RunSpec) -> Result<ProviderConfig, CloudError> {
    let mut selected = base.clone();
    if let Some(id) = &spec.asr_model {
        let model = cloud::models::catalog().asr.into_iter().find(|model| model.id == id)
            .ok_or_else(|| lab_error("unsupported_asr_model", "实验语音模型不在兼容清单中"))?;
        selected.asr_model = id.clone();
        selected.adapter_version = model.adapter_version
            .ok_or_else(|| lab_error("asr_adapter_not_implemented", "实验语音模型的接口尚未接入"))?.into();
    }
    if let Some(id) = &spec.rewrite_model { selected.rewrite_model = id.clone(); }
    cloud::models::validate_recognition(&selected, &spec.language)?;
    cloud::models::validate_rewrite_model(&selected)?;
    cloud::endpoints(&selected)?;
    Ok(selected)
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    configured_ms: Option<u64>, first_caption_ms: Option<u64>, first_final_caption_ms: Option<u64>,
    caption_events: u64, revision_changes: u64, speech_starts: u64,
    last_revisions: HashMap<String, u64>, latest_captions: Vec<cloud::asr::CaptionItem>,
    feed_started_ms: Option<u64>, finish_enqueued_ms: Option<u64>, max_enqueue_wait_ms: u64,
    sent_frames: u64, sent_real_samples: u64, padded_tail_samples: u64,
    /// Production client-write/event metadata; existing sent_* fields count queue admission only.
    wire: Option<cloud::asr::AsrTelemetryView>,
    /// Aggregate only: no PCM, frame contents, or additional fixture text.
    conditioning: Option<Value>,
}
fn ms(start: Instant) -> u64 { start.elapsed().as_millis().try_into().unwrap_or(u64::MAX) }
fn condition_frame(chunk: &[i16], profile: ConditioningProfile, conditioner: &mut AudioConditioner) -> Vec<i16> {
    // Measure/process real samples only. Zero padding must not drive the gain controller.
    let mut samples = chunk.iter().map(|sample| f32::from(*sample) / 32768.0).collect::<Vec<_>>();
    conditioner.process(&mut samples);
    let mut frame = if profile == ConditioningProfile::Bypass {
        // Never requantize the baseline: even an identity float roundtrip could change PCM.
        chunk.to_vec()
    } else { samples.into_iter().map(quantize).collect() };
    frame.resize(FRAME_SAMPLES, 0);
    frame
}
async fn feed(pcm: Arc<Vec<i16>>, tx: mpsc::Sender<AsrInput>, configured: Arc<AtomicBool>, notify: Arc<Notify>,
    progress: Arc<Mutex<Progress>>, started: Instant, profile: ConditioningProfile) -> Result<(), CloudError> {
    while !configured.load(Ordering::Acquire) { notify.notified().await; }
    progress.lock().unwrap().feed_started_ms = Some(ms(started));
    // A fresh controller per request; every arm reads the immutable original fixture once.
    let mut conditioner = AudioConditioner::new(profile.mode());
    let mut sequence = 0u64;
    for chunk in pcm.chunks(FRAME_SAMPLES) {
        // At least the real audio duration elapses between sends. No catch-up burst or acceleration.
        tokio::time::sleep(Duration::from_secs_f64(chunk.len() as f64 / 16_000.0)).await;
        let padding = FRAME_SAMPLES - chunk.len();
        let frame = condition_frame(chunk, profile, &mut conditioner);
        progress.lock().unwrap().conditioning = Some(serde_json::to_value(conditioner.summary()).expect("conditioning serializes"));
        let enqueue = Instant::now();
        tx.send(AsrInput::Frame { sequence, pcm16: frame, padding_samples: padding }).await
            .map_err(|_| lab_error("experiment_audio_closed", "ASR 已结束，固定音频发送停止"))?;
        {
            let mut metrics = progress.lock().unwrap();
            metrics.max_enqueue_wait_ms = metrics.max_enqueue_wait_ms.max(ms(enqueue));
            metrics.sent_frames += 1; metrics.sent_real_samples += chunk.len() as u64;
            metrics.padded_tail_samples += padding as u64;
        }
        sequence += 1;
    }
    tx.send(AsrInput::Finish { next_sequence: sequence }).await
        .map_err(|_| lab_error("experiment_audio_closed", "ASR 已结束，未发送结束请求"))?;
    progress.lock().unwrap().finish_enqueued_ms = Some(ms(started));
    Ok(())
}
async fn recognize(run: &PreparedRun, config: &ProviderConfig, secret: &str) -> (Value, Option<SourceRevision>, Option<CloudError>) {
    let started = Instant::now();
    let progress = Arc::new(Mutex::new(Progress::default()));
    let configured = Arc::new(AtomicBool::new(false)); let notify = Arc::new(Notify::new());
    let callback_progress = progress.clone(); let callback_configured = configured.clone(); let callback_notify = notify.clone();
    let callback = Arc::new(move |event: AsrProgress| {
        let mut metrics = callback_progress.lock().unwrap();
        match event {
            AsrProgress::Configured => {
                metrics.configured_ms = Some(ms(started));
                callback_configured.store(true, Ordering::Release); callback_notify.notify_one();
            }
            AsrProgress::SpeechStart => metrics.speech_starts += 1,
            AsrProgress::Diagnostics(metadata) => metrics.wire = Some(metadata.relative_to(started)),
            AsrProgress::Caption(captions) => {
                metrics.caption_events += 1;
                if captions.iter().any(|item| !item.text.is_empty()) && metrics.first_caption_ms.is_none() { metrics.first_caption_ms = Some(ms(started)); }
                if captions.iter().any(|item| item.is_final && !item.text.is_empty()) && metrics.first_final_caption_ms.is_none() { metrics.first_final_caption_ms = Some(ms(started)); }
                for item in &captions {
                    let old = metrics.last_revisions.insert(item.id.clone(), item.revision).unwrap_or(0);
                    metrics.revision_changes += item.revision.saturating_sub(old);
                }
                metrics.latest_captions = captions;
            }
        }
    });
    let (tx, rx) = mpsc::channel(4); let cancel = CancellationToken::new();
    let attempt = Uuid::new_v4().to_string();
    let deadline = Duration::from_secs_f64(run.fixture.pcm.len() as f64 / 16_000.0 + 40.0);
    let job = async {
        let (items, ()) = tokio::try_join!(
            cloud::asr::run(config.clone(), secret.to_owned(), run.spec.language.clone(), run.dictionary.clone(),
                attempt.clone(), rx, cancel.clone(), callback),
            feed(run.fixture.pcm.clone(), tx, configured, notify, progress.clone(), started, run.spec.conditioning_profile)
        )?;
        Ok::<_, CloudError>(items)
    };
    let result = match tokio::time::timeout(deadline, job).await {
        Ok(result) => result,
        Err(_) => { cancel.cancel(); Err(lab_error("experiment_asr_timeout", "固定音频实验超过音频时长加 40 秒的包络期限")) }
    };
    let ended_ms = ms(started);
    let mut row = json!({"kind":"asr", "runId":run.spec.run_id, "fixtureId":run.spec.fixture_id,
        "fixtureSha256":run.fixture.sha256, "audioSamples":run.fixture.pcm.len(),
        "audioDurationMs":run.fixture.pcm.len() as u64 * 1000 / 16_000, "dictionaryTerms":run.spec.dictionary_terms,
        "language":run.spec.language, "conditioningProfile":run.spec.conditioning_profile,
        "elapsedMs":ended_ms, "automaticRetries":0 });
    let metrics = progress.lock().unwrap();
    row["progress"] = serde_json::to_value(&*metrics).expect("progress serializes");
    row["finishEnqueuedToAsrEndMs"] = metrics.finish_enqueued_ms.map(|time| ended_ms.saturating_sub(time)).map_or(Value::Null, |time| json!(time));
    drop(metrics);
    match result {
        Ok(items) => {
            let source = SourceRevision { id: Uuid::new_v4().to_string(), session_id: format!("voice-lab-{}", run.spec.run_id),
                asr_attempt_id: attempt, generation: 1, provider: config.provider.clone(), model: config.asr_model.clone(),
                adapter_version: config.adapter_version.clone(), complete: true, items, created_at: Utc::now().to_rfc3339() };
            row["status"] = json!("passed"); row["source"] = serde_json::to_value(&source).expect("source serializes");
            (row, Some(source), None)
        }
        Err(error) => { row["status"] = json!("failed"); row["error"] = safe_error(&error); (row, None, Some(error)) }
    }
}
fn safe_error(error: &CloudError) -> Value {
    // Production CloudError contains static public messages, never raw bodies or HTTP error text.
    json!({"code":error.code, "stage":error.stage, "message":error.message})
}
fn mode(mode: BaseMode) -> ModeSnapshot {
    let label = if mode == BaseMode::Light { "light" } else { "structure" };
    ModeSnapshot { mode_id: label.into(), name: label.into(), base_mode: mode, rules: String::new(), revision: 1 }
}
struct Evidence { receipt: File, journal: File }
impl Evidence {
    fn new(out: &Path) -> Result<Self, CloudError> {
        absolute(out)?;
        let resolved_parent = out.parent().and_then(|path| path.canonicalize().ok())
            .ok_or_else(|| lab_error("output_directory", "实验输出父目录必须预先存在"))?;
        let normal = resolved_parent.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
        if !normal.starts_with("//?/d:/codex_project/speakmode_dev/evidence/") && !normal.starts_with("d:/codex_project/speakmode_dev/evidence/") {
            return Err(lab_error("output_directory", "实验结果只能写入项目 evidence 目录"));
        }
        if out.extension().is_none_or(|extension| extension != "json") { return Err(lab_error("output_extension", "实验输出必须为 JSON 文件")); }
        let journal_path = out.with_extension("jsonl");
        if out.exists() || journal_path.exists() { return Err(lab_error("output_exists", "实验结果已存在，不能覆盖")); }
        let receipt = OpenOptions::new().write(true).create_new(true).open(out)
            .map_err(|_| lab_error("output_unavailable", "不能创建实验结果文件"))?;
        let journal = OpenOptions::new().write(true).create_new(true).open(journal_path)
            .map_err(|_| lab_error("output_unavailable", "不能创建实验增量记录"))?;
        Ok(Self { receipt, journal })
    }
    fn append(&mut self, row: &Value) -> Result<(), CloudError> {
        serde_json::to_writer(&mut self.journal, row).map_err(|_| lab_error("evidence_write_failed", "不能写入实验增量记录"))?;
        self.journal.write_all(b"\n").and_then(|_| self.journal.flush()).and_then(|_| self.journal.sync_data())
            .map_err(|_| lab_error("evidence_write_failed", "不能持久化实验增量记录"))
    }
    fn checkpoint(&mut self, receipt: &Value) -> Result<(), CloudError> {
        self.receipt.seek(SeekFrom::Start(0)).and_then(|_| self.receipt.set_len(0))
            .map_err(|_| lab_error("evidence_write_failed", "不能更新实验结果"))?;
        serde_json::to_writer_pretty(&mut self.receipt, receipt).map_err(|_| lab_error("evidence_write_failed", "不能写入实验结果"))?;
        self.receipt.flush().and_then(|_| self.receipt.sync_data()).map_err(|_| lab_error("evidence_write_failed", "不能持久化实验结果"))
    }
}
fn stop_reason(error: &CloudError, consecutive_transport: &mut u32) -> Option<Value> {
    if matches!(error.code.as_str(), "authentication_failed" | "rate_limited" | "asr_service_error" | "key_required" | "invalid_key") {
        return Some(json!({"code":error.code,"reason":"service_rejected_or_credentials_required"}));
    }
    if matches!(error.code.as_str(), "asr_connect_timeout" | "asr_connect_failed" | "asr_config_timeout" |
        "asr_disconnected" | "asr_send_timeout" | "asr_send_failed" | "asr_finish_timeout" |
        "rewrite_timeout" | "rewrite_network" | "experiment_asr_timeout") {
        *consecutive_transport += 1;
    } else { *consecutive_transport = 0; }
    (*consecutive_transport >= 3).then(|| json!({"code":"consecutive_transport_failures","count":*consecutive_transport}))
}
async fn execute(manifest: PreparedManifest, config: ProviderConfig, out: &Path) -> Result<(), CloudError> {
    // Validate every arm before credentials or network: a bad later arm must not spend a partial budget.
    let configurations = manifest.runs.iter().map(|run| config_for_run(&config, &run.spec))
        .collect::<Result<Vec<_>, _>>()?;
    let mut evidence = Evidence::new(out)?;
    let mut receipt = json!({"schemaVersion":1,"tool":"speakmode-voice-lab","appVersion":env!("CARGO_PKG_VERSION"),
        "startedAt":Utc::now().to_rfc3339(),"scope":manifest.scope,"configuration":config_metadata(&config),
        "manifestSha256":manifest.manifest_sha256,
        "preregisteredRuns":manifest.runs.iter().zip(&configurations).map(|(run, selected)|json!({"runId":run.spec.run_id,"fixtureId":run.spec.fixture_id,
            "fixtureSha256":run.fixture.sha256,"audioSamples":run.fixture.pcm.len(),"dictionaryTerms":run.spec.dictionary_terms,
            "configuration":config_metadata(selected),
            "language":run.spec.language,"baseModes":run.spec.base_modes,"conditioningProfile":run.spec.conditioning_profile})).collect::<Vec<_>>(),
        "totalPreregisteredAudioSamples":manifest.total_samples,"maximumAsrRuns":18,"maximumRewriteCalls":36,
        "sampleFormat":"PCM16 mono 16000Hz","automaticRetries":0,"status":"running","results":[],
        "timingNotes":["Audio is fed after Configured at no faster than its real duration; Tokio scheduling and backpressure may add delay.",
            "finishEnqueuedToAsrEndMs measures enqueueing Finish into the bounded provider queue to production ASR completion, not its network send timestamp.",
            "This exercise does not measure microphone capture, UI rendering, hotkey behavior, or cross-app insertion.",
            "Conditioning runs once on the original PCM16 fixture before padding; it cannot recover information lost before recording. Bypass preserves fixture PCM exactly.",
            "passed means the production request and its response contract succeeded; semantic and recognition accuracy must be scored separately."] });
    evidence.append(&json!({"kind":"header","metadata":receipt}))?; evidence.checkpoint(&receipt)?;
    let mut halted = None::<Value>; let mut consecutive_transport = 0u32;
    let secret = match cloud::credentials::read(&config) {
        Ok(secret) => Some(secret),
        Err(error) => { evidence.append(&json!({"kind":"credentialFailure","error":safe_error(&error)}))?;
            halted = Some(json!({"code":error.code,"reason":"credential_access_failed"})); None }
    };
    for (run, selected) in manifest.runs.iter().zip(&configurations) {
        if let Some(reason) = &halted {
            let row = json!({"kind":"asr","runId":run.spec.run_id,"fixtureId":run.spec.fixture_id,"status":"skipped","reason":reason});
            evidence.append(&row)?; receipt["results"].as_array_mut().unwrap().push(row);
            for base in &run.spec.base_modes {
                let row = json!({"kind":"rewrite","runId":run.spec.run_id,"fixtureId":run.spec.fixture_id,"baseMode":base,"status":"skipped","reason":reason});
                evidence.append(&row)?; receipt["results"].as_array_mut().unwrap().push(row);
            }
            evidence.checkpoint(&receipt)?; continue;
        }
        println!("run {}: fixed audio {} ms, dictionary {} terms", run.spec.run_id, run.fixture.pcm.len() as u64 * 1000 / 16000, run.dictionary.entries.len());
        let (mut row, source, error) = recognize(run, selected, secret.as_deref().unwrap()).await;
        row["configuration"] = config_metadata(selected);
        println!("run {}: ASR {}", run.spec.run_id, row["status"].as_str().unwrap_or("unknown"));
        evidence.append(&row)?; receipt["results"].as_array_mut().unwrap().push(row); evidence.checkpoint(&receipt)?;
        if let Some(error) = &error { halted = stop_reason(error, &mut consecutive_transport); }
        else { consecutive_transport = 0; }
        for base in &run.spec.base_modes {
            let mut row = json!({"kind":"rewrite","runId":run.spec.run_id,"fixtureId":run.spec.fixture_id,"baseMode":base,"configuration":config_metadata(selected),"automaticRetries":0});
            if source.is_none() || halted.is_some() {
                row["status"] = json!("skipped"); row["reason"] = halted.clone().unwrap_or_else(|| json!({"code":"asr_not_complete"}));
            } else {
                let started = Instant::now();
                match cloud::rewrite::rewrite(selected, secret.as_deref().unwrap(), &mode(*base), source.as_ref().unwrap(), CancellationToken::new()).await {
                    Ok(result) => { row["status"] = json!("passed"); row["output"] = serde_json::to_value(result).expect("rewrite serializes"); consecutive_transport = 0; }
                    Err(error) => { row["status"] = json!("failed"); row["error"] = safe_error(&error); halted = stop_reason(&error, &mut consecutive_transport); }
                }
                row["elapsedMs"] = json!(ms(started));
            }
            println!("run {}: rewrite {:?} {}", run.spec.run_id, base, row["status"].as_str().unwrap_or("unknown"));
            evidence.append(&row)?; receipt["results"].as_array_mut().unwrap().push(row); evidence.checkpoint(&receipt)?;
        }
    }
    receipt["finishedAt"] = json!(Utc::now().to_rfc3339());
    receipt["status"] = json!(if halted.is_some() { "halted" } else { "completed" });
    receipt["haltReason"] = halted.unwrap_or(Value::Null);
    let rows = receipt["results"].as_array().unwrap();
    let counts = json!({"passed":rows.iter().filter(|row|row["status"] == "passed").count(),
        "failed":rows.iter().filter(|row|row["status"] == "failed").count(),"skipped":rows.iter().filter(|row|row["status"] == "skipped").count()});
    receipt["counts"] = counts.clone(); evidence.append(&json!({"kind":"summary","status":receipt["status"],"counts":counts}))?;
    evidence.checkpoint(&receipt)?; println!("experiment {}: {}", receipt["status"].as_str().unwrap(), counts);
    Ok(())
}
fn argument(args: &[String], name: &str) -> Result<PathBuf, CloudError> {
    let positions = args.iter().enumerate().filter(|(_, value)| value.as_str() == name).map(|(index, _)| index).collect::<Vec<_>>();
    if positions.len() != 1 || positions[0] + 1 >= args.len() { return Err(lab_error("usage", "缺少或重复指定实验 CLI 参数")); }
    Ok(PathBuf::from(&args[positions[0] + 1]))
}
#[tokio::main]
async fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let result = async {
        if args.first().is_some_and(|value| value == "--inspect-config") && args.len() == 3 && args[1] == "--config-db" {
            let config = config(&PathBuf::from(&args[2]))?; println!("{}", config_metadata(&config)); return Ok(());
        }
        if args.len() != 7 || args.first().is_none_or(|value| value != "cloud") {
            return Err(lab_error("usage", "使用 cloud --manifest ABS --config-db ABS --out ABS，或 --inspect-config --config-db ABS"));
        }
        let manifest_path = argument(&args, "--manifest")?;
        let config_path = argument(&args, "--config-db")?; let out = argument(&args, "--out")?;
        let manifest = prepare(&manifest_path)?; let config = config(&config_path)?;
        execute(manifest, config, &out).await
    }.await;
    if let Err(error) = result { eprintln!("{}", safe_error(&error)); std::process::exit(1); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conditioning_profiles_are_explicit_and_existing_manifests_remain_bypass() {
        let mut value = json!({"runId":"condition-01","fixtureId":"same-audio",
            "wavPath":"D:/synthetic.wav","dictionaryTerms":[],"language":"auto","baseModes":[]});
        assert_eq!(serde_json::from_value::<RunSpec>(value.clone()).unwrap().conditioning_profile, ConditioningProfile::Bypass);
        for (name, expected) in [("bypass", ConditioningProfile::Bypass), ("softVoice", ConditioningProfile::SoftVoice),
            ("fixedGain6Db", ConditioningProfile::FixedGain6Db)] {
            value["conditioningProfile"] = json!(name);
            assert_eq!(serde_json::from_value::<RunSpec>(value.clone()).unwrap().conditioning_profile, expected);
        }
        value["conditioningProfile"] = json!("unknown");
        assert!(serde_json::from_value::<RunSpec>(value).is_err());
    }
    #[test]
    fn baseline_keeps_all_pcm16_values_exact_and_candidates_do_not_mutate_fixtures() {
        let fixture = (i16::MIN..=i16::MAX).collect::<Vec<_>>();
        let original = fixture.clone();
        let mut baseline = AudioConditioner::new(ConditioningMode::Bypass);
        for chunk in fixture.chunks(FRAME_SAMPLES) {
            let frame = condition_frame(chunk, ConditioningProfile::Bypass, &mut baseline);
            assert_eq!(&frame[..chunk.len()], chunk);
            assert!(frame[chunk.len()..].iter().all(|sample| *sample == 0));
        }
        let mut candidate = AudioConditioner::new(ConditioningMode::FixedGain6Db);
        let frame = condition_frame(&[512; 101], ConditioningProfile::FixedGain6Db, &mut candidate);
        assert!(frame[..101].iter().any(|sample| *sample > 512));
        assert!(frame[101..].iter().all(|sample| *sample == 0));
        assert_eq!(candidate.summary().real_samples, 101, "padding must not enter the conditioner");
        assert_eq!(fixture, original);
    }
    #[test]
    fn per_run_models_keep_scope_and_default_configuration_frozen() {
        let mut base = ProviderConfig::default();
        base.region=Some("beijing".into()); base.workspace=Some("owned-lab".into());
        let frozen=base.clone();
        let spec:RunSpec=serde_json::from_value(json!({"runId":"candidate-01","fixtureId":"same-audio",
            "wavPath":"D:/synthetic.wav","dictionaryTerms":[],"language":"auto","baseModes":[],
            "asrModel":"qwen3-asr-flash-realtime-2026-02-10","rewriteModel":"qwen-plus"})).unwrap();
        let candidate=config_for_run(&base,&spec).unwrap();
        assert_eq!(candidate.asr_model,"qwen3-asr-flash-realtime-2026-02-10");
        assert_eq!(candidate.rewrite_model,"qwen-plus");
        assert_eq!(candidate.region,base.region); assert_eq!(candidate.workspace,base.workspace);
        assert_eq!(candidate.adapter_version,"qwen-realtime-v1");
        assert_eq!(base,frozen,"an experiment must not mutate the user's default configuration");
    }
    #[test]
    fn model_arms_derive_adapter_and_reject_invalid_overrides_before_execution() {
        let mut base = ProviderConfig::default(); base.region=Some("beijing".into());
        let mut spec:RunSpec=serde_json::from_value(json!({"runId":"candidate-02","fixtureId":"same-audio",
            "wavPath":"D:/synthetic.wav","dictionaryTerms":[],"language":"auto","baseModes":[]})).unwrap();
        assert_eq!(config_for_run(&base,&spec).unwrap(),base);
        spec.asr_model=Some("qwen-audio-3.1-asr-flash-streaming".into());
        assert_eq!(config_for_run(&base,&spec).unwrap().adapter_version,"qwen-task-streaming-v1");
        spec.asr_model=Some("unknown-model".into());
        assert_eq!(config_for_run(&base,&spec).unwrap_err().code,"unsupported_asr_model");
        spec.asr_model=Some("fun-asr-realtime".into());
        assert_eq!(config_for_run(&base,&spec).unwrap_err().code,"asr_adapter_not_implemented");
        spec.asr_model=None; spec.rewrite_model=Some("unknown-text-model".into());
        assert_eq!(config_for_run(&base,&spec).unwrap_err().code,"unsupported_rewrite_model");
    }
    #[test]
    fn dictionary_context_matches_production_capacity_and_unicode_identity() {
        assert!(dictionary(&["SpeakMode".into(), "ＳＰＥＡＫＭＯＤＥ".into()]).is_err());
        assert!(dictionary(&["test\u{200b}".into()]).is_err());
        assert!(dictionary(&vec!["汉".repeat(80); 201]).is_err());
        let terms = (0..30).map(|index| format!("{index}{}", "汉".repeat(70))).collect::<Vec<_>>();
        assert_eq!(dictionary(&terms).unwrap_err().code, "dictionary_context_limit");
        assert!(dictionary(&["SpeakMode".into(), "百炼".into()]).unwrap().asr_context().unwrap().contains("SpeakMode"));
    }
    #[test]
    fn transport_stop_requires_three_consecutive_failures_and_service_rejection_stops_now() {
        let mut count = 0;
        let network = CloudError::new("rewrite_network", "rewrite", "safe");
        assert!(stop_reason(&network, &mut count).is_none());
        assert!(stop_reason(&network, &mut count).is_none());
        assert!(stop_reason(&network, &mut count).is_some());
        let mut count = 0;
        assert!(stop_reason(&CloudError::new("authentication_failed", "rewrite", "safe"), &mut count).is_some());
    }
    #[tokio::test(start_paused = true)]
    async fn fixed_audio_waits_for_configured_is_not_accelerated_and_preserves_padded_tail() {
        let pcm = Arc::new((0..1701).map(|index| (index - 850) as i16).collect::<Vec<_>>());
        let (tx, mut rx) = mpsc::channel(4);
        let configured = Arc::new(AtomicBool::new(false)); let notify = Arc::new(Notify::new());
        let progress = Arc::new(Mutex::new(Progress::default()));
        let feeder = tokio::spawn(feed(pcm.clone(), tx, configured.clone(), notify.clone(), progress.clone(), Instant::now(), ConditioningProfile::Bypass));
        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_err());
        configured.store(true, Ordering::Release); notify.notify_one();
        tokio::task::yield_now().await;
        let origin = tokio::time::Instant::now();
        let first = rx.recv().await.unwrap();
        assert!(origin.elapsed() >= Duration::from_millis(100));
        match first { AsrInput::Frame { sequence, pcm16, padding_samples } => { assert_eq!(sequence, 0); assert_eq!(pcm16, pcm[..1600]); assert_eq!(padding_samples, 0); }, _ => panic!("expected frame") }
        let tail = rx.recv().await.unwrap();
        assert!(origin.elapsed() >= Duration::from_micros(106_312));
        match tail { AsrInput::Frame { sequence, pcm16, padding_samples } => { assert_eq!(sequence, 1); assert_eq!(&pcm16[..101], &pcm[1600..]); assert_eq!(padding_samples, 1499); assert!(pcm16[101..].iter().all(|sample| *sample == 0)); }, _ => panic!("expected tail") }
        assert!(matches!(rx.recv().await.unwrap(), AsrInput::Finish { next_sequence: 2 }));
        feeder.await.unwrap().unwrap();
        let metrics = progress.lock().unwrap(); assert_eq!(metrics.sent_real_samples, 1701); assert_eq!(metrics.sent_frames, 2);
        assert_eq!(metrics.conditioning.as_ref().unwrap()["realSamples"], json!(1701));
    }
}
