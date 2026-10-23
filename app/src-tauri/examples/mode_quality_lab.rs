//! Fixed, paired synthetic-text diagnostic: 4 cases x 3 modes x 2 prompts.
//! No history/audio access, model changes, retries or automated quality scores.
#[path="lab_provider_scope/mod.rs"] mod lab_provider_scope;
use chrono::Utc;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use speakmode_lib::{cloud::{self, CloudError}, core::{BaseMode, ModeSnapshot, ProviderConfig, SourceItem, SourceRevision}};
use std::{fs::{self, File, OpenOptions}, io::{Seek, SeekFrom, Write}, path::{Path, PathBuf}, time::{Duration, Instant}};

const FIXTURE: &str = include_str!("../../../docs/experiments/fixtures/mode-quality-v1.json");
const CASE_IDS: [&str; 4] = ["prose-01", "correction-01", "topics-01", "conditions-01"];
const BASELINE_COMMIT: &str = "5d5047c936898f6ab56b511c908e0eca3d5b3018";
const MAX_CALLS: usize = 24;
const HELP: &str = "mode_quality_lab --fixture ABS --baseline-contract ABS --config-db ABS --out ABS_NEW.json [--plan-only]\nRuns up to 24 new cloud calls unless --plan-only is supplied; no retries. --help performs no I/O.";

fn error(code: &str, message: &str) -> CloudError { CloudError::new(code, "mode_quality_lab", message) }
fn hash(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn safe(failure: &CloudError) -> Value { json!({"code":failure.code,"stage":failure.stage,"message":failure.message}) }
fn read_json(path: &Path) -> Result<(Value, String), CloudError> {
    if !path.is_absolute() { return Err(error("absolute_path_required", "实验输入必须为绝对路径")); }
    let metadata = fs::metadata(path).map_err(|_|error("input_unavailable", "实验输入不可读"))?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 { return Err(error("input_limit", "实验输入超过容量限制")); }
    let bytes = fs::read(path).map_err(|_|error("input_unavailable", "实验输入不可读"))?;
    let value = serde_json::from_slice(&bytes).map_err(|_|error("input_invalid", "实验输入 JSON 无效"))?;
    Ok((value, hash(&bytes)))
}
fn read_config(path: &Path) -> Result<ProviderConfig, CloudError> {
    if !path.is_absolute() { return Err(error("absolute_path_required", "配置路径必须为绝对路径")); }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_|error("config_unavailable", "不能只读访问云端配置"))?;
    // Deliberately the only database query: no session, transcript or audio.
    let text: String = connection.query_row("SELECT data FROM provider_config WHERE singleton=1", [], |row|row.get(0))
        .map_err(|_|error("config_unavailable", "没有有效云端配置"))?;
    if text.len() > 16 * 1024 { return Err(error("config_invalid", "云端配置超过容量限制")); }
    let config: ProviderConfig = serde_json::from_str(&text).map_err(|_|error("config_invalid", "云端配置格式无效"))?;
    lab_provider_scope::rewrite_endpoint(&config)?; Ok(config)
}
fn mode(base: BaseMode, rules: &str) -> ModeSnapshot {
    ModeSnapshot {mode_id:format!("lab-{base:?}"),name:format!("{base:?}"),base_mode:base,rules:rules.into(),revision:1}
}
fn source(case: &Value) -> Result<SourceRevision, CloudError> {
    let id = case["id"].as_str().ok_or_else(||error("fixture_invalid", "固定案例没有编号"))?;
    let texts = case["sourceItems"].as_array().filter(|items|!items.is_empty() && items.len() <= 16)
        .ok_or_else(||error("fixture_invalid", "固定来源分片无效"))?;
    let items = texts.iter().enumerate().map(|(index, text)| Ok(SourceItem {
        id:format!("{id}:{index}"),provider_item_id:index.to_string(),capture_order:index as u64,
        text:text.as_str().filter(|text|!text.trim().is_empty() && text.chars().count() <= 2_000)
            .ok_or_else(||error("fixture_invalid", "固定来源文本无效"))?.into(),
        is_final:true,start_ms:None,end_ms:None,
    })).collect::<Result<Vec<_>, CloudError>>()?;
    Ok(SourceRevision {id:format!("source-{id}"),session_id:format!("synthetic-{id}"),asr_attempt_id:format!("fixed-{id}"),
        generation:1,provider:"synthetic".into(),model:"fixed-text".into(),adapter_version:"mode-quality-v1".into(),
        complete:true,created_at:"2026-10-09T00:00:00Z".into(),items})
}
struct PlannedCall {case_id:String,arm:String,mode:ModeSnapshot,source:SourceRevision,body:Value}
fn plan(fixture: &Value, baseline: &Value, config: &ProviderConfig) -> Result<Vec<PlannedCall>, CloudError> {
    // Bind the external path to this checked-in development corpus. This CLI
    // cannot accidentally send a similarly shaped private transcript file.
    let frozen: Value = serde_json::from_str(FIXTURE).expect("checked-in corpus is valid");
    if *fixture != frozen || baseline["commit"] != BASELINE_COMMIT {
        return Err(error("fixture_invalid", "实验必须使用冻结语料和指定基线提交"));
    }
    for key in ["light", "structure", "auto"] {
        if baseline[key].as_str().is_none_or(|text|text.trim().is_empty() || text.len() > 64_000) {
            return Err(error("baseline_invalid", "基线缺少有效模式提示词"));
        }
    }
    let cases = fixture["cases"].as_array().ok_or_else(||error("fixture_invalid", "固定案例无效"))?;
    let mut calls = Vec::with_capacity(MAX_CALLS);
    for (case_index, id) in CASE_IDS.iter().enumerate() {
        let matched = cases.iter().filter(|case|case["id"] == *id).collect::<Vec<_>>();
        if matched.len() != 1 { return Err(error("fixture_invalid", "固定案例缺失或重复")); }
        let case = matched[0]; let source = source(case)?;
        for (mode_index, (base, key)) in [(BaseMode::Light,"light"),(BaseMode::Structure,"structure"),(BaseMode::Auto,"auto")].into_iter().enumerate() {
            let mode = mode(base, case["formatRules"].as_str().unwrap_or(""));
            let candidate = cloud::rewrite::experimental_request_body(config, &mode, &source)?;
            let arms = if (case_index + mode_index) % 2 == 0 {["baseline","candidate"]} else {["candidate","baseline"]};
            for arm in arms {
                let mut body = candidate.clone();
                if arm == "baseline" { body["messages"][0]["content"] = baseline[key].clone(); }
                calls.push(PlannedCall {case_id:(*id).into(),arm:arm.into(),mode:mode.clone(),source:source.clone(),body});
            }
        }
    }
    if calls.len() != MAX_CALLS { return Err(error("plan_limit", "固定实验调用数不符")); }
    Ok(calls)
}

