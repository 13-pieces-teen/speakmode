//! Frozen synthetic-text screening and confirmation. No microphone/history
//! access, configuration writes, automatic retry, or automated quality claims.
use chrono::Utc;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use speakmode_lib::{cloud::{self, CloudError}, core::{BaseMode, ModeSnapshot, ProviderConfig, SourceItem, SourceRevision}};
use std::{collections::{HashMap, HashSet}, fs::{self, File, OpenOptions}, io::{Seek, SeekFrom, Write}, path::{Path, PathBuf}, time::{Duration, Instant}};

#[path = "mode_lab/candidates.rs"]
mod candidates;

const FIXTURE: &str = include_str!("../../../docs/experiments/fixtures/mode-quality-v2.json");
const TIMEOUT_SECONDS: u64 = 20;
const HELP: &str = "mode_comparison_lab --fixture ABS --config-db ABS --out ABS_NEW.json --split screen|confirmation --arms COMMA_SEPARATED --repeats 1|2 [--plan-only]\nScreen: all four arms, 8 cases x 3 Modes x 1 repeat = 96 requests. Confirmation: baseline-flash plus exactly one candidate, 12 cases x 3 Modes x 2 repeats = 144 requests. No retries. --plan-only never accesses credentials/network. --help performs no I/O.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Arm { name: &'static str, model: &'static str, variant: Option<&'static str> }
const ARMS: [Arm; 4] = [
    Arm {name:"baseline-flash", model:"qwen-flash", variant:None},
    Arm {name:"fewshot-flash", model:"qwen-flash", variant:Some("fewshot")},
    Arm {name:"typed-flash", model:"qwen-flash", variant:Some("typed")},
    Arm {name:"typed-plus", model:"qwen-plus", variant:Some("typed")},
];

fn error(code: &str, message: &str) -> CloudError { CloudError::new(code, "mode_comparison_lab", message) }
fn hash(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn safe(failure: &CloudError) -> Value { json!({"code":failure.code,"stage":failure.stage,"message":failure.message}) }

fn read_fixture(path: &Path) -> Result<(Value, String), CloudError> {
    if !path.is_absolute() { return Err(error("absolute_path_required", "语料路径必须为绝对路径")); }
    let metadata = fs::metadata(path).map_err(|_|error("fixture_unavailable", "冻结语料不可读"))?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 { return Err(error("fixture_limit", "冻结语料超过容量限制")); }
    let bytes = fs::read(path).map_err(|_|error("fixture_unavailable", "冻结语料不可读"))?;
    // Byte equality binds this executable to the checked-in synthetic corpus;
    // even a similarly shaped private transcript cannot enter a request.
    if bytes != FIXTURE.as_bytes() { return Err(error("fixture_mismatch", "外部语料必须与本程序编译时冻结语料逐字节相同")); }
    let fixture = serde_json::from_slice(&bytes).map_err(|_|error("fixture_invalid", "冻结语料 JSON 无效"))?;
    Ok((fixture, hash(&bytes)))
}

fn read_config(path: &Path) -> Result<ProviderConfig, CloudError> {
    if !path.is_absolute() { return Err(error("absolute_path_required", "配置路径必须为绝对路径")); }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_|error("config_unavailable", "不能只读访问云端配置"))?;
    // The only database query. Never read sessions, history, audio or drafts.
    let text: String = connection.query_row("SELECT data FROM provider_config WHERE singleton=1", [], |row|row.get(0))
        .map_err(|_|error("config_unavailable", "没有有效云端配置"))?;
    if text.len() > 16 * 1024 { return Err(error("config_invalid", "云端配置超过容量限制")); }
    let config: ProviderConfig = serde_json::from_str(&text).map_err(|_|error("config_invalid", "云端配置格式无效"))?;
    cloud::endpoints(&config)?;
    Ok(config)
}

