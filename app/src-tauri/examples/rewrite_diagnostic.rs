//! Two explicitly preregistered NEW synthetic requests, never an automatic r1 retry.
//! Preserve rejected synthetic output to diagnose the production source mapping guard.
use chrono::Utc;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use speakmode_lib::{cloud::{self, CloudError}, core::{BaseMode, ModeSnapshot, ProviderConfig, SourceRevision}};
use std::{collections::HashSet, fs::{self, File, OpenOptions}, io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf}, time::{Duration, Instant}};

fn error(code: &str, message: &str) -> CloudError { CloudError::new(code, "diagnostic", message) }
fn safe(error: &CloudError) -> Value { json!({"code":error.code,"stage":error.stage,"message":error.message}) }
fn absolute(path: &Path) -> Result<(), CloudError> {
    if !path.is_absolute() { return Err(error("absolute_path_required", "诊断输入与输出必须使用绝对路径")); }
    Ok(())
}
fn config(path: &Path) -> Result<ProviderConfig, CloudError> {
    absolute(path)?;
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_| error("config_unavailable", "指定的配置数据库不可只读访问"))?;
    let text: String = connection.query_row("SELECT data FROM provider_config WHERE singleton=1", [], |row| row.get(0))
        .map_err(|_| error("config_unavailable", "数据库中没有有效云端配置"))?;
    if text.len() > 16 * 1024 { return Err(error("config_invalid", "云端配置超过容量限制")); }
    let config: ProviderConfig = serde_json::from_str(&text).map_err(|_| error("config_invalid", "云端配置格式无效"))?;
    cloud::endpoints(&config)?; Ok(config)
}
fn source(path: &Path, run_id: &str) -> Result<(SourceRevision, String, String), CloudError> {
    absolute(path)?;
    if run_id.is_empty() || run_id.len() > 128 || !run_id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) {
        return Err(error("invalid_run_id", "固定音频实验编号无效"));
    }
    let metadata = fs::metadata(path).map_err(|_| error("receipt_unavailable", "固定音频实验结果不可读"))?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 { return Err(error("receipt_limit", "固定音频实验结果超过容量限制")); }
    let bytes = fs::read(path).map_err(|_| error("receipt_unavailable", "固定音频实验结果不可读"))?;
    let receipt: Value = serde_json::from_slice(&bytes).map_err(|_| error("receipt_invalid", "固定音频实验结果格式无效"))?;
    if receipt["tool"] != "speakmode-voice-lab" || receipt["scope"].is_null() {
        return Err(error("receipt_scope", "诊断只能读取带实验范围的 voice-lab 结果"));
    }
    let candidates = receipt["results"].as_array().ok_or_else(|| error("receipt_invalid", "实验结果缺少请求记录"))?
        .iter().filter(|row| row["kind"] == "asr" && row["runId"] == run_id && row["status"] == "passed").collect::<Vec<_>>();
    if candidates.len() != 1 { return Err(error("source_missing", "指定实验没有唯一完整 ASR 来源")); }
    let row = candidates[0];
    let source: SourceRevision = serde_json::from_value(row["source"].clone()).map_err(|_| error("source_invalid", "固定音频识别来源格式无效"))?;
    let fixture_hash = row["fixtureSha256"].as_str().ok_or_else(|| error("source_invalid", "固定音频来源缺少文件哈希"))?;
    if fixture_hash.len() != 64 || !fixture_hash.bytes().all(|b| b.is_ascii_hexdigit()) { return Err(error("source_invalid", "固定音频文件哈希无效")); }
    Ok((source, format!("{:x}", Sha256::digest(bytes)), fixture_hash.into()))
}
fn mode(base: BaseMode) -> ModeSnapshot {
    let label = if base == BaseMode::Light { "light" } else { "structure" };
    ModeSnapshot { mode_id:label.into(), name:label.into(), base_mode:base, rules:String::new(), revision:1 }
}
fn missing_refs(content: &str, source: &SourceRevision) -> Value {
    match serde_json::from_str::<Value>(content) {
        Ok(value) => {
            let mut covered = HashSet::<String>::new(); let mut returned = Vec::<String>::new();
            if let Some(blocks) = value["blocks"].as_array() {
                for block in blocks {
                    if let Some(refs) = block["sourceRefs"].as_array() {
                        for source_ref in refs.iter().filter_map(Value::as_str) { covered.insert(source_ref.into()); returned.push(source_ref.into()); }
                    }
                }
            }
            let allowed = source.items.iter().map(|item| item.id.as_str()).collect::<HashSet<_>>();
            json!({"jsonDecoded":true,"sourceCount":source.items.len(),"coveredSourceCount":covered.iter().filter(|id|allowed.contains(id.as_str())).count(),
                "returnedRefs":returned,"unknownRefs":covered.iter().filter(|id|!allowed.contains(id.as_str())).collect::<Vec<_>>(),
                "missingSources":source.items.iter().filter(|item|!covered.contains(&item.id)).map(|item|json!({"id":item.id,"text":item.text,"captureOrder":item.capture_order})).collect::<Vec<_>>()})
        }
        Err(_) => json!({"jsonDecoded":false,"mappingIndeterminate":true}),
    }
}
async fn request(config: &ProviderConfig, secret: &str, mode: &ModeSnapshot, source: &SourceRevision, client: &reqwest::Client) -> Result<(String, Option<String>), CloudError> {
    let body = cloud::rewrite::request_body(config, mode, source)?;
    let endpoint = cloud::endpoints(config)?.rewrite;
    let operation = async {
        let mut request = client.post(endpoint).timeout(Duration::from_secs(20)).bearer_auth(secret).json(&body);
        if let Some(workspace) = config.workspace.as_deref().filter(|value| !value.is_empty()) { request = request.header("X-DashScope-WorkSpace", workspace); }
        let response = request.send().await.map_err(|failure| if failure.is_timeout() { error("rewrite_timeout", "诊断整理请求超时") } else { error("rewrite_network", "诊断整理请求连接失败") })?;
        if !response.status().is_success() {
            let code = match response.status().as_u16() { 401 | 403 => "authentication_failed", 429 => "rate_limited", _ => "rewrite_service_error" };
            // Do not read or retain a provider error body.
            return Err(error(code, "诊断整理请求未被服务接受"));
        }
        let mut response = response; let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| error("rewrite_network", "诊断整理响应中断"))? {
            if bytes.len() + chunk.len() > 1024 * 1024 { return Err(error("response_too_large", "诊断整理响应超过容量限制")); }
            bytes.extend_from_slice(&chunk);
        }
        let response: Value = serde_json::from_slice(&bytes).map_err(|_| error("invalid_response", "诊断整理响应格式无效"))?;
        let content = response.pointer("/choices/0/message/content").and_then(Value::as_str).ok_or_else(|| error("empty_output", "诊断整理响应缺少输出内容"))?;
        let finish = response.pointer("/choices/0/finish_reason").and_then(Value::as_str).map(str::to_owned);
        Ok((content.to_owned(), finish))
    };
    tokio::time::timeout(Duration::from_secs(20), operation).await.map_err(|_| error("rewrite_timeout", "诊断整理请求超过 20 秒期限"))?
}
struct Evidence { receipt: File, journal: File }
impl Evidence {
    fn new(out: &Path) -> Result<Self, CloudError> {
        absolute(out)?;
        let parent = out.parent().and_then(|path|path.canonicalize().ok()).ok_or_else(|| error("output_directory", "诊断输出父目录必须预先存在"))?;
        let parent = parent.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
        if !parent.starts_with("//?/d:/codex_project/speakmode_dev/evidence/") && !parent.starts_with("d:/codex_project/speakmode_dev/evidence/") {
            return Err(error("output_directory", "诊断结果只能写入项目 evidence 子目录"));
        }
        if out.extension().is_none_or(|extension|extension != "json") { return Err(error("output_extension", "诊断输出必须是 JSON 文件")); }
        if out.exists() || out.with_extension("jsonl").exists() { return Err(error("output_exists", "诊断结果已存在，不能覆盖")); }
        let receipt = OpenOptions::new().write(true).create_new(true).open(out).map_err(|_| error("output_unavailable", "不能创建诊断结果"))?;
        let journal = OpenOptions::new().write(true).create_new(true).open(out.with_extension("jsonl")).map_err(|_| error("output_unavailable", "不能创建诊断增量记录"))?;
        Ok(Self { receipt, journal })
    }
    fn append(&mut self, row: &Value) -> Result<(), CloudError> {
        serde_json::to_writer(&mut self.journal, row).map_err(|_| error("evidence_write_failed", "不能写入诊断增量记录"))?;
        self.journal.write_all(b"\n").and_then(|_|self.journal.flush()).and_then(|_|self.journal.sync_data()).map_err(|_| error("evidence_write_failed", "不能持久化诊断增量记录"))
    }
    fn checkpoint(&mut self, receipt: &Value) -> Result<(), CloudError> {
        self.receipt.seek(SeekFrom::Start(0)).and_then(|_|self.receipt.set_len(0)).map_err(|_| error("evidence_write_failed", "不能更新诊断结果"))?;
        serde_json::to_writer_pretty(&mut self.receipt, receipt).map_err(|_| error("evidence_write_failed", "不能写入诊断结果"))?;
        self.receipt.flush().and_then(|_|self.receipt.sync_data()).map_err(|_| error("evidence_write_failed", "不能持久化诊断结果"))
    }
}
async fn execute(source: SourceRevision, receipt_hash: String, fixture_hash: String, run_id: &str, config: ProviderConfig, out: &Path) -> Result<(), CloudError> {
    // Validate both exact production requests before any credential access or network call.
    for base in [BaseMode::Light, BaseMode::Structure] { cloud::rewrite::request_body(&config, &mode(base), &source)?; }
    let mut evidence = Evidence::new(out)?;
    let mut receipt = json!({"schemaVersion":1,"tool":"speakmode-rewrite-diagnostic","appVersion":env!("CARGO_PKG_VERSION"),
        "scope":"DIAGNOSTIC_NEW_REQUESTS_NOT_R1_RETRY","automaticRetries":0,"maximumCalls":2,"sourceRunId":run_id,
        "sourceReceiptSha256":receipt_hash,"fixtureSha256":fixture_hash,"source":source,"startedAt":Utc::now().to_rfc3339(),
        "configuration":{"region":config.region,"asrModel":config.asr_model,"rewriteModel":config.rewrite_model,"adapterVersion":config.adapter_version},
        "status":"running","results":[],"note":"Only successful HTTP message.content from this fixed synthetic source is retained; this is a new diagnostic experiment, not a repaired r1 result."});
    evidence.append(&json!({"kind":"header","metadata":receipt}))?; evidence.checkpoint(&receipt)?;
    let secret = match cloud::credentials::read(&config) {
        Ok(secret) => secret,
        Err(failure) => {
            receipt["status"] = json!("halted"); receipt["error"] = safe(&failure);
            evidence.append(&json!({"kind":"credentialFailure","error":safe(&failure)}))?; evidence.checkpoint(&receipt)?;
            return Ok(());
        }
    };
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).pool_max_idle_per_host(2)
        .pool_idle_timeout(Duration::from_secs(60)).build().map_err(|_| error("network_setup", "无法建立诊断请求"))?;
    let mut halted = false;
    for base in [BaseMode::Light, BaseMode::Structure] {
        let mut row = json!({"kind":"rewriteDiagnostic","baseMode":base,"automaticRetries":0});
        if halted { row["status"] = json!("skipped"); row["reason"] = json!("service_rejected"); }
        else {
            let started = Instant::now();
            match request(&config, &secret, &mode(base), &source, &client).await {
                Ok((content, finish_reason)) => {
                    row["httpSucceeded"] = json!(true); row["modelContent"] = json!(content); row["finishReason"] = json!(finish_reason);
                    row["mapping"] = missing_refs(&content, &source);
                    match cloud::rewrite::parse_model_output(&content, &mode(base), &source) {
                        Ok(output) => { row["productionParseStatus"] = json!("passed"); row["validatedOutput"] = serde_json::to_value(output).expect("rewrite result serializes"); }
                        Err(failure) => { row["productionParseStatus"] = json!("failed"); row["validationError"] = safe(&failure); }
                    }
                    if finish_reason.as_deref() != Some("stop") {
                        row["status"] = json!("failed"); row["error"] = safe(&error("truncated_output", "诊断整理响应未完整结束"));
                    } else { row["status"] = row["productionParseStatus"].clone(); }
                }
                Err(failure) => {
                    halted = matches!(failure.code.as_str(), "authentication_failed" | "rate_limited");
                    row["status"] = json!("failed"); row["httpSucceeded"] = json!(false); row["error"] = safe(&failure);
                }
            }
            row["elapsedMs"] = json!(started.elapsed().as_millis() as u64);
        }
        println!("diagnostic {:?}: {}", base, row["status"].as_str().unwrap());
        evidence.append(&row)?; receipt["results"].as_array_mut().unwrap().push(row); evidence.checkpoint(&receipt)?;
    }
    receipt["status"] = json!(if halted { "halted" } else { "completed" }); receipt["finishedAt"] = json!(Utc::now().to_rfc3339());
    evidence.append(&json!({"kind":"summary","status":receipt["status"]}))?; evidence.checkpoint(&receipt)
}
fn arg(args: &[String], label: &str) -> Result<String, CloudError> {
    let positions = args.iter().enumerate().filter(|(_,value)|value.as_str() == label).map(|(index,_)|index).collect::<Vec<_>>();
    if positions.len() != 1 || positions[0]+1 >= args.len() { return Err(error("usage", "缺少或重复指定诊断参数")); }
    Ok(args[positions[0]+1].clone())
}
#[tokio::main]
async fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let result = async {
        if args.len() != 8 { return Err(error("usage", "使用 --source-receipt ABS --run-id LABEL --config-db ABS --out ABS_NEW.json")); }
        let source_path = PathBuf::from(arg(&args,"--source-receipt")?); let run_id = arg(&args,"--run-id")?;
        let config_path = PathBuf::from(arg(&args,"--config-db")?); let out = PathBuf::from(arg(&args,"--out")?);
        let (source, receipt_hash, fixture_hash) = source(&source_path,&run_id)?;
        let config = config(&config_path)?; execute(source,receipt_hash,fixture_hash,&run_id,config,&out).await
    }.await;
    if let Err(failure) = result { eprintln!("{}",safe(&failure)); std::process::exit(1); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use speakmode_lib::core::SourceItem;
    fn fixture() -> SourceRevision {
        SourceRevision { id:"source".into(),session_id:"diagnostic".into(),asr_attempt_id:"fixed".into(),generation:1,
            provider:"aliyun".into(),model:"fixed".into(),adapter_version:"fixed".into(),complete:true,created_at:"fixed".into(),
            items:[("s1","文。"),("s2","预算先五千。"),("s3","不对，是三千五百元。")].into_iter().enumerate().map(|(index,(id,text))|
                SourceItem {id:id.into(),provider_item_id:id.into(),capture_order:index as u64,text:text.into(),is_final:true,start_ms:None,end_ms:None}).collect() }
    }
    #[test]
    fn omitted_discarded_source_is_reported_and_production_rejects_it() {
        let source = fixture(); let content = r#"{"resolvedMode":"light","blocks":[{"text":"预算是三千五百元。","sourceRefs":["s3"]}]}"#;
        let report = missing_refs(content,&source);
        assert_eq!(report["coveredSourceCount"],1); assert_eq!(report["missingSources"][0]["text"],"文。");
        assert_eq!(report["missingSources"][1]["text"],"预算先五千。");
        assert_eq!(cloud::rewrite::parse_model_output(content,&mode(BaseMode::Light),&source).unwrap_err().code,"incomplete_source_coverage");
    }
    #[test]
    fn full_mapping_can_cleanup_source_without_rendering_clutter() {
        let source = fixture(); let content = r#"{"resolvedMode":"light","blocks":[{"text":"预算是三千五百元。","sourceRefs":["s1","s2","s3"]}]}"#;
        assert_eq!(missing_refs(content,&source)["missingSources"],json!([]));
        assert!(cloud::rewrite::parse_model_output(content,&mode(BaseMode::Light),&source).is_ok());
    }
}
