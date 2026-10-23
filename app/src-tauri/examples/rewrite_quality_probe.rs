//! Bounded, preregistered synthetic prompt comparison. Never reads history/audio.
//! Only --execute-cloud accesses the application's credential and sends requests.
#[path="lab_provider_scope/mod.rs"] mod lab_provider_scope;
use chrono::Utc;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use speakmode_lib::{cloud::{self, CloudError}, core::{BaseMode, ModeSnapshot, ProviderConfig, SourceItem, SourceRevision}};
use std::{fs::{File, OpenOptions}, io::{Seek, SeekFrom, Write}, path::{Path, PathBuf}, time::{Duration, Instant}};

const FIXTURE: &str = include_str!("../tests/fixtures/rewrite_quality_0_2_0.json");
fn error(code: &str, message: &str) -> CloudError { CloudError::new(code, "rewrite_quality_probe", message) }
fn hash(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn safe(failure: &CloudError) -> Value { json!({"code":failure.code,"stage":failure.stage,"message":failure.message}) }

fn read_config(path: &Path) -> Result<ProviderConfig, CloudError> {
    if !path.is_absolute() { return Err(error("absolute_path_required", "配置路径必须为绝对路径")); }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_|error("config_unavailable", "不能只读访问云端配置"))?;
    // This is the only database query. No sessions, transcripts or audio paths.
    let text: String = connection.query_row("SELECT data FROM provider_config WHERE singleton=1", [], |row|row.get(0))
        .map_err(|_|error("config_unavailable", "没有有效云端配置"))?;
    if text.len() > 16 * 1024 { return Err(error("config_invalid", "云端配置超过容量限制")); }
    let config: ProviderConfig = serde_json::from_str(&text).map_err(|_|error("config_invalid", "云端配置格式无效"))?;
    lab_provider_scope::rewrite_endpoint(&config)?; Ok(config)
}

fn mode() -> ModeSnapshot {
    ModeSnapshot { mode_id:"structure".into(), name:"结构".into(), base_mode:BaseMode::Structure, rules:String::new(), revision:1 }
}

fn source(case: &Value) -> Result<SourceRevision, CloudError> {
    let id = case["id"].as_str().ok_or_else(||error("fixture_invalid", "合成实验缺少编号"))?;
    let texts = case["items"].as_array().filter(|items|!items.is_empty() && items.len() <= 16)
        .ok_or_else(||error("fixture_invalid", "合成实验文本无效"))?;
    let items = texts.iter().enumerate().map(|(index, text)| Ok(SourceItem {
        id:format!("{id}:{index}"), provider_item_id:index.to_string(), capture_order:index as u64,
        text:text.as_str().filter(|text|text.chars().count()<=2_000).ok_or_else(||error("fixture_invalid", "合成实验文本超出范围"))?.into(),
        is_final:true, start_ms:None, end_ms:None,
    })).collect::<Result<Vec<_>, CloudError>>()?;
    Ok(SourceRevision { id:format!("source-{id}"), session_id:format!("synthetic-{id}"),
        asr_attempt_id:format!("fixed-{id}"), generation:1, provider:"synthetic".into(), model:"fixed-text".into(),
        adapter_version:"quality-text-v1".into(), complete:true, created_at:"2026-10-08T00:00:00Z".into(), items })
}

struct PlannedCall { case_id:String, arm:String, source:SourceRevision, body:Value }
fn plan(fixture: &Value, config: &ProviderConfig, candidate_only: bool) -> Result<Vec<PlannedCall>, CloudError> {
    if fixture["schemaVersion"] != 1 || fixture["maximumCalls"] != 8 || fixture["automaticRetries"] != 0 {
        return Err(error("fixture_invalid", "实验上限或版本无效"));
    }
    let baseline = fixture["baselineSystem"].as_str().ok_or_else(||error("fixture_invalid", "没有冻结基线提示词"))?;
    if fixture["baselineSystemSha256"] != hash(baseline.as_bytes()) { return Err(error("fixture_invalid", "冻结基线哈希不一致")); }
    let cases = fixture["cases"].as_array().filter(|cases|cases.len()==4).ok_or_else(||error("fixture_invalid", "必须有四个冻结案例"))?;
    let mut calls = Vec::new();
    for case in cases {
        let source = source(case)?;
        let candidate = cloud::rewrite::request_body(config, &mode(), &source)?;
        let arms = case["armOrder"].as_array().filter(|arms|arms.len()==2 && arms[0]!=arms[1])
            .ok_or_else(||error("fixture_invalid", "实验顺序无效"))?;
        for arm in arms {
            let arm = arm.as_str().filter(|arm|matches!(*arm,"baseline"|"candidate")).ok_or_else(||error("fixture_invalid", "实验臂无效"))?;
            if candidate_only && arm != "candidate" { continue; }
            let mut body = candidate.clone();
            if arm == "baseline" { body["messages"][0]["content"] = json!(baseline); }
            calls.push(PlannedCall { case_id:case["id"].as_str().unwrap().into(), arm:arm.into(), source:source.clone(), body });
        }
    }
    Ok(calls)
}