fn source(case: &Value) -> Result<SourceRevision, CloudError> {
    let id = case["id"].as_str().filter(|id|!id.is_empty() && id.len() <= 80)
        .ok_or_else(||error("fixture_invalid", "固定案例没有有效编号"))?;
    let texts = case["sourceItems"].as_array().filter(|items|!items.is_empty() && items.len() <= 32)
        .ok_or_else(||error("fixture_invalid", "固定来源分片无效"))?;
    let items = texts.iter().enumerate().map(|(index, text)| Ok(SourceItem {
        id:format!("{id}:{index}"),provider_item_id:index.to_string(),capture_order:index as u64,
        text:text.as_str().filter(|text|!text.trim().is_empty() && text.chars().count() <= 2_000)
            .ok_or_else(||error("fixture_invalid", "固定来源文本无效"))?.into(),
        is_final:true,start_ms:None,end_ms:None,
    })).collect::<Result<Vec<_>, CloudError>>()?;
    Ok(SourceRevision {id:format!("source-{id}"),session_id:format!("synthetic-{id}"),asr_attempt_id:format!("fixed-{id}"),
        generation:1,provider:"synthetic".into(),model:"fixed-text".into(),adapter_version:"mode-quality-v2".into(),
        complete:true,created_at:"2026-10-09T00:00:00Z".into(),items})
}

fn selected_arms(split: &str, names: &[String], repeats: usize) -> Result<Vec<Arm>, CloudError> {
    let unique = names.iter().map(String::as_str).collect::<HashSet<_>>();
    if unique.len() != names.len() || unique.iter().any(|name|!ARMS.iter().any(|arm|arm.name == *name)) {
        return Err(error("invalid_arms", "实验组不能重复或超出固定允许列表"));
    }
    match split {
        "screen" if unique.len() == 4 && repeats == 1 => {},
        "confirmation" if unique.len() == 2 && unique.contains("baseline-flash") && repeats == 2 => {},
        _ => return Err(error("invalid_stage", "筛选必须四组一次；确认必须基线加一个候选并重复两次")),
    }
    // CLI order never changes allocation or execution order.
    Ok(ARMS.into_iter().filter(|arm|unique.contains(arm.name)).collect())
}

struct PlannedCall {case_id:String,arm:Arm,repeat:usize,mode:ModeSnapshot,source:SourceRevision,config:ProviderConfig,body:Value}
fn plan(fixture: &Value, config: &ProviderConfig, split: &str, names: &[String], repeats: usize) -> Result<Vec<PlannedCall>, CloudError> {
    let frozen: Value = serde_json::from_str(FIXTURE).expect("checked-in corpus must be valid");
    if *fixture != frozen { return Err(error("fixture_mismatch", "实验只能使用冻结合成语料")); }
    let arms = selected_arms(split,names,repeats)?;
    let all_cases = fixture["cases"].as_array().ok_or_else(||error("fixture_invalid", "固定案例列表无效"))?;
    let mut unique_ids = HashSet::new();
    for case in all_cases {
        let id = case["id"].as_str().ok_or_else(||error("fixture_invalid", "固定案例编号无效"))?;
        if !unique_ids.insert(id) || !matches!(case["split"].as_str(), Some("screen" | "confirmation")) {
            return Err(error("fixture_invalid", "案例编号重复或分组无效"));
        }
    }
    if all_cases.len() != 20 || all_cases.iter().filter(|case|case["split"] == "screen").count() != 8 {
        return Err(error("fixture_invalid", "冻结语料必须为 8 条筛选和 12 条确认案例"));
    }
    let cases = all_cases.iter().filter(|case|case["split"] == split).collect::<Vec<_>>();
    let expected_calls = if split == "screen" {96} else {144};
    let mut calls = Vec::with_capacity(expected_calls);
    for repeat in 0..repeats {
        for (case_index, case) in cases.iter().enumerate() {
            let source = source(case)?;
            for (mode_index, base) in [BaseMode::Light, BaseMode::Structure, BaseMode::Auto].into_iter().enumerate() {
                let mode = ModeSnapshot {mode_id:format!("lab-{base:?}"),name:format!("{base:?}"),base_mode:base,
                    rules:case["formatRules"].as_str().unwrap_or("").into(),revision:1};
                let rotation = (case_index + mode_index + repeat) % arms.len();
                for arm_offset in 0..arms.len() {
                    let arm = arms[(arm_offset + rotation) % arms.len()];
                    let mut arm_config = config.clone();
                    arm_config.rewrite_model = arm.model.into();
                    let body = match arm.variant {
                        None => cloud::rewrite::request_body(&arm_config,&mode,&source)?,
                        Some(variant) => candidates::request(&arm_config,&mode,&source,variant)?,
                    };
                    if body["model"] != arm.model || body["max_tokens"] != 8192 || body["temperature"] != 0.2
                        || body["stream"] != false || body["enable_thinking"] != false {
                        return Err(error("parameter_mismatch", "实验组参数与冻结协议不符"));
                    }
                    calls.push(PlannedCall {case_id:case["id"].as_str().unwrap().into(),arm,repeat,
                        mode:mode.clone(),source:source.clone(),config:arm_config,body});
                }
            }
        }
    }
    if calls.len() != expected_calls { return Err(error("plan_limit", "固定实验调用数不符")); }
    Ok(calls)
}

