//! Frozen micro-cleanup diagnostic: 24 synthetic cases x 3 modes x 2 prompts.
//! No history/audio access, configuration writes, retries or automated quality scores.
#[path="lab_provider_scope/mod.rs"] mod lab_provider_scope;
use chrono::Utc;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use speakmode_lib::{cloud::{self, CloudError}, core::{BaseMode, ModeSnapshot, ProviderConfig, SourceItem, SourceRevision}};
use std::{fs::{self, File, OpenOptions}, io::{Seek, SeekFrom, Write}, path::{Path, PathBuf}, time::{Duration, Instant}};

const FIXTURE: &str = include_str!("../../../docs/experiments/fixtures/cleanup-quality-v1.json");
const MAX_CALLS: usize = 144;
const MODEL_DIAGNOSTIC_CALLS: usize = 72;
const HELP: &str = "cleanup_quality_lab --config-db ABS --out ABS_NEW.json [--candidate-model qwen-plus] [--plan-only]\ncleanup_quality_lab --replay ABS_RECEIPT.json --out ABS_NEW.json\nDefault: fixed 144 synthetic cloud requests using configured model. Explicit candidate-model: 72 candidate-only requests, in-memory model override without configuration writes. No retries. Plan-only has no credential or network access. Replay is entirely offline and never reads configuration or credentials.";

fn error(code: &str, message: &str) -> CloudError { CloudError::new(code, "cleanup_quality_lab", message) }
fn hash(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn safe(failure: &CloudError) -> Value { json!({"code":failure.code,"stage":failure.stage,"message":failure.message}) }
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
        generation:1,provider:"synthetic".into(),model:"fixed-text".into(),adapter_version:"cleanup-quality-v1".into(),
        complete:true,created_at:"2026-10-10T00:00:00Z".into(),items})
}
struct PlannedCall {case_id:String,arm:String,mode:ModeSnapshot,source:SourceRevision,body:Value}
fn plan(fixture: &Value, config: &ProviderConfig) -> Result<Vec<PlannedCall>, CloudError> {
    let frozen: Value = serde_json::from_str(FIXTURE).expect("checked-in corpus is valid");
    if *fixture != frozen { return Err(error("fixture_invalid", "只能运行固定的公开合成语料")); }
    let cases = fixture["cases"].as_array().filter(|cases|cases.len()==24).ok_or_else(||error("fixture_invalid", "冻结语料必须有24条"))?;
    let mut ids=std::collections::HashSet::new();let mut calls=Vec::with_capacity(MAX_CALLS);
    for (case_index,case) in cases.iter().enumerate() {
        let id=case["id"].as_str().ok_or_else(||error("fixture_invalid", "案例缺少编号"))?;
        if !ids.insert(id) {return Err(error("fixture_invalid", "案例编号重复"));}
        let source=source(case)?;
        for (mode_index,base) in [BaseMode::Light,BaseMode::Structure,BaseMode::Auto].into_iter().enumerate() {
            let mode=mode(base,case["formatRules"].as_str().unwrap_or(""));
            let arms=if (case_index+mode_index)%2==0 {["baseline","candidate"]}else{["candidate","baseline"]};
            for arm in arms {
                let body=if arm=="baseline" {cloud::rewrite::baseline_request_body(config,&mode,&source)?}else{cloud::rewrite::cleanup_request_body(config,&mode,&source)?};
                calls.push(PlannedCall{case_id:id.into(),arm:arm.into(),mode:mode.clone(),source:source.clone(),body});
            }
        }
    }
    if calls.len()!=MAX_CALLS{return Err(error("plan_limit", "固定调用数不符"));}Ok(calls)
}
fn unvalidated_text(content:&str)->Option<String>{
    let value:Value=serde_json::from_str(content).ok()?;
    let blocks=value["blocks"].as_array()?;
    let texts=blocks.iter().map(|block|block["text"].as_str()).collect::<Option<Vec<_>>>()?;
    (!texts.is_empty()).then(||texts.join("\n\n"))
}
fn necessary_checks(text:&str,case:&Value)->Value {
    let hits=|key:&str|case[key].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|term|text.contains(term)).collect::<Vec<_>>();
    let missing=case["requiredAny"].as_array().into_iter().flatten().enumerate().filter_map(|(i,group)|{
        let present=group.as_array().is_some_and(|terms|terms.iter().filter_map(Value::as_str).any(|term|text.contains(term)));
        (!present).then_some(i)
    }).collect::<Vec<_>>();
    let redundant=hits("mustRemove");let forbidden=hits("forbidden");
    json!({"necessaryChecksPass":redundant.is_empty()&&forbidden.is_empty()&&missing.is_empty(),"remainingRemovableTerms":redundant,"forbiddenHits":forbidden,"missingRequiredAnyGroups":missing,"semanticVerdict":"unreviewed","note":"String checks cannot establish semantic fidelity or duplicate removal."})
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

