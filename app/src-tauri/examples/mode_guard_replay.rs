//! Offline replay of frozen model responses against the currently compiled
//! production parser. Never reads credentials, history or audio, and has no
//! request path. Historical Agent grades apply only to unchanged prior text.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use speakmode_lib::{
    cloud::{self, CloudError},
    core::{BaseMode, ModeSnapshot, SourceItem, SourceRevision},
};
use std::{collections::{BTreeMap, HashMap, HashSet}, fs::{self, OpenOptions}, io::Write, path::{Path, PathBuf}};

#[allow(dead_code)]
#[path = "mode_lab/candidates.rs"]
mod candidates;

const CORPUS: &str = include_str!("../../../docs/experiments/fixtures/mode-quality-v2.json");
const REGRESSIONS: &str = include_str!("../../../docs/experiments/fixtures/mode-guard-regression-v1.json");
const CHALLENGES: &str = include_str!("../../../docs/experiments/fixtures/mode-guard-challenges-v1.json");
const HELP: &str = "mode_guard_replay --evidence-dir ABS --out ABS_NEW.json [--label TEXT]\nmode_guard_replay --regressions-only --out ABS_NEW.json [--label TEXT]\nOffline only: replay the frozen 96 screening + 144 confirmation responses, retain original parser decisions, and attach prior Agent labels only for unchanged reviewed outputs. New acceptances require a new independent review. No cloud calls or credentials. --help performs no I/O.";

fn failure(message: impl Into<String>) -> String { message.into() }
fn hash(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn safe(error: &CloudError) -> Value { json!({"code":error.code,"stage":error.stage,"message":error.message}) }

fn read(path: &Path) -> Result<(Value, String), String> {
    if !path.is_absolute() { return Err(failure("Every input path must be absolute")); }
    let metadata = fs::metadata(path).map_err(|_|failure(format!("Cannot read {}", path.display())))?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 { return Err(failure("Input must be a bounded JSON file")); }
    let bytes = fs::read(path).map_err(|_|failure("Cannot read input"))?;
    let value = serde_json::from_slice(&bytes).map_err(|_|failure("Invalid input JSON"))?;
    Ok((value,hash(&bytes)))
}

fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    value[key].as_array().ok_or_else(||failure(format!("Missing array: {key}")))
}

fn source(case: &Value) -> Result<SourceRevision, String> {
    let id = case["id"].as_str().ok_or_else(||failure("Case needs ID"))?;
    let texts = array(case,"sourceItems")?;
    if texts.is_empty() { return Err(failure("Case needs nonempty source")); }
    let items = texts.iter().enumerate().map(|(index,text)| Ok(SourceItem {
        id:format!("{id}:{index}"), provider_item_id:index.to_string(), capture_order:index as u64,
        text:text.as_str().filter(|text|!text.trim().is_empty()).ok_or_else(||failure("Invalid source text"))?.into(),
        is_final:true,start_ms:None,end_ms:None,
    })).collect::<Result<Vec<_>,String>>()?;
    Ok(SourceRevision {id:format!("source-{id}"),session_id:format!("synthetic-{id}"),asr_attempt_id:format!("fixed-{id}"),
        generation:1,provider:"synthetic".into(),model:"fixed-text".into(),adapter_version:"mode-quality-v2".into(),
        complete:true,created_at:"2026-10-09T00:00:00Z".into(),items})
}

fn mode(base: BaseMode, rules: &str) -> ModeSnapshot {
    ModeSnapshot {mode_id:format!("lab-{base:?}"),name:format!("{base:?}"),base_mode:base,rules:rules.into(),revision:1}
}

fn parse(content: &str, arm: &str, mode: &ModeSnapshot, source: &SourceRevision) -> Result<cloud::rewrite::RewriteResult, CloudError> {
    match arm {
        "baseline-flash" => cloud::rewrite::parse_model_output(content,mode,source),
        "fewshot-flash" => candidates::parse(content,mode,source,"fewshot"),
        "typed-flash" | "typed-plus" => candidates::parse(content,mode,source,"typed"),
        _ => Err(CloudError::new("invalid_arm","offline_replay","Unknown frozen arm")),
    }
}