struct Evidence {receipt:File,journal:File}
impl Evidence {
    fn new(path: &Path) -> Result<Self, CloudError> {
        if !path.is_absolute() || path.extension().is_none_or(|ext|ext != "json") || path.exists() || path.with_extension("jsonl").exists() {
            return Err(error("fresh_output_required", "输出必须是不存在的绝对 JSON 路径，且同名 JSONL 不存在"));
        }
        let receipt = OpenOptions::new().create_new(true).write(true).open(path).map_err(|_|error("evidence_unavailable", "不能创建实验结果"))?;
        let journal = OpenOptions::new().create_new(true).write(true).open(path.with_extension("jsonl")).map_err(|_|error("evidence_unavailable", "不能创建增量记录"))?;
        Ok(Self {receipt,journal})
    }
    fn write(&mut self, row: &Value, receipt: &Value) -> Result<(), CloudError> {
        serde_json::to_writer(&mut self.journal, row).map_err(|_|error("evidence_write_failed", "不能写入增量记录"))?;
        self.journal.write_all(b"\n").and_then(|_|self.journal.flush()).and_then(|_|self.journal.sync_data())
            .map_err(|_|error("evidence_write_failed", "不能持久化增量记录"))?;
        self.receipt.seek(SeekFrom::Start(0)).and_then(|_|self.receipt.set_len(0)).map_err(|_|error("evidence_write_failed", "不能更新实验结果"))?;
        serde_json::to_writer_pretty(&mut self.receipt, receipt).map_err(|_|error("evidence_write_failed", "不能写入实验结果"))?;
        self.receipt.flush().and_then(|_|self.receipt.sync_data()).map_err(|_|error("evidence_write_failed", "不能持久化实验结果"))
    }
}

fn http_failure(status: u16, body: &[u8]) -> CloudError {
    let parsed = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
    let code = parsed.pointer("/error/code").or_else(||parsed.get("code")).and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    // Inspect bounded provider error text only to classify unavailable models;
    // never copy it, a URL, a request header or credential into evidence.
    let message = parsed.pointer("/error/message").or_else(||parsed.get("message")).and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    let unavailable = status == 404 || code.contains("modelnotfound") || code.contains("model_not_found")
        || code.contains("invalid_model") || code.contains("unsupported_model")
        || (status == 400 && message.contains("model") && ["not exist", "not found", "unsupported", "not support", "not available"].iter().any(|fragment|message.contains(fragment)));
    error(match status {401|403=>"authentication_failed",429=>"rate_limited",_ if unavailable=>"model_unavailable",_=>"rewrite_service_error"}, "云端服务未接受合成实验请求")
}

async fn request(client: &reqwest::Client, call: &PlannedCall, secret: &str) -> Result<Value,CloudError> {
    let operation = async {
        let mut request = client.post(cloud::endpoints(&call.config)?.rewrite).bearer_auth(secret).json(&call.body).timeout(Duration::from_secs(TIMEOUT_SECONDS));
        if let Some(workspace) = call.config.workspace.as_deref().filter(|workspace|!workspace.is_empty()) { request=request.header("X-DashScope-WorkSpace", workspace); }
        let mut response = request.send().await.map_err(|failure|error(if failure.is_timeout(){"rewrite_timeout"}else{"rewrite_network"}, "合成实验请求未完成"))?;
        let status = response.status();
        // Apply stop/deny policy from headers even if an error body is malformed,
        // oversized or hangs. There is no reason to preserve raw service errors.
        if matches!(status.as_u16(),401|403|404|429) { return Err(http_failure(status.as_u16(),b"")); }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_|error("rewrite_network", "合成实验响应中断"))? {
            if bytes.len()+chunk.len()>1024*1024 { return Err(error("response_too_large", "合成实验响应超过容量限制")); }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() { return Err(http_failure(status.as_u16(), &bytes)); }
        let data: Value = serde_json::from_slice(&bytes).map_err(|_|error("invalid_response", "合成实验响应格式无效"))?;
        let content = data.pointer("/choices/0/message/content").and_then(Value::as_str).ok_or_else(||error("empty_output", "合成实验没有响应文本"))?;
        if content.len() > 256 * 1024 { return Err(error("response_too_large", "合成输出超过诊断保留上限")); }
        Ok(json!({"modelContent":content,"finishReason":data.pointer("/choices/0/finish_reason").and_then(Value::as_str),
            "modelReported":data.get("model").and_then(Value::as_str).map(|s|s.chars().take(256).collect::<String>()),
            "responseId":data.get("id").and_then(Value::as_str).map(|s|s.chars().take(256).collect::<String>()),
            "usage":{"promptTokens":data.pointer("/usage/prompt_tokens").and_then(Value::as_u64),
                "completionTokens":data.pointer("/usage/completion_tokens").and_then(Value::as_u64),
                "totalTokens":data.pointer("/usage/total_tokens").and_then(Value::as_u64)}}))
    };
    tokio::time::timeout(Duration::from_secs(TIMEOUT_SECONDS), operation).await.map_err(|_|error("rewrite_timeout", "合成实验超过 20 秒期限"))?
}

