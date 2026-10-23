//! Explicit fixed-draft review experiment. No recording, persistence or delivery.
#![recursion_limit="256"]
use anyhow::{Context,ensure};
use rusqlite::{Connection,OpenFlags};
use serde::{Deserialize,Serialize};
use serde_json::{Value,json};
use sha2::{Digest,Sha256};
use speakmode_lib::{cloud::{self,rewrite::{minimum_pipeline::PersonalizationLabEvent,semantic_review::coverage_lab}},
    core::{AppWritingScene,BaseMode,DraftLanguage,ExpressionStyle,ModeSnapshot,OutputBlock,PersonalizationConfig,
        ProviderConfig,SentenceStyle,SourceItem,SourceRevision,writing_context::WritingContext}};
use std::{fs::{self,File,OpenOptions},io::{Seek,SeekFrom,Write},path::Path,sync::{Arc,Mutex},time::Duration};

const LIMIT:usize=24;
const WAIT_MS:u64=20_000;
#[derive(Deserialize,Serialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Criteria {good_facts:Vec<String>,bad_missing:Vec<String>}
#[derive(Deserialize,Serialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Case {id:String,scene:AppWritingScene,source_units:Vec<String>,good_draft_units:Vec<String>,bad_draft_units:Vec<String>,criteria:Criteria}
#[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Fixture {schema_version:u32,split:String,cases:Vec<Case>}
struct Arm {
    case_id:String,draft_kind:&'static str,aligned:bool,source:SourceRevision,
    draft:cloud::rewrite::RewriteResult,mode:ModeSnapshot,context:WritingContext,manifest:coverage_lab::UnitManifest,plan:Value,
}
fn sha(bytes:&[u8])->String{format!("{:x}",Sha256::digest(bytes))}
fn hash(value:&impl Serialize)->anyhow::Result<String>{Ok(sha(&serde_json::to_vec(value)?))}
fn provider_config(path:&str)->anyhow::Result<ProviderConfig>{
    let db=Connection::open_with_flags(path,OpenFlags::SQLITE_OPEN_READ_ONLY|OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let data:String=db.query_row("SELECT data FROM provider_config WHERE singleton=1",[],|row|row.get(0))?;
    ensure!(data.len()<=16*1024,"provider config too large");
    let config:ProviderConfig=serde_json::from_str(&data)?;cloud::rewrite_endpoint(&config)?;Ok(config)
}
fn mode(scene:AppWritingScene)->ModeSnapshot{
    let id=match scene{AppWritingScene::General=>"auto",AppWritingScene::Chat=>"chat",AppWritingScene::Document=>"document",AppWritingScene::Email=>"email",AppWritingScene::Technical=>"technical"};
    ModeSnapshot{mode_id:id.into(),name:id.into(),base_mode:BaseMode::Auto,rules:String::new(),revision:1}
}
fn prepare(config:&ProviderConfig,fixture:&Fixture)->anyhow::Result<Vec<Arm>>{
    ensure!(fixture.schema_version==1&&fixture.split=="development"&&fixture.cases.len()==6,"exactly six frozen development cases required");
    let mut ids=std::collections::HashSet::new();let mut arms=Vec::new();
    for (index,case) in fixture.cases.iter().enumerate(){
        ensure!(!case.id.is_empty()&&case.id.len()<=64&&case.id.bytes().all(|c|c.is_ascii_alphanumeric()||b"_-".contains(&c))&&ids.insert(&case.id),"invalid/duplicate case ID");
        ensure!(!case.criteria.good_facts.is_empty()&&!case.criteria.bad_missing.is_empty(),"missing evaluator criteria");
        let source_id=format!("source-{}",case.id);let item_id=format!("item-{}",case.id);
        let source=SourceRevision{id:source_id,session_id:"fixed-review-lab".into(),asr_attempt_id:case.id.clone(),generation:1,
            provider:"synthetic-text".into(),model:"fixed-source".into(),adapter_version:"coverage-lab-v1".into(),complete:true,created_at:"2026-10-23T00:00:00Z".into(),
            items:vec![SourceItem{id:item_id.clone(),provider_item_id:"0".into(),capture_order:0,text:case.source_units.concat(),is_final:true,start_ms:None,end_ms:None}]};
        let mode=mode(case.scene);
        let profile=PersonalizationConfig{enabled:true,preferences:ExpressionStyle{sentence_style:SentenceStyle::Concise,paragraphs:true,..Default::default()},..Default::default()};
        let context=WritingContext{scene:case.scene,language:DraftLanguage::AsSpoken,input:None,personalization:profile.freeze(case.scene)};
        for (draft_index,(kind,units)) in [("good",&case.good_draft_units),("bad",&case.bad_draft_units)].into_iter().enumerate(){
            let draft=cloud::rewrite::RewriteResult{resolved_mode:BaseMode::Light,blocks:vec![OutputBlock{id:format!("draft-{}-{kind}",case.id),text:units.concat(),source_refs:vec![item_id.clone()]}]};
            let manifest=coverage_lab::UnitManifest{source_units:case.source_units.clone(),draft_units:units.clone()};
            // Counterbalance order within each fixed draft pair, not two new drafts.
            for aligned in if (index+draft_index)%2==0{[false,true]}else{[true,false]}{
                let request=coverage_lab::prepare(config,&mode,&source,&draft,Some(&context),&manifest,aligned)?;
                let plan=json!({"caseId":case.id,"draftKindEvaluatorOnly":kind,"unitAligned":aligned,
                    "sourceSha256":hash(&source)?,"draftSha256":hash(&draft)?,"contextSha256":hash(&context)?,
                    "unitManifest":manifest,"unitManifestSha256":hash(&manifest)?,"requestBodySha256":hash(&request.body)?,"metadata":request.metadata});
                arms.push(Arm{case_id:case.id.clone(),draft_kind:kind,aligned,source:source.clone(),draft:draft.clone(),mode:mode.clone(),context:context.clone(),manifest:coverage_lab::UnitManifest{source_units:case.source_units.clone(),draft_units:units.clone()},plan});
            }
        }
    }
    ensure!(arms.len()==LIMIT,"unexpected request count");Ok(arms)
}
fn durable(file:&mut File,value:&Value)->std::io::Result<()>{serde_json::to_writer(&mut *file,value)?;file.write_all(b"\n")?;file.flush()?;file.sync_data()}
fn receipt(file:&mut File,value:&Value)->anyhow::Result<()>{let bytes=serde_json::to_vec_pretty(value)?;file.seek(SeekFrom::Start(0))?;file.write_all(&bytes)?;file.set_len(bytes.len()as u64)?;file.flush()?;file.sync_all()?;Ok(())}
struct Ledger{file:File,starts:usize,failed:bool}
impl Ledger{
    fn append(&mut self,value:&Value)->bool{if self.failed{return false;}if durable(&mut self.file,value).is_err(){self.failed=true;return false;}true}
    fn observe(&mut self,id:&str,kind:&str,aligned:bool,event:PersonalizationLabEvent)->bool{
        let start=event.event=="started";
        if start&&self.starts>=LIMIT{self.failed=true;return false;}
        let allowed=self.append(&json!({"kind":"request_event","caseId":id,"draftKind":kind,"unitAligned":aligned,"event":event,"clientStartedRequestsBefore":self.starts}));
        if allowed&&start{self.starts+=1;}allowed
    }
}
fn lock(ledger:&Arc<Mutex<Ledger>>)->std::sync::MutexGuard<'_,Ledger>{ledger.lock().unwrap_or_else(|poison|{let mut guard=poison.into_inner();guard.failed=true;guard})}
fn fatal(error:&cloud::CloudError)->bool{matches!(error.code.as_str(),"authentication_failed"|"rate_limited"|"rewrite_timeout"|"lab_ledger_failed")}

#[tokio::main]
async fn main()->anyhow::Result<()>{
    let args=std::env::args().skip(1).collect::<Vec<_>>();
    ensure!(args.len()==4&&["--plan","--run"].contains(&args[3].as_str()),"CONFIG_DB FIXTURE NEW_OUT --plan|--run");
    ensure!(args[..3].iter().all(|arg|Path::new(arg).is_absolute()),"absolute paths required");
    let bytes=fs::read(&args[1])?;ensure!(bytes.len()<=256*1024,"fixture too large");
    let fixture:Fixture=serde_json::from_slice(&bytes)?;let config=provider_config(&args[0])?;let arms=prepare(&config,&fixture)?;
    let mut result=json!({"tool":"personalization-coverage-review-lab-v1","status":"PLANNED","scope":"FIXED_DRAFT_REVIEW_ONLY_NOT_NORMAL_LATENCY",
        "fixtureSha256":sha(&bytes),"executableSha256":sha(&fs::read(std::env::current_exe()?)?),
        "runnerSourceSha256":sha(include_bytes!("personalization_coverage_review_lab.rs")),"coverageModuleSha256":sha(include_bytes!("../src/cloud/rewrite_coverage_lab.rs")),
        "providerConfigSha256":hash(&config)?,"rewriteProvider":config.rewrite_provider,"rewriteModel":config.rewrite_model,
        "maximumClientStartedRequests":LIMIT,"clientStartedRequests":0,"deadlinePerRequestMs":WAIT_MS,"automaticRetries":0,"redirects":0,
        "profile":"fixed concise and paragraphs only; no occupational background","dictionary":"not used; fixed source/draft review only",
        "requestCountBoundary":"Authorized pipeline starts; not billing or provider receipt","actualTokenUsage":null,
        "tokenUsageBoundary":"Current shared transport does not expose response usage","sourceUnitBoundary":"Manually frozen exact clauses, not a claim of automatic semantic segmentation",
        "criteriaEvaluatorOnly":fixture.cases.iter().map(|case|json!({"id":case.id,"criteria":case.criteria})).collect::<Vec<_>>(),
        "plan":arms.iter().map(|arm|&arm.plan).collect::<Vec<_>>(),"rows":[],"deliveryAuthorized":false,"audio":"NOT_RUN","userQuality":"NOT_RUN"});
    let plan_path=format!("{}.plan.json",args[2]);let ledger_path=format!("{}.jsonl",args[2]);
    ensure!(!Path::new(&args[2]).exists()&&!Path::new(&ledger_path).exists(),"fresh receipt and ledger required");
    if args[3]=="--plan"{let mut file=OpenOptions::new().write(true).create_new(true).open(plan_path)?;receipt(&mut file,&result)?;println!("PLANNED: 6 cases, 12 fixed drafts, maximum24 requests; no credentials/network");return Ok(());}
    let frozen:Value=serde_json::from_slice(&fs::read(plan_path).context("run --plan first")?)?;ensure!(frozen==result,"frozen plan differs; no request sent");
    let mut output=OpenOptions::new().write(true).create_new(true).open(&args[2])?;
    let journal=match OpenOptions::new().write(true).create_new(true).open(ledger_path){Ok(file)=>file,Err(error)=>{result["status"]=json!("HALTED");result["artifactError"]=json!(error.to_string());receipt(&mut output,&result)?;return Err(error.into());}};
    let ledger=Arc::new(Mutex::new(Ledger{file:journal,starts:0,failed:false}));
    receipt(&mut output,&result)?;
    if !lock(&ledger).append(&json!({"kind":"plan","receipt":result})){
        result["status"]=json!("HALTED");result["haltReason"]=json!("initial_ledger_failure");
        result["rows"]=json!(arms.iter().map(|arm|json!({"caseId":arm.case_id,"draftKind":arm.draft_kind,"unitAligned":arm.aligned,"status":"NOT_RUN","reason":"initial_ledger_failure"})).collect::<Vec<_>>());
        receipt(&mut output,&result)?;anyhow::bail!("initial ledger failure; no credentials/network");
    }
    let credential=cloud::credentials::read_for(&config,cloud::credentials::CredentialStage::Rewrite);
    let mut stop=None;let secret=match credential{Ok(secret)=>Some(secret),Err(error)=>{result["credentialError"]=json!(error);stop=Some("credential_failure");None}};
    let mut rows=Vec::new();
    for arm in arms{
        if let Some(reason)=stop{rows.push(json!({"caseId":arm.case_id,"draftKind":arm.draft_kind,"unitAligned":arm.aligned,"status":"NOT_RUN","reason":reason}));continue;}
        let before=lock(&ledger).starts;
        if !lock(&ledger).append(&json!({"kind":"arm_started","caseId":arm.case_id,"draftKind":arm.draft_kind,"unitAligned":arm.aligned,"reservedRequests":1})){stop=Some("ledger_failure");rows.push(json!({"caseId":arm.case_id,"draftKind":arm.draft_kind,"unitAligned":arm.aligned,"status":"NOT_RUN","reason":"ledger_failure"}));continue;}
        let observed=Arc::clone(&ledger);let id=arm.case_id.clone();let kind=arm.draft_kind;let aligned=arm.aligned;
        let outcome=coverage_lab::run(&config,secret.as_deref().expect("credential failure halts"),&arm.mode,&arm.source,&arm.draft,Some(&arm.context),&arm.manifest,arm.aligned,
            Duration::from_millis(WAIT_MS),move|event|lock(&observed).observe(&id,kind,aligned,event)).await;
        let mut row=match outcome{Ok(value)=>json!({"caseId":arm.case_id,"draftKind":arm.draft_kind,"unitAligned":arm.aligned,"status":"COMPLETED","outcome":value}),
            Err(error)=>{if fatal(&error){stop=Some("fatal_provider_or_timeout");}json!({"caseId":arm.case_id,"draftKind":arm.draft_kind,"unitAligned":arm.aligned,"status":"COMPLETED","error":error})}};
        {let mut guard=lock(&ledger);row["clientStartedRequests"]=json!(guard.starts-before);guard.append(&json!({"kind":"arm_completed","row":row}));if guard.failed{stop=Some("ledger_failure");}result["clientStartedRequests"]=json!(guard.starts);}
        println!("{} {} aligned={}: accepted={} error={}",arm.case_id,arm.draft_kind,arm.aligned,row["outcome"]["accepted"],row["error"]["code"]);
        rows.push(row);result["status"]=json!("RUNNING");result["rows"]=json!(rows);
        if receipt(&mut output,&result).is_err(){stop=Some("receipt_failure");}
    }
    result["status"]=json!(if stop.is_some(){"HALTED"}else{"COMPLETED"});result["rows"]=json!(rows);result["haltReason"]=json!(stop);
    if !lock(&ledger).append(&json!({"kind":"run_finished","receipt":result})){result["status"]=json!("HALTED");result["haltReason"]=json!("ledger_failure");}
    receipt(&mut output,&result)?;if result["status"]=="HALTED"{anyhow::bail!("run halted; evidence preserved");}Ok(())
}

#[cfg(test)]
mod tests{
    use super::*;
    fn fixture()->Fixture{serde_json::from_str(include_str!("../../../docs/experiments/fixtures/personalization-coverage-review.dev.json")).unwrap()}
    fn config()->ProviderConfig{ProviderConfig{region:Some("beijing".into()),..Default::default()}}
    #[test]
    fn fixed_draft_pairs_only_change_the_review_mechanism_and_counterbalance_order(){
        let config=config();let arms=prepare(&config,&fixture()).unwrap();assert_eq!(arms.len(),24);
        assert_eq!(arms.iter().map(|arm|arm.aligned).take(8).collect::<Vec<_>>(),[false,true,true,false,true,false,false,true]);
        for pair in arms.chunks_exact(2){
            assert_eq!(pair[0].case_id,pair[1].case_id);assert_eq!(pair[0].draft_kind,pair[1].draft_kind);assert_ne!(pair[0].aligned,pair[1].aligned);
            for key in ["sourceSha256","draftSha256","contextSha256","unitManifestSha256"]{assert_eq!(pair[0].plan[key],pair[1].plan[key]);}
            for arm in pair{
                let request=coverage_lab::prepare(&config,&arm.mode,&arm.source,&arm.draft,Some(&arm.context),&arm.manifest,arm.aligned).unwrap();
                let data:Value=serde_json::from_str(request.body["messages"][1]["content"].as_str().unwrap()).unwrap();
                assert!(data.get("criteria").is_none());assert!(data.get("goodFacts").is_none());assert!(data.get("badMissing").is_none());assert!(data.get("draftKindEvaluatorOnly").is_none());
                assert_eq!(data.get("sourceUnits").is_some(),arm.aligned);
                assert_eq!(request.body["max_tokens"],if arm.aligned{768}else{512});
            }
        }
    }
    #[test]
    fn failed_journal_never_authorizes_a_request(){
        let path=std::env::temp_dir().join(format!("coverage-journal-{}",uuid::Uuid::new_v4()));fs::write(&path,b"seed").unwrap();
        {let mut ledger=Ledger{file:File::open(&path).unwrap(),starts:0,failed:false};
            assert!(!ledger.observe("test","good",true,PersonalizationLabEvent{stage:"review",event:"started",duration_ms:None,remaining_budget_ms:1000,outcome:None}));
            assert!(ledger.failed);assert_eq!(ledger.starts,0);}
        assert_eq!(fs::read(&path).unwrap(),b"seed");fs::remove_file(path).unwrap();
    }
}