fn validate_receipt(receipt: &Value, split: &str, count: usize) -> Result<(), String> {
    if receipt["split"] != split || receipt["maximumCalls"] != count
        || receipt["fixtureSha256"] != hash(CORPUS.as_bytes()) || receipt["status"] != "completed" {
        return Err(failure("Receipt split/count/hash/completion does not match frozen experiment"));
    }
    let plans = array(receipt,"plan")?;
    let results = array(receipt,"results")?;
    let indices = |rows:&Vec<Value>| rows.iter().map(|row|row["index"].as_u64().ok_or_else(||failure("Missing index"))).collect::<Result<HashSet<_>,String>>();
    let planned = indices(plans)?;
    let recorded = indices(results)?;
    if plans.len() != count || results.len() != count || planned.len() != count
        || recorded.len() != count || planned != recorded || !(0..count as u64).all(|index|planned.contains(&index)) {
        return Err(failure("Incomplete/duplicate receipt rows cannot shrink the denominator"));
    }
    Ok(())
}

fn bump(value: &mut Value, field: &str) {
    let previous = value[field].as_u64().unwrap_or(0);
    value[field] = json!(previous+1);
}

fn stats(rows: &[Value], state: &str) -> Value {
    let mut summary = json!({"total":rows.len(),"accepted":0,"rejected":0,"notReplayed":0,
        "acceptedPriorReviewed":0,"acceptedPriorFidelityPass":0,"acceptedPriorCritical":0,
        "acceptedPriorTaskPass":0,"acceptedMissingSemanticReview":0});
    for row in rows {
        match row[state]["status"].as_str() {
            Some("accepted") => {
                bump(&mut summary,"accepted");
                let grade = &row["priorSemanticGrade"];
                if grade.is_object() && (state == "before" || row["renderedTextUnchanged"] == true) {
                    bump(&mut summary,"acceptedPriorReviewed");
                    if grade["fidelity"] == true {bump(&mut summary,"acceptedPriorFidelityPass");}
                    if grade["critical"] == true {bump(&mut summary,"acceptedPriorCritical");}
                    if grade["fidelity"] == true && grade["modeTask"] == true && grade["route"] == true {
                        bump(&mut summary,"acceptedPriorTaskPass");
                    }
                } else {bump(&mut summary,"acceptedMissingSemanticReview");}
            },
            Some("rejected") => bump(&mut summary,"rejected"),
            _ => bump(&mut summary,"notReplayed"),
        }
    }
    summary
}

