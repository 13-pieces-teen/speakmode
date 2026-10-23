//! Finite natural-cleanup text experiment. Dev 12x2, holdout 8x2, private user 1x2.
//! Same qwen3.8-flash model in both arms, one trial, no retries.
//! No history/audio reads, configuration writes, clipboard or native input.
#![recursion_limit = "256"]

#[path="lab_provider_scope/mod.rs"] mod lab_provider_scope;
use chrono::Utc;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use speakmode_lib::{
    cloud::{self, CloudError},
    core::{BaseMode, DictionaryEntry, DictionarySnapshot, ModeSnapshot, ProviderConfig, SourceItem, SourceRevision},
};
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const DEV: &str = include_str!("../../../docs/experiments/fixtures/natural-cleanup-v1.dev.json");
const HOLDOUT: &str = include_str!("../../../docs/experiments/fixtures/natural-cleanup-v1.holdout.json");
const TOOL: &str = "speakmode-natural-quality-lab";
const HELP: &str = "natural_quality_lab --config-db ABS --out ABS_NEW.json [--split dev|holdout|user] [--fixture ABS_PRIVATE.json] (--plan-only|--run)\nDev: 24 calls. Holdout: 16 calls. User: 2 calls, requires a separate private fixture. Both arms use qwen3.8-flash in memory; no retries or extra trials. Plan-only does not read credentials or use the network. No history/audio/dictionary reads, configuration writes, clipboard access or native delivery. Freeze before holdout/private probe. This is text cleanup, not ASR or Typeless measurement.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Split { Dev, Holdout, User }
impl Split {
    fn name(self) -> &'static str { match self { Self::Dev => "dev", Self::Holdout => "holdout", Self::User => "user" } }
    fn fixture_name(self) -> &'static str { match self { Self::Dev => "dev", Self::Holdout => "holdout_reserved", Self::User => "user_provided" } }
    fn case_count(self) -> usize { match self { Self::Dev => 12, Self::Holdout => 8, Self::User => 1 } }
    fn trials(self) -> usize { 1 }
    fn maximum_calls(self) -> usize { self.case_count() * 2 }
    fn source_scope(self) -> &'static str { match self { Self::User => "USER_PROVIDED_TRANSCRIPT_NO_AUDIO", _ => "SYNTHETIC_TRANSCRIPT_NO_AUDIO" } }
}

fn error(code: &str, message: &str) -> CloudError { CloudError::new(code, "natural_quality_lab", message) }
fn hash(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn safe(failure: &CloudError) -> Value { json!({"code":failure.code,"stage":failure.stage,"message":failure.message}) }

fn read_config(path: &Path) -> Result<ProviderConfig, CloudError> {
    if !path.is_absolute() { return Err(error("absolute_path_required", "配置路径必须为绝对路径")); }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_| error("config_unavailable", "不能只读访问云端配置"))?;
    // Deliberately the only query: never read personal history or dictionary.
    let text: String = connection.query_row("SELECT data FROM provider_config WHERE singleton=1", [], |row| row.get(0))
        .map_err(|_| error("config_unavailable", "没有有效云端配置"))?;
    if text.len() > 16 * 1024 { return Err(error("config_invalid", "云端配置超过容量限制")); }
    let config: ProviderConfig = serde_json::from_str(&text).map_err(|_| error("config_invalid", "云端配置格式无效"))?;
    // Existing endpoint validation permits only the configured official provider.
    lab_provider_scope::rewrite_endpoint(&config)?;
    Ok(config)
}