fn unvalidated_text(content: &str) -> Option<String> {
    let decoded: Value = serde_json::from_str(content).ok()?;
    for field in ["blocks","units"] {
        if let Some(items) = decoded[field].as_array() {
            let texts = items.iter().filter_map(|item|item["text"].as_str()).collect::<Vec<_>>();
            if !texts.is_empty() { return Some(texts.join("\n\n")); }
        }
    }
    None
}

fn totals(receipt: &Value) -> Value {
    let rows = receipt["results"].as_array().unwrap();
    let count = |status:&str| rows.iter().filter(|row|row["status"] == status).count();
    json!({"planned":receipt["maximumCalls"],"recorded":rows.len(),"attempted":rows.len()-count("skipped"),
        "parsed":count("parsed"),"rejected":count("rejected"),"truncated":count("truncated"),"failed":count("failed"),"skipped":count("skipped"),
        "note":"Parser pass is not semantic correctness; all planned calls stay in the denominator."})
}

async fn run(options: Options) -> Result<(),CloudError> {
    let (fixture, fixture_hash) = read_fixture(&options.fixture)?;
    let config = read_config(&options.config)?;
    let calls = plan(&fixture,&config,&options.split,&options.arms,options.repeats)?;
    let planned = calls.iter().enumerate().map(|(index,call)| {
        let mut parameters = call.body.clone();parameters.as_object_mut().unwrap().remove("messages");
        json!({"index":index,"caseId":call.case_id,"arm":call.arm.name,"model":call.arm.model,"variant":call.arm.variant,
            "repeat":call.repeat+1,"mode":call.mode.base_mode,"requestSha256":hash(&serde_json::to_vec(&call.body).unwrap()),
            "systemSha256":hash(call.body["messages"][0]["content"].as_str().unwrap().as_bytes()),
            "messagesSha256":hash(&serde_json::to_vec(&call.body["messages"]).unwrap()),
            "sourceSha256":hash(&serde_json::to_vec(&call.source).unwrap()),"source":call.source,"parameters":parameters,"requestBody":call.body})
    }).collect::<Vec<_>>();
    let mut receipt = json!({"schemaVersion":1,"tool":"speakmode-mode-comparison-lab","appVersion":env!("CARGO_PKG_VERSION"),
        "scope":"FROZEN_SYNTHETIC_TEXT_COMPARISON_NOT_HUMAN_OR_AUDIO_QUALITY",
        "fixtureSha256":fixture_hash,"candidateSourceSha256":hash(include_bytes!("mode_lab/candidates.rs")),
        "runnerSourceSha256":hash(include_bytes!("mode_comparison_lab.rs")),
        "productionSourceSha256":{"rewrite":hash(include_bytes!("../src/cloud/rewrite.rs")),
            "baseline":hash(include_bytes!("../src/cloud/rewrite_baseline.rs")),"integrity":hash(include_bytes!("../src/cloud/rewrite_integrity.rs"))},
        "productionPromptVersion":cloud::rewrite::REWRITE_PROMPT_VERSION,"split":options.split,"repeats":options.repeats,
        "arms":selected_arms(&options.split,&options.arms,options.repeats)?.iter().map(|arm|arm.name).collect::<Vec<_>>(),
        "orderRule":"repeat outermost, fixed fixture case order, Light/Structure/Auto; rotate canonical arm order by (case_index + mode_index + repeat_index) modulo arm_count",
        "maximumCalls":calls.len(),"wholeExperimentPlannedMaximum":240,"automaticRetries":0,"perRequestTimeoutSeconds":TIMEOUT_SECONDS,
        "deniedModelPolicy":"skip remaining calls to a model after authentication or model unavailable errors",
        "globalHaltPolicy":"429 or three cumulative network, timeout or service failures",
        "humanQualityMeasured":false,"historyRead":false,"configurationWritten":false,
        "configuration":{"region":config.region,"provider":config.provider,"configuredRewriteModel":config.rewrite_model},
        "reviewChecklists":fixture["cases"].as_array().unwrap().iter().filter(|case|case["split"] == options.split).collect::<Vec<_>>(),
        "plan":planned,"results":[],"startedAt":Utc::now().to_rfc3339(),"status":if options.plan_only{"planned_only"}else{"running"}});
    let mut evidence = Evidence::new(&options.out)?;
    // Complete plan and request hashes are durable before credential access.
    evidence.write(&json!({"kind":"preregistration","receipt":receipt}), &receipt)?;
    if options.plan_only {println!("planned_only: {} calls; no credential access or network",calls.len());return Ok(());}
    let secret = match cloud::credentials::read(&config) {
        Ok(secret)=>secret,
        Err(failure)=>{receipt["status"]=json!("halted");receipt["error"]=safe(&failure);receipt["finishedAt"]=json!(Utc::now().to_rfc3339());
            evidence.write(&json!({"kind":"credentialFailure","error":safe(&failure)}),&receipt)?;return Ok(());}
    };
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).pool_max_idle_per_host(2)
        .pool_idle_timeout(Duration::from_secs(60)).build().map_err(|_|error("network_setup", "无法建立合成实验请求"))?;
    let mut halted = None::<String>;
    let mut denied_models = HashMap::<String,String>::new();
    let mut network_failures = 0;
    for (index, call) in calls.into_iter().enumerate() {
        let mut row = json!({"kind":"syntheticModeRewrite","index":index,"caseId":call.case_id,"arm":call.arm.name,
            "model":call.arm.model,"repeat":call.repeat+1,"mode":call.mode.base_mode});
        let skip = halted.as_ref().or_else(||denied_models.get(call.arm.model));
        if let Some(reason) = skip {row["status"]=json!("skipped");row["reason"]=json!(reason);}
        else {
            let started=Instant::now();
            receipt["pendingCall"]=json!({"index":index,"caseId":call.case_id,"arm":call.arm.name,"mode":call.mode.base_mode,"repeat":call.repeat+1});
            // A killed process leaves a durable unknown call; never silently retry.
            evidence.write(&json!({"kind":"callStarted","call":receipt["pendingCall"]}), &receipt)?;
            match request(&client,&call,&secret).await {
                Ok(response)=>{
                    row["response"]=response;
                    let content=row["response"]["modelContent"].as_str().unwrap().to_owned();
                    if let Some(text)=unvalidated_text(&content) {row["unvalidatedText"]=json!(text);}
                    if row["response"]["finishReason"] != "stop" {
                        row["status"]=json!("truncated");row["parser"]=json!("not_run");
                    } else {
                        let parsed = match call.arm.variant {
                            None=>cloud::rewrite::parse_model_output(&content,&call.mode,&call.source),
                            Some(variant)=>candidates::parse(&content,&call.mode,&call.source,variant),
                        };
                        match parsed {
                            Ok(parsed)=>{row["status"]=json!("parsed");row["parser"]=json!("passed");
                                row["renderedText"]=json!(parsed.blocks.iter().map(|block|block.text.as_str()).collect::<Vec<_>>().join("\n\n"));
                                row["validatedOutput"]=serde_json::to_value(parsed).unwrap();}
                            Err(failure)=>{row["status"]=json!("rejected");row["parser"]=json!("failed");row["validationError"]=safe(&failure);}
                        }
                    }
                },
                Err(failure)=>{
                    if matches!(failure.code.as_str(),"authentication_failed"|"model_unavailable") {denied_models.insert(call.arm.model.into(),failure.code.clone());}
                    if failure.code == "rate_limited" {halted=Some(failure.code.clone());}
                    if matches!(failure.code.as_str(),"rewrite_network"|"rewrite_timeout"|"rewrite_service_error") {
                        network_failures+=1;if network_failures>=3 {halted=Some("three_network_or_service_failures".into());}
                    }
                    row["status"]=json!("failed");row["error"]=safe(&failure);
                },
            }
            row["elapsedMs"]=json!(started.elapsed().as_millis() as u64);
            receipt.as_object_mut().unwrap().remove("pendingCall");
        }
        println!("synthetic {} {:?} {} repeat{}: {}",call.case_id,call.mode.base_mode,call.arm.name,call.repeat+1,row["status"].as_str().unwrap());
        receipt["results"].as_array_mut().unwrap().push(row.clone());
        receipt["totals"]=totals(&receipt);
        evidence.write(&row,&receipt)?;
    }
    receipt["status"]=json!(if halted.is_some(){"halted"}else if !denied_models.is_empty(){"completed_with_denied_models"}else{"completed"});
    receipt["deniedModels"]=json!(denied_models);receipt["networkOrServiceFailures"]=json!(network_failures);
    receipt["finishedAt"]=json!(Utc::now().to_rfc3339());
    evidence.write(&json!({"kind":"summary","status":receipt["status"],"totals":receipt["totals"]}),&receipt)
}