fn replay(evidence: &Path) -> Result<(Vec<Value>,Vec<Value>,Vec<Value>),String> {
    let fixture:Value = serde_json::from_str(CORPUS).map_err(|_|failure("Compiled corpus invalid"))?;
    let cases = array(&fixture,"cases")?.iter().map(|case|(case["id"].as_str().unwrap(),case)).collect::<HashMap<_,_>>();
    let mut rows = Vec::new();
    let mut inputs = Vec::new();
    let mut review_queue = BTreeMap::<String,Value>::new();
    for (split,count) in [("screen",96),("confirmation",144)] {
        let receipt_path = evidence.join(format!("{split}-results.json"));
        let score_path = evidence.join(format!("{split}-score.json"));
        let (receipt,receipt_hash) = read(&receipt_path)?;
        let (score,score_hash) = read(&score_path)?;
        validate_receipt(&receipt,split,count)?;
        let grades = array(&score,"rows")?;
        if grades.len() != count {return Err(failure("Historical score must cover every row"));}
        let grades = grades.iter().map(|grade|Ok((grade["index"].as_u64().ok_or_else(||failure("Score lacks index"))?,grade))).collect::<Result<HashMap<_,_>,String>>()?;
        if grades.len() != count {return Err(failure("Duplicate historical score index"));}
        inputs.push(json!({"path":receipt_path,"sha256":receipt_hash}));
        inputs.push(json!({"path":score_path,"sha256":score_hash}));
        for previous in array(&receipt,"results")? {
            let index = previous["index"].as_u64().unwrap();
            let case_id = previous["caseId"].as_str().ok_or_else(||failure("Missing case ID"))?;
            let case = cases.get(case_id).ok_or_else(||failure("Unexpected case ID"))?;
            if case["split"] != split {return Err(failure("Wrong case split"));}
            let grade_row = grades.get(&index).ok_or_else(||failure("Missing historical grade index"))?;
            for key in ["caseId","mode","arm","status"] {
                if previous[key] != grade_row[key] {return Err(failure("Historical score mapping mismatch"));}
            }
            let prior_accepted = previous["status"] == "parsed";
            let prior_grade = grade_row["grade"].clone();
            if prior_accepted != prior_grade.is_object() {return Err(failure("Prior semantic label/acceptance mismatch"));}
            if prior_accepted {
                for flag in ["fidelity","critical","modeTask","route"] {
                    if !prior_grade[flag].is_boolean() {return Err(failure("Historical semantic labels must be booleans"));}
                }
                if prior_grade["critical"] == true && prior_grade["fidelity"] == true {return Err(failure("Inconsistent historical critical label"));}
            }
            let old_status = match previous["status"].as_str() {Some("parsed")=>"accepted",Some("rejected")=>"rejected",_=>"not_replayed"};
            let mut row = json!({"split":split,"index":index,"caseId":case_id,"arm":previous["arm"],"mode":previous["mode"],"repeat":previous["repeat"],
                "before":{"status":old_status,"error":previous["validationError"]},"after":{"status":"not_replayed"},
                "priorSemanticGrade":prior_grade,"priorReviewId":grade_row["reviewId"],"renderedTextUnchanged":false,
                "previousRenderedText":previous["renderedText"]});
            let content = previous.pointer("/response/modelContent").and_then(Value::as_str);
            if previous.pointer("/response/finishReason") == Some(&json!("stop")) && content.is_some() {
                let base:BaseMode = serde_json::from_value(previous["mode"].clone()).map_err(|_|failure("Unknown base Mode"))?;
                let source = source(case)?;
                let mode = mode(base,case["formatRules"].as_str().unwrap_or(""));
                let arm = previous["arm"].as_str().ok_or_else(||failure("Missing arm"))?;
                row["rawResponseSha256"] = json!(hash(content.unwrap().as_bytes()));
                match parse(content.unwrap(),arm,&mode,&source) {
                    Ok(result) => {
                        let text = result.blocks.iter().map(|block|block.text.as_str()).collect::<Vec<_>>().join("\n\n");
                        row["after"] = json!({"status":"accepted","resolvedMode":result.resolved_mode,"renderedText":text});
                        row["renderedTextUnchanged"] = json!(prior_accepted && previous["renderedText"] == text
                            && previous["validatedOutput"]["resolvedMode"] == json!(result.resolved_mode));
                        if !prior_accepted || row["renderedTextUnchanged"] != true {
                            let key = json!([case_id,base,result.resolved_mode,text]).to_string();
                            let id = format!("new-{}",&hash(key.as_bytes())[..16]);
                            row["newReviewId"] = json!(id);
                            review_queue.entry(id.clone()).or_insert_with(||json!({"reviewId":id,"caseId":case_id,
                                "mode":base,"resolvedMode":result.resolved_mode,"text":text,"sourceItems":case["sourceItems"],
                                "formatRules":case["formatRules"],"mustPreserve":case["mustPreserve"],
                                "expectedOrganization":case["expectedOrganization"],"acceptableAutoModes":case["acceptableAutoModes"],
                                "reviewStatus":"NOT_REVIEWED","reviewType":"requires-independent-agent-or-human-review"}));
                        }
                    },
                    Err(error) => {row["after"] = json!({"status":"rejected","error":safe(&error)});},
                }
            }
            row["transition"] = json!(format!("{}->{}",row["before"]["status"].as_str().unwrap(),row["after"]["status"].as_str().unwrap()));
            rows.push(row);
        }
    }
    Ok((rows,inputs,review_queue.into_values().collect()))
}

