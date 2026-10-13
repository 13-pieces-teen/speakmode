//! Replay retained natural lab responses with the current finite guards.
//! No cloud/credentials/profile reads. Original receipts remain untouched.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use speakmode_lib::{cloud::rewrite, core::{ModeSnapshot, SourceRevision}};
use std::{fs::{self, OpenOptions}, io::Write, path::Path};

fn hash(bytes:&[u8])->String {format!("{:x}",Sha256::digest(bytes))}
fn current_guard_hashes()->Value {
    json!({
        "parser":hash(include_str!("../src/cloud/rewrite.rs").as_bytes()),
        "integrity":hash(include_str!("../src/cloud/rewrite_integrity.rs").as_bytes()),
        "facts":hash(include_str!("../src/cloud/rewrite_facts.rs").as_bytes()),
        "semantics":hash(include_str!("../src/cloud/rewrite_semantics.rs").as_bytes()),
        "disfluency":hash(include_str!("../src/cloud/rewrite_disfluency.rs").as_bytes()),
        "evaluationDisfluency":hash(include_str!("../src/cloud/rewrite_evaluation_disfluency.rs").as_bytes()),
        "quantityRoles":hash(include_str!("../src/cloud/rewrite_quantity_roles.rs").as_bytes()),
    })
}
fn initial_deadline_ms(source:&SourceRevision)->u64 {if source.text().chars().count()<=800 {5000} else {20000}}

fn replay(mut receipt: Value, original_hash: &str) -> Result<Value> {
    ensure!(receipt["tool"] == "speakmode-natural-quality-lab" && receipt["status"] == "completed", "Expected a completed natural lab receipt");
    let expected = match receipt["split"].as_str() {Some("dev")=>24, Some("holdout")=>16, Some("user")=>2, _=>anyhow::bail!("Invalid split")};
    let plans = receipt["plan"].as_array().context("Missing plan")?.clone();
    let rows = receipt["results"].as_array_mut().context("Missing results")?;
    ensure!(plans.len()==expected && rows.len()==expected, "Incomplete receipt");
    for (index, row) in rows.iter_mut().enumerate() {
        let plan=&plans[index];
        ensure!(row["index"]==index && plan["index"]==index && row["arm"]==plan["arm"] && row["caseId"]==plan["caseId"], "Identity mismatch");
        let source:SourceRevision=serde_json::from_value(plan["source"].clone())?;
        let mode:ModeSnapshot=serde_json::from_value(plan["mode"].clone())?;
        let original = json!({"status":row["status"],"parser":row["parser"],"validationError":row["validationError"],"projectedDelivery":row["projectedDelivery"]});
        // Transport/truncation failures are never converted to parser successes.
        if !matches!(row["status"].as_str(),Some("parsed"|"rejected")) {continue;}
        let content=row["response"]["modelContent"].as_str().context("Missing model content")?;
        let result=match row["arm"].as_str() {
            Some("baseline")=>rewrite::parse_model_output(content,&mode,&source),
            Some("candidate")=>rewrite::parse_natural_output(content,&mode,&source),
            _=>anyhow::bail!("Unknown arm"),
        };
        let deadline=initial_deadline_ms(&source);
        let timed_out=row["elapsedMs"].as_u64().context("Missing duration")? >= deadline;
        let (text,reason)=match result {
            Ok(output)=>{
                let text=output.blocks.iter().map(|b|b.text.as_str()).collect::<Vec<_>>().join("\n\n");
                row["status"]=json!("parsed");row["parser"]=json!("passed");
                row["validatedText"]=json!(text);row["validatedOutput"]=serde_json::to_value(output)?;
                row.as_object_mut().unwrap().remove("validationError");
                if timed_out {(source.text(),Some("timeout"))}else{(text,None)}
            },
            Err(error)=>{
                row["status"]=json!("rejected");row["parser"]=json!("failed");
                row["validationError"]=json!({"code":error.code,"stage":error.stage,"message":error.message});
                row.as_object_mut().unwrap().remove("validatedText");row.as_object_mut().unwrap().remove("validatedOutput");
                (source.text(),Some(if timed_out{"timeout"}else if error.code=="rewrite_meaning_risk"{"meaningRisk"}else{"invalidOutput"}))
            }
        };
        row["projectedDelivery"]=json!({"scope":"SIMULATED_CURRENT_INITIAL_BUDGET","applicationDeadlineMs":deadline,
            "status":if reason.is_none(){"rewrite_candidate"}else{"source_fallback"},"reason":reason,"text":text,"actualNativeDelivery":"NOT_RUN"});
        row["originalDecision"]=original;
    }
    receipt["offlineReplay"]=json!({"scope":"OFFLINE_REPARSE_OF_EXISTING_RESPONSES_NOT_NEW_CLOUD_RUN","originalReceiptSha256":original_hash,"cloudRequests":0,
        "currentGuardHashes":current_guard_hashes(),
        "retainedLatency":"Original request elapsedMs retained; not a new latency measurement","meaningChecks":"Current compiled guards; original decisions retained per row"});
    Ok(receipt)
}

