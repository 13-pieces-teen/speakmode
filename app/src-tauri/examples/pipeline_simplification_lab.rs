//! Bounded, fixed-text comparison; config-only read, no user history or input.
use anyhow::{Context,ensure};
use rusqlite::{Connection,OpenFlags};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest,Sha256};
use speakmode_lib::{cloud::{self,rewrite},core::{*,writing_context::WritingContext}};
use std::{fs::{self,OpenOptions},io::Write,path::Path,time::Duration};

#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Case {id:String,source:String,scene:AppWritingScene,language:DraftLanguage,terms:Vec<String>,criteria:Vec<String>}
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Fixture {schema_version:u32,cases:Vec<Case>}
fn row(file:&mut std::fs::File,value:serde_json::Value)->anyhow::Result<()>{writeln!(file,"{value}")?;file.flush()?;file.sync_data()?;Ok(())}

#[tokio::main]
async fn main()->anyhow::Result<()> {
    let args:Vec<_>=std::env::args().skip(1).collect();
    ensure!(args.len()==4&&["--plan","--run"].contains(&args[3].as_str()),"pipeline_simplification_lab CONFIG_DB FIXTURE NEW_OUTPUT --plan|--run");
    ensure!(args[..3].iter().all(|value|Path::new(value).is_absolute()),"absolute paths required");
    let bytes=fs::read(&args[1])?;ensure!(bytes.len()<=1024*1024,"fixture too large");
    let fixture:Fixture=serde_json::from_slice(&bytes)?;
    ensure!(fixture.schema_version==1&&!fixture.cases.is_empty()&&fixture.cases.len()<=8,"maximum eight fixed cases");
    let mut ids=std::collections::HashSet::new();
    for case in &fixture.cases {
        ensure!(!case.id.is_empty()&&case.id.len()<=80&&case.id.bytes().all(|c|c.is_ascii_alphanumeric()||b"_-".contains(&c))&&ids.insert(&case.id),"invalid case ID");
        ensure!(!case.source.trim().is_empty()&&case.source.chars().count()<=4000&&!case.source.contains('\0')&&!case.criteria.is_empty(),"invalid synthetic source or criteria");
        ensure!(case.terms.len()<=32&&case.terms.iter().all(|term|!term.is_empty()&&term.len()<=80),"bounded dictionary required");
    }
    let db=Connection::open_with_flags(&args[0],OpenFlags::SQLITE_OPEN_READ_ONLY|OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let raw:String=db.query_row("SELECT data FROM provider_config WHERE singleton=1",[],|row|row.get(0))?;
    ensure!(raw.len()<=16384,"config too large");let config:ProviderConfig=serde_json::from_str(&raw)?;
    drop(db);cloud::rewrite_endpoint(&config)?;
    let mut file=OpenOptions::new().write(true).create_new(true).open(&args[2])?;
    row(&mut file,json!({"kind":"plan","fixtureSha256":format!("{:x}",Sha256::digest(&bytes)),"cases":fixture.cases.len(),"maximumRequests":fixture.cases.len()*8,
        "provider":config.rewrite_provider,"model":config.rewrite_model,"normalInitialBudgets":true,"riskRouting":"shadowOnlyFullReviewInBothArms","privateHistoryRead":false,"delivery":false,
        "executableSha256":format!("{:x}",Sha256::digest(fs::read(std::env::current_exe()?)?))}))?;
    if args[3]=="--plan" {println!("PLANNED {} cases; credentials/network NOT_RUN",fixture.cases.len());return Ok(());}
    let secret=match cloud::credentials::read_for(&config,cloud::credentials::CredentialStage::Rewrite) {
        Ok(secret)=>secret,Err(error)=>{row(&mut file,json!({"kind":"halt","code":error.code,"requests":0}))?;anyhow::bail!("configured rewrite credential unavailable");}
    };
    let mut calls=0usize;
    for (index,case) in fixture.cases.iter().enumerate() {
        let source=SourceRevision{id:format!("source-{}",case.id),session_id:case.id.clone(),asr_attempt_id:"synthetic".into(),generation:1,
            provider:"synthetic-fixed-text".into(),model:"fixed".into(),adapter_version:"simplification-lab-v1".into(),complete:true,created_at:"2026-10-28T00:00:00Z".into(),
            items:vec![SourceItem{id:"item-0".into(),provider_item_id:"0".into(),capture_order:0,text:case.source.clone(),is_final:true,start_ms:None,end_ms:None}]};
        let mode=ModeSnapshot{mode_id:"auto".into(),name:"自动".into(),base_mode:BaseMode::Auto,rules:String::new(),revision:1};
        let context=WritingContext{scene:case.scene,language:case.language,input:None,personalization:None};
        let dictionary=DictionarySnapshot{version:1,entries:case.terms.iter().enumerate().map(|(i,term)|DictionaryEntry{id:format!("term-{i}"),term:term.clone(),created_at:"2026-10-28T00:00:00Z".into(),updated_at:"2026-10-28T00:00:00Z".into()}).collect()};
        let budget=rewrite::initial_wait_ms(&source);
        for precise in if index%2==0{[false,true]}else{[true,false]} {
            ensure!(calls+4<=fixture.cases.len()*8,"request cap reached");
            let arm=if precise{"precise"}else{"baseline"};
            row(&mut file,json!({"kind":"armStarted","caseId":case.id,"arm":arm,"budgetMs":budget,"criteria":case.criteria,"guardedLong":rewrite::uses_guarded_long(&source)}))?;
            let outcome=rewrite::minimum_pipeline::experiment_simplification(&config,&secret,&mode,&source,&dictionary,Some(&context),precise,Duration::from_millis(budget)).await.context("experiment setup failed")?;
            calls+=outcome.trace.calls;
            let result=match outcome.result {Ok(draft)=>json!({"status":"success","text":draft.blocks.iter().map(|block|block.text.as_str()).collect::<Vec<_>>().join("\n\n")}),Err(error)=>json!({"status":"failed","code":error.code})};
            println!("{} {arm}: {} ({} requests, {}ms)",case.id,result["status"],outcome.trace.calls,outcome.trace.elapsed_ms);
            row(&mut file,json!({"kind":"armResult","caseId":case.id,"arm":arm,"result":result,"trace":outcome.trace}))?;
        }
    }
    row(&mut file,json!({"kind":"complete","cases":fixture.cases.len(),"requests":calls,"humanQuality":"NOT_RUN","realRecording":"NOT_RUN"}))?;
    Ok(())
}
