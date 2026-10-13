//! Offline explanation of a frozen semantic-lab generation's admission result.
//! Reads only the two named JSON files. No profile, credentials, cloud or input.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use speakmode_lib::{cloud::rewrite, core::{ModeSnapshot, SourceRevision}};
use std::{collections::HashSet, fs::{self, OpenOptions}, io::Write, path::Path};

fn hash(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn read(path: &Path) -> Result<(Value, String)> {
    ensure!(path.is_absolute(), "Absolute input paths required");
    ensure!(fs::metadata(path)?.len() <= 32 * 1024 * 1024, "Input exceeds 32 MiB");
    let bytes = fs::read(path)?;
    Ok((serde_json::from_slice(&bytes).context("Invalid JSON file")?, hash(&bytes)))
}

fn probe(fixture: &Value, receipt: &Value) -> Result<Vec<Value>> {
    ensure!(receipt["tool"] == "speakmode-semantic-quality-lab" && receipt["stage"] == "flow"
        && receipt["status"] == "completed", "Expected completed semantic-lab flow receipt");
    let cases = fixture["cases"].as_array().context("Missing fixture cases")?;
    let plans = receipt["plan"].as_array().context("Missing lab plan")?;
    let rows = receipt["results"].as_array().context("Missing lab results")?;
    let calls = receipt["calls"].as_array().context("Missing lab calls")?;
    ensure!(!cases.is_empty() && cases.len() <= 16 && cases.len() == plans.len()
        && cases.len() == rows.len(), "Case count mismatch or outside 1..16");
    let mode: ModeSnapshot = serde_json::from_value(receipt["mode"].clone()).context("Invalid recorded Mode")?;
    let mut ids = HashSet::new();
    let mut results = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let id = case["id"].as_str().context("Missing case ID")?;
        ensure!(!id.is_empty() && id.len() <= 80 && id.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
            && ids.insert(id), "Invalid or duplicate case ID");
        let plan = &plans[index]; let row = &rows[index];
        ensure!(plan["index"] == index && row["index"] == index && plan["caseId"] == id && row["caseId"] == id,
            "Case identity mismatch");
        let source: SourceRevision = serde_json::from_value(plan["source"].clone()).context("Invalid recorded source")?;
        let expected: Vec<String> = serde_json::from_value(case["sourceItems"].clone()).context("Invalid fixture sourceItems")?;
        ensure!(source.items.iter().map(|item| &item.text).eq(expected.iter()), "Fixture and recorded source differ");
        let call_index = row["generationCallIndex"].as_u64().context("Missing generation call index")?;
        let call_index: usize = call_index.try_into().context("Generation call index overflow")?;
        let call = calls.get(call_index).context("Missing generation call")?;
        ensure!(call["index"] == call_index && call["caseId"] == id && call["phase"] == "generation"
            && call["status"] == "response" && call["response"]["finishReason"] == "stop", "Invalid generation receipt");
        let content = call["response"]["modelContent"].as_str().context("Missing frozen model content")?;
        let draft = rewrite::parse_fluent_draft(content, &mode, &source)
            .map_err(|_| anyhow::anyhow!("Frozen generation does not satisfy fluent draft shape"))?;
        results.push(json!({
            "caseId": id,
            "sourceSha256": hash(&serde_json::to_vec(&source)?),
            "modelContentSha256": hash(content.as_bytes()),
            "evidence": rewrite::admission_evidence(&source, &draft)
        }));
    }
    Ok(results)
}

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    ensure!(args.len() == 6 && args[0] == "--fixture" && args[2] == "--receipt" && args[4] == "--out",
        "Use admission_probe --fixture ABS --receipt ABS --out ABS_NEW");
    let out = Path::new(&args[5]); ensure!(out.is_absolute(), "Absolute output path required");
    let (fixture, fixture_hash) = read(Path::new(&args[1]))?;
    let (receipt, receipt_hash) = read(Path::new(&args[3]))?;
    ensure!(receipt["fixtureSha256"] == fixture_hash, "Recorded fixture digest differs");
    let results = probe(&fixture, &receipt)?;
    let output = json!({
        "schemaVersion": 1, "tool": "speakmode-admission-probe", "appVersion": env!("CARGO_PKG_VERSION"),
        "scope": "OFFLINE_FROZEN_RESPONSE_ADMISSION_EVIDENCE", "cloudRequests": 0,
        "profileRead": false, "nativeInputDispatched": false, "transcriptIncluded": false,
        "fixtureSha256": fixture_hash, "receiptSha256": receipt_hash,
        "compiledHashes": {
            "admission": hash(include_bytes!("../src/cloud/rewrite_review_admission.rs")),
            "integrity": hash(include_bytes!("../src/cloud/rewrite_integrity.rs")),
            "facts": hash(include_bytes!("../src/cloud/rewrite_facts.rs")),
            "semantics": hash(include_bytes!("../src/cloud/rewrite_semantics.rs")),
            "probe": hash(include_bytes!("admission_probe.rs"))
        },
        "results": results
    });
    let mut file = OpenOptions::new().write(true).create_new(true).open(out)?;
    file.write_all(&serde_json::to_vec_pretty(&output)?)?; file.sync_all()?;
    for row in output["results"].as_array().unwrap() {
        println!("{}", json!({"caseId": row["caseId"], "admissionPassed": row["evidence"]["admissionPassed"],
            "differences": row["evidence"]["differences"]}));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use speakmode_lib::core::{BaseMode, SourceItem};
    fn fixture() -> (Value, Value) {
        let source = SourceRevision { id: "s".into(), session_id: "x".into(), asr_attempt_id: "a".into(), generation: 1,
            provider: "synthetic".into(), model: "fixed".into(), adapter_version: "test".into(), complete: true, created_at: "now".into(),
            items: vec![SourceItem {id: "a:0".into(), provider_item_id: "0".into(), capture_order: 0,
                text: "private-marker预算3000元，明天发布。".into(), is_final: true, start_ms: None, end_ms: None}] };
        let mode = ModeSnapshot {mode_id: "auto".into(), name: "Auto".into(), base_mode: BaseMode::Auto, rules: String::new(), revision: 1};
        let fixture = json!({"cases": [{"id": "A1", "sourceItems": [source.items[0].text]}]});
        let receipt = json!({"tool": "speakmode-semantic-quality-lab", "status": "completed", "stage": "flow", "mode": mode,
            "plan": [{"index": 0, "caseId": "A1", "source": source}],
            "results": [{"index": 0, "caseId": "A1", "generationCallIndex": 0}],
            "calls": [{"index": 0, "caseId": "A1", "phase": "generation", "status": "response", "response": {
                "finishReason": "stop", "modelContent": "{\"resolvedMode\":\"light\",\"text\":\"预算5000元，今天发布。\"}"}}]});
        (fixture, receipt)
    }
    #[test]
    fn evidence_contains_differences_without_echoing_source_or_mutating_inputs() {
        let (fixture, receipt) = fixture(); let before = (fixture.clone(), receipt.clone());
        let rows = probe(&fixture, &receipt).unwrap(); let evidence = &rows[0]["evidence"];
        assert_eq!(evidence["admissionPassed"], false);
        assert!(!evidence["differences"]["numericRemoved"].as_array().unwrap().is_empty());
        assert_eq!(evidence["differences"]["timeRemoved"], json!(["time:1"]));
        assert_eq!(evidence["differences"]["timeAdded"], json!(["time:0"]));
        assert!(!serde_json::to_string(&rows).unwrap().contains("private-marker"));
        assert_eq!((fixture, receipt), before);
    }
    #[test]
    fn probe_rejects_source_identity_and_truncation_changes() {
        let (mut fixture, mut receipt) = fixture();
        fixture["cases"][0]["sourceItems"][0] = json!("different");
        assert!(probe(&fixture, &receipt).is_err());
        let (fixture, _) = self::fixture();
        receipt["calls"][0]["response"]["finishReason"] = json!("length");
        assert!(probe(&fixture, &receipt).is_err());
    }
}