fn dictionary() -> DictionarySnapshot {
    DictionarySnapshot { version: 1, entries: ["Typeless", "SpeakMode", "GitHub", "Intel"].into_iter().enumerate().map(|(index, term)| DictionaryEntry {
        id: format!("synthetic-term-{index}"), term: term.into(),
        created_at: "2026-10-10T00:00:00Z".into(), updated_at: "2026-10-10T00:00:00Z".into(),
    }).collect() }
}
fn mode() -> ModeSnapshot {
    ModeSnapshot { mode_id: "lab-auto".into(), name: "Auto".into(), base_mode: BaseMode::Auto, rules: String::new(), revision: 1 }
}
fn fixture(split: Split, private_path: Option<&Path>) -> Result<(Value, String), CloudError> {
    let text = match (split, private_path) {
        (Split::Dev, None) => DEV.to_owned(),
        (Split::Holdout, None) => HOLDOUT.to_owned(),
        (Split::User, Some(path)) if path.is_absolute() && path.extension().is_some_and(|extension| extension == "json") => {
            let metadata = std::fs::metadata(path).map_err(|_| error("fixture_unavailable", "不能读取私有案例"))?;
            if !metadata.is_file() || metadata.len() > 64 * 1024 { return Err(error("fixture_invalid", "私有案例必须为64KiB以内的JSON文件")); }
            let mut bytes = Vec::new();
            File::open(path).map_err(|_| error("fixture_unavailable", "不能读取私有案例"))?
                .take(64 * 1024 + 1).read_to_end(&mut bytes).map_err(|_| error("fixture_unavailable", "不能读取私有案例"))?;
            if bytes.len() > 64 * 1024 { return Err(error("fixture_invalid", "私有案例超过容量限制")); }
            String::from_utf8(bytes).map_err(|_|error("fixture_invalid", "私有案例必须使用UTF-8"))?
        }
        _ => return Err(error("fixture_invalid", "仅user分组可指定私有案例文件")),
    };
    let digest = hash(text.as_bytes());
    let mut value: Value = serde_json::from_str(&text).map_err(|_|error("fixture_invalid", "固定语料格式无效"))?;
    if !value.is_object() { return Err(error("fixture_invalid", "固定语料必须为JSON对象")); }
    value["labSplit"] = json!(split.name());
    value["status"] = json!(split.source_scope());
    Ok((value, digest))
}

