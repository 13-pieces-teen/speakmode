//! Offline public synthetic quantity-role challenges, with an unchanged-source
//! control for every case. No provider, credentials, microphone, app profile,
//! user transcript, network call or native input is accessed by this probe.
use serde::Deserialize;
use serde_json::json;
use speakmode_lib::{
    cloud::rewrite::{parse_model_output, parse_natural_output},
    core::{BaseMode, ModeSnapshot, SourceItem, SourceRevision},
};

const FIXTURE: &str = include_str!(
    "../../../docs/experiments/fixtures/quantity-role-challenges-v1.json"
);

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    id: String,
    source_items: Vec<String>,
    expected: bool,
    text: String,
}

fn main() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("valid embedded public fixture");
    assert_eq!(fixture.cases.len(), 12, "fixed twelve-case challenge design");
    let positive_cases = fixture.cases.iter().filter(|case| case.expected).count();
    let negative_cases = fixture.cases.len() - positive_cases;
    assert!(positive_cases >= 4 && negative_cases >= 4);

    let mode = ModeSnapshot {
        mode_id: "auto".into(),
        name: "自动".into(),
        base_mode: BaseMode::Auto,
        rules: String::new(),
        revision: 1,
    };
    let mut rows = Vec::new();
    for case in &fixture.cases {
        assert!(!case.source_items.is_empty() && case.source_items.len() <= 16);
        assert!(case.source_items.iter().all(|text| !text.trim().is_empty()));
        let source = SourceRevision {
            id: format!("source-{}", case.id),
            session_id: format!("session-{}", case.id),
            asr_attempt_id: format!("attempt-{}", case.id),
            generation: 1,
            provider: "offline".into(),
            model: "fixed-text".into(),
            adapter_version: "quantity-role-probe-v1".into(),
            complete: true,
            created_at: "2026-10-11T00:00:00Z".into(),
            items: case
                .source_items
                .iter()
                .enumerate()
                .map(|(index, text)| SourceItem {
                    id: format!("item-{index}"),
                    provider_item_id: index.to_string(),
                    capture_order: index as u64,
                    text: text.clone(),
                    is_final: true,
                    start_ms: None,
                    end_ms: None,
                })
                .collect(),
        };
        let unchanged = case.source_items.join("\n");
        for (check, text, expected) in [
            ("challenge", case.text.as_str(), case.expected),
            ("source_identity_control", unchanged.as_str(), true),
        ] {
            let natural = json!({"resolvedMode": "light", "text": text});
            let blocks = json!({
                "resolvedMode": "light",
                "blocks": [{"text": text, "sourceRange": [0, source.items.len() - 1]}]
            });
            for (parser, result) in [
                (
                    "natural",
                    parse_natural_output(&natural.to_string(), &mode, &source),
                ),
                (
                    "baseline",
                    parse_model_output(&blocks.to_string(), &mode, &source),
                ),
            ] {
                rows.push(json!({
                    "id": case.id,
                    "check": check,
                    "parser": parser,
                    "expectedAccepted": expected,
                    "actualAccepted": result.is_ok(),
                    "expectationMet": result.is_ok() == expected,
                    "errorCode": result.as_ref().err().map(|error| error.code.as_str()),
                    "errorMessage": result.as_ref().err().map(|error| error.message.as_str())
                }));
            }
        }
    }
    let failures: Vec<_> = rows
        .iter()
        .filter(|row| row["expectationMet"] == false)
        .cloned()
        .collect();
    let mismatches = failures.len();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "tool": "speakmode-quantity-role-probe",
            "scope": "OFFLINE_PUBLIC_SYNTHETIC_TEXT_NOT_AUDIO_ASR_OR_NATIVE_PASTE",
            "evidence": {
                "fixtureEmbeddedAtCompileTime": true,
                "cloudRequests": 0,
                "networkAccess": false,
                "credentialsAccessed": false,
                "userProfileAccessed": false,
                "privateTranscriptsAccessed": false,
                "microphoneAccessed": false,
                "nativeInputDispatched": false
            },
            "cases": fixture.cases.len(),
            "positiveCases": positive_cases,
            "negativeCases": negative_cases,
            "challengeChecks": fixture.cases.len() * 2,
            "identityControlChecks": fixture.cases.len() * 2,
            "total": rows.len(),
            "expectationsMet": rows.len() - mismatches,
            "mismatches": mismatches,
            "failures": failures,
            "rows": rows
        }))
        .expect("serializable offline result")
    );
    if mismatches > 0 {
        std::process::exit(1);
    }
}
