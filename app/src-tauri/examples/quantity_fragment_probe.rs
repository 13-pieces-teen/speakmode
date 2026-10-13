//! Recheck two retained responses against the exact user-supplied single item.
//! This is a synthetic provenance-remapping check, not new model/audio output.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use speakmode_lib::{cloud::rewrite, core::{ModeSnapshot, SourceItem, SourceRevision}};
use std::{fs::{self, OpenOptions}, io::Write, path::Path};

fn read(path: &Path) -> Result<(Value, String)> {
    ensure!(path.is_absolute() && fs::metadata(path)?.len() <= 1024 * 1024, "Expected bounded absolute input");
    let bytes = fs::read(path)?;
    Ok((serde_json::from_slice(&bytes)?, format!("{:x}", Sha256::digest(&bytes))))
}
fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    ensure!(args.len() == 6 && args[0] == "--receipt" && args[2] == "--fixture" && args[4] == "--out",
        "Use --receipt ABS --fixture ABS --out ABS_NEW");
    let (receipt, receipt_hash) = read(Path::new(&args[1]))?;
    let (fixture, fixture_hash) = read(Path::new(&args[3]))?;
    let out = Path::new(&args[5]);
    ensure!(out.is_absolute() && !out.exists(), "Expected fresh absolute output");
    ensure!(receipt["tool"] == "speakmode-natural-quality-lab" && receipt["split"] == "user" && receipt["status"] == "completed", "Expected completed two-arm user receipt");
    let plans = receipt["plan"].as_array().context("Missing plan")?;
    let results = receipt["results"].as_array().context("Missing results")?;
    let cases = fixture["cases"].as_array().context("Missing fixture cases")?;
    ensure!(plans.len() == 2 && results.len() == 2 && cases.len() == 1, "Expected one case and two arms");
    let script = cases[0]["spokenScript"].as_str().context("Missing exact source")?;
    ensure!(!script.trim().is_empty() && script.chars().count() <= 2000, "Invalid exact source");
    let mut checks = Vec::new();
    for (index, (plan, row)) in plans.iter().zip(results).enumerate() {
        ensure!(plan["index"] == index && row["index"] == index && plan["arm"] == row["arm"]
            && plan["caseId"] == row["caseId"] && plan["caseId"] == cases[0]["id"], "Identity mismatch");
        ensure!(matches!(row["status"].as_str(), Some("parsed" | "rejected")), "No complete model output to recheck");
        let mut source: SourceRevision = serde_json::from_value(plan["source"].clone())?;
        ensure!(source.items.iter().map(|i| i.text.as_str()).collect::<String>() == script,
            "Original synthetic fragments must concatenate to exact provided text");
        source.items = vec![SourceItem { id: "single-item:0".into(), provider_item_id: "0".into(), capture_order: 0,
            text: script.into(), is_final: true, start_ms: None, end_ms: None }];
        let mode: ModeSnapshot = serde_json::from_value(plan["mode"].clone())?;
        let original = row["response"]["modelContent"].as_str().context("Missing model content")?;
        let mut content: Value = serde_json::from_str(original)?;
        let result = match row["arm"].as_str() {
            Some("baseline") => {
                let blocks = content["blocks"].as_array_mut().context("Missing blocks")?;
                ensure!(blocks.len() == 1 && blocks[0]["sourceRange"] == json!([0, plan["source"]["items"].as_array().unwrap().len() - 1]),
                    "Only a single whole-source block can be remapped without inference");
                blocks[0]["sourceRange"] = json!([0, 0]);
                rewrite::parse_model_output(&content.to_string(), &mode, &source)
            }
            Some("candidate") => rewrite::parse_natural_output(&content.to_string(), &mode, &source),
            _ => anyhow::bail!("Unexpected arm"),
        };
        let (passed, error) = match result {
            Ok(output) => {
                ensure!(output.blocks.iter().all(|b| b.source_refs == vec!["single-item:0"]), "Unexpected provenance");
                (true, Value::Null)
            }
            Err(error) => (false, json!({"code":error.code,"message":error.message})),
        };
        checks.push(json!({"index":index,"arm":row["arm"],"passed":passed,"error":error,
            "modelContentSha256":format!("{:x}",Sha256::digest(original.as_bytes())),
            "originalSourceItemCount":plan["source"]["items"].as_array().unwrap().len(),"checkedItemCount":1}));
    }
    let all_passed = checks.iter().all(|c| c["passed"] == true);
    let evidence = json!({"scope":"OFFLINE_SINGLE_ITEM_EQUIVALENCE_NOT_NEW_GENERATION", "cloudRequests":0,
        "profileReadOrWritten":false,"actualNativeDelivery":"NOT_RUN","sourceReceiptSha256":receipt_hash,
        "sourceFixtureSha256":fixture_hash,"outputTextChanged":false,"provenanceRemapped":true,
        "allPassed":all_passed,"checks":checks});
    OpenOptions::new().create_new(true).write(true).open(out)?.write_all(&serde_json::to_vec_pretty(&evidence)?)?;
    println!("Single-item equivalence: {} / 2 passed", evidence["checks"].as_array().unwrap().iter().filter(|c| c["passed"] == true).count());
    ensure!(all_passed, "Some retained response fails with exact single source item");
    Ok(())
}
