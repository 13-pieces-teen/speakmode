//! Bounded semantic-review experiment; no production selection or native input.
//! Review: <=16 fixed drafts, one call each. Flow: <=16 cases, <=2 calls each.
//! Flow reviews every shape-valid, admitted draft; legacy checks are diagnostics.
//! Credentials/config are read-only. Every call is preregistered and journaled.
#![recursion_limit="256"]
use chrono::Utc;
use rusqlite::{Connection,OpenFlags};
use serde::Deserialize;
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
use speakmode_lib::{cloud::{self,rewrite,CloudError},core::{BaseMode,DictionarySnapshot,ModeSnapshot,ProviderConfig,SourceItem,SourceRevision}};
use std::{collections::HashSet,fs::{File,OpenOptions},io::{Read,Seek,SeekFrom,Write},path::{Path,PathBuf},time::{Duration,Instant}};

const TOOL:&str="speakmode-semantic-quality-lab";
const HELP:&str="semantic_quality_lab --config-db ABS --fixture ABS --out ABS_NEW.json --stage review|flow [--model MODEL] [--prompt-profile fluent-v1|long-v1|protected-long-v1|protected-long-v2] (--plan-only|--run). Review<=16 calls; flow<=32 calls. No retries, history/audio/dictionary reads, config writes, clipboard or native input. Plan-only never reads credentials or uses network.";
fn error(code:&str,message:&str)->CloudError{CloudError::new(code,"semantic_quality_lab",message)}
fn safe(e:&CloudError)->Value{json!({"code":e.code,"stage":e.stage,"message":e.message})}
fn hash(bytes:&[u8])->String{format!("{:x}",Sha256::digest(bytes))}
#[derive(Clone,Copy,PartialEq,Eq,Debug)] enum Stage{Review,Flow}
impl Stage{fn name(self)->&'static str{match self{Self::Review=>"review",Self::Flow=>"flow"}}fn maximum(self,count:usize)->usize{count*if self==Self::Flow{2}else{1}}}
#[derive(Clone,Copy,PartialEq,Eq,Debug)]enum PromptProfile{FluentV1,LongV1,ProtectedLongV1,ProtectedLongV2,GuardedLongV3}
impl PromptProfile{fn name(self)->&'static str{match self{Self::FluentV1=>"fluent-v1",Self::LongV1=>"long-v1",Self::ProtectedLongV1=>"protected-long-v1",Self::ProtectedLongV2=>"protected-long-v2",Self::GuardedLongV3=>"guarded-long-v3"}}}
struct Options{config:PathBuf,fixture:PathBuf,out:PathBuf,stage:Stage,model:Option<String>,prompt_profile:PromptProfile,plan_only:bool}
fn options(args:&[String])->Result<Options,CloudError>{
    let mut values=std::collections::HashMap::new();let mut mode=None;let mut index=0;
    while index<args.len(){
        let key=args[index].as_str();
        if matches!(key,"--plan-only"|"--run") {if mode.replace(key=="--plan-only").is_some(){return Err(error("usage",HELP));}index+=1;continue;}
        if !["--config-db","--fixture","--out","--stage","--model","--prompt-profile"].contains(&key)||index+1>=args.len()
            ||values.insert(key,args[index+1].clone()).is_some(){return Err(error("usage",HELP));}
        index+=2;
    }
    let path=|key|->Result<PathBuf,CloudError>{let value=values.get(key).ok_or_else(||error("usage",HELP))?;let path=PathBuf::from(value);
        if !path.is_absolute(){return Err(error("absolute_path_required","所有文件路径必须为绝对路径"));}Ok(path)};
    Ok(Options{config:path("--config-db")?,fixture:path("--fixture")?,out:path("--out")?,
        stage:match values.get("--stage").map(String::as_str){Some("review")=>Stage::Review,Some("flow")=>Stage::Flow,_=>return Err(error("usage",HELP))},
        model:values.get("--model").cloned(),
        prompt_profile:match values.get("--prompt-profile").map(String::as_str){None|Some("fluent-v1")=>PromptProfile::FluentV1,Some("long-v1")=>PromptProfile::LongV1,Some("protected-long-v1")=>PromptProfile::ProtectedLongV1,Some("protected-long-v2")=>PromptProfile::ProtectedLongV2,Some("guarded-long-v3")=>PromptProfile::GuardedLongV3,_=>return Err(error("usage",HELP))},
        plan_only:mode.ok_or_else(||error("usage",HELP))?})
}
fn read_config(path:&Path)->Result<ProviderConfig,CloudError>{
    let connection=Connection::open_with_flags(path,OpenFlags::SQLITE_OPEN_READ_ONLY|OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_|error("config_unavailable","不能只读访问云端配置"))?;
    let text:String=connection.query_row("SELECT data FROM provider_config WHERE singleton=1",[],|row|row.get(0))
        .map_err(|_|error("config_unavailable","没有有效云端配置"))?;
    if text.len()>16*1024{return Err(error("config_invalid","云端配置超过容量限制"));}
    let config:ProviderConfig=serde_json::from_str(&text).map_err(|_|error("config_invalid","云端配置格式无效"))?;
    cloud::endpoints(&config)?;Ok(config)
}
#[derive(Deserialize)]#[serde(deny_unknown_fields,rename_all="camelCase")]
struct Case{id:String,source_items:Vec<String>,draft:Option<Draft>,expected_decision:Option<String>,reason:Option<String>,category:Option<String>}
#[derive(Deserialize)]#[serde(untagged)]enum Draft{Text(String),Object(DraftObject)}
#[derive(Deserialize)]#[serde(deny_unknown_fields,rename_all="camelCase")]
struct DraftObject{resolved_mode:BaseMode,text:String}
fn read_fixture(path:&Path,stage:Stage)->Result<(Value,Vec<Case>,String),CloudError>{
    let mut bytes=Vec::new();File::open(path).map_err(|_|error("fixture_unavailable","不能读取指定案例"))?
        .take(1024*1024+1).read_to_end(&mut bytes).map_err(|_|error("fixture_unavailable","不能读取指定案例"))?;
    if bytes.len()>1024*1024{return Err(error("fixture_invalid","案例超过1MiB限制"));}
    let value:Value=serde_json::from_slice(&bytes).map_err(|_|error("fixture_invalid","案例必须为UTF-8 JSON对象"))?;
    let cases:Vec<Case>=serde_json::from_value(value.get("cases").cloned().ok_or_else(||error("fixture_invalid","案例缺少cases"))?)
        .map_err(|_|error("fixture_invalid","案例字段无效"))?;
    if cases.is_empty()||cases.len()>16{return Err(error("fixture_invalid","案例数必须为1到16"));}
    let mut ids=HashSet::new();
    for case in &cases{
        if case.id.is_empty()||case.id.len()>80||!case.id.bytes().all(|c|c.is_ascii_alphanumeric()||matches!(c,b'-'|b'_'))||!ids.insert(&case.id)
            ||case.source_items.is_empty()||case.source_items.len()>32||case.source_items.iter().any(|text|text.trim().is_empty())
            ||case.source_items.iter().map(|text|text.chars().count()).sum::<usize>()>8000
            ||case.expected_decision.as_deref().is_some_and(|value|!matches!(value,"accept"|"reject"))
            ||case.reason.as_ref().is_some_and(|value|value.chars().count()>1000)
            ||case.category.as_ref().is_some_and(|value|value.chars().count()>200)
            ||(stage==Stage::Review)!=case.draft.is_some(){return Err(error("fixture_invalid","案例身份、长度、分组或预期无效"));}
    }
    Ok((value,cases,hash(&bytes)))
}
fn mode()->ModeSnapshot{ModeSnapshot{mode_id:"lab-auto".into(),name:"Auto".into(),base_mode:BaseMode::Auto,rules:String::new(),revision:1}}
fn source(case:&Case)->SourceRevision{
    SourceRevision{id:format!("source-{}",case.id),session_id:format!("synthetic-{}",case.id),asr_attempt_id:format!("fixed-{}",case.id),generation:1,
        provider:"synthetic".into(),model:"fixed-text".into(),adapter_version:"semantic-review-lab-v1".into(),complete:true,created_at:"2026-10-11T00:00:00Z".into(),
        items:case.source_items.iter().enumerate().map(|(index,text)|SourceItem{id:format!("{}:{index}",case.id),provider_item_id:index.to_string(),capture_order:index as u64,
            text:text.clone(),is_final:true,start_ms:None,end_ms:None}).collect()}
}
fn fixed_draft(case:&Case,source:&SourceRevision)->Result<rewrite::RewriteResult,CloudError>{
    let (resolved,text)=match case.draft.as_ref().ok_or_else(||error("fixture_invalid","review案例必须提供草稿"))?{
        Draft::Text(text)=>(BaseMode::Light,text),Draft::Object(draft)=>(draft.resolved_mode,&draft.text)};
    rewrite::parse_fluent_draft(&json!({"resolvedMode":resolved,"text":text}).to_string(),&mode(),source)
}
struct Planned{source:SourceRevision,draft:Option<rewrite::RewriteResult>,body:Value,binding:Option<rewrite::semantic_review::ReviewBinding>,protected:Option<rewrite::ProtectedLongRequest>}
fn plan(cases:&[Case],stage:Stage,profile:PromptProfile,config:&ProviderConfig)->Result<Vec<Planned>,CloudError>{
    cases.iter().map(|case|{
        let source=source(case);
        if stage==Stage::Review{let draft=fixed_draft(case,&source)?;let request=rewrite::semantic_review::prepare(config,&mode(),&source,&draft)?;
            Ok(Planned{source,draft:Some(draft),body:request.body,binding:Some(request.binding),protected:None})}
        else{let (body,protected)=match profile{
            PromptProfile::FluentV1=>(rewrite::fluent_request_body(config,&mode(),&source,&DictionarySnapshot::default())?,None),
            PromptProfile::LongV1=>(rewrite::long_request_body(config,&mode(),&source,&DictionarySnapshot::default())?,None),
            PromptProfile::ProtectedLongV1=>{let request=rewrite::prepare_protected_long(config,&mode(),&source,&DictionarySnapshot::default())?;
                (request.body.clone(),Some(request))},
            PromptProfile::GuardedLongV3=>{let request=rewrite::prepare_guarded_long(config,&mode(),&source,&DictionarySnapshot::default())?;(request.body.clone(),Some(request))},
            PromptProfile::ProtectedLongV2=>{let request=rewrite::prepare_protected_long_compact(config,&mode(),&source,&DictionarySnapshot::default())?;
                (request.body.clone(),Some(request))},
        };Ok(Planned{source,draft:None,body,binding:None,protected})}
    }).collect()
}
fn generation_prompt_version(body:&Value)->Option<String>{
    let versions=body.get("messages")?.as_array()?.iter().find(|message|message["role"]=="system")?
        .get("content")?.as_str()?.lines().filter_map(|line|line.strip_prefix("协议版本：")
            .or_else(||line.strip_prefix("协议附加版本："))).map(str::trim).collect::<Vec<_>>();
    (!versions.is_empty()).then(||versions.join("+"))
}
fn parse_flow_draft(profile:PromptProfile,content:&str,planned:&Planned)->Result<rewrite::RewriteResult,CloudError>{
    match (profile,planned.protected.as_ref()){
        // The frozen protection context must be used before any draft is
        // recorded, admitted or reviewed. Never fall back to the plain parser.
        (PromptProfile::ProtectedLongV1|PromptProfile::ProtectedLongV2|PromptProfile::GuardedLongV3,Some(request))=>rewrite::parse_protected_long(content,&mode(),&planned.source,request),
        (PromptProfile::FluentV1|PromptProfile::LongV1,None)=>rewrite::parse_fluent_draft(content,&mode(),&planned.source),
        _=>Err(error("invalid_protection_context","实验提示词与保护上下文不一致")),
    }
}
struct Evidence{receipt:File,journal:File}
impl Evidence{
    fn new(path:&Path)->Result<Self,CloudError>{
        if !path.is_absolute()||path.extension().is_none_or(|e|e!="json")||path.exists()||path.with_extension("jsonl").exists(){return Err(error("fresh_output_required","输出须为不存在的绝对JSON路径"));}
        Ok(Self{receipt:OpenOptions::new().create_new(true).write(true).open(path).map_err(|_|error("evidence_unavailable","不能创建结果"))?,
            journal:OpenOptions::new().create_new(true).write(true).open(path.with_extension("jsonl")).map_err(|_|error("evidence_unavailable","不能创建日志"))?})
    }
    fn write(&mut self,event:&Value,receipt:&Value)->Result<(),CloudError>{
        serde_json::to_writer(&mut self.journal,event).map_err(|_|error("evidence_write_failed","不能写入日志"))?;
        self.journal.write_all(b"\n").and_then(|_|self.journal.flush()).and_then(|_|self.journal.sync_data()).map_err(|_|error("evidence_write_failed","不能持久化日志"))?;
        self.receipt.seek(SeekFrom::Start(0)).and_then(|_|self.receipt.set_len(0)).map_err(|_|error("evidence_write_failed","不能更新结果"))?;
        serde_json::to_writer_pretty(&mut self.receipt,receipt).map_err(|_|error("evidence_write_failed","不能写入结果"))?;
        self.receipt.flush().and_then(|_|self.receipt.sync_data()).map_err(|_|error("evidence_write_failed","不能持久化结果"))
    }
}
struct RawResponse{status:u16,body:String}
async fn request(client:&reqwest::Client,config:&ProviderConfig,secret:&str,body:&Value)->Result<RawResponse,CloudError>{
    let operation=async{
        let mut request=client.post(cloud::endpoints(config)?.rewrite).bearer_auth(secret).json(body).timeout(Duration::from_secs(20));
        if let Some(workspace)=config.workspace.as_deref().filter(|value|!value.is_empty()){request=request.header("X-DashScope-WorkSpace",workspace);}
        let mut response=request.send().await.map_err(|e|error(if e.is_timeout(){"rewrite_timeout"}else{"rewrite_network"},"实验请求未完成"))?;
        let status=response.status().as_u16();let mut bytes=Vec::new();
        while let Some(chunk)=response.chunk().await.map_err(|e|error(if e.is_timeout(){"rewrite_timeout"}else{"rewrite_network"},"实验响应中断"))?{
            if bytes.len()+chunk.len()>1024*1024{return Err(error("response_too_large","实验响应超过1MiB限制"));}bytes.extend_from_slice(&chunk);
        }
        Ok(RawResponse{status,body:String::from_utf8(bytes).map_err(|_|error("invalid_response","实验响应不是UTF-8"))?})
    };
    tokio::time::timeout(Duration::from_secs(20),operation).await.map_err(|_|error("rewrite_timeout","实验请求超过20秒"))?
}
#[derive(Default)]struct RunState{calls:usize,network_failures:usize,halted:Option<String>}
impl RunState{fn failure(&mut self,code:&str){
    if matches!(code,"authentication_failed"|"rate_limited"){self.halted=Some(code.into());}
    if matches!(code,"rewrite_network"|"rewrite_timeout"|"rewrite_service_error"){self.network_failures+=1;if self.network_failures>=3{self.halted=Some("three_network_or_service_failures".into());}}
}}
fn decode(raw:&RawResponse)->Result<Value,CloudError>{
    if !(200..300).contains(&raw.status){return Err(error(match raw.status{401|403=>"authentication_failed",429=>"rate_limited",_=>"rewrite_service_error"},"云服务未接受实验请求"));}
    let body:Value=serde_json::from_str(&raw.body).map_err(|_|error("invalid_response","实验响应JSON无效"))?;
    let content=body.pointer("/choices/0/message/content").and_then(Value::as_str).filter(|text|!text.trim().is_empty()).ok_or_else(||error("empty_output","模型没有返回文本"))?;
    Ok(json!({"modelContent":content,"finishReason":body.pointer("/choices/0/finish_reason"),"modelReported":body.get("model"),"usage":body.get("usage")}))
}
async fn call(client:&reqwest::Client,config:&ProviderConfig,secret:&str,case_id:&str,phase:&str,body:&Value,
    binding:Option<&rewrite::semantic_review::ReviewBinding>,state:&mut RunState,evidence:&mut Evidence,receipt:&mut Value)->Result<Value,CloudError>{
    if state.halted.is_some()||state.calls>=receipt["maximumCalls"].as_u64().unwrap() as usize{return Err(error("call_limit","实验已停止或达到固定调用上限"));}
    let index=state.calls;state.calls+=1;receipt["cloudRequests"]=json!(state.calls);
    receipt["pendingCall"]=json!({"index":index,"caseId":case_id,"phase":phase,"requestBody":body,"requestSha256":hash(&serde_json::to_vec(body).unwrap()),"binding":binding});
    evidence.write(&json!({"kind":"callStarted","call":receipt["pendingCall"]}),receipt)?;
    let started=Instant::now();let response=request(client,config,secret,body).await;
    let mut row=json!({"index":index,"caseId":case_id,"phase":phase,"elapsedMs":started.elapsed().as_millis() as u64,"binding":binding,
        "requestSha256":hash(&serde_json::to_vec(body).unwrap()),"requestedModel":body.get("model"),
        "messagesSha256":hash(&serde_json::to_vec(&body["messages"]).unwrap())});
    match response{
        Ok(mut raw)=>{
            // An official error should not echo credentials, but never persist
            // one if it does. Record that explicit exception to raw retention.
            let redacted=!secret.is_empty()&&raw.body.contains(secret);
            if redacted{raw.body=raw.body.replace(secret,"[REDACTED_CREDENTIAL]");}
            row["httpStatus"]=json!(raw.status);row["rawResponse"]=json!(raw.body);row["credentialRedacted"]=json!(redacted);
            match decode(&raw){Ok(decoded)=>{row["status"]=json!("response");row["response"]=decoded;},Err(failure)=>{state.failure(&failure.code);row["status"]=json!("failed");row["error"]=safe(&failure);}}
        },
        Err(failure)=>{state.failure(&failure.code);row["status"]=json!("failed");row["error"]=safe(&failure);}
    }
    receipt.as_object_mut().unwrap().remove("pendingCall");receipt["calls"].as_array_mut().unwrap().push(row.clone());
    evidence.write(&json!({"kind":"callCompleted","call":row}),receipt)?;Ok(row)
}
fn model_content(row:&Value)->Result<&str,CloudError>{
    if row["status"]!="response"{return Err(error("request_failed","实验请求失败，未运行解析"));}
    if row["response"]["finishReason"]!="stop"{return Err(error("truncated_output","模型响应未完整结束"));}
    row["response"]["modelContent"].as_str().ok_or_else(||error("empty_output","模型没有返回文本"))
}
fn judgment(result:Result<(),CloudError>)->Value{match result{Ok(())=>json!({"passed":true}),Err(failure)=>json!({"passed":false,"error":safe(&failure)})}}
fn flow_review_diagnostics(source:&SourceRevision,draft:&rewrite::RewriteResult)->(Value,Value,bool){
    let legacy=judgment(rewrite::validate_draft_meaning(source,draft));
    let admission=rewrite::review_admission(source,draft);let requires_review=admission.is_ok();
    (legacy,judgment(admission),requires_review)
}
fn review_response(response:&Value,binding:&rewrite::semantic_review::ReviewBinding,source:&SourceRevision,draft:&rewrite::RewriteResult)->Value{
    let started=Instant::now();
    let parsed=model_content(response).and_then(|content|rewrite::semantic_review::parse(content,binding,source,draft));
    match parsed{Ok(review)=>json!({"parser":"passed","decision":if review.accepted(){"accept"}else{"reject"},"review":review,"parserElapsedMicros":started.elapsed().as_micros() as u64}),
        Err(failure)=>json!({"parser":"failed","decision":"reject","error":safe(&failure),"parserElapsedMicros":started.elapsed().as_micros() as u64})}
}
fn initial_budget_ms(source:&SourceRevision)->u64{
    // Match RewriteWaitPolicy::for_source(NormalInitial): Unicode scalar count
    // of the exact frozen source, including the newlines between ASR items.
    rewrite::initial_wait_ms(source)
}
fn projection(stage:Stage,source:&SourceRevision,accepted:bool,elapsed:u64)->Value{
    let deadline=initial_budget_ms(source);
    json!({"scope":if stage==Stage::Flow{"SIMULATED_INITIAL_FLOW_SOURCE_LENGTH_BUDGET_NOT_NATIVE_DELIVERY"}else{"REVIEW_ONLY_SOURCE_LENGTH_BUDGET_GENERATION_LATENCY_NOT_MEASURED"},
        "budgetPolicy":"normal_initial_unicode_le_400_5s_le_800_8s_else_20s","sourceChars":source.text().chars().count(),
        "deadlineMs":deadline,"timedOut":elapsed>=deadline,"eligibleWithinBudget":accepted&&elapsed<deadline,
        "status":if stage==Stage::Review{"review_stage_only"}else if accepted&&elapsed<deadline{"rewrite_candidate"}else{"source_fallback"},
        "reason":if elapsed>=deadline{"timeout"}else if accepted{"accepted"}else{"rejected"},"actualNativeDelivery":"NOT_RUN"})
}
async fn run(options:Options)->Result<(),CloudError>{
    let (fixture,cases,fixture_hash)=read_fixture(&options.fixture,options.stage)?;
    let saved=read_config(&options.config)?;let mut config=saved.clone();if let Some(model)=&options.model{config.rewrite_model=model.clone();}cloud::endpoints(&config)?;
    let plans=plan(&cases,options.stage,options.prompt_profile,&config)?;
    let plan_json=cases.iter().zip(&plans).enumerate().map(|(index,(case,planned))|json!({"index":index,"caseId":case.id,"source":planned.source,"draft":planned.draft,
        "requestBody":planned.body,"requestSha256":hash(&serde_json::to_vec(&planned.body).unwrap()),"messagesSha256":hash(&serde_json::to_vec(&planned.body["messages"]).unwrap()),"binding":planned.binding,
        "sourceChars":planned.source.text().chars().count(),"normalInitialBudgetMs":initial_budget_ms(&planned.source),
        "generationPromptVersion":if options.stage==Stage::Flow{generation_prompt_version(&planned.body)}else{None},
        "expectedDecision":case.expected_decision,"reason":case.reason,"category":case.category,"expectationSentToModel":false})).collect::<Vec<_>>();
    let mut receipt=json!({"schemaVersion":2,"tool":TOOL,"stage":options.stage.name(),"status":if options.plan_only{"planned_only"}else{"running"},"appVersion":env!("CARGO_PKG_VERSION"),
        "requestedPromptProfile":options.prompt_profile.name(),"generationPromptProfile":if options.stage==Stage::Flow{Some(options.prompt_profile.name())}else{None},
        "generationPromptVersion":if options.stage==Stage::Flow{plans.first().and_then(|planned|generation_prompt_version(&planned.body))}else{None},
        "protectedDraftPolicy":"protected-long-v1 and protected-long-v2 must restore and validate using their frozen ProtectedLongRequest before admission and semantic review; no ordinary-parser fallback; v1 remains independently selectable",
        "scope":"FIXED_TEXT_EXPERIMENT_NOT_ASR_OR_NATIVE_DELIVERY","fixtureScope":fixture.get("scope"),"fixtureSha256":fixture_hash,"runnerSha256":hash(include_bytes!("semantic_quality_lab.rs")),
        "reviewSha256":hash(include_bytes!("../src/cloud/rewrite_semantic_review.rs")),"rewriteSha256":hash(include_bytes!("../src/cloud/rewrite.rs")),
        "reviewPromptVersion":rewrite::semantic_review::SEMANTIC_REVIEW_PROMPT_VERSION,"caseCount":cases.len(),"maximumCalls":options.stage.maximum(cases.len()),
        "cloudRequests":0,"automaticRetries":0,"perRequestTimeoutSeconds":20,"configuredModel":saved.rewrite_model,"experimentalModel":config.rewrite_model,"modelOverridePersisted":false,
        "sourceConstruction":"synthetic sourceItems preserved exactly and in order; sourceText inserts newline between items","mode":mode(),"dictionaryPolicy":"EMPTY_NO_PROFILE_DICTIONARY_READ",
        "configurationWritten":false,"historyRead":false,"microphoneRead":false,"nativeInputDispatched":false,"plan":plan_json,"calls":[],"results":[],
        "reviewOnlyPolicy":"Every supplied draft receives one review; legacy/admission results are diagnostics, not stage-review call gates.",
        "flowPolicy":"One generation using the recorded prompt profile; every strict-shape-valid draft passing deterministic review_admission receives exactly one semantic review. Legacy meaning checks are diagnostics only, never an acceptance or review-skip path. Review never rewrites text.",
        "callCountMeaning":"Attempts preregistered immediately before dispatch; a network failure may occur before the service receives it.",
        "semanticVerdict":"MODEL_REVIEW_NOT_INDEPENDENT_HUMAN_PROOF","startedAt":Utc::now().to_rfc3339()});
    let mut evidence=Evidence::new(&options.out)?;evidence.write(&json!({"kind":"preregistration","receipt":receipt}),&receipt)?;
    if options.plan_only{println!("planned_only: {} cases, <= {} calls; no credentials or network",cases.len(),options.stage.maximum(cases.len()));return Ok(());}
    let secret=match cloud::credentials::read(&config){Ok(secret)=>secret,Err(failure)=>{receipt["status"]=json!("halted");receipt["error"]=safe(&failure);evidence.write(&json!({"kind":"credentialFailure","error":safe(&failure)}),&receipt)?;return Err(error("credential_unavailable","未发请求；凭据不可用，实验记录已保留"));}};
    let client=match reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).pool_max_idle_per_host(2).pool_idle_timeout(Duration::from_secs(60)).build(){
        Ok(client)=>client,Err(_)=>{let failure=error("network_setup","不能创建实验客户端");receipt["status"]=json!("halted");receipt["error"]=safe(&failure);evidence.write(&json!({"kind":"networkSetupFailure","error":safe(&failure)}),&receipt)?;return Err(failure);}};
    let mut state=RunState::default();
    for (index,(case,planned)) in cases.iter().zip(plans).enumerate(){
        let started=Instant::now();let mut row=json!({"index":index,"caseId":case.id,"stage":options.stage.name(),"generationPromptProfile":if options.stage==Stage::Flow{Some(options.prompt_profile.name())}else{None},
            "generationPromptVersion":if options.stage==Stage::Flow{generation_prompt_version(&planned.body)}else{None},
            "expectedDecision":case.expected_decision,"reason":case.reason,"category":case.category,"status":"evaluated"});
        if let Some(reason)=&state.halted{row["status"]=json!("skipped");row["reason"]=json!(reason);}
        else if options.stage==Stage::Review{
            let draft=planned.draft.as_ref().unwrap();let binding=planned.binding.as_ref().unwrap();
            row["legacyGuard"]=judgment(rewrite::validate_draft_meaning(&planned.source,draft));row["admission"]=judgment(rewrite::review_admission(&planned.source,draft));
            let response=call(&client,&config,&secret,&case.id,"review",&planned.body,Some(binding),&mut state,&mut evidence,&mut receipt).await?;
            row["reviewCallIndex"]=response["index"].clone();row["reviewElapsedMs"]=response["elapsedMs"].clone();row["review"]=review_response(&response,binding,&planned.source,draft);
            row["decision"]=row["review"]["decision"].clone();row["binding"]=json!(binding);
        }else{
            let response=call(&client,&config,&secret,&case.id,"generation",&planned.body,None,&mut state,&mut evidence,&mut receipt).await?;
            row["generationCallIndex"]=response["index"].clone();row["generationElapsedMs"]=response["elapsedMs"].clone();
            let parse_started=Instant::now();let draft=model_content(&response).and_then(|content|parse_flow_draft(options.prompt_profile,content,&planned));
            row["shapeParserElapsedMicros"]=json!(parse_started.elapsed().as_micros() as u64);
            match draft{
                Err(failure)=>{row["shapeParser"]=json!("failed");row["error"]=safe(&failure);row["decision"]=json!("reject");row["reviewSkipped"]=json!("invalid_generation_or_shape");},
                Ok(draft)=>{
                    row["shapeParser"]=json!("passed");row["draft"]=json!(draft);
                    let guard_started=Instant::now();let (legacy,admission,requires_review)=flow_review_diagnostics(&planned.source,&draft);row["legacyGuard"]=legacy;row["admission"]=admission;
                    row["guardDiagnosticsElapsedMicros"]=json!(guard_started.elapsed().as_micros() as u64);
                    if !requires_review{row["decision"]=json!("reject");row["reviewSkipped"]=json!("admission_rejected");}
                    else if state.halted.is_some(){row["decision"]=json!("reject");row["reviewSkipped"]=json!("experiment_halted");}
                    else{
                        let prepare_started=Instant::now();let request=rewrite::semantic_review::prepare(&config,&mode(),&planned.source,&draft)?;row["binding"]=json!(request.binding);
                        row["reviewPrepareElapsedMicros"]=json!(prepare_started.elapsed().as_micros() as u64);
                        let response=call(&client,&config,&secret,&case.id,"review",&request.body,Some(&request.binding),&mut state,&mut evidence,&mut receipt).await?;
                        row["reviewCallIndex"]=response["index"].clone();row["reviewElapsedMs"]=response["elapsedMs"].clone();row["review"]=review_response(&response,&request.binding,&planned.source,&draft);
                        row["decision"]=row["review"]["decision"].clone();if row["decision"]=="accept"{row["acceptedBy"]=json!("semantic_reviewer");}
                    }
                }
            }
        }
        row["elapsedMs"]=json!(started.elapsed().as_millis() as u64);
        row["projection"]=if row["status"]=="skipped"{json!({"status":"not_projected","actualNativeDelivery":"NOT_RUN"})}else{projection(options.stage,&planned.source,row["decision"]=="accept",row["elapsedMs"].as_u64().unwrap())};
        row["expectationMet"]=case.expected_decision.as_ref().map(|expected|json!(row["decision"].as_str()==Some(expected.as_str()))).unwrap_or(Value::Null);
        receipt["results"].as_array_mut().unwrap().push(row.clone());evidence.write(&json!({"kind":"caseCompleted","result":row}),&receipt)?;
        println!("{} {}: {}",options.stage.name(),case.id,row["decision"].as_str().unwrap_or("skipped"));
    }
    receipt["status"]=json!(if state.halted.is_some(){"halted"}else{"completed"});receipt["haltReason"]=json!(state.halted);receipt["finishedAt"]=json!(Utc::now().to_rfc3339());
    evidence.write(&json!({"kind":"summary","status":receipt["status"],"cloudRequests":state.calls}),&receipt)
}
#[tokio::main]async fn main(){
    let args=std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty()||args==["--help"]{println!("{HELP}");return;}
    let result=match options(&args){Ok(options)=>run(options).await,Err(failure)=>Err(failure)};
    if let Err(failure)=result{eprintln!("{}: {}",failure.code,failure.message);std::process::exit(1);}
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn semantic_lab_options_require_one_stage_and_one_execution_mode(){
        let args=["--config-db","C:/config.sqlite","--fixture","C:/fixture.json","--out","C:/fresh.json","--stage","review","--plan-only"].map(str::to_owned);
        assert_eq!(options(&args).unwrap().stage,Stage::Review);
        assert_eq!(options(&args).unwrap().prompt_profile,PromptProfile::FluentV1);
        let mut with_profile=args.to_vec();with_profile.extend(["--prompt-profile".into(),"long-v1".into()]);
        assert_eq!(options(&with_profile).unwrap().prompt_profile,PromptProfile::LongV1);
        *with_profile.last_mut().unwrap()="protected-long-v1".into();assert_eq!(options(&with_profile).unwrap().prompt_profile,PromptProfile::ProtectedLongV1);
        *with_profile.last_mut().unwrap()="protected-long-v2".into();assert_eq!(options(&with_profile).unwrap().prompt_profile,PromptProfile::ProtectedLongV2);
        *with_profile.last_mut().unwrap()="unknown".into();assert!(options(&with_profile).is_err());
        let mut invalid=args.to_vec();invalid.push("--run".into());assert!(options(&invalid).is_err());
        assert!(options(&args[..8]).is_err());let mut invalid=args.to_vec();invalid[7]="all".into();assert!(options(&invalid).is_err());
    }
    #[test]fn semantic_lab_protected_profile_never_uses_plain_parser_without_context(){
        let case=Case{id:"PROTECTED".into(),source_items:vec!["测试麦克风。".into()],draft:None,expected_decision:None,reason:None,category:None};
        let planned=Planned{source:source(&case),draft:None,body:Value::Null,binding:None,protected:None};
        let content=r#"{"resolvedMode":"light","text":"测试麦克风。"}"#;
        assert!(parse_flow_draft(PromptProfile::FluentV1,content,&planned).is_ok());
        assert_eq!(parse_flow_draft(PromptProfile::ProtectedLongV1,content,&planned).unwrap_err().code,"invalid_protection_context");
        assert_eq!(parse_flow_draft(PromptProfile::ProtectedLongV2,content,&planned).unwrap_err().code,"invalid_protection_context");
        assert_eq!(generation_prompt_version(&json!({"messages":[{"role":"system","content":"协议版本：fixture-v7\nRules"}]})).as_deref(),Some("fixture-v7"));
        assert_eq!(generation_prompt_version(&json!({"messages":[{"role":"system","content":"协议版本：long-dictation-v1\nRules\n协议附加版本：protected-long-v1"}]})).as_deref(),Some("long-dictation-v1+protected-long-v1"));
        assert_eq!(generation_prompt_version(&json!({"messages":[{"role":"system","content":"协议版本：long-dictation-v1\nRules\n协议附加版本：protected-long-v2"}]})).as_deref(),Some("long-dictation-v1+protected-long-v2"));
    }
    #[test]fn semantic_lab_timeout_equality_and_review_only_scope_are_explicit(){
        let case=Case{id:"BUDGET".into(),source_items:vec!["字".repeat(399),"字".repeat(400)],draft:None,expected_decision:None,reason:None,category:None};
        let short=source(&case);assert_eq!(short.text().chars().count(),800);assert_eq!(initial_budget_ms(&short),8000);
        assert_eq!(projection(Stage::Flow,&short,true,7999)["status"],"rewrite_candidate");assert_eq!(projection(Stage::Flow,&short,true,8000)["status"],"source_fallback");
        let mut long=short.clone();long.items[0].text.push('字');assert_eq!(initial_budget_ms(&long),20000);
        assert_eq!(projection(Stage::Flow,&long,true,19999)["status"],"rewrite_candidate");assert_eq!(projection(Stage::Flow,&long,true,20000)["status"],"source_fallback");
        assert_eq!(projection(Stage::Review,&long,true,20)["status"],"review_stage_only");
        assert_eq!(projection(Stage::Review,&long,true,20)["scope"],"REVIEW_ONLY_SOURCE_LENGTH_BUDGET_GENERATION_LATENCY_NOT_MEASURED");
    }
    #[test]fn semantic_lab_stops_auth_and_three_transport_failures_without_retry(){
        let mut state=RunState::default();state.failure("invalid_response");assert!(state.halted.is_none());
        for code in ["rewrite_network","rewrite_timeout"]{state.failure(code);assert!(state.halted.is_none());}state.failure("rewrite_service_error");assert!(state.halted.is_some());
        let mut state=RunState::default();state.failure("authentication_failed");assert_eq!(state.halted.as_deref(),Some("authentication_failed"));
    }
    #[test]fn semantic_lab_expectations_never_enter_request_body(){
        let case=Case{id:"PUBLIC01".into(),source_items:vec!["嗯，测试麦克风。".into()],draft:Some(Draft::Text("测试麦克风。".into())),expected_decision:Some("accept".into()),reason:Some("EVAL_ONLY_SECRET_MARKER".into()),category:Some("EVAL_ONLY_CATEGORY_MARKER".into())};
        let plans=plan(&[case],Stage::Review,PromptProfile::LongV1,&ProviderConfig::default()).unwrap();let body=plans[0].body.to_string();
        assert!(!body.contains("EVAL_ONLY_SECRET_MARKER"));assert!(!body.contains("EVAL_ONLY_CATEGORY_MARKER"));assert!(!body.contains("expectedDecision"));
        assert_eq!(plans[0].source.items[0].text,"嗯，测试麦克风。");
    }
    #[test]fn semantic_lab_legacy_pass_does_not_skip_review_of_a_changed_condition_scope(){
        // Public development counterexample D04. The keyword/value checks can
        // pass while punctuation has made an independent action conditional.
        let case=Case{id:"D04".into(),source_items:vec!["如果测试通过，才发给同学。正式报告今天照常提交。".into()],
            draft:Some(Draft::Text("如果测试通过，就发给同学并在今天提交正式报告。".into())),expected_decision:Some("reject".into()),
            reason:None,category:None};
        let source=source(&case);let draft=fixed_draft(&case,&source).unwrap();
        let (legacy,admission,requires_review)=flow_review_diagnostics(&source,&draft);
        assert_eq!(legacy["passed"],true);assert_eq!(admission["passed"],true);assert!(requires_review);
    }
}
