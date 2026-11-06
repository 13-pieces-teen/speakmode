//! Frozen reviewer-only factorial. Plan first; at most eight calls, no retries,
//! generation, repair, microphone, profile writes or delivery authorization.
use anyhow::{Context,ensure};
use rusqlite::{Connection,OpenFlags};
use serde::{Deserialize,Serialize};
use serde_json::{Value,json};
use sha2::{Digest,Sha256};
use speakmode_lib::{cloud::{self,rewrite::{self,minimum_pipeline::PersonalizationLabEvent,semantic_review::preference_lab}},
    core::{ModeSnapshot,OutputBlock,ProviderConfig,SourceRevision,writing_context::WritingContext}};
use std::{fs::{self,File,OpenOptions},io::{Seek,SeekFrom,Write},path::Path,sync::{Arc,Mutex},time::Duration};

const LIMIT:usize=8;
const WAIT_MS:u64=20_000;
const FIXTURE_SHA256:&str="3e20ad2cf4d0408f81ad122dd4f0f0a654e3e32bb92f4918a929d354f44d69a8";
#[derive(Clone,Deserialize,Serialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
struct FrozenDraft{resolved_mode:speakmode_lib::core::BaseMode,blocks:Vec<OutputBlock>}
impl FrozenDraft{fn draft(&self)->rewrite::RewriteResult{rewrite::RewriteResult{resolved_mode:self.resolved_mode,blocks:self.blocks.clone()}}}
#[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Fixture{schema_version:u32,split:String,source:SourceRevision,draft_good:FrozenDraft,draft_bad:FrozenDraft,
    mode:ModeSnapshot,context:WritingContext,criteria_evaluator_only:Value,provenance:Value}