struct Options {fixture:PathBuf,config:PathBuf,out:PathBuf,split:String,arms:Vec<String>,repeats:usize,plan_only:bool}
fn options(args: &[String]) -> Result<Options,CloudError> {
    let mut values = HashMap::<&str,&str>::new();
    let mut plan_only = false;
    let mut index = 0;
    while index < args.len() {
        let key = args[index].as_str();
        if key == "--plan-only" {
            if plan_only {return Err(error("usage",HELP));}plan_only=true;index+=1;continue;
        }
        if !matches!(key,"--fixture"|"--config-db"|"--out"|"--split"|"--arms"|"--repeats") || index+1 >= args.len()
            || args[index+1].starts_with("--") || values.insert(key,&args[index+1]).is_some() {return Err(error("usage",HELP));}
        index+=2;
    }
    if values.len() != 6 {return Err(error("usage",HELP));}
    let split=values["--split"].to_owned();
    let arms=values["--arms"].split(',').map(str::to_owned).collect::<Vec<_>>();
    let repeats=values["--repeats"].parse::<usize>().map_err(|_|error("usage",HELP))?;
    selected_arms(&split,&arms,repeats)?;
    Ok(Options {fixture:values["--fixture"].into(),config:values["--config-db"].into(),out:values["--out"].into(),split,arms,repeats,plan_only})
}