fn diagnostic_plan(fixture:&Value, config:&ProviderConfig, candidate_model:Option<&str>)->Result<Vec<PlannedCall>,CloudError>{
    if candidate_model.is_some_and(|model|model!="qwen-plus"){return Err(error("model_not_allowed","此诊断只允许显式 qwen-plus 候选臂"));}
    let mut calls=plan(fixture,config)?;
    if candidate_model.is_some(){calls.retain(|call|call.arm=="candidate");}
    let expected=if candidate_model.is_some(){MODEL_DIAGNOSTIC_CALLS}else{MAX_CALLS};
    if calls.len()!=expected{return Err(error("plan_limit","固定诊断调用数不符"));}Ok(calls)
}
async fn run(config_path: &Path, out: &Path, plan_only: bool, candidate_model:Option<&str>) -> Result<(),CloudError> {
    let fixture:Value=serde_json::from_str(FIXTURE).expect("checked-in fixture");
    let fixture_hash=hash(FIXTURE.as_bytes());
    let mut config=read_config(config_path)?;
    let configured_model=config.rewrite_model.clone();
    if let Some(model)=candidate_model{config.rewrite_model=model.into();}
    let calls=diagnostic_plan(&fixture,&config,candidate_model)?;
    let maximum_calls=calls.len();
    let planned = calls.iter().enumerate().map(|(index,call)| {
        let mut parameters = call.body.clone();parameters.as_object_mut().unwrap().remove("messages");
        json!({"index":index,"caseId":call.case_id,"arm":call.arm,"mode":call.mode.base_mode,
            "requestSha256":hash(&serde_json::to_vec(&call.body).unwrap()),
            "systemSha256":hash(call.body["messages"][0]["content"].as_str().unwrap().as_bytes()),
            "sourceSha256":hash(&serde_json::to_vec(&call.source).unwrap()),"source":call.source,"parameters":parameters,"requestBody":call.body})
    }).collect::<Vec<_>>();
    let mut receipt = json!({"schemaVersion":1,"tool":"speakmode-cleanup-quality-lab","appVersion":env!("CARGO_PKG_VERSION"),
        "scope":if candidate_model.is_some(){"POSTHOC_SAME_CORPUS_MODEL_DIAGNOSTIC_NOT_NEW_HOLDOUT"}else{"FROZEN_SYNTHETIC_TEXT_NOT_HUMAN_AUDIO_OR_NATIVE_DELIVERY"},
        "fixtureSha256":fixture_hash,"runnerSha256":hash(include_bytes!("cleanup_quality_lab.rs")),
        "candidateSha256":hash(include_bytes!("../src/cloud/rewrite_cleanup.rs")),
        "baselineSha256":hash(include_bytes!("../src/cloud/rewrite_baseline.rs")),
        "parserSha256":hash(include_bytes!("../src/cloud/rewrite.rs")),
        "integritySha256":hash(include_bytes!("../src/cloud/rewrite_integrity.rs")),
        "factsSha256":hash(include_bytes!("../src/cloud/rewrite_facts.rs")),
        "semanticsSha256":hash(include_bytes!("../src/cloud/rewrite_semantics.rs")),
        "candidatePromptVersion":cloud::rewrite::CLEANUP_REWRITE_PROMPT_VERSION,
        "productionPromptVersion":cloud::rewrite::REWRITE_PROMPT_VERSION,
        "orderRule":if candidate_model.is_some(){"One frozen candidate arm, corpus order then Light Structure Auto; separate stage from prior Flash requests"}else{"Adjacent pairs, baseline first when (case_index + mode_index) is even; candidate first otherwise"},
        "maximumCalls":maximum_calls,"candidateModelOverride":candidate_model,"originalConfiguredModel":configured_model,"automaticRetries":0,"repeats":1,"perRequestTimeoutSeconds":20,
        "parserForBothArms":"same-current-parser","humanQualityMeasured":false,"historyRead":false,
        "configurationWritten":false,"configuration":{"region":config.region,"rewriteModel":config.rewrite_model},
        "reviewChecklists":fixture["cases"],"scoring":fixture["scoring"],
        "plan":planned,"results":[],"startedAt":Utc::now().to_rfc3339(),"status":if plan_only{"planned_only"}else{"running"}});
    let mut evidence = Evidence::new(out)?;
    // Durable, immutable-per-run preregistration precedes credential access.
    evidence.write(&json!({"kind":"preregistration","receipt":receipt}), &receipt)?;
    if plan_only {println!("planned_only: {maximum_calls} calls; no credential access or network");return Ok(());}
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
            receipt["pendingCall"]=json!({"index":index,"caseId":call.case_id,"arm":call.arm});
            evidence.write(&json!({"kind":"callStarted","index":index,"caseId":call.case_id,"arm":call.arm,"mode":call.mode.base_mode}), &receipt)?;
            let cloud_started=Instant::now();
            let response=request(&client,&config,&secret,&call.body).await;
            row["cloudElapsedMs"]=json!(cloud_started.elapsed().as_millis() as u64);
            match response {
                Ok(response)=>{
                    row["response"]=response;
                    if let Some(text)=unvalidated_text(row["response"]["modelContent"].as_str().unwrap()) {
                        let case=fixture["cases"].as_array().unwrap().iter().find(|case|case["id"]==call.case_id).unwrap();
                        row["necessaryChecks"]=necessary_checks(&text,case);row["unvalidatedText"]=json!(text);
                    }
                    let parser_started=Instant::now();
                    if row["response"]["finishReason"] != "stop" {
                        row["status"]=json!("truncated");row["currentParser"]=json!("not_run");
                    } else {
                        match cloud::rewrite::parse_model_output(row["response"]["modelContent"].as_str().unwrap(),&call.mode,&call.source) {
                            Ok(parsed)=>{row["status"]=json!("parsed");row["currentParser"]=json!("passed");row["validatedOutput"]=serde_json::to_value(parsed).unwrap();}
                            Err(failure)=>{row["status"]=json!("rejected");row["currentParser"]=json!("failed");row["validationError"]=safe(&failure);}
                        }
                    }
                    row["parserElapsedMs"]=json!(parser_started.elapsed().as_micros() as u64/1000);
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
        receipt.as_object_mut().unwrap().remove("pendingCall");
        receipt["results"].as_array_mut().unwrap().push(row.clone());evidence.write(&row,&receipt)?;
    }
    receipt["status"]=json!(if halted.is_some(){"halted"}else{"completed"});receipt["finishedAt"]=json!(Utc::now().to_rfc3339());
    evidence.write(&json!({"kind":"summary","status":receipt["status"]}),&receipt)
}

fn replay(receipt_path:&Path,out:&Path)->Result<(),CloudError>{
    if !receipt_path.is_absolute()||!out.is_absolute()||out.exists(){return Err(error("fresh_output_required","离线重放需要原收据绝对路径和全新输出"));}
    let metadata=fs::metadata(receipt_path).map_err(|_|error("input_unavailable","原实验收据不可读"))?;
    if !metadata.is_file()||metadata.len()>32*1024*1024{return Err(error("input_limit","原实验收据超过容量限制"));}
    let bytes=fs::read(receipt_path).map_err(|_|error("input_unavailable","原实验收据不可读"))?;
    let receipt:Value=serde_json::from_slice(&bytes).map_err(|_|error("input_invalid","原实验收据格式无效"))?;
    let is_diagnostic=receipt["candidateModelOverride"]=="qwen-plus";
    let maximum_calls=if is_diagnostic{MODEL_DIAGNOSTIC_CALLS}else{MAX_CALLS};
    if receipt["tool"]!="speakmode-cleanup-quality-lab"||receipt["fixtureSha256"]!=hash(FIXTURE.as_bytes())||receipt["maximumCalls"]!=maximum_calls||receipt["status"]!="completed"{
        return Err(error("receipt_mismatch","离线重放只接受完整的冻结合成实验"));
    }
    let fixture:Value=serde_json::from_str(FIXTURE).unwrap();
    let original=receipt["results"].as_array().filter(|rows|rows.len()==maximum_calls).ok_or_else(||error("receipt_mismatch","原实验结果数不符"))?;
    let plan=receipt["plan"].as_array().filter(|rows|rows.len()==maximum_calls).ok_or_else(||error("receipt_mismatch","原实验计划数不符"))?;
    let mut seen=std::collections::HashSet::new();let mut identities=std::collections::HashSet::new();let mut rows=Vec::new();
    for row in original {
        let index=row["index"].as_u64().filter(|index|*index<maximum_calls as u64).ok_or_else(||error("receipt_mismatch","结果编号无效"))? as usize;
        if !seen.insert(index)||["caseId","arm","mode"].iter().any(|field|row[*field]!=plan[index][*field]){
            return Err(error("receipt_mismatch","结果与冻结计划身份不一致"));
        }
        if (is_diagnostic&&row["arm"]!="candidate")||!matches!(row["arm"].as_str(),Some("baseline"|"candidate"))||!identities.insert(format!("{}:{}:{}",row["caseId"],row["mode"],row["arm"])){
            return Err(error("receipt_mismatch","结果实验组重复或未知"));
        }
        let case=fixture["cases"].as_array().unwrap().iter().find(|case|case["id"]==row["caseId"]).ok_or_else(||error("receipt_mismatch","未知固定案例"))?;
        let source=source(case)?;
        if serde_json::to_value(&source).unwrap()!=plan[index]["source"] {return Err(error("receipt_mismatch","来源偏离冻结语料"));}
        let base:BaseMode=serde_json::from_value(row["mode"].clone()).map_err(|_|error("receipt_mismatch","模式无效"))?;
        let mode=mode(base,case["formatRules"].as_str().unwrap_or(""));
        let mut result=json!({"index":index,"caseId":row["caseId"],"arm":row["arm"],"mode":row["mode"],"originalStatus":row["status"],"cloudElapsedMs":row["cloudElapsedMs"]});
        if row["response"]["finishReason"]=="stop" {
            if let Some(content)=row["response"]["modelContent"].as_str(){
                result["replayedParser"]=match cloud::rewrite::parse_model_output(content,&mode,&source){
                    Ok(parsed)=>json!({"accepted":true,"output":parsed}),
                    Err(failure)=>json!({"accepted":false,"error":safe(&failure)}),
                };
            }else{result["replayedParser"]=json!({"accepted":false,"notRunReason":"missing_model_content"});}
        }else{result["replayedParser"]=json!({"accepted":false,"notRunReason":"not_complete_generation"});}
        rows.push(result);
    }
    let count=|arm:&str|rows.iter().filter(|row|row["arm"]==arm&&row["replayedParser"]["accepted"]==true).count();
    let summary=json!({"planned":maximum_calls,"baselineAccepted":count("baseline"),"candidateAccepted":count("candidate"),"cloudCalls":0});
    let result=json!({"tool":"speakmode-cleanup-parser-replay","schemaVersion":1,"scope":"OFFLINE_SAME_GENERATED_RESPONSES_NEW_PARSER_NOT_NEW_MODEL_QUALITY",
        "originalReceiptSha256":hash(&bytes),"fixtureSha256":hash(FIXTURE.as_bytes()),"replayedAt":Utc::now().to_rfc3339(),
        "parserSha256":hash(include_bytes!("../src/cloud/rewrite.rs")),"integritySha256":hash(include_bytes!("../src/cloud/rewrite_integrity.rs")),
        "factsSha256":hash(include_bytes!("../src/cloud/rewrite_facts.rs")),"semanticsSha256":hash(include_bytes!("../src/cloud/rewrite_semantics.rs")),
        "summary":summary,"results":rows});
    let mut file=OpenOptions::new().write(true).create_new(true).open(out).map_err(|_|error("evidence_unavailable","无法新建离线收据"))?;
    serde_json::to_writer_pretty(&mut file,&result).map_err(|_|error("evidence_write_failed","不能写入离线收据"))?;
    file.flush().and_then(|_|file.sync_data()).map_err(|_|error("evidence_write_failed","不能持久化离线收据"))?;
    println!("offline replay: {}",summary);Ok(())
}

struct Options {config:PathBuf,out:PathBuf,plan_only:bool,candidate_model:Option<String>}
fn options(args:&[String])->Result<Options,CloudError>{
    if args.len()<4||args[0]!="--config-db"||args[2]!="--out"{
        return Err(error("usage",HELP));
    }
    let mut tail=&args[4..];let mut candidate_model=None;
    if tail.first().is_some_and(|arg|arg=="--candidate-model"){
        if tail.get(1).is_none_or(|model|model!="qwen-plus"){return Err(error("usage",HELP));}
        candidate_model=Some(tail[1].clone());tail=&tail[2..];
    }
    let plan_only=tail==["--plan-only"];
    if !tail.is_empty()&&!plan_only{return Err(error("usage",HELP));}
    Ok(Options{config:args[1].clone().into(),out:args[3].clone().into(),plan_only,candidate_model})
}
#[tokio::main]
async fn main(){
    let args=std::env::args().skip(1).collect::<Vec<_>>();
    if args==["--help"]{println!("{HELP}");return;}
    let result=async{
        if args.len()==4&&args[0]=="--replay"&&args[2]=="--out" {return replay(Path::new(&args[1]),Path::new(&args[3]));}
        let args=options(&args)?;run(&args.config,&args.out,args.plan_only,args.candidate_model.as_deref()).await
    }.await;
    if let Err(failure)=result{eprintln!("{}",safe(&failure));std::process::exit(1);}
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn paired_plan_has_144_calls_and_only_the_system_differs(){
        let fixture:Value=serde_json::from_str(FIXTURE).unwrap();
        let calls=plan(&fixture,&ProviderConfig::default()).unwrap();assert_eq!(calls.len(),144);
        assert_eq!(calls.chunks_exact(2).filter(|pair|pair[0].arm=="baseline").count(),36);
        for pair in calls.chunks_exact(2){
            assert_eq!(pair[0].case_id,pair[1].case_id);assert_ne!(pair[0].arm,pair[1].arm);
            assert_eq!(pair[0].source,pair[1].source);assert_eq!(pair[0].mode.base_mode,pair[1].mode.base_mode);
            let mut a=pair[0].body.clone();let mut b=pair[1].body.clone();
            a["messages"][0]["content"]=json!("system");b["messages"][0]["content"]=json!("system");assert_eq!(a,b);
        }
    }
    #[test]
    fn explicit_correction_process_cannot_pass_necessary_checks(){
        let case=json!({"mustRemove":["我说错了","十二"],"forbidden":[],"requiredAny":[["十五","15"]]});
        assert_eq!(necessary_checks("我说错了，不是十二份，是十五份。",&case)["necessaryChecksPass"],false);
        assert_eq!(necessary_checks("十五份。",&case)["necessaryChecksPass"],true);
        assert_eq!(necessary_checks("十五份。",&case)["semanticVerdict"],"unreviewed");
    }
    #[test]
    fn cannot_substitute_private_corpus(){let mut fixture:Value=serde_json::from_str(FIXTURE).unwrap();fixture["cases"][0]["sourceItems"]=json!(["private"]);assert!(plan(&fixture,&ProviderConfig::default()).is_err());}
    #[test]
    fn unvalidated_text_is_available_even_when_semantic_parser_will_reject(){assert_eq!(unvalidated_text(r#"{"resolvedMode":"light","blocks":[{"text":"预算已安排。","sourceRange":[0,0]}]}"#).unwrap(),"预算已安排。");}
    #[test]
    fn explicit_model_diagnostic_is_exactly_72_candidate_calls(){
        let fixture:Value=serde_json::from_str(FIXTURE).unwrap();let mut config=ProviderConfig::default();config.rewrite_model="qwen-plus".into();
        let calls=diagnostic_plan(&fixture,&config,Some("qwen-plus")).unwrap();assert_eq!(calls.len(),72);
        assert!(calls.iter().all(|call|call.arm=="candidate"&&call.body["model"]=="qwen-plus"));
        assert!(diagnostic_plan(&fixture,&config,Some("unknown")).is_err());
    }
}
