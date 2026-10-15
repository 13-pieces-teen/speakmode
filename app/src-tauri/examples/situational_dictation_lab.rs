//! Explicit finite text evaluation. Read-only config; no audio/history/host capture.
use speakmode_lib::{cloud::{self,rewrite},core::{BaseMode,ModeSnapshot,ProviderConfig,SourceRevision,SourceItem,writing_context::WritingContext}};
use serde::Deserialize;
use serde_json::json;
use std::{fs::{self,OpenOptions},io::Write,path::PathBuf,time::Duration};
use rusqlite::{Connection,OpenFlags};
use sha2::{Digest,Sha256};

#[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Case {id:String,mode_id:String,source_items:Vec<String>,context:WritingContext,expectation:String,baseline:bool}
#[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Fixture {scope:String,protocol:String,cases:Vec<Case>}
fn source(case:&Case)->SourceRevision {
    SourceRevision{id:format!("source-{}",case.id),session_id:format!("session-{}",case.id),asr_attempt_id:case.id.clone(),generation:1,
        provider:"synthetic-text".into(),model:"fixed-text".into(),adapter_version:"situational-lab-v1".into(),complete:true,created_at:"2026-10-15T00:00:00Z".into(),
        items:case.source_items.iter().enumerate().map(|(index,text)|SourceItem{id:format!("{}:{index}",case.id),provider_item_id:index.to_string(),capture_order:index as u64,text:text.clone(),is_final:true,start_ms:None,end_ms:None}).collect()}
}
fn mode(case:&Case,baseline:bool)->ModeSnapshot {
    let base_mode=BaseMode::Auto;
    ModeSnapshot{mode_id:if baseline {if base_mode==BaseMode::Auto{"auto"}else{"light"}.into()}else{case.mode_id.clone()},
        name:case.mode_id.clone(),base_mode,rules:String::new(),revision:1}
}
#[tokio::main]
async fn main()->anyhow::Result<()> {
    let args=std::env::args().skip(1).collect::<Vec<_>>();
    anyhow::ensure!(args.len()==4 && matches!(args[3].as_str(),"--plan"|"--run"),"CONFIG_DB FIXTURE NEW_OUT --plan|--run");
    for path in &args[..3] {anyhow::ensure!(PathBuf::from(path).is_absolute(),"absolute paths required");}
    let bytes=fs::read(&args[1])?;anyhow::ensure!(bytes.len()<=256*1024,"fixture too large");
    let fixture:Fixture=serde_json::from_slice(&bytes)?;
    anyhow::ensure!(fixture.protocol==rewrite::writing_context::VERSION && !fixture.scope.is_empty(),"protocol mismatch");
    let paths=fixture.cases.len()+fixture.cases.iter().filter(|case|case.baseline).count();
    anyhow::ensure!(!fixture.cases.is_empty()&&fixture.cases.len()<=8&&paths<=12,"maximum 12 paths / 48 requests");
    let mut ids=std::collections::HashSet::new();
    for case in &fixture.cases {
        anyhow::ensure!(ids.insert(case.id.clone())&&!case.id.is_empty()&&case.id.len()<=80&&case.id.bytes().all(|c|c.is_ascii_alphanumeric()||b"-_".contains(&c)),"invalid case ID");
        anyhow::ensure!(["auto","chat","document","email","technical"].contains(&case.mode_id.as_str()),"unknown scenario");
        anyhow::ensure!(!case.source_items.is_empty()&&case.source_items.len()<=32&&case.source_items.iter().all(|s|!s.trim().is_empty())&&case.source_items.iter().map(|s|s.chars().count()).sum::<usize>()<=8000,"invalid source");
        case.context.validate().map_err(anyhow::Error::msg)?;
    }
    let db=Connection::open_with_flags(&args[0],OpenFlags::SQLITE_OPEN_READ_ONLY|OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let raw:String=db.query_row("SELECT data FROM provider_config WHERE singleton=1",[],|r|r.get(0))?;
    anyhow::ensure!(raw.len()<16*1024,"config too large");let config:ProviderConfig=serde_json::from_str(&raw)?;cloud::endpoints(&config)?;drop(db);
    let metadata=json!({"scope":fixture.scope,"protocol":fixture.protocol,"maximumCalls":paths*4,"maximumPaths":paths,
        "fixtureSha256":format!("{:x}",Sha256::digest(&bytes)),"provider":config.provider,"model":config.rewrite_model,"region":config.region,
        "waitPolicy":"same production source-length deadline for baseline and context","deliveryAuthorized":false});
    if args[3]=="--plan" {println!("{}",serde_json::to_string_pretty(&metadata)?);return Ok(());}
    // Durable reservation precedes credentials/network; existing output forbids reruns.
    let mut output=OpenOptions::new().write(true).create_new(true).open(&args[2])?;
    let mut journal=OpenOptions::new().write(true).create_new(true).open(format!("{}.jsonl",args[2]))?;
    writeln!(journal,"{}",metadata)?;journal.flush()?;
    let secret=cloud::credentials::read(&config)?;let mut rows=Vec::new();let mut actual_calls=0;
    'cases: for case in &fixture.cases {
        for baseline in [true,false] {
            if baseline&&!case.baseline{continue;}
            let source=source(case);let mode=mode(case,baseline);let wait=Duration::from_millis(rewrite::initial_wait_ms(&source));
            writeln!(journal,"{}",json!({"caseId":case.id,"baseline":baseline,"state":"started","reservedCalls":4}))?;journal.flush()?;
            let outcome=if baseline {rewrite::minimum_pipeline::experiment(&config,&secret,&mode,&source,wait).await?}
                else{rewrite::minimum_pipeline::experiment_with_writing_context(&config,&secret,&mode,&source,&case.context,wait).await?};
            actual_calls+=outcome.trace.calls;
            let stop=outcome.result.as_ref().err().is_some_and(|e|matches!(e.code.as_str(),"authentication_failed"|"rate_limited"));
            let row=json!({"id":case.id,"baseline":baseline,"mode":mode,"source":source,"writingContext":if baseline{None}else{Some(&case.context)},
                "expectation":case.expectation,"waitMs":wait.as_millis(),"candidate":outcome.result.as_ref().ok(),"error":outcome.result.as_ref().err(),"trace":outcome.trace});
            writeln!(journal,"{}",row)?;journal.flush()?;rows.push(row);
            if stop{break 'cases;}
        }
    }
    output.write_all(serde_json::to_string_pretty(&json!({"metadata":metadata,"actualCalls":actual_calls,"stopped":true,"rows":rows}))?.as_bytes())?;output.flush()?;
    println!("situational text evaluation complete: requests={actual_calls}, paths={}",rows.len());Ok(())
}