struct Arm{draft_kind:&'static str,profile_arm:&'static str,separated:bool,draft:rewrite::RewriteResult,context:WritingContext,plan:Value}
fn sha(bytes:&[u8])->String{format!("{:x}",Sha256::digest(bytes))}
fn hash(value:&impl Serialize)->anyhow::Result<String>{Ok(sha(&serde_json::to_vec(value)?))}
fn provider_config(path:&str)->anyhow::Result<ProviderConfig>{
    let db=Connection::open_with_flags(path,OpenFlags::SQLITE_OPEN_READ_ONLY|OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let data:String=db.query_row("SELECT data FROM provider_config WHERE singleton=1",[],|row|row.get(0))?;
    ensure!(data.len()<=16*1024,"provider config too large");let config:ProviderConfig=serde_json::from_str(&data)?;
    cloud::rewrite_endpoint(&config)?;
    ensure!(config.rewrite_provider=="aliyun"&&config.rewrite_model=="qwen3.8-flash"&&config.region.as_deref()==Some("beijing"),"this frozen diagnostic requires Aliyun Beijing qwen3.8-flash");
    Ok(config)
}
fn prepare(config:&ProviderConfig,fixture:&Fixture)->anyhow::Result<Vec<Arm>>{
    ensure!(fixture.schema_version==1&&fixture.split=="development","frozen development fixture required");
    ensure!(hash(&fixture.source)?==fixture.provenance["receiptSourceSha256"].as_str().unwrap_or(""),"original source binding mismatch");
    ensure!(hash(&fixture.draft_good)?==fixture.provenance["receiptDraftSha256"].as_str().unwrap_or(""),"original D draft binding mismatch");
    ensure!(hash(&fixture.context)?==fixture.provenance["receiptContextSha256"].as_str().unwrap_or(""),"original D context binding mismatch");
    ensure!(fixture.source.text().chars().count()<=4000,"short fixed diagnostic only");
    fixture.context.validate().map_err(|_|anyhow::anyhow!("invalid context"))?;
    let profile=fixture.context.personalization.as_ref().context("full D profile required")?;
    ensure!(profile.rules.iter().filter(|rule|rule.key=="instructions").count()==1,"exactly one instructions rule required");
    let good=fixture.draft_good.draft();let bad=fixture.draft_bad.draft();
    ensure!(good.blocks.len()==1&&bad.blocks.len()==1,"single fixed block required");
    ensure!(good.resolved_mode==bad.resolved_mode&&good.blocks[0].id==bad.blocks[0].id&&good.blocks[0].source_refs==bad.blocks[0].source_refs,"negative changes text only");
    let from="只有收到供应商发票后我才提交报销申请";let to="我会提交报销申请";
    ensure!(good.blocks[0].text.matches(from).count()==1&&bad.blocks[0].text==good.blocks[0].text.replacen(from,to,1),"negative must remove only invoice prerequisite");
    let mut stripped=fixture.context.clone();stripped.personalization.as_mut().unwrap().rules.retain(|rule|rule.key!="instructions");
    // Do not re-freeze a config or refill omitted fields. Only this one rule changes.
    let mut arms=Vec::new();
    for (profile_index,(profile_arm,context)) in [("fullD",&fixture.context),("withoutInstructions",&stripped)].into_iter().enumerate(){
        for (draft_index,(draft_kind,draft)) in [("good",&good),("missingInvoice",&bad)].into_iter().enumerate(){
            for separated in if (profile_index+draft_index)%2==0{[false,true]}else{[true,false]}{
                let request=preference_lab::prepare(config,&fixture.mode,&fixture.source,draft,Some(context),separated)?;
                ensure!(request.body["max_tokens"]==512,"output budget must remain512 in both arms");
                let plan=json!({"draftKindEvaluatorOnly":draft_kind,"profileArm":profile_arm,"separated":separated,
                    "sourceSha256":hash(&fixture.source)?,"draftSha256":hash(draft)?,"modeSha256":hash(&fixture.mode)?,
                    "contextSha256":hash(context)?,"providerPersonalizationSha256":hash(&context.personalization.as_ref().unwrap().provider_data())?,
                    "requestBodySha256":hash(&request.body)?,"metadata":request.metadata});
                arms.push(Arm{draft_kind,profile_arm,separated,draft:draft.clone(),context:context.clone(),plan});
            }
        }
    }
    ensure!(arms.len()==LIMIT,"exactly8 fixed review arms required");Ok(arms)
}
fn durable(file:&mut File,value:&Value)->std::io::Result<()>{serde_json::to_writer(&mut *file,value)?;file.write_all(b"\n")?;file.flush()?;file.sync_data()}
fn receipt(file:&mut File,value:&Value)->anyhow::Result<()>{let bytes=serde_json::to_vec_pretty(value)?;file.seek(SeekFrom::Start(0))?;file.write_all(&bytes)?;file.set_len(bytes.len()as u64)?;file.flush()?;file.sync_all()?;Ok(())}
struct Ledger{file:File,starts:usize,failed:bool}
impl Ledger{
    fn append(&mut self,value:&Value)->bool{if self.failed{return false;}if durable(&mut self.file,value).is_err(){self.failed=true;return false;}true}
    fn observe(&mut self,profile:&str,kind:&str,separated:bool,event:PersonalizationLabEvent)->bool{
        let start=event.event=="started";if start&&self.starts>=LIMIT{self.failed=true;return false;}
        let allowed=self.append(&json!({"kind":"request_event","profileArm":profile,"draftKind":kind,"separated":separated,"event":event,"clientStartedRequestsBefore":self.starts}));
        if start&&allowed{self.starts+=1;}allowed
    }
}
fn lock(ledger:&Arc<Mutex<Ledger>>)->std::sync::MutexGuard<'_,Ledger>{ledger.lock().unwrap_or_else(|poison|{let mut guard=poison.into_inner();guard.failed=true;guard})}
fn evaluate(row:&Value)->Value{
    if !row["outcome"]["review"]["checks"].is_object(){return json!({"status":"NOT_EVALUATED","contentLossDetected":null,"contentPassed":null,"instructionsMarkedConflict":null});}
    let checks=&row["outcome"]["review"]["checks"];
    let loss_caught=checks["coverage"]=="fail"||checks["scope"]=="fail";
    let content_pass=row["outcome"]["contentAccepted"]==true;
    let ignored=row["outcome"]["preferenceChecks"].as_array().and_then(|rules|rules.iter().find(|rule|rule["key"]=="instructions")).map(|rule|rule["applicability"]=="ignore_conflict");
    json!({"status":"MODEL_JUDGMENT","contentLossDetected":loss_caught,"contentPassed":content_pass,"instructionsMarkedConflict":ignored,
        "interpretation":"MODEL_JUDGMENT_NOT_HUMAN_QUALITY_OR_SEMANTIC_PROOF"})
}

#[tokio::main]
async fn main()->anyhow::Result<()>{
    let args=std::env::args().skip(1).collect::<Vec<_>>();ensure!(args.len()==4&&["--plan","--run"].contains(&args[3].as_str()),"CONFIG_DB FIXTURE NEW_OUT --plan|--run");
    ensure!(args[..3].iter().all(|arg|Path::new(arg).is_absolute()),"absolute paths required");
    let bytes=fs::read(&args[1])?;ensure!(bytes.len()<=256*1024,"fixture too large");ensure!(sha(&bytes)==FIXTURE_SHA256,"frozen fixture bytes differ; no requests authorized");
    let fixture:Fixture=serde_json::from_slice(&bytes)?;let config=provider_config(&args[0])?;let arms=prepare(&config,&fixture)?;
    let mut result=json!({"tool":"personalization-preference-review-lab-v1","status":"PLANNED","scope":"FIXED_DRAFT_REVIEW_ONLY_NOT_NORMAL_LATENCY",
        "fixtureSha256":sha(&bytes),"executableSha256":sha(&fs::read(std::env::current_exe()?)?),
        "runnerSourceSha256":sha(include_bytes!("personalization_preference_review_lab.rs")),"moduleSourceSha256":sha(include_bytes!("../src/cloud/rewrite_personalization_review_lab.rs")),
        "semanticReviewSourceSha256":sha(include_bytes!("../src/cloud/rewrite_semantic_review.rs")),"writingContextSourceSha256":sha(include_bytes!("../src/cloud/rewrite_writing_context.rs")),
        "providerConfigSha256":hash(&config)?,"rewriteProvider":config.rewrite_provider,"rewriteModel":config.rewrite_model,
        "maximumClientStartedRequests":LIMIT,"clientStartedRequests":0,"deadlinePerRequestMs":WAIT_MS,"outputLimitTokens":512,"automaticRetries":0,"redirects":0,
        "requestCountBoundary":"Authorized pipeline starts; not billing or provider receipt","actualTokenUsage":null,
        "baseline":"EXPLICIT_NAMED_CONTROL_NOT_ALIYUN_DEFAULT_ORDERED","profileContrast":"Only remove the frozen instructions rule; do not reselect/refill other data",
        "criteriaEvaluatorOnly":fixture.criteria_evaluator_only,"provenance":fixture.provenance,
        "plan":arms.iter().map(|arm|&arm.plan).collect::<Vec<_>>(),"rows":[],"deliveryAuthorized":false,"generation":"NOT_RUN","repair":"NOT_RUN","audio":"NOT_RUN","userQuality":"NOT_RUN"});
    let plan_path=format!("{}.plan.json",args[2]);let ledger_path=format!("{}.jsonl",args[2]);
    ensure!(!Path::new(&args[2]).exists()&&!Path::new(&ledger_path).exists(),"fresh receipt and ledger required");
    if args[3]=="--plan"{let mut file=OpenOptions::new().write(true).create_new(true).open(plan_path)?;receipt(&mut file,&result)?;println!("PLANNED:8 fixed reviews; credentials/network NOT_RUN");return Ok(());}
    let frozen:Value=serde_json::from_slice(&fs::read(plan_path).context("run --plan first")?)?;ensure!(frozen==result,"frozen plan differs; no requests sent");
    let mut output=OpenOptions::new().write(true).create_new(true).open(&args[2])?;
    let journal=match OpenOptions::new().write(true).create_new(true).open(ledger_path){Ok(file)=>file,Err(error)=>{
        result["status"]=json!("HALTED");result["haltReason"]=json!("journal_create_failed");
        result["rows"]=json!(arms.iter().map(|arm|json!({"draftKind":arm.draft_kind,"profileArm":arm.profile_arm,"separated":arm.separated,"status":"NOT_RUN","reason":"journal_create_failed"})).collect::<Vec<_>>());
        receipt(&mut output,&result)?;return Err(error.into());
    }};
    let ledger=Arc::new(Mutex::new(Ledger{file:journal,starts:0,failed:false}));receipt(&mut output,&result)?;
    if !lock(&ledger).append(&json!({"kind":"plan","receipt":result})){
        result["status"]=json!("HALTED");result["haltReason"]=json!("initial_ledger_failure");
        result["rows"]=json!(arms.iter().map(|arm|json!({"draftKind":arm.draft_kind,"profileArm":arm.profile_arm,"separated":arm.separated,"status":"NOT_RUN","reason":"initial_ledger_failure"})).collect::<Vec<_>>());
        receipt(&mut output,&result)?;anyhow::bail!("initial ledger failed; no credentials/network");
    }
    let credential=cloud::credentials::read_for(&config,cloud::credentials::CredentialStage::Rewrite);
    let mut stop=None;let secret=match credential{Ok(secret)=>Some(secret),Err(error)=>{result["credentialError"]=json!(error);stop=Some("credential_failure");None}};
    let mut rows=Vec::new();
    for arm in arms{
        if let Some(reason)=stop{rows.push(json!({"draftKind":arm.draft_kind,"profileArm":arm.profile_arm,"separated":arm.separated,"status":"NOT_RUN","reason":reason}));continue;}
        let before=lock(&ledger).starts;
        if !lock(&ledger).append(&json!({"kind":"arm_started","profileArm":arm.profile_arm,"draftKind":arm.draft_kind,"separated":arm.separated,"reservedRequests":1})){stop=Some("ledger_failure");rows.push(json!({"draftKind":arm.draft_kind,"profileArm":arm.profile_arm,"separated":arm.separated,"status":"NOT_RUN","reason":"ledger_failure"}));continue;}
        let observed=Arc::clone(&ledger);let profile=arm.profile_arm;let kind=arm.draft_kind;let separated=arm.separated;
        let outcome=preference_lab::run(&config,secret.as_deref().unwrap(),&fixture.mode,&fixture.source,&arm.draft,Some(&arm.context),separated,
            Duration::from_millis(WAIT_MS),move|event|lock(&observed).observe(profile,kind,separated,event)).await;
        let mut row=match outcome{Ok(value)=>json!({"draftKind":kind,"profileArm":profile,"separated":separated,"status":"COMPLETED","outcome":value}),
            Err(error)=>{stop=Some("review_error_no_retry");json!({"draftKind":kind,"profileArm":profile,"separated":separated,"status":"ERROR","error":error})}};
        row["evaluation"]=evaluate(&row);
        // Inspect model content checks independently of native admission. A
        // native guard's rejection must not conceal a candidate reviewer miss.
        if separated&&kind=="missingInvoice"&&row["outcome"].is_object()&&row["evaluation"]["contentLossDetected"]!=true{stop=Some("candidate_did_not_detect_invoice_condition_loss");}
        {let mut guard=lock(&ledger);row["clientStartedRequests"]=json!(guard.starts-before);guard.append(&json!({"kind":"arm_completed","row":row}));if guard.failed{stop=Some("ledger_failure");}result["clientStartedRequests"]=json!(guard.starts);}
        println!("{profile} {kind} separated={separated}: accepted={} error={}",row["outcome"]["accepted"],row["error"]["code"]);
        rows.push(row);result["status"]=json!("RUNNING");result["rows"]=json!(rows);
        if receipt(&mut output,&result).is_err(){stop=Some("receipt_failure");}
    }
    result["status"]=json!(if stop.is_some(){"HALTED"}else{"COMPLETED"});result["rows"]=json!(rows);result["haltReason"]=json!(stop);
    if !lock(&ledger).append(&json!({"kind":"run_finished","receipt":result})){result["status"]=json!("HALTED");result["haltReason"]=json!("ledger_failure");}
    receipt(&mut output,&result)?;if result["status"]=="HALTED"{anyhow::bail!("halted; evidence preserved");}Ok(())
}

#[cfg(test)]mod tests{
    use super::*;
    fn fixture()->Fixture{serde_json::from_str(include_str!("../../../docs/experiments/fixtures/personalization-preference-review.dev.json")).unwrap()}
    fn config()->ProviderConfig{ProviderConfig{region:Some("beijing".into()),..Default::default()}}
    #[test]fn fixed_factorial_changes_only_assigned_context_rule_and_review_protocol(){
        let fixture=fixture();let config=config();let arms=prepare(&config,&fixture).unwrap();assert_eq!(arms.len(),8);
        assert_eq!(arms.iter().map(|arm|arm.separated).collect::<Vec<_>>(),[false,true,true,false,true,false,false,true]);
        for pair in arms.chunks_exact(2){for key in ["sourceSha256","draftSha256","modeSha256","contextSha256","providerPersonalizationSha256"]{assert_eq!(pair[0].plan[key],pair[1].plan[key]);}
            for arm in pair{let request=preference_lab::prepare(&config,&fixture.mode,&fixture.source,&arm.draft,Some(&arm.context),arm.separated).unwrap();
                let data:Value=serde_json::from_str(request.body["messages"][1]["content"].as_str().unwrap()).unwrap();
                assert_eq!(data["sourceText"],fixture.source.text());assert_eq!(data["draftText"],arm.draft.blocks[0].text);
                for key in ["criteriaEvaluatorOnly","provenance","draftKindEvaluatorOnly","expected","goodFacts","badMissing"]{assert!(data.get(key).is_none());}
                assert_eq!(request.body["max_tokens"],512);
            }
        }
        let mut expected=fixture.context;expected.personalization.as_mut().unwrap().rules.retain(|rule|rule.key!="instructions");
        for arm in arms.iter().skip(4){assert_eq!(arm.context,expected);}
    }
    #[test]fn model_miss_is_visible_even_when_native_guard_rejected_the_draft(){
        let row=json!({"outcome":{"contentAccepted":true,"nativeAdmissionAccepted":false,"accepted":false,"review":{"checks":{"coverage":"pass","scope":"pass"}}}});
        let result=evaluate(&row);assert_eq!(result["contentLossDetected"],false);assert_eq!(result["contentPassed"],true);
    }
    #[test]fn errors_are_not_counted_as_model_rejections_and_original_bindings_are_enforced(){
        let result=evaluate(&json!({"status":"ERROR","error":{"code":"invalid_preference_review"}}));
        assert_eq!(result["status"],"NOT_EVALUATED");assert!(result["contentPassed"].is_null());assert!(result["contentLossDetected"].is_null());
        assert_eq!(sha(include_bytes!("../../../docs/experiments/fixtures/personalization-preference-review.dev.json")),FIXTURE_SHA256);
        let mut modified=fixture();modified.context.personalization.as_mut().unwrap().rules[0].text.push('。');
        assert!(prepare(&config(),&modified).err().expect("modified context must fail").to_string().contains("context binding"));
    }
    #[test]fn failed_journal_and_exhausted_budget_cannot_authorize_more_starts(){
        let path=std::env::temp_dir().join(format!("preference-journal-{}",uuid::Uuid::new_v4()));fs::write(&path,b"seed").unwrap();
        let event=||PersonalizationLabEvent{stage:"review",event:"started",duration_ms:None,remaining_budget_ms:1000,outcome:None};
        {let mut ledger=Ledger{file:File::open(&path).unwrap(),starts:0,failed:false};assert!(!ledger.observe("fullD","good",true,event()));assert!(ledger.failed);assert_eq!(ledger.starts,0);}
        {let mut ledger=Ledger{file:OpenOptions::new().append(true).open(&path).unwrap(),starts:LIMIT,failed:false};assert!(!ledger.observe("fullD","good",true,event()));assert!(ledger.failed);assert_eq!(ledger.starts,LIMIT);}
        assert_eq!(fs::read(&path).unwrap(),b"seed");fs::remove_file(path).unwrap();
    }
}
