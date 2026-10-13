//! Bounded three-arm text experiment. No app capture, audio, history or delivery.
use speakmode_lib::{cloud::{self, rewrite::{self, scene}}, core::{BaseMode, ModeSnapshot, ProviderConfig, SourceItem, SourceRevision}};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use rusqlite::{Connection, OpenFlags};
use std::{fs::{self, OpenOptions}, io::Write, path::PathBuf, time::Duration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Case {
    id: String, category: String, base_mode: BaseMode, rules: String, source_items: Vec<String>,
    scene: scene::WritingScene, use_nearby_text: bool, nearby_text: Option<scene::NearbyText>,
    expectations: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Fixture { schema_version: u32, split: String, cases: Vec<Case> }

fn sha(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn config(path: &str) -> anyhow::Result<ProviderConfig> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let raw: String = db.query_row("SELECT data FROM provider_config WHERE singleton=1", [], |r| r.get(0))?;
    anyhow::ensure!(raw.len() <= 16 * 1024, "config too large");
    let config: ProviderConfig = serde_json::from_str(&raw)?;
    cloud::endpoints(&config)?;
    anyhow::ensure!(config.region.as_deref() == Some("beijing") && config.rewrite_model == "qwen3.8-flash", "experiment requires the frozen Beijing/qwen3.8-flash configuration");
    Ok(config)
}
fn source(case: &Case) -> SourceRevision {
    SourceRevision { id: format!("source-{}", case.id), session_id: format!("scene-{}", case.id), asr_attempt_id: case.id.clone(), generation: 1,
        provider: "synthetic-fixed-text".into(), model: "fixed-text".into(), adapter_version: "scene-lab-v1".into(), complete: true,
        created_at: "2026-10-13T00:00:00Z".into(), items: case.source_items.iter().enumerate().map(|(i,t)| SourceItem {
            id: format!("{}:{i}",case.id), provider_item_id: i.to_string(), capture_order: i as u64, text: t.clone(),
            is_final: true, start_ms: None, end_ms: None }).collect() }
}
fn text(result: &rewrite::RewriteResult) -> String { result.blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n") }
fn write_row(file: &mut std::fs::File, value: &Value) -> anyhow::Result<()> { writeln!(file, "{value}")?; file.flush()?; Ok(()) }

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    anyhow::ensure!(args.len() == 4 && ["--plan-only", "--run", "--probe", "--refine-run"].contains(&args[3].as_str()), "scene_writing_lab CONFIG_DB FIXTURE NEW_OUT --plan-only|--run|--probe|--refine-run");
    let probe = args[3] == "--probe";
    let refinement = args[3] == "--refine-run";
    anyhow::ensure!(args[..3].iter().all(|p|PathBuf::from(p).is_absolute()), "absolute paths required");
    anyhow::ensure!(!PathBuf::from(&args[2]).exists() && !PathBuf::from(format!("{}.jsonl",args[2])).exists(), "fresh output required");
    let bytes = fs::read(&args[1])?; anyhow::ensure!(bytes.len() <= 1024 * 1024, "fixture too large");
    let fixture: Fixture = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(fixture.schema_version == 1 && ["dev","holdout"].contains(&fixture.split.as_str()) && fixture.cases.len() == 8, "frozen split must contain eight cases");
    let mut ids = std::collections::HashSet::new();
    for case in &fixture.cases {
        anyhow::ensure!(ids.insert(&case.id) && !case.id.is_empty() && case.id.len() <= 80 && case.id.bytes().all(|v|v.is_ascii_alphanumeric() || b"-_".contains(&v))
            && !case.source_items.is_empty() && case.source_items.len() <= 32 && case.source_items.iter().all(|t|!t.trim().is_empty())
            && case.source_items.iter().map(|s|s.chars().count()).sum::<usize>() <= 8000 && case.rules.chars().count() <= 4000
            && !case.expectations.is_empty(), "invalid fixture case");
    }
    let config = config(&args[0])?;
    anyhow::ensure!((!probe && !refinement) || fixture.split == "dev", "diagnostics may not expose the holdout");
    let maximum = if probe {4} else if refinement {16} else {fixture.cases.len() * 8};
    let candidate_hash = sha(include_bytes!("../src/cloud/rewrite_scene.rs"));
    let baseline_hash = sha(include_bytes!("../src/cloud/rewrite_minimum_pipeline.rs"));
    let executable_hash = sha(&fs::read(std::env::current_exe()?)?);
    let mut plan = Vec::new();
    for (index, case) in fixture.cases.iter().enumerate() {
        if (probe && index != 0) || (refinement && ![0,1,6,7].contains(&index)) { continue; }
        let source = source(case);
        let mode = ModeSnapshot { mode_id: format!("scene-lab-{}",case.id), name: "实验".into(), base_mode: case.base_mode, rules: case.rules.clone(), revision: 1 };
        let scene = scene::SceneSnapshot { session_id: source.session_id.clone(), generation: source.generation, target_id: format!("synthetic-field-{}",case.id), revision: 1,
            scene: case.scene, use_nearby_text: case.use_nearby_text, nearby_text: case.nearby_text.clone() };
        let request = scene::prepare(&config, &mode, &source, &Default::default(), &scene)?;
        for arm in match index % 3 { 0 => ["baseline","natural","scene"], 1 => ["natural","scene","baseline"], _ => ["scene","baseline","natural"] } {
            if (probe || refinement) && arm == "baseline" { continue; }
            plan.push(json!({"caseId":case.id,"arm":arm,"maximumCalls":if arm=="baseline"{4}else{2},"deadlineMs":if probe {20_000}else{rewrite::initial_wait_ms(&source)},
                "source":source,"mode":mode,"sceneSnapshot":scene,"category":case.category,"expectations":case.expectations,
                "sceneRequestSha256":sha(&serde_json::to_vec(&request.body)?)}));
        }
    }
    let mut receipt = json!({"tool":"scene-writing-lab-v1","status":"planned","split":fixture.split,"fixtureSha256":sha(&bytes),
        "candidateSourceSha256":candidate_hash,"baselinePipelineSha256":baseline_hash,"executableSha256":executable_hash,
        "candidateProtocol":scene::VERSION,"reviewProtocol":scene::REVIEW_VERSION,"model":config.rewrite_model,"region":config.region,
        "maximumCalls":maximum,"clientStartedCalls":0,"automaticRetries":0,"scope":if probe{"EXTENDED_DEADLINE_DIAGNOSTIC_NOT_NORMAL_LATENCY"}else if refinement{"DEVELOPMENT_REFINEMENT_BASELINES_NOT_RERUN"}else{"THREE_ARM_FIXED_COMPARISON"},"actualNativeDelivery":"NOT_RUN","actualAudioRecognition":"NOT_RUN",
        "appContextCapture":"SYNTHETIC_FIXTURE_ONLY","evaluation":"AGENT_REVIEW_NOT_INDEPENDENT","plan":plan,"results":[]});
    let mut output = OpenOptions::new().write(true).create_new(true).open(&args[2])?;
    let mut journal = OpenOptions::new().write(true).create_new(true).open(format!("{}.jsonl",args[2]))?;
    write_row(&mut journal,&json!({"kind":"plan","receipt":receipt}))?;
    if args[3] == "--plan-only" {
        output.write_all(serde_json::to_string_pretty(&receipt)?.as_bytes())?; output.flush()?;
        println!("planned: {} cases, {} arms, <= {maximum} requests; no credentials/network", fixture.cases.len(), plan.len()); return Ok(());
    }
    let secret = cloud::credentials::read(&config)?;
    let mut calls = 0; let mut rows = Vec::new(); let mut fatal = false;
    for task in &plan {
        if fatal { rows.push(json!({"caseId":task["caseId"],"arm":task["arm"],"status":"not_run_fatal_provider"})); continue; }
        let source: SourceRevision = serde_json::from_value(task["source"].clone())?;
        let mode: ModeSnapshot = serde_json::from_value(task["mode"].clone())?;
        let mut scene: scene::SceneSnapshot = serde_json::from_value(task["sceneSnapshot"].clone())?;
        let arm = task["arm"].as_str().unwrap();
        let reserve = task["maximumCalls"].as_u64().unwrap() as usize;
        anyhow::ensure!(calls + reserve <= maximum, "request budget exhausted");
        write_row(&mut journal,&json!({"kind":"arm_started","caseId":task["caseId"],"arm":arm,"callsBefore":calls,"reservedMaximum":reserve}))?;
        let wait = Duration::from_millis(task["deadlineMs"].as_u64().unwrap());
        let (result, trace, count, elapsed) = if arm == "baseline" {
            let outcome = rewrite::minimum_pipeline::experiment(&config,&secret,&mode,&source,wait).await?;
            let count = outcome.trace.calls; let elapsed = outcome.trace.elapsed_ms;
            (outcome.result, serde_json::to_value(outcome.trace)?, count, elapsed)
        } else {
            if arm == "natural" { scene = scene::SceneSnapshot::speech_only(&source); }
            let outcome = scene::experiment(&config,&secret,&mode,&source,&scene,wait).await?;
            let count = outcome.trace.calls; let elapsed = outcome.trace.elapsed_ms;
            (outcome.result, serde_json::to_value(outcome.trace)?, count, elapsed)
        };
        calls += count;
        fatal = result.as_ref().err().is_some_and(|e|["authentication_failed","rate_limited"].contains(&e.code.as_str()));
        let row = json!({"caseId":task["caseId"],"arm":arm,"status":"completed","pipelineAccepted":result.is_ok(),
            "deliveredText":result.as_ref().map(text).unwrap_or_else(|_|source.text()),"fallback":result.is_err(),"elapsedMs":elapsed,
            "clientStartedCalls":count,"error":result.err(),"trace":trace});
        println!("{} {arm}: accepted={} calls={count} elapsedMs={elapsed}",task["caseId"].as_str().unwrap(),row["pipelineAccepted"]);
        write_row(&mut journal,&json!({"kind":"arm_completed","row":row,"callsTotal":calls}))?; rows.push(row);
    }
    receipt["status"] = json!(if fatal{"halted"}else{"completed"}); receipt["clientStartedCalls"] = json!(calls); receipt["results"] = json!(rows);
    output.write_all(serde_json::to_string_pretty(&receipt)?.as_bytes())?; output.flush()?;
    println!("{}: requests={calls}/{maximum}", receipt["status"]); Ok(())
}