fn source(case: &Value) -> Result<SourceRevision, CloudError> {
    let id = case["id"].as_str().filter(|id| !id.is_empty() && id.len() <= 80 && id.bytes().all(|byte|byte.is_ascii_alphanumeric() || matches!(byte,b'-'|b'_')))
        .ok_or_else(|| error("fixture_invalid", "固定案例编号无效"))?;
    let text = case["spokenScript"].as_str().filter(|text| !text.trim().is_empty() && text.chars().count() <= 2_000)
        .ok_or_else(|| error("fixture_invalid", "固定来源文本无效"))?;
    // Synthetic fragments only: preserve every original character, including
    // punctuation/whitespace. Never silently truncate to the 16-fragment limit.
    let fragments = text.split_inclusive(|character| matches!(character, '。' | '！' | '？' | '；')).collect::<Vec<_>>();
    if fragments.is_empty() || fragments.len() > 16 {
        return Err(error("fixture_invalid", "固定来源分片必须为1至16片，不能截断"));
    }
    let items = fragments.into_iter().enumerate().map(|(index, fragment)| SourceItem {
        id: format!("{id}:{index}"), provider_item_id: index.to_string(), capture_order: index as u64,
        text: fragment.into(), is_final: true, start_ms: None, end_ms: None,
    }).collect();
    Ok(SourceRevision {
        id: format!("source-{id}"), session_id: format!("synthetic-{id}"), asr_attempt_id: format!("fixed-{id}"), generation: 1,
        provider: "synthetic".into(), model: "fixed-text".into(), adapter_version: "natural-cleanup-v1".into(),
        complete: true, created_at: "2026-10-10T00:00:00Z".into(),
        items,
    })
}
struct PlannedCall { case_id: String, arm: &'static str, trial: usize, mode: ModeSnapshot, source: SourceRevision, body: Value }
fn plan(fixture: &Value, split: Split, config: &ProviderConfig) -> Result<Vec<PlannedCall>, CloudError> {
    // Equal-model comparison: neither arm inherits a potentially older saved
    // model. The configured value remains recorded and is never overwritten.
    let mut experiment_config = config.clone();
    experiment_config.rewrite_model = "qwen3.8-flash".into();
    lab_provider_scope::rewrite_endpoint(&experiment_config)?;
    if split != Split::User && *fixture != self::fixture(split, None)?.0 {
        return Err(error("fixture_invalid", "不能替换冻结语料"));
    }
    let cases = fixture["cases"].as_array().filter(|cases| cases.len() == split.case_count())
        .ok_or_else(|| error("fixture_invalid", "固定案例数不符"))?;
    let mut ids = HashSet::new();
    for case in cases {
        let id = case["id"].as_str().ok_or_else(|| error("fixture_invalid", "案例没有编号"))?;
        if !ids.insert(id) || case["split"] != split.fixture_name() { return Err(error("fixture_invalid", "案例编号重复或分组不符")); }
        for field in ["mustPreserve", "allowedCleanup", "mustNotDo"] {
            if case[field].as_array().is_none_or(|rows| rows.is_empty() || rows.iter().any(|row| row.as_str().is_none())) {
                return Err(error("fixture_invalid", "案例缺少语义检查清单"));
            }
        }
    }
    let mode = mode();
    let dictionary = dictionary();
    let mut calls = Vec::with_capacity(split.maximum_calls());
    for trial in 0..split.trials() {
        for (case_index, case) in cases.iter().enumerate() {
            let source = source(case)?;
            let arms = if (case_index + trial) % 2 == 0 { ["baseline", "candidate"] } else { ["candidate", "baseline"] };
            for arm in arms {
                let body = if arm == "baseline" {
                    cloud::rewrite::baseline_request_body(&experiment_config, &mode, &source)?
                } else {
                    cloud::rewrite::natural_request_body(&experiment_config, &mode, &source, &dictionary)?
                };
                calls.push(PlannedCall { case_id: case["id"].as_str().unwrap().into(), arm, trial: trial + 1,
                    mode: mode.clone(), source: source.clone(), body });
            }
        }
    }
    if calls.len() != split.maximum_calls() { return Err(error("plan_limit", "固定调用数不符")); }
    Ok(calls)
}

struct Evidence { receipt: File, journal: File }
impl Evidence {
    fn new(path: &Path) -> Result<Self, CloudError> {
        if !path.is_absolute() || path.extension().is_none_or(|extension| extension != "json") || path.exists() || path.with_extension("jsonl").exists() {
            return Err(error("fresh_output_required", "输出必须为不存在的绝对 JSON 路径"));
        }
        let receipt = OpenOptions::new().create_new(true).write(true).open(path).map_err(|_| error("evidence_unavailable", "不能新建实验结果"))?;
        let journal = OpenOptions::new().create_new(true).write(true).open(path.with_extension("jsonl")).map_err(|_| error("evidence_unavailable", "不能新建增量日志"))?;
        Ok(Self { receipt, journal })
    }
    fn write(&mut self, row: &Value, receipt: &Value) -> Result<(), CloudError> {
        serde_json::to_writer(&mut self.journal, row).map_err(|_| error("evidence_write_failed", "不能写入增量日志"))?;
        self.journal.write_all(b"\n").and_then(|_| self.journal.flush()).and_then(|_| self.journal.sync_data())
            .map_err(|_| error("evidence_write_failed", "不能持久化增量日志"))?;
        self.receipt.seek(SeekFrom::Start(0)).and_then(|_| self.receipt.set_len(0)).map_err(|_| error("evidence_write_failed", "不能更新结果"))?;
        serde_json::to_writer_pretty(&mut self.receipt, receipt).map_err(|_| error("evidence_write_failed", "不能写入结果"))?;
        self.receipt.flush().and_then(|_| self.receipt.sync_data()).map_err(|_| error("evidence_write_failed", "不能持久化结果"))
    }
}

async fn request(client: &reqwest::Client, config: &ProviderConfig, secret: &str, body: &Value) -> Result<Value, CloudError> {
    let operation = async {
        let mut request = client.post(lab_provider_scope::rewrite_endpoint(config)?).bearer_auth(secret).json(body).timeout(Duration::from_secs(20));
        if let Some(workspace) = config.workspace.as_deref().filter(|workspace| !workspace.is_empty()) {
            request = request.header("X-DashScope-WorkSpace", workspace);
        }
        let mut response = request.send().await.map_err(|failure| error(if failure.is_timeout() { "rewrite_timeout" } else { "rewrite_network" }, "固定文本实验请求未完成"))?;
        if !response.status().is_success() {
            return Err(error(match response.status().as_u16() { 401 | 403 => "authentication_failed", 429 => "rate_limited", _ => "rewrite_service_error" }, "整理服务未接受固定文本实验请求"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|failure| error(if failure.is_timeout() { "rewrite_timeout" } else { "rewrite_network" }, "实验响应中断"))? {
            if bytes.len() + chunk.len() > 1024 * 1024 { return Err(error("response_too_large", "实验响应超过容量限制")); }
            bytes.extend_from_slice(&chunk);
        }
        let data: Value = serde_json::from_slice(&bytes).map_err(|_| error("invalid_response", "实验响应格式无效"))?;
        let content = data.pointer("/choices/0/message/content").and_then(Value::as_str)
            .ok_or_else(|| error("empty_output", "实验没有返回文本"))?;
        if content.len() > 256 * 1024 { return Err(error("response_too_large", "实验成稿超过保留上限")); }
        Ok(json!({"modelContent":content,"finishReason":data.pointer("/choices/0/finish_reason").and_then(Value::as_str),
            "modelReported":data.get("model").and_then(Value::as_str).map(|value|value.chars().take(256).collect::<String>()),
            "responseId":data.get("id").and_then(Value::as_str).map(|value|value.chars().take(256).collect::<String>()),
            "usage":{"promptTokens":data.pointer("/usage/prompt_tokens").and_then(Value::as_u64),
                "completionTokens":data.pointer("/usage/completion_tokens").and_then(Value::as_u64),
                "totalTokens":data.pointer("/usage/total_tokens").and_then(Value::as_u64)}}))
    };
    tokio::time::timeout(Duration::from_secs(20), operation).await.map_err(|_| error("rewrite_timeout", "固定文本实验超过20秒期限"))?
}
fn unvalidated_text(content: &str) -> Option<String> {
    let value: Value = serde_json::from_str(content).ok()?;
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return (!text.trim().is_empty()).then(||text.to_owned());
    }
    let texts = value["blocks"].as_array()?.iter().map(|block| block["text"].as_str()).collect::<Option<Vec<_>>>()?;
    (!texts.is_empty()).then(|| texts.join("\n\n"))
}
fn parse(arm: &str, content: &str, mode: &ModeSnapshot, source: &SourceRevision) -> Result<cloud::rewrite::RewriteResult, CloudError> {
    match arm {
        "baseline" => cloud::rewrite::parse_model_output(content, mode, source),
        "candidate" => cloud::rewrite::parse_natural_output(content, mode, source),
        _ => Err(error("unknown_arm", "未知实验组")),
    }
}
fn projected_fallback_reason(code: &str) -> Option<&'static str> {
    match code {
        "rewrite_timeout" => Some("timeout"),
        "rewrite_network" | "network_setup" | "rewrite_service_error" | "authentication_failed" | "rate_limited" => Some("serviceUnavailable"),
        "invalid_response" | "invalid_output" | "empty_output" | "response_too_large" | "truncated_output" | "invalid_source_mapping" | "incomplete_source_coverage" => Some("invalidOutput"),
        "rewrite_meaning_risk" => Some("meaningRisk"),
        _ => None,
    }
}
fn projection(source: &SourceRevision, row: &Value) -> Value {
    // This is an explicitly counterfactual application-policy projection. The lab
    // never creates a real session, acquires a target or verifies clipboard input.
    let deadline_ms = if source.text().chars().count() <= 800 { 5_000 } else { 20_000 };
    let mut result = json!({"scope":"SIMULATED_CURRENT_INITIAL_BUDGET","actualNativeDelivery":"NOT_RUN",
        "assumptions":["normalInitial","completeSource","durableCheckpoint","matchingCurrentIdentity","notCancelled","eligibleTarget"],
        "applicationDeadlineMs":deadline_ms,"status":"not_projected","text":null,
        "semanticVerdict":"UNREVIEWED","note":"Cloud lab timeout is 20 seconds. App deadline projection does not measure native paste or real ASR. Original request failure/status remains unchanged."});
    if !source.complete || row["status"] == "skipped" { return result; }
    let error_code = row["validationError"]["code"].as_str().or_else(|| row["error"]["code"].as_str());
    if error_code.is_some_and(|code| projected_fallback_reason(code).is_none()) { return result; }
    if row["elapsedMs"].as_u64().is_some_and(|elapsed| elapsed >= deadline_ms) {
        result["status"] = json!("source_fallback");
        result["reason"] = json!("timeout");
        result["text"] = json!(source.text());
    } else if row["status"] == "parsed" {
        result["status"] = json!("rewrite_candidate");
        result["text"] = row["validatedText"].clone();
    } else if let Some(reason) = error_code.and_then(projected_fallback_reason) {
        result["status"] = json!("source_fallback");
        result["reason"] = json!(reason);
        result["text"] = json!(source.text());
    }
    result
}

async fn run(config_path: &Path, out: &Path, split: Split, plan_only: bool, private_fixture: Option<&Path>) -> Result<(), CloudError> {
    let (fixture, fixture_digest) = fixture(split, private_fixture)?;
    let config = read_config(config_path)?;
    let calls = plan(&fixture, split, &config)?;
    let planned = calls.iter().enumerate().map(|(index, call)| json!({
        "index":index,"caseId":call.case_id,"arm":call.arm,"trial":call.trial,"mode":call.mode,
        "source":call.source,"sourceSha256":hash(&serde_json::to_vec(&call.source).unwrap()),
        "requestBody":call.body,"requestSha256":hash(&serde_json::to_vec(&call.body).unwrap()),
        "systemSha256":hash(call.body["messages"][0]["content"].as_str().unwrap().as_bytes()),
        "parser":if call.arm=="baseline"{"parse_model_output"}else{"parse_natural_output"}
    })).collect::<Vec<_>>();
    let mut receipt = json!({"schemaVersion":1,"tool":TOOL,"appVersion":env!("CARGO_PKG_VERSION"),
        "scope":"FIXED_TEXT_COMBINED_CANDIDATE_NOT_HUMAN_AUDIO_ASR_OR_TYPELESS_COMPARISON",
        "sourceScope":split.source_scope(),
        "split":split.name(),"maximumCalls":split.maximum_calls(),"caseCount":split.case_count(),"trials":split.trials(),
        "automaticRetries":0,"perRequestTimeoutSeconds":20,"runnerSha256":hash(include_bytes!("natural_quality_lab.rs")),
        "fixtureSha256":hash(&serde_json::to_vec(&fixture).unwrap()),"fixtureFileSha256":fixture_digest,
        "baselineSha256":hash(include_bytes!("../src/cloud/rewrite_baseline.rs")),
        "candidateSha256":hash(include_bytes!("../src/cloud/rewrite_natural.rs")),
        "dictionaryPolicySha256":hash(include_bytes!("../src/cloud/rewrite_terms.rs")),
        "parserSha256":hash(include_bytes!("../src/cloud/rewrite.rs")),
        "integritySha256":hash(include_bytes!("../src/cloud/rewrite_integrity.rs")),
        "factsSha256":hash(include_bytes!("../src/cloud/rewrite_facts.rs")),
        "semanticsSha256":hash(include_bytes!("../src/cloud/rewrite_semantics.rs")),
        "disfluencySha256":hash(include_bytes!("../src/cloud/rewrite_disfluency.rs")),
        "fallbackPolicySha256":hash(include_bytes!("../src/engine/rewrite_fallback.rs")),
        "baselinePromptVersion":"mode-contract-v1","candidatePromptVersion":cloud::rewrite::NATURAL_REWRITE_PROMPT_VERSION,
        "candidateChangeScope":["natural cleanup prompt","whole text input and output contract","frozen synthetic dictionary context","candidate parser"],
        "causalLimitation":"Combined candidate: differences cannot be attributed to prompt, dictionary or parser alone.",
        "dictionary":dictionary(),"baselineDictionarySupplied":false,
        "orderRule":"Trial then frozen case order; adjacent AB/BA pairs alternate with (case_index + zero_based_trial) parity.",
        "configuration":{"region":config.region,"configuredModel":config.rewrite_model},
        "experimentalOverride":{"baseline":"qwen3.8-flash","candidate":"qwen3.8-flash","persisted":false},
        "configurationWritten":false,"historyRead":false,"personalDictionaryRead":false,"humanQualityMeasured":false,
        "nativeDelivery":"NOT_RUN","semanticReview":"UNREVIEWED","reviewChecklists":fixture["cases"],
        "reviewPolicy":"Review generated text and projected final text separately against mustPreserve/allowedCleanup/mustNotDo. Parser PASS is not semantic PASS; punctuation and exact-case identifiers need separate review.",
        "plan":planned,"results":[],"startedAt":Utc::now().to_rfc3339(),"status":if plan_only{"planned_only"}else{"running"}});
    receipt["segmentation"] = json!({"kind":"synthetic_not_observed_asr","strategy":"split_inclusive at Chinese 。！？；, preserving every character and delimiter; at most 16 fragments, reject rather than truncate; same source for both arms","sourceTextJoin":"newline_between_fragments","originalScriptRecovery":"concatenate fragment text without separators"});
    let mut evidence = Evidence::new(out)?;
    // Durable preregistration before any credential access or network request.
    evidence.write(&json!({"kind":"preregistration","receipt":receipt}), &receipt)?;
    if plan_only { println!("planned_only: {} {} calls; no credential access or network", split.name(), split.maximum_calls()); return Ok(()); }
    let secret = match cloud::credentials::read_for(&config, cloud::credentials::CredentialStage::Rewrite) {
        Ok(secret) => secret,
        Err(failure) => {
            receipt["status"] = json!("halted"); receipt["error"] = safe(&failure);
            evidence.write(&json!({"kind":"credentialFailure","error":safe(&failure)}), &receipt)?;
            return Err(error("credential_unavailable", "未发送请求；凭据不可用，已保留实验记录"));
        }
    };
    let client = match reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(2).pool_idle_timeout(Duration::from_secs(60)).build()
    {
        Ok(client) => client,
        Err(_) => {
            let failure = error("network_setup", "无法建立实验请求客户端");
            receipt["status"] = json!("halted"); receipt["error"] = safe(&failure);
            evidence.write(&json!({"kind":"networkSetupFailure","error":safe(&failure)}), &receipt)?;
            return Err(failure);
        }
    };
    let mut halted: Option<String> = None;
    let mut network_failures = 0;
    for (index, call) in calls.into_iter().enumerate() {
        let mut row = json!({"kind":"naturalCleanupText","index":index,"caseId":call.case_id,"arm":call.arm,
            "trial":call.trial,"mode":call.mode.base_mode,"semanticVerdict":"UNREVIEWED",
            "sourceScope":split.source_scope(),"sourceText":call.source.text(),"sourceTextSha256":hash(call.source.text().as_bytes())});
        if let Some(reason) = &halted {
            row["status"] = json!("skipped"); row["reason"] = json!(reason);
        } else {
            receipt["pendingCall"] = json!({"index":index,"caseId":call.case_id,"arm":call.arm,"trial":call.trial});
            evidence.write(&json!({"kind":"callStarted","index":index,"caseId":call.case_id,"arm":call.arm,"trial":call.trial}), &receipt)?;
            let started = Instant::now();
            let response = request(&client, &config, &secret, &call.body).await;
            row["cloudElapsedMs"] = json!(started.elapsed().as_millis() as u64);
            match response {
                Ok(response) => {
                    row["response"] = response;
                    let content = row["response"]["modelContent"].as_str().unwrap().to_owned();
                    row["unvalidatedText"] = json!(unvalidated_text(&content));
                    let parser_started = Instant::now();
                    if row["response"]["finishReason"] != "stop" {
                        row["status"] = json!("truncated"); row["parser"] = json!("not_run");
                        row["validationError"] = safe(&error("truncated_output", "模型未完整结束"));
                    } else {
                        match parse(call.arm, &content, &call.mode, &call.source) {
                            Ok(parsed) => {
                                row["status"] = json!("parsed"); row["parser"] = json!("passed");
                                row["validatedText"] = json!(parsed.blocks.iter().map(|block| block.text.as_str()).collect::<Vec<_>>().join("\n\n"));
                                row["validatedOutput"] = serde_json::to_value(parsed).unwrap();
                            }
                            Err(failure) => {
                                row["status"] = json!("rejected"); row["parser"] = json!("failed"); row["validationError"] = safe(&failure);
                            }
                        }
                    }
                    row["parserElapsedMicros"] = json!(parser_started.elapsed().as_micros() as u64);
                }
                Err(failure) => {
                    if matches!(failure.code.as_str(), "authentication_failed" | "rate_limited") { halted = Some(failure.code.clone()); }
                    if matches!(failure.code.as_str(), "rewrite_network" | "rewrite_timeout" | "rewrite_service_error") {
                        network_failures += 1;
                        if network_failures >= 3 { halted = Some("three_network_or_service_failures".into()); }
                    }
                    row["status"] = json!("failed"); row["parser"] = json!("not_run"); row["error"] = safe(&failure);
                }
            }
            row["elapsedMs"] = json!(started.elapsed().as_millis() as u64);
        }
        row["projectedDelivery"] = projection(&call.source, &row);
        println!("{} {} trial {} {}: {}", split.name(), call.case_id, call.trial, call.arm, row["status"].as_str().unwrap());
        receipt.as_object_mut().unwrap().remove("pendingCall");
        receipt["results"].as_array_mut().unwrap().push(row.clone());
        evidence.write(&row, &receipt)?;
    }
    receipt["status"] = json!(if halted.is_some() { "halted" } else { "completed" });
    receipt["haltReason"] = json!(halted);
    receipt["finishedAt"] = json!(Utc::now().to_rfc3339());
    evidence.write(&json!({"kind":"summary","status":receipt["status"],"semanticReview":"UNREVIEWED"}), &receipt)
}

struct Options { config: PathBuf, out: PathBuf, split: Split, plan_only: bool, private_fixture: Option<PathBuf> }
fn options(args: &[String]) -> Result<Options, CloudError> {
    if args.len() < 5 || args[0] != "--config-db" || args[2] != "--out" { return Err(error("usage", HELP)); }
    let mut tail = &args[4..];
    let split = if tail.first().is_some_and(|argument| argument == "--split") {
        let split = match tail.get(1).map(String::as_str) { Some("dev") => Split::Dev, Some("holdout") => Split::Holdout, Some("user") => Split::User, _ => return Err(error("usage", HELP)) };
        tail = &tail[2..]; split
    } else { Split::Dev };
    let private_fixture = if tail.first().is_some_and(|argument|argument == "--fixture") {
        let path = PathBuf::from(tail.get(1).ok_or_else(||error("usage", HELP))?);
        if split != Split::User || !path.is_absolute() { return Err(error("usage", HELP)); }
        tail = &tail[2..]; Some(path)
    } else { None };
    if split == Split::User && private_fixture.is_none() { return Err(error("usage", HELP)); }
    let plan_only = match tail { [argument] if argument == "--plan-only" => true, [argument] if argument == "--run" => false, _ => return Err(error("usage", HELP)) };
    let config = PathBuf::from(&args[1]); let out = PathBuf::from(&args[3]);
    if !config.is_absolute() || !out.is_absolute() { return Err(error("absolute_path_required", "配置和输出都必须为绝对路径")); }
    Ok(Options { config, out, split, plan_only, private_fixture })
}
#[tokio::main]
async fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args == ["--help"] { println!("{HELP}"); return; }
    let result = async {
        let options = options(&args)?;
        run(&options.config, &options.out, options.split, options.plan_only, options.private_fixture.as_deref()).await
    }.await;
    if let Err(failure) = result { eprintln!("{}", safe(&failure)); std::process::exit(1); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private_fixture() -> Value {
        json!({"cases":[{"id":"private-test", "split":"user_provided", "spokenScript":"嗯，测试麦克风。呃，保留配色。",
            "mustPreserve":["测试麦克风和保留配色"], "allowedCleanup":["清理无意义停顿"], "mustNotDo":["添加任务"]}]})
    }

    #[test]
    fn development_has_24_adjacent_ab_ba_calls_with_equal_model_and_source() {
        let (fixture, _) = fixture(Split::Dev, None).unwrap();
        let mut config = ProviderConfig::default(); config.region = Some("beijing".into());
        config.rewrite_model = "qwen-flash".into();
        let calls = plan(&fixture, Split::Dev, &config).unwrap();
        assert_eq!(calls.len(), 24);
        assert_eq!(calls.chunks_exact(2).filter(|pair|pair[0].arm == "baseline").count(), 6);
        for pair in calls.chunks_exact(2) {
            assert_eq!(pair[0].source, pair[1].source); assert_ne!(pair[0].arm, pair[1].arm);
            assert_eq!(pair[0].case_id, pair[1].case_id);
            for call in pair {
                assert_eq!(call.body["model"], "qwen3.8-flash"); assert_eq!(call.trial, 1);
                assert_eq!(call.mode.base_mode, BaseMode::Auto); assert!(call.mode.rules.is_empty());
            }
        }
        assert_eq!(config.rewrite_model, "qwen-flash");
    }

    #[test]
    fn holdout_budget_is_fixed_without_inspecting_reserved_content() {
        assert_eq!(Split::Holdout.case_count(), 8); assert_eq!(Split::Holdout.trials(), 1);
        assert_eq!(Split::Holdout.maximum_calls(), 16);
        assert_eq!(Split::Dev.maximum_calls() + Split::Holdout.maximum_calls() + Split::User.maximum_calls(), 42);
    }

    #[test]
    fn only_one_private_case_is_permitted_and_does_not_expand_other_splits() {
        let mut config = ProviderConfig::default(); config.region = Some("beijing".into());
        let fixture = private_fixture();
        let calls = plan(&fixture, Split::User, &config).unwrap();
        assert_eq!(calls.len(), 2); assert_eq!(calls[0].arm, "baseline"); assert_eq!(calls[1].arm, "candidate");
        assert!(plan(&fixture, Split::Dev, &config).is_err());
        let mut two = fixture.clone(); two["cases"].as_array_mut().unwrap().push(fixture["cases"][0].clone());
        assert!(plan(&two, Split::User, &config).is_err());
        let mut wrong_split = fixture; wrong_split["cases"][0]["split"] = json!("dev");
        assert!(plan(&wrong_split, Split::User, &config).is_err());
    }

    #[test]
    fn user_file_requires_explicit_user_split_and_run_never_defaults() {
        let prefix = ["--config-db", "C:/config.db", "--out", "C:/new.json"];
        let arguments = |tail:&[&str]|prefix.iter().chain(tail.iter()).map(|value|value.to_string()).collect::<Vec<_>>();
        let opts = options(&arguments(&["--split","user","--fixture","C:/private.json","--plan-only"])).unwrap();
        assert!(opts.plan_only); assert_eq!(opts.split, Split::User);
        for tail in [vec![],vec!["--split","user","--run"],vec!["--fixture","C:/private.json","--run"],
            vec!["--split","holdout","--fixture","C:/private.json","--run"],vec!["--run","--trials","2"],
            vec!["--candidate-model","qwen-plus","--run"],vec!["--split","user","--fixture","relative.json","--run"]] {
            assert!(options(&arguments(&tail)).is_err());
        }
    }

    #[test]
    fn synthetic_segmentation_preserves_every_character_and_rejects_overflow() {
        let fixture = private_fixture(); let case = &fixture["cases"][0];
        let revision = source(case).unwrap();
        assert_eq!(revision.items.iter().map(|item|item.text.as_str()).collect::<String>(), case["spokenScript"].as_str().unwrap());
        assert!(source(&json!({"id":"too-many","spokenScript":"句子。".repeat(17)})).is_err());
    }

    #[test]
    fn projected_budget_never_claims_native_delivery_or_semantic_success() {
        let fixture = private_fixture(); let source = source(&fixture["cases"][0]).unwrap();
        let good = projection(&source,&json!({"status":"parsed","validatedText":"candidate","elapsedMs":4999}));
        assert_eq!(good["scope"],"SIMULATED_CURRENT_INITIAL_BUDGET");
        assert_eq!(good["status"],"rewrite_candidate"); assert_eq!(good["semanticVerdict"],"UNREVIEWED");
        assert_eq!(good["actualNativeDelivery"],"NOT_RUN"); assert_eq!(good["applicationDeadlineMs"],5000);
        let late = projection(&source,&json!({"status":"parsed","validatedText":"candidate","elapsedMs":5000}));
        assert_eq!(late["reason"],"timeout"); assert_eq!(late["text"],source.text());
        let reject = projection(&source,&json!({"status":"rejected","validationError":{"code":"rewrite_meaning_risk"},"elapsedMs":100}));
        assert_eq!(reject["reason"],"meaningRisk"); assert_eq!(reject["text"],source.text());
    }

    #[test]
    fn cancellation_invalid_source_and_unstarted_calls_never_project_deliverable_text() {
        let fixture = private_fixture(); let source = source(&fixture["cases"][0]).unwrap();
        for row in [json!({"status":"failed","error":{"code":"cancelled"},"elapsedMs":21000}),
            json!({"status":"failed","error":{"code":"invalid_source"},"elapsedMs":21000}),json!({"status":"skipped"})] {
            let result=projection(&source,&row); assert_eq!(result["status"],"not_projected"); assert!(result["text"].is_null());
        }
    }

    #[test]
    fn rejected_model_text_remains_available_for_independent_review() {
        assert_eq!(unvalidated_text(r#"{"resolvedMode":"light","text":"候选"}"#).as_deref(),Some("候选"));
        assert_eq!(unvalidated_text(r#"{"blocks":[{"text":"甲"},{"text":"乙"}]}"#).as_deref(),Some("甲\n\n乙"));
        assert!(unvalidated_text(r#"{"text":null}"#).is_none());
    }
}