fn main()->Result<()> {
    let args=std::env::args().skip(1).collect::<Vec<_>>();
    ensure!(args.len()==4 && args[0]=="--receipt" && args[2]=="--out", "Use --receipt ABS --out ABS_NEW");
    let input=Path::new(&args[1]);let output=Path::new(&args[3]);
    ensure!(input.is_absolute()&&output.is_absolute(),"Absolute paths required");
    ensure!(fs::metadata(input)?.len()<=32*1024*1024,"Receipt too large");
    let bytes=fs::read(input)?;let hash=format!("{:x}",Sha256::digest(&bytes));
    let result=replay(serde_json::from_slice(&bytes)?,&hash)?;
    let mut file=OpenOptions::new().create_new(true).write(true).open(output)?;
    file.write_all(&serde_json::to_vec_pretty(&result)?)?;
    println!("Offline replay complete: {} rows, 0 new requests",result["results"].as_array().unwrap().len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use speakmode_lib::core::{BaseMode,SourceItem};

    fn source(text:String)->SourceRevision {
        SourceRevision{id:"source".into(),session_id:"session".into(),asr_attempt_id:"attempt".into(),generation:1,
            provider:"synthetic".into(),model:"fixed-text".into(),adapter_version:"test".into(),complete:true,
            created_at:"2026-10-11T00:00:00Z".into(),items:vec![SourceItem{id:"attempt:0".into(),provider_item_id:"0".into(),
                capture_order:0,text,is_final:true,start_ms:None,end_ms:None}]}
    }
    fn fixture(elapsed:u64)->Value {
        let source=source("嗯，测试麦克风。".into());
        let mode=ModeSnapshot{mode_id:"auto".into(),name:"auto".into(),base_mode:BaseMode::Auto,rules:String::new(),revision:1};
        let plans=["baseline","candidate"].iter().enumerate().map(|(index,arm)|json!({"index":index,"caseId":"test",
            "arm":arm,"trial":1,"source":source,"mode":mode,"requestBody":{"model":"qwen3.8-flash"}})).collect::<Vec<_>>();
        let rows=["baseline","candidate"].iter().enumerate().map(|(index,arm)|{
            let content=if *arm=="baseline" {json!({"resolvedMode":"light","blocks":[{"text":"测试麦克风。","sourceRange":[0,0]}]})}
                else {json!({"resolvedMode":"light","text":"测试麦克风。"})};
            json!({"index":index,"caseId":"test","arm":arm,"trial":1,"status":"parsed","parser":"passed",
                "response":{"modelContent":content.to_string(),"finishReason":"stop"},"elapsedMs":elapsed,"cloudElapsedMs":elapsed-1,
                "parserElapsedMicros":123,"validatedText":"测试麦克风。","projectedDelivery":{"status":"rewrite_candidate","text":"测试麦克风。"}})
        }).collect::<Vec<_>>();
        json!({"tool":"speakmode-natural-quality-lab","status":"completed","split":"user","plan":plans,"results":rows,
            "parserSha256":"original-parser-hash","integritySha256":"original-integrity-hash"})
    }
    #[test]
    fn cutoff_equality_matches_cloud_lab_projection_and_original_evidence_is_preserved() {
        for (elapsed,expected) in [(4999,"rewrite_candidate"),(5000,"source_fallback"),(5001,"source_fallback")] {
            let original=fixture(elapsed);let result=replay(original.clone(),&"a".repeat(64)).unwrap();
            assert_eq!(result["plan"],original["plan"]);
            assert_eq!(result["parserSha256"],original["parserSha256"]);
            assert_eq!(result["integritySha256"],original["integritySha256"]);
            for index in 0..2 {
                let row=&result["results"][index];
                for field in ["response","elapsedMs","cloudElapsedMs","parserElapsedMicros"] {
                    assert_eq!(row[field],original["results"][index][field]);
                }
                assert_eq!(row["projectedDelivery"]["status"],expected);
                assert_eq!(row["originalDecision"]["projectedDelivery"],original["results"][index]["projectedDelivery"]);
            }
            assert_eq!(result["offlineReplay"]["cloudRequests"],0);
            assert_eq!(result["offlineReplay"]["originalReceiptSha256"],"a".repeat(64));
            assert_eq!(result["offlineReplay"]["currentGuardHashes"],current_guard_hashes());
        }
    }
    #[test]
    fn long_source_budget_and_compiled_guard_hashes_are_explicit() {
        assert_eq!(initial_deadline_ms(&source("轻".repeat(800))),5000);
        assert_eq!(initial_deadline_ms(&source("轻".repeat(801))),20000);
        let hashes=current_guard_hashes();let entries=hashes.as_object().unwrap();assert_eq!(entries.len(),7);
        for value in entries.values() {let digest=value.as_str().unwrap();assert_eq!(digest.len(),64);assert!(digest.bytes().all(|byte|byte.is_ascii_hexdigit()));}
    }
}