struct Evidence { receipt: File, journal: File }
impl Evidence {
    fn new(path: &Path) -> Result<Self, CloudError> {
        if !path.is_absolute() || path.exists() || path.with_extension("jsonl").exists() {
            return Err(error("fresh_output_required", "必须指定不存在的绝对输出路径"));
        }
        let receipt = OpenOptions::new().create_new(true).write(true).open(path).map_err(|_|error("evidence_unavailable", "不能创建实验结果"))?;
        let journal = OpenOptions::new().create_new(true).write(true).open(path.with_extension("jsonl")).map_err(|_|error("evidence_unavailable", "不能创建增量记录"))?;
        Ok(Self { receipt, journal })
    }
    fn write(&mut self, row: &Value, receipt: &Value) -> Result<(), CloudError> {
        serde_json::to_writer(&mut self.journal,row).map_err(|_|error("evidence_write_failed", "实验增量记录写入失败"))?;
        self.journal.write_all(b"\n").and_then(|_|self.journal.flush()).and_then(|_|self.journal.sync_data())
            .map_err(|_|error("evidence_write_failed", "实验增量记录未持久化"))?;
        self.receipt.seek(SeekFrom::Start(0)).and_then(|_|self.receipt.set_len(0)).map_err(|_|error("evidence_write_failed", "实验结果更新失败"))?;
        serde_json::to_writer_pretty(&mut self.receipt,receipt).map_err(|_|error("evidence_write_failed", "实验结果写入失败"))?;
        self.receipt.flush().and_then(|_|self.receipt.sync_data()).map_err(|_|error("evidence_write_failed", "实验结果未持久化"))
    }
}