struct Evidence {receipt:File,journal:File}
impl Evidence {
    fn new(path: &Path) -> Result<Self, CloudError> {
        if !path.is_absolute() || path.extension().is_none_or(|ext|ext != "json") || path.exists() || path.with_extension("jsonl").exists() {
            return Err(error("fresh_output_required", "输出必须是不存在的绝对 JSON 路径"));
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
async fn request(client: &reqwest::Client, config: &ProviderConfig, secret: &str, body: &Value) -> Result<Value,CloudError> {
    let operation = async {
        let mut request = client.post(lab_provider_scope::rewrite_endpoint(config)?).bearer_auth(secret).json(body).timeout(Duration::from_secs(20));
        if let Some(workspace) = config.workspace.as_deref().filter(|workspace|!workspace.is_empty()) { request=request.header("X-DashScope-WorkSpace", workspace); }
        let mut response = request.send().await.map_err(|failure|error(if failure.is_timeout(){"rewrite_timeout"}else{"rewrite_network"}, "合成实验请求未完成"))?;
        if !response.status().is_success() {
            return Err(error(match response.status().as_u16(){401|403=>"authentication_failed",429=>"rate_limited",_=>"rewrite_service_error"}, "整理服务未接受合成实验请求"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_|error("rewrite_network", "合成实验响应中断"))? {
            if bytes.len()+chunk.len()>1024*1024 { return Err(error("response_too_large", "合成实验响应超过容量限制")); }
            bytes.extend_from_slice(&chunk);
        }
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
    tokio::time::timeout(Duration::from_secs(20), operation).await.map_err(|_|error("rewrite_timeout", "合成实验超过20秒期限"))?
}

async fn run(fixture_path: &Path, baseline_path: &Path, config_path: &Path, out: &Path, plan_only: bool) -> Result<(),CloudError> {
    let (fixture, fixture_hash) = read_json(fixture_path)?;
    let (baseline, baseline_hash) = read_json(baseline_path)?;
    let config = read_config(config_path)?;
    let calls = plan(&fixture, &baseline, &config)?;
    let planned = calls.iter().enumerate().map(|(index,call)| {
        let mut parameters = call.body.clone();parameters.as_object_mut().unwrap().remove("messages");
        json!({"index":index,"caseId":call.case_id,"arm":call.arm,"mode":call.mode.base_mode,
            "requestSha256":hash(&serde_json::to_vec(&call.body).unwrap()),
            "systemSha256":hash(call.body["messages"][0]["content"].as_str().unwrap().as_bytes()),
            "sourceSha256":hash(&serde_json::to_vec(&call.source).unwrap()),"source":call.source,"parameters":parameters})
    }).collect::<Vec<_>>();
    let mut receipt = json!({"schemaVersion":1,"tool":"speakmode-mode-quality-lab","appVersion":env!("CARGO_PKG_VERSION"),
        "scope":"PAIRED_SYNTHETIC_DEVELOPMENT_DIAGNOSTIC_NOT_HUMAN_OR_AUDIO_QUALITY",
        "fixtureSha256":fixture_hash,"baselineFileSha256":baseline_hash,"baselineCommit":BASELINE_COMMIT,
        "candidatePromptVersion":cloud::rewrite::EXPERIMENTAL_REWRITE_PROMPT_VERSION,
        "productionPromptVersion":cloud::rewrite::REWRITE_PROMPT_VERSION,"selectedCaseIds":CASE_IDS,
        "orderRule":"Adjacent pairs, baseline first when (case_index + mode_index) is even; candidate first otherwise",
        "maximumCalls":MAX_CALLS,"automaticRetries":0,"parserForBothArms":"current-production-parser",
        "humanQualityMeasured":false,"historyRead":false,"configuration":{"region":config.region,"rewriteModel":config.rewrite_model},
        "reviewChecklists":fixture["cases"].as_array().unwrap().iter().filter(|case|CASE_IDS.iter().any(|id|case["id"]==*id))
            .map(|case|json!({"caseId":case["id"],"mustPreserve":case["mustPreserve"],"protectedDetails":case["protectedDetails"],
                "expectedOrganization":case["expectedOrganization"],"acceptableAutoModes":case["acceptableAutoModes"]})).collect::<Vec<_>>(),
        "plan":planned,"results":[],"startedAt":Utc::now().to_rfc3339(),"status":if plan_only{"planned_only"}else{"running"}});
    let mut evidence = Evidence::new(out)?;
    // Durable, immutable-per-run preregistration precedes credential access.
    evidence.write(&json!({"kind":"preregistration","receipt":receipt}), &receipt)?;
    if plan_only {println!("planned_only: 24 calls; no credential access or network");return Ok(());}
    let secret = match cloud::credentials::read_for(&config, cloud::credentials::CredentialStage::Rewrite) {
        Ok(secret)=>secret,
        Err(failure)=>{receipt["status"]=json!("halted");receipt["error"]=safe(&failure);evidence.write(&json!({"kind":"credentialFailure","error":safe(&failure)}),&receipt)?;return Ok(());}
    };
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).pool_max_idle_per_host(2)
        .pool_idle_timeout(Duration::from_secs(60)).build().map_err(|_|error("network_setup", "无法建立合成实验请求"))?;
    let mut halted = None::<String>;let mut network_failures = 0;
    for (index, call) in calls.into_iter().enumerate() {
        let mut row = json!({"kind":"syntheticModeRewrite","index":index,"caseId":call.case_id,"arm":call.arm,"mode":call.mode.base_mode});
        if let Some(reason) = &halted {row["status"]=json!("skipped");row["reason"]=json!(reason);}
        else {
            let started=Instant::now();
            // Persist intent before calling. On a killed process an unresolved
            // call remains unknown rather than being retried or disappearing.
            evidence.write(&json!({"kind":"callStarted","index":index,"caseId":call.case_id,"arm":call.arm,"mode":call.mode.base_mode}), &receipt)?;
            match request(&client,&config,&secret,&call.body).await {
                Ok(response)=>{
                    row["response"]=response;
                    if row["response"]["finishReason"] != "stop" {
                        row["status"]=json!("truncated");row["currentParser"]=json!("not_run");
                    } else {
                        match cloud::rewrite::parse_model_output(row["response"]["modelContent"].as_str().unwrap(),&call.mode,&call.source) {
                            Ok(parsed)=>{row["status"]=json!("parsed");row["currentParser"]=json!("passed");row["validatedOutput"]=serde_json::to_value(parsed).unwrap();}
                            Err(failure)=>{row["status"]=json!("rejected");row["currentParser"]=json!("failed");row["validationError"]=safe(&failure);}
                        }
                    }
                },
                Err(failure)=>{
                    if matches!(failure.code.as_str(),"authentication_failed"|"rate_limited") {halted=Some(failure.code.clone());}
                    if matches!(failure.code.as_str(),"rewrite_network"|"rewrite_timeout"|"rewrite_service_error") {
                        network_failures+=1;if network_failures>=3 {halted=Some("three_network_or_service_failures".into());}
                    }
                    row["status"]=json!("failed");row["error"]=safe(&failure);
                },
            }
            row["elapsedMs"]=json!(started.elapsed().as_millis() as u64);
        }
        println!("synthetic {} {:?} {}: {}",call.case_id,call.mode.base_mode,call.arm,row["status"].as_str().unwrap());
        receipt["results"].as_array_mut().unwrap().push(row.clone());evidence.write(&row,&receipt)?;
    }
    receipt["status"]=json!(if halted.is_some(){"halted"}else{"completed"});receipt["finishedAt"]=json!(Utc::now().to_rfc3339());
    evidence.write(&json!({"kind":"summary","status":receipt["status"]}),&receipt)
}

struct Options {fixture:PathBuf,baseline:PathBuf,config:PathBuf,out:PathBuf,plan_only:bool}
fn options(args: &[String]) -> Result<Options,CloudError> {
    if !(args.len()==8 || (args.len()==9 && args[8]=="--plan-only"))
        || args.get(0).is_none_or(|v|v!="--fixture") || args.get(2).is_none_or(|v|v!="--baseline-contract")
        || args.get(4).is_none_or(|v|v!="--config-db") || args.get(6).is_none_or(|v|v!="--out") {
        return Err(error("usage", HELP));
    }
    Ok(Options {fixture:args[1].clone().into(),baseline:args[3].clone().into(),config:args[5].clone().into(),out:args[7].clone().into(),plan_only:args.len()==9})
}
#[tokio::main]
async fn main() {
    let args=std::env::args().skip(1).collect::<Vec<_>>();
    if args==["--help"] {println!("{HELP}");return;}
    let result=async {
        let args=options(&args)?;
        run(&args.fixture,&args.baseline,&args.config,&args.out,args.plan_only).await
    }.await;
    if let Err(failure)=result {eprintln!("{}",safe(&failure));std::process::exit(1);}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inputs() -> (Value,Value,ProviderConfig) {
        (serde_json::from_str(FIXTURE).unwrap(),json!({"commit":BASELINE_COMMIT,"light":"frozen light","structure":"frozen structure","auto":"frozen auto"}),ProviderConfig::default())
    }
    #[test]
    fn paired_plan_has_24_calls_and_only_the_system_differs() {
        let (fixture,baseline,config)=inputs();let calls=plan(&fixture,&baseline,&config).unwrap();
        assert_eq!(calls.len(),24);
        assert_eq!(calls.chunks_exact(2).filter(|pair|pair[0].arm=="baseline").count(),6);
        for pair in calls.chunks_exact(2) {
            assert_eq!(pair[0].case_id,pair[1].case_id);assert_ne!(pair[0].arm,pair[1].arm);
            assert_eq!(pair[0].source,pair[1].source);assert_eq!(pair[0].mode.base_mode,pair[1].mode.base_mode);
            let mut left=pair[0].body.clone();let mut right=pair[1].body.clone();
            left["messages"][0]["content"]=Value::Null;right["messages"][0]["content"]=Value::Null;
            assert_eq!(left,right);
            let candidate=pair.iter().find(|call|call.arm=="candidate").unwrap();
            assert!(candidate.body["messages"][0]["content"].as_str().unwrap()
                .starts_with(&format!("CONTRACT VERSION: {}\n",cloud::rewrite::EXPERIMENTAL_REWRITE_PROMPT_VERSION)));
            assert_ne!(candidate.body["messages"][0]["content"],
                cloud::rewrite::request_body(&config,&candidate.mode,&candidate.source).unwrap()["messages"][0]["content"]);
        }
        for id in CASE_IDS {assert_eq!(calls.iter().filter(|call|call.case_id==id).count(),6);}
    }
    #[test]
    fn altered_corpus_and_unknown_baseline_are_rejected_before_execution() {
        let (mut fixture,mut baseline,config)=inputs();
        fixture["cases"][0]["sourceItems"][0]=json!("private replacement");
        assert!(plan(&fixture,&baseline,&config).is_err());
        fixture=serde_json::from_str(FIXTURE).unwrap();baseline["commit"]=json!("unknown");
        assert!(plan(&fixture,&baseline,&config).is_err());
    }
    #[test]
    fn explicit_plan_only_is_parsed_but_unknown_or_duplicate_flags_fail() {
        let raw=["--fixture","a","--baseline-contract","b","--config-db","c","--out","d"];
        let mut args=raw.iter().map(|s|s.to_string()).collect::<Vec<_>>();
        assert!(!options(&args).unwrap().plan_only);args.push("--plan-only".into());
        assert!(options(&args).unwrap().plan_only);args.push("--plan-only".into());assert!(options(&args).is_err());
        args.pop();args[8]="--execute-unbounded".into();assert!(options(&args).is_err());
    }
}