fn regressions(frozen: &str) -> Result<Value,String> {
    let fixture:Value = serde_json::from_str(frozen).map_err(|_|failure("Compiled regressions invalid"))?;
    let mut rows = Vec::new();
    let mut ids = HashSet::new();
    for case in array(&fixture,"cases")? {
        let source = source(case)?;
        for output in array(case,"outputs")? {
            let id = format!("{}/{}",case["id"].as_str().unwrap(),output["id"].as_str().ok_or_else(||failure("Regression needs output ID"))?);
            if !ids.insert(id.clone()) {return Err(failure("Regression IDs must be unique"));}
            let expected = output["expectedAccepted"].as_bool().ok_or_else(||failure("Expected acceptance must be boolean"))?;
            let text = output["text"].as_str().ok_or_else(||failure("Regression needs text"))?;
            let base:BaseMode = if output["mode"].is_null() {BaseMode::Light} else {
                serde_json::from_value(output["mode"].clone()).map_err(|_|failure("Invalid regression Mode"))?
            };
            let wire = json!({"resolvedMode":if base == BaseMode::Auto {BaseMode::Light} else {base},
                "blocks":[{"text":text,"sourceRange":[0,source.items.len()-1]}]});
            let result = cloud::rewrite::parse_model_output(&wire.to_string(),&mode(base,""),&source);
            rows.push(json!({"id":id,"expectedAccepted":expected,"actualAccepted":result.is_ok(),
                "passed":result.is_ok()==expected,"error":result.as_ref().err().map(safe),
                "sourceItems":case["sourceItems"],"text":text,"reason":output["reason"]}));
        }
    }
    let passed = rows.iter().filter(|row|row["passed"]==true).count();
    Ok(json!({"corpusId":fixture["corpusId"],"provenance":fixture["provenance"],"scope":fixture["scope"],
        "sha256":hash(frozen.as_bytes()),"total":rows.len(),"passed":passed,"failed":rows.len()-passed,"rows":rows}))
}

struct Options {evidence:Option<PathBuf>,out:PathBuf,label:String,only:bool}
fn options() -> Result<Options,String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut evidence = None; let mut out = None; let mut label = "current-production-parser".to_owned(); let mut only = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--regressions-only" => only=true,
            flag @ ("--evidence-dir"|"--out"|"--label") => {
                index+=1;
                let value = args.get(index).ok_or_else(||failure("Missing argument value"))?;
                match flag {"--evidence-dir"=>evidence=Some(PathBuf::from(value)),"--out"=>out=Some(PathBuf::from(value)),_=>label=value.clone()}
            },
            _ => return Err(failure("Unknown argument; see --help")),
        }
        index+=1;
    }
    let out = out.ok_or_else(||failure("--out is required"))?;
    if !out.is_absolute() || out.exists() || label.is_empty() || label.len()>200
        || (!only && evidence.as_ref().is_none_or(|path|!path.is_absolute())) || (only && evidence.is_some()) {
        return Err(failure("Use fresh absolute output and absolute evidence directory, or regressions-only"));
    }
    Ok(Options {evidence,out,label,only})
}