async fn request(client: &reqwest::Client, config: &ProviderConfig, secret: &str, body: &Value) -> Result<Value,CloudError> {
    let mut request = client.post(lab_provider_scope::rewrite_endpoint(config)?).bearer_auth(secret).json(body).timeout(Duration::from_secs(20));
    if let Some(workspace) = config.workspace.as_deref().filter(|workspace|!workspace.is_empty()) { request=request.header("X-DashScope-WorkSpace",workspace); }
    let mut response = request.send().await.map_err(|failure|error(if failure.is_timeout(){"rewrite_timeout"}else{"rewrite_network"},"合成实验请求未完成"))?;
    if !response.status().is_success() {
        return Err(error(match response.status().as_u16(){401|403=>"authentication_failed",429=>"rate_limited",_=>"rewrite_service_error"}, "整理服务未接受合成实验请求"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|failure|error(if failure.is_timeout(){"rewrite_timeout"}else{"rewrite_network"},"合成实验响应未完成"))? {
        if bytes.len()+chunk.len()>1024*1024 { return Err(error("response_too_large", "合成实验响应超过容量限制")); }
        bytes.extend_from_slice(&chunk);
    }
    let data: Value = serde_json::from_slice(&bytes).map_err(|_|error("invalid_response", "合成实验响应格式无效"))?;
    let content = data.pointer("/choices/0/message/content").and_then(Value::as_str).ok_or_else(||error("empty_output", "合成实验没有响应文本"))?;
    Ok(json!({"modelContent":content,"finishReason":data.pointer("/choices/0/finish_reason").and_then(Value::as_str),
        "usage":{"promptTokens":data.pointer("/usage/prompt_tokens").and_then(Value::as_u64),
            "completionTokens":data.pointer("/usage/completion_tokens").and_then(Value::as_u64),
            "totalTokens":data.pointer("/usage/total_tokens").and_then(Value::as_u64)}}))
}

async fn run(config: ProviderConfig, out: PathBuf, execute: bool, candidate_only: bool) -> Result<(),CloudError> {
    let fixture: Value = serde_json::from_str(FIXTURE).map_err(|_|error("fixture_invalid", "冻结实验格式无效"))?;
    let calls = plan(&fixture, &config, candidate_only)?;
    let maximum_calls = calls.len();
    let planned = calls.iter().enumerate().map(|(index,call)| json!({"index":index,"caseId":call.case_id,"arm":call.arm,
        "requestSha256":hash(&serde_json::to_vec(&call.body).unwrap()),
        "systemSha256":hash(call.body["messages"][0]["content"].as_str().unwrap().as_bytes()), "source":call.source})).collect::<Vec<_>>();
    let mut receipt = json!({"schemaVersion":1,"tool":"speakmode-rewrite-quality-probe", "appVersion":env!("CARGO_PKG_VERSION"),
        "scope":if candidate_only { json!("DIAGNOSTIC_CANDIDATE_ONLY_SYNTHETIC_TEXT_NOT_PAIRED_NOT_HUMAN_QUALITY") } else { fixture["scope"].clone() },
        "runKind":if candidate_only {"diagnostic-candidate-only"}else{"paired-comparison"},
        "fixtureSha256":hash(FIXTURE.as_bytes()),"baselineCommit":fixture["baselineCommit"],
        "baselineRewriteSourceSha256":fixture["baselineRewriteSourceSha256"],"orderSeed":fixture["orderSeed"],
        "maximumCalls":maximum_calls,"automaticRetries":0,"parserForBothArms":"current-candidate-parser",
        "humanQualityMeasured":false,"historyRead":false,"configuration":{"region":config.region,"rewriteModel":config.rewrite_model},
        "reviewChecklists":fixture["cases"].as_array().unwrap().iter().map(|case|json!({"caseId":case["id"],"checklist":case["reviewChecklist"]})).collect::<Vec<_>>(),
        "plan":planned,"results":[],"startedAt":Utc::now().to_rfc3339(),"status":if execute{"running"}else{"planned_only"}});
    let mut evidence = Evidence::new(&out)?;
    evidence.write(&json!({"kind":"header","receipt":receipt}),&receipt)?;
    if !execute { println!("planned_only: {maximum_calls} fixed synthetic calls; no credential access or network"); return Ok(()); }
    let secret = match cloud::credentials::read_for(&config, cloud::credentials::CredentialStage::Rewrite) {
        Ok(secret)=>secret,
        Err(failure)=>{receipt["status"]=json!("halted");receipt["error"]=safe(&failure);evidence.write(&json!({"kind":"credentialFailure","error":safe(&failure)}),&receipt)?;return Ok(());}
    };
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).pool_max_idle_per_host(2)
        .build().map_err(|_|error("network_setup", "无法建立合成实验请求"))?;
    let mut halted = None::<String>; let mut network_failures = 0;
    for (index, call) in calls.into_iter().enumerate() {
        let mut row = json!({"kind":"syntheticRewrite","index":index,"caseId":call.case_id,"arm":call.arm});
        if let Some(reason) = &halted { row["status"]=json!("skipped");row["reason"]=json!(reason); }
        else {
            let started=Instant::now();
            match request(&client,&config,&secret,&call.body).await {
                Ok(result)=>{
                    row["response"]=result;
                    let content = row["response"]["modelContent"].as_str().unwrap();
                    match cloud::rewrite::parse_model_output(content,&mode(),&call.source) {
                        Ok(parsed)=>{row["currentParser"]=json!("passed");row["validatedOutput"]=serde_json::to_value(parsed).unwrap();}
                        Err(failure)=>{row["currentParser"]=json!("failed");row["validationError"]=safe(&failure);}
                    }
                    row["status"]=json!(if row["response"]["finishReason"]!="stop"{"truncated"}else if row["currentParser"]=="passed"{"parsed"}else{"rejected"});
                }
                Err(failure)=>{
                    if matches!(failure.code.as_str(),"authentication_failed"|"rate_limited") { halted=Some(failure.code.clone()); }
                    if matches!(failure.code.as_str(),"rewrite_network"|"rewrite_timeout"|"rewrite_service_error") {
                        network_failures+=1;if network_failures>=3 { halted=Some("three_network_or_service_failures".into()); }
                    }
                    row["status"]=json!("failed");row["error"]=safe(&failure);
                }
            }
            row["elapsedMs"]=json!(started.elapsed().as_millis() as u64);
        }
        println!("synthetic {} {}: {}",call.case_id,call.arm,row["status"].as_str().unwrap());
        receipt["results"].as_array_mut().unwrap().push(row.clone());evidence.write(&row,&receipt)?;
    }
    receipt["status"]=json!(if halted.is_some(){"halted"}else{"completed"});receipt["finishedAt"]=json!(Utc::now().to_rfc3339());
    evidence.write(&json!({"kind":"summary","status":receipt["status"]}),&receipt)
}

fn execution_options(flags: &[String]) -> Result<(bool,bool),CloudError> {
    let mut execute = false; let mut candidate_only = false;
    for flag in flags {
        match flag.as_str() {
            "--execute-cloud" if !execute => execute = true,
            "--candidate-only" if !candidate_only => candidate_only = true,
            _ => return Err(error("usage", "实验选项未知或重复")),
        }
    }
    Ok((execute,candidate_only))
}

#[tokio::main]
async fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let result = async {
        if !(4..=6).contains(&args.len()) || args[0]!="--config-db" || args[2]!="--out" {
            return Err(error("usage", "使用 --config-db ABS --out ABS_NEW.json [--execute-cloud] [--candidate-only]"));
        }
        let (execute,candidate_only) = execution_options(&args[4..])?;
        run(read_config(Path::new(&args[1]))?,PathBuf::from(&args[3]),execute,candidate_only).await
    }.await;
    if let Err(failure)=result { eprintln!("{}",safe(&failure));std::process::exit(1); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frozen_plan_has_eight_calls_and_only_system_prompt_differs_between_arms() {
        let fixture: Value=serde_json::from_str(FIXTURE).unwrap();
        let mut config=ProviderConfig::default();config.region=Some("beijing".into());
        let calls=plan(&fixture,&config,false).unwrap();assert_eq!(calls.len(),8);
        for pair in calls.chunks_exact(2) {
            assert_eq!(pair[0].source,pair[1].source);assert_ne!(pair[0].arm,pair[1].arm);
            let mut one=pair[0].body.clone();let mut two=pair[1].body.clone();
            one["messages"][0]["content"]=Value::Null;two["messages"][0]["content"]=Value::Null;
            assert_eq!(one,two);assert!(pair[0].source.items.iter().all(|item|item.is_final));
        }
    }
    #[test]
    fn tampered_baseline_is_rejected_before_credentials_or_requests() {
        let mut fixture: Value=serde_json::from_str(FIXTURE).unwrap();fixture["baselineSystem"]=json!("changed");
        assert!(plan(&fixture,&ProviderConfig::default(),false).is_err());
    }
    #[test]
    fn candidate_only_plan_is_four_diagnostic_calls_with_identical_frozen_sources() {
        let fixture: Value=serde_json::from_str(FIXTURE).unwrap();
        let config=ProviderConfig::default();
        let paired=plan(&fixture,&config,false).unwrap();let diagnostic=plan(&fixture,&config,true).unwrap();
        assert_eq!(diagnostic.len(),4);
        for (original,call) in paired.iter().filter(|call|call.arm=="candidate").zip(diagnostic) {
            assert_eq!(call.arm,"candidate");assert_eq!(call.case_id,original.case_id);
            assert_eq!(call.source,original.source);assert_eq!(call.body,original.body);
        }
    }
    #[test]
    fn candidate_only_does_not_implicitly_authorize_cloud_execution() {
        assert_eq!(execution_options(&["--candidate-only".into()]).unwrap(),(false,true));
        assert_eq!(execution_options(&["--execute-cloud".into()]).unwrap(),(true,false));
        assert_eq!(execution_options(&["--candidate-only".into(),"--execute-cloud".into()]).unwrap(),(true,true));
        assert!(execution_options(&["--candidate-only".into(),"--candidate-only".into()]).is_err());
        assert!(execution_options(&["--execute-cloudd".into()]).is_err());
    }
}