#[tokio::main]
async fn main() {
    let args=std::env::args().skip(1).collect::<Vec<_>>();
    if args==["--help"] {println!("{HELP}");return;}
    let result=async {run(options(&args)?).await}.await;
    if let Err(failure)=result {eprintln!("{}",safe(&failure));std::process::exit(1);}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Value {serde_json::from_str(FIXTURE).unwrap()}
    fn names() -> Vec<String> {ARMS.iter().map(|arm|arm.name.into()).collect()}
    #[test]
    fn screen_is_96_calls_with_balanced_rotated_arms_and_identical_sources() {
        let calls=plan(&fixture(),&ProviderConfig::default(),"screen",&names(),1).unwrap();
        assert_eq!(calls.len(),96);
        for arm in ARMS {assert_eq!(calls.iter().filter(|call|call.arm==arm).count(),24);}
        let mut first=HashMap::new();
        for group in calls.chunks_exact(4) {
            *first.entry(group[0].arm.name).or_insert(0)+=1;
            assert_eq!(group.iter().map(|call|call.arm.name).collect::<HashSet<_>>().len(),4);
            for call in group {assert_eq!(call.source,group[0].source);assert_eq!(call.mode,group[0].mode);}
        }
        assert!(first.values().all(|count|*count==6));
        let mut reversed=names();reversed.reverse();
        let alternate=plan(&fixture(),&ProviderConfig::default(),"screen",&reversed,1).unwrap();
        assert_eq!(calls.iter().map(|call|call.arm.name).collect::<Vec<_>>(),alternate.iter().map(|call|call.arm.name).collect::<Vec<_>>());
    }
    #[test]
    fn confirmation_is_144_calls_with_two_repeats_and_reversed_pairs() {
        for candidate in ARMS.iter().skip(1) {
            let arms=vec!["baseline-flash".into(),candidate.name.into()];
            let calls=plan(&fixture(),&ProviderConfig::default(),"confirmation",&arms,2).unwrap();
            assert_eq!(calls.len(),144);
            assert_eq!(calls.iter().filter(|call|call.arm.name=="baseline-flash").count(),72);
            for (first,second) in calls[..72].chunks_exact(2).zip(calls[72..].chunks_exact(2)) {
                assert_eq!(first[0].case_id,second[0].case_id);assert_eq!(first[0].mode,second[0].mode);
                assert_ne!(first[0].arm,second[0].arm);assert_eq!(first[0].arm,second[1].arm);
                assert_eq!(first[0].repeat,0);assert_eq!(second[0].repeat,1);
            }
        }
    }
    #[test]
    fn invalid_stage_arms_repeats_and_modified_fixture_are_rejected() {
        assert!(selected_arms("screen",&names(),2).is_err());
        assert!(selected_arms("confirmation",&names(),2).is_err());
        assert!(selected_arms("other",&names(),1).is_err());
        assert!(selected_arms("confirmation",&["typed-plus".into(),"fewshot-flash".into()],2).is_err());
        assert!(selected_arms("confirmation",&["baseline-flash".into(),"baseline-flash".into()],2).is_err());
        let mut altered=fixture();altered["cases"][0]["sourceItems"][0]=json!("private transcript");
        assert!(plan(&altered,&ProviderConfig::default(),"screen",&names(),1).is_err());
    }
    #[test]
    fn cli_requires_explicit_finite_stage_and_rejects_duplicate_flags() {
        let raw=["--fixture","a","--config-db","b","--out","c","--split","screen","--arms","baseline-flash,fewshot-flash,typed-flash,typed-plus","--repeats","1"];
        let mut args=raw.iter().map(|value|value.to_string()).collect::<Vec<_>>();
        assert!(!options(&args).unwrap().plan_only);
        args.push("--plan-only".into());assert!(options(&args).unwrap().plan_only);
        args.push("--plan-only".into());assert!(options(&args).is_err());
        args.pop();args.push("--repeats".into());args.push("2".into());assert!(options(&args).is_err());
    }
    #[test]
    fn evidence_requires_fresh_absolute_output_and_preserves_existing_files() {
        let folder=std::env::temp_dir().join(format!("speakmode-lab-test-{}",uuid::Uuid::new_v4()));
        fs::create_dir(&folder).unwrap();
        let path=folder.join("receipt.json");
        let mut evidence=Evidence::new(&path).unwrap();
        evidence.write(&json!({"kind":"test"}),&json!({"test":true})).unwrap();
        drop(evidence);
        let before=fs::read(&path).unwrap();
        assert!(Evidence::new(&path).is_err());assert_eq!(before,fs::read(&path).unwrap());
        assert!(Evidence::new(Path::new("relative.json")).is_err());
        assert!(Evidence::new(&folder.join("wrong.txt")).is_err());
        fs::remove_file(&path).unwrap();
        assert!(Evidence::new(&path).is_err()); // existing journal also blocks reuse
        fs::remove_file(path.with_extension("jsonl")).unwrap();fs::remove_dir(folder).unwrap();
    }
    #[test]
    fn provider_failure_classification_does_not_echo_response_details() {
        assert_eq!(http_failure(429,b"secret").code,"rate_limited");
        assert_eq!(http_failure(401,b"secret").code,"authentication_failed");
        let denied=http_failure(400,br#"{"error":{"code":"ModelNotFound","message":"private provider diagnostic"}}"#);
        assert_eq!(denied.code,"model_unavailable");assert!(!denied.message.contains("private"));
        assert_eq!(http_failure(500,b"internal error").code,"rewrite_service_error");
    }
    #[test]
    fn rejected_outputs_keep_only_explicit_unvalidated_text_fields() {
        assert_eq!(unvalidated_text(r#"{"blocks":[{"text":"一"},{"text":"二"}]}"#),Some("一\n\n二".into()));
        assert_eq!(unvalidated_text(r#"{"units":[{"text":"三"}]}"#),Some("三".into()));
        assert_eq!(unvalidated_text("not-json"),None);
    }
}