fn execute(options:Options) -> Result<(),String> {
    let (rows,inputs,review_queue) = if options.only {(vec![],vec![],vec![])} else {replay(options.evidence.as_ref().unwrap())?};
    let mut grouped = BTreeMap::<String,Vec<Value>>::new();
    let mut transitions = BTreeMap::<String,usize>::new();
    for row in &rows {
        let group = format!("{}/{}",row["split"].as_str().unwrap(),row["arm"].as_str().unwrap());
        grouped.entry(group).or_default().push(row.clone());
        *transitions.entry(row["transition"].as_str().unwrap().into()).or_default()+=1;
    }
    let per_arm = grouped.iter().map(|(arm,rows)|(arm.clone(),json!({"before":stats(rows,"before"),"after":stats(rows,"after")}))).collect::<BTreeMap<_,_>>();
    let report = json!({"tool":"speakmode-mode-guard-replay","schemaVersion":1,"label":options.label,"appVersion":env!("CARGO_PKG_VERSION"),
        "networkCalls":0,"credentialReads":0,"historyReads":0,"humanQualityMeasured":false,
        "evaluationBoundary":"Same frozen raw model responses, new local parser only. Prior statuses are immutable historical decisions. Prior Agent grades apply only to unchanged accepted texts. New acceptance is NOT evidence of semantic improvement until reviewed. Regression corpus is an engineering specification, not a held-out quality benchmark. No latency/model/ASR/host-delivery claims.",
        "sourceHashes":{"rewrite":hash(include_str!("../src/cloud/rewrite.rs").as_bytes()),
            "rewriteIntegrity":hash(include_str!("../src/cloud/rewrite_integrity.rs").as_bytes()),
            "rewriteFacts":hash(include_str!("../src/cloud/rewrite_facts.rs").as_bytes()),
            "rewriteSemantics":hash(include_str!("../src/cloud/rewrite_semantics.rs").as_bytes()),
            "candidates":hash(include_str!("mode_lab/candidates.rs").as_bytes()),"corpus":hash(CORPUS.as_bytes())},
        "inputs":inputs,"summary":{"before":stats(&rows,"before"),"after":stats(&rows,"after"),"transitions":transitions,"bySplitArm":per_arm},
        "newAcceptanceIndices":rows.iter().filter(|row|row["before"]["status"] != "accepted" && row["after"]["status"] == "accepted").map(|row|json!({"split":row["split"],"index":row["index"],"reviewId":row["newReviewId"]})).collect::<Vec<_>>(),
        "newRejections":rows.iter().filter(|row|row["before"]["status"] == "accepted" && row["after"]["status"] == "rejected").cloned().collect::<Vec<_>>(),
        "newAcceptanceReviewQueue":review_queue,"regressions":regressions(REGRESSIONS)?,
        "postReviewChallenges":regressions(CHALLENGES)?,"rows":rows});
    let mut file = OpenOptions::new().write(true).create_new(true).open(&options.out).map_err(|_|failure("Cannot create fresh output"))?;
    serde_json::to_writer_pretty(&mut file,&report).map_err(|_|failure("Cannot serialize replay"))?;
    file.write_all(b"\n").and_then(|_|file.sync_all()).map_err(|_|failure("Cannot persist replay"))?;
    println!("{}",json!({"out":options.out,"summary":report["summary"],"regressions":{"total":report["regressions"]["total"],"passed":report["regressions"]["passed"]},
        "postReviewChallenges":{"total":report["postReviewChallenges"]["total"],"passed":report["postReviewChallenges"]["passed"]},
        "newReviewCount":report["newAcceptanceReviewQueue"].as_array().unwrap().len()}));
    Ok(())
}

fn main() {
    if std::env::args().skip(1).any(|argument|argument=="--help" || argument=="-h") {println!("{HELP}");return;}
    if let Err(error) = options().and_then(execute) {eprintln!("mode_guard_replay: {error}");std::process::exit(1);}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn denominator_rejects_duplicate_and_missing_indices() {
        let mut receipt = json!({"split":"screen","maximumCalls":2,"status":"completed","fixtureSha256":hash(CORPUS.as_bytes()),
            "plan":[{"index":0},{"index":1}],"results":[{"index":0},{"index":1}]});
        assert!(validate_receipt(&receipt,"screen",2).is_ok());
        receipt["results"][1]["index"]=json!(0);
        assert!(validate_receipt(&receipt,"screen",2).is_err());
    }
    #[test]
    fn new_acceptances_never_inherit_unreviewed_success() {
        let rows = vec![json!({"before":{"status":"rejected"},"after":{"status":"accepted"},"priorSemanticGrade":null,"renderedTextUnchanged":false})];
        let result = stats(&rows,"after");
        assert_eq!(result["accepted"],1);
        assert_eq!(result["acceptedMissingSemanticReview"],1);
        assert_eq!(result["acceptedPriorFidelityPass"],0);
    }
    #[test]
    fn prior_critical_errors_are_counted_only_when_still_accepted() {
        let rows = vec![json!({"before":{"status":"accepted"},"after":{"status":"rejected"},
            "priorSemanticGrade":{"fidelity":false,"critical":true,"modeTask":true,"route":true},"renderedTextUnchanged":false})];
        assert_eq!(stats(&rows,"before")["acceptedPriorCritical"],1);
        assert_eq!(stats(&rows,"after")["acceptedPriorCritical"],0);
    }
    #[test]
    fn changed_rendered_text_invalidates_prior_grade() {
        let rows = vec![json!({"after":{"status":"accepted"},
            "priorSemanticGrade":{"fidelity":true,"critical":false,"modeTask":true,"route":true},"renderedTextUnchanged":false})];
        assert_eq!(stats(&rows,"after")["acceptedMissingSemanticReview"],1);
        assert_eq!(stats(&rows,"after")["acceptedPriorTaskPass"],0);
    }
}
