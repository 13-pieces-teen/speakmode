//! One explicitly selected anonymous text case, four frozen ablations, no delivery.
//! --plan reads no credentials and performs no network I/O. --run is opt-in.
#![recursion_limit = "256"]
use anyhow::{Context, ensure};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use speakmode_lib::{
    cloud::{self, CloudError, rewrite::{self, minimum_pipeline::PersonalizationLabEvent}},
    core::{AppWritingScene, BaseMode, DictionarySnapshot, DraftLanguage, EffectivePersonalization,
        ModeSnapshot, PersonalizationConfig, ProviderConfig, SourceItem, SourceRevision,
        writing_context::WritingContext},
};
use std::{collections::HashSet, fs::{self, File, OpenOptions}, io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf}, sync::{Arc, Mutex}, time::Duration};

const MAX_REQUESTS: usize = 16;
const ARM_RESERVE: usize = 4;
const DEADLINE_MS: u64 = 20_000;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Criteria { facts: Vec<String>, preferences: Vec<String>, risks: Vec<String> }
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    id: String, scene: AppWritingScene, language: DraftLanguage, source: String,
    profile: PersonalizationConfig, criteria: Criteria, category: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Fixture { schema_version: u32, split: String, cases: Vec<Case> }
#[derive(Clone)]
struct Arm { name: &'static str, context: WritingContext, manifest: Value }

fn sha(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn hash(value: &impl Serialize) -> anyhow::Result<String> { Ok(sha(&serde_json::to_vec(value)?)) }
fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
fn parse_fixture(bytes: &[u8], selected: &str) -> anyhow::Result<(Fixture, usize)> {
    ensure!(bytes.len() <= 1024 * 1024 && id_ok(selected), "invalid fixture size or explicit case ID");
    let fixture: Fixture = serde_json::from_slice(bytes)?;
    ensure!(fixture.schema_version == 1 && match fixture.split.as_str() {
        "development" => fixture.cases.len() == 4, "holdout" => fixture.cases.len() == 60, _ => false,
    }, "expected frozen development (4) or holdout (60) fixture");
    let mut ids = HashSet::new();
    for case in &fixture.cases {
        ensure!(id_ok(&case.id) && ids.insert(&case.id), "invalid or duplicate case ID");
        ensure!(!case.source.trim().is_empty() && case.source.chars().count() <= 8000
            && !case.source.contains('\0') && !case.category.trim().is_empty(), "invalid anonymous source/category");
        case.profile.validate().map_err(anyhow::Error::msg)?;
        for criteria in [&case.criteria.facts, &case.criteria.preferences, &case.criteria.risks] {
            ensure!(!criteria.is_empty() && criteria.len() <= 32
                && criteria.iter().all(|v| !v.trim().is_empty() && v.chars().count() <= 1500), "invalid evaluator metadata");
        }
    }
    let index = fixture.cases.iter().position(|case| case.id == selected)
        .context("case ID not present; choose one exact ID, never a split or wildcard")?;
    Ok((fixture, index))
}
fn config(path: &Path) -> anyhow::Result<ProviderConfig> {
    // Deliberately never construct Store: opening Store can migrate persistent state.
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let raw: String = db.query_row("SELECT data FROM provider_config WHERE singleton=1", [], |row| row.get(0))?;
    ensure!(raw.len() <= 16 * 1024, "provider config exceeds size limit");
    let config: ProviderConfig = serde_json::from_str(&raw)?;
    cloud::rewrite_endpoint(&config)?;
    Ok(config)
}
fn source(case: &Case) -> SourceRevision {
    SourceRevision {
        id: "synthetic-source".into(), session_id: "synthetic-session".into(), asr_attempt_id: "synthetic-attempt".into(),
        generation: 1, provider: "synthetic-anonymous-text".into(), model: "fixed-text".into(),
        adapter_version: "personalization-lab-v1".into(), complete: true, created_at: "2026-10-23T00:00:00Z".into(),
        items: vec![SourceItem { id: "synthetic-item".into(), provider_item_id: "0".into(), capture_order: 0,
            text: case.source.clone(), is_final: true, start_ms: None, end_ms: None }],
    }
}
fn mode(scene: AppWritingScene) -> ModeSnapshot {
    let id = match scene { AppWritingScene::General => "auto", AppWritingScene::Chat => "chat",
        AppWritingScene::Document => "document", AppWritingScene::Email => "email", AppWritingScene::Technical => "technical" };
    ModeSnapshot { mode_id: id.into(), name: id.into(),
        base_mode: BaseMode::Auto, rules: String::new(), revision: 1 }
}
fn empty_background(profile: &mut EffectivePersonalization) {
    if profile.background.as_ref().is_some_and(|b| b.profession.is_empty() && b.domain.is_empty()
        && b.description.is_empty() && b.tasks.is_empty() && b.audience.is_empty()) { profile.background = None; }
}
fn contexts(case: &Case) -> anyhow::Result<[WritingContext; 4]> {
    // Freeze exactly once. Re-freezing a reduced config would refill budget-omitted fields.
    let c = case.profile.freeze(case.scene).context("selected case must have an effective personalization")?;
    c.validate().map_err(anyhow::Error::msg)?;
    let mut b = c.clone();
    b.rules.clear();
    if let Some(background) = &mut b.background { background.tasks.clear(); background.audience.clear(); }
    empty_background(&mut b);
    let mut d = c.clone();
    if let Some(background) = &mut d.background {
        background.profession.clear(); background.domain.clear(); background.description.clear();
    }
    empty_background(&mut d);
    for value in [&b, &c, &d] { value.validate().map_err(anyhow::Error::msg)?; }
    // An empty ablation must also remove the personalization contract itself.
    let b = (b.background.is_some() || !b.rules.is_empty()).then_some(b);
    let d = (d.background.is_some() || !d.rules.is_empty()).then_some(d);
    let values = [None, b, Some(c), d].map(|personalization| WritingContext {
        scene: case.scene, language: case.language, input: None, personalization,
    });
    for value in &values { value.validate().map_err(anyhow::Error::msg)?; }
    Ok(values)
}
fn occupation_fields(context: &WritingContext) -> Option<(&str, &str, &str)> {
    let background = context.personalization.as_ref()?.background.as_ref()?;
    if background.profession.is_empty() && background.domain.is_empty() && background.description.is_empty() { return None; }
    Some((&background.profession, &background.domain, &background.description))
}
fn stable_generation_body(body: &Value, source: &SourceRevision) -> anyhow::Result<Value> {
    // Production's long-source literal protection uses a fresh collision-checked
    // four-hex prefix. Hash its shape with only that ephemeral prefix normalized;
    // do not change the request actually passed to production or source literals.
    let mut body = body.clone();
    let mut data: Value = serde_json::from_str(body["messages"][1]["content"].as_str().context("missing prompt data")?)?;
    if let Some(masked) = data["sourceText"].as_str() {
        let original = source.text();
        let mut prefixes = HashSet::new();
        for (index, _) in masked.match_indices("⟪SM") {
            let start = index + "⟪SM".len();
            if let Some(suffix) = masked.get(start..) {
                if suffix.as_bytes().get(..4).is_some_and(|bytes| bytes.iter().all(u8::is_ascii_hexdigit))
                    && suffix.as_bytes().get(4) == Some(&b'_') {
                    let prefix = &masked[index..start + 5];
                    if !original.contains(prefix) { prefixes.insert(prefix.to_owned()); }
                }
            }
        }
        let mut canonical = masked.to_owned();
        for prefix in prefixes { canonical = canonical.replace(&prefix, "⟪SM_NONCE_"); }
        data["sourceText"] = json!(canonical);
    }
    body["messages"][1]["content"] = json!(serde_json::to_string(&data)?);
    Ok(body)
}
fn prepare_arms(case: &Case, config: &ProviderConfig, source: &SourceRevision,
    mode: &ModeSnapshot, dictionary: &DictionarySnapshot) -> anyhow::Result<Vec<Arm>> {
    let mut arms = Vec::new();
    let contexts = contexts(case)?;
    let occupation_contrast_applicable = occupation_fields(&contexts[1]).is_some()
        && occupation_fields(&contexts[1]) == occupation_fields(&contexts[2]);
    for (name, context) in ["A", "B", "C", "D"].into_iter().zip(contexts) {
        let request = rewrite::minimum::prepare_with_writing_context(config, mode, source, dictionary, true, Some(&context))?;
        let data: Value = serde_json::from_str(request.body["messages"][1]["content"].as_str().context("missing prompt data")?)?;
        let provider_data = context.personalization.as_ref().map(EffectivePersonalization::provider_data);
        ensure!(data.get("personalization") == provider_data.as_ref(), "provider personalization differs from frozen projection");
        ensure!(data.get("criteria").is_none() && data["writingContext"].get("personalization").is_none()
            && data["writingContext"]["input"].is_null(), "evaluator or local context leaked into prompt");
        let manifest = json!({"arm":name,"maximumClientStartedRequests":ARM_RESERVE,
            "explicitOccupationalBackgroundPresent":occupation_fields(&context).is_some(),
            "occupationContrastApplicable":occupation_contrast_applicable,
            "contextSha256":hash(&context)?,"personalizationProviderData":provider_data,
            "personalizationProviderDataSha256":hash(&provider_data)?,
            "normalizedGenerationBodySha256":hash(&stable_generation_body(&request.body, source)?)?,
            "hashBasis":"PREFLIGHT_PRODUCTION_BODY_WITH_EPHEMERAL_GUARDED_PREFIX_CANONICALIZED_NOT_ACTUAL_HTTP_BODY_HASH",
            "frozenPersonalizationLocal":context.personalization});
        arms.push(Arm { name, context, manifest });
    }
    Ok(arms)
}

fn durable_row(file: &mut File, value: &Value) -> std::io::Result<()> {
    serde_json::to_writer(&mut *file, value)?;
    file.write_all(b"\n")?; file.flush()?; file.sync_data()
}
fn durable_receipt(file: &mut File, value: &Value) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    file.seek(SeekFrom::Start(0))?; file.write_all(&bytes)?;
    file.set_len(bytes.len() as u64)?; file.flush()?; file.sync_all()?; Ok(())
}
// Started counts represent callback-authorized application transport starts.
// Conversion/cancellation may stop them before HTTP dispatch. They are not
// provider billing, token use, connection attempts or trace.calls.
struct Ledger { file: File, started: usize, arm_started: usize, failure: Option<String> }
impl Ledger {
    fn append(&mut self, value: &Value) -> bool {
        if self.failure.is_some() { return false; }
        if let Err(error) = durable_row(&mut self.file, value) { self.failure = Some(error.to_string()); return false; }
        true
    }
    fn reserve_arm(&mut self, case: &str, arm: &str) -> bool {
        if !budget_allows(self.started, ARM_RESERVE) {
            self.failure = Some("request reservation exceeded maximum".into()); return false;
        }
        self.arm_started = 0;
        self.append(&json!({"kind":"arm_started","caseId":case,"arm":arm,
            "clientStartedRequestsBefore":self.started,"reservedMaximumRequests":ARM_RESERVE}))
    }
    fn observe(&mut self, case: &str, arm: &str, event: PersonalizationLabEvent) -> bool {
        if self.failure.is_some() { return false; }
        let started = event.event == "started";
        if started && (self.started >= MAX_REQUESTS || self.arm_started >= ARM_RESERVE) {
            self.failure = Some("observed request maximum exceeded; dispatch cancelled".into()); return false;
        }
        let allowed = self.append(&json!({"kind":"request_event","caseId":case,"arm":arm,
            "event":event,"clientStartedRequestsBefore":self.started,
            "clientStartedRequestsIfAllowed":self.started + usize::from(started)}));
        if allowed && started { self.started += 1; self.arm_started += 1; }
        allowed
    }
}
fn budget_allows(started: usize, reserve: usize) -> bool {
    reserve == ARM_RESERVE && started.checked_add(reserve).is_some_and(|sum| sum <= MAX_REQUESTS)
}
fn fatal(error: &CloudError) -> bool {
    matches!(error.code.as_str(), "authentication_failed" | "rate_limited" | "rewrite_timeout" | "request_timeout"
        | "timeout" | "credential_unavailable" | "key_required" | "lab_ledger_failed")
}
fn not_run(case: &str, arm: &str, reason: &str) -> Value {
    json!({"caseId":case,"arm":arm,"status":"NOT_RUN","reason":reason,
        "clientStartedRequests":0,"actualNativeDelivery":"NOT_RUN","qualityEvaluation":"NOT_EVALUATED"})
}
fn text(result: &rewrite::RewriteResult) -> String {
    result.blocks.iter().map(|block| block.text.as_str()).collect::<Vec<_>>().join("\n")
}
fn ledger_lock(ledger: &Arc<Mutex<Ledger>>) -> std::sync::MutexGuard<'_, Ledger> {
    // Observer panics must not turn a failed journal into permission to dispatch.
    ledger.lock().unwrap_or_else(|poison| { let mut guard = poison.into_inner();
        guard.failure = Some("ledger mutex poisoned".into()); guard })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(args.len() == 5 && ["--plan", "--run"].contains(&args[4].as_str()),
        "personalization_quality_lab CONFIG_DB FIXTURE NEW_OUT CASE_ID --plan|--run");
    ensure!(args[..3].iter().all(|arg| Path::new(arg).is_absolute()), "absolute paths required");
    let bytes = fs::read(&args[1])?;
    let (fixture, index) = parse_fixture(&bytes, &args[3])?;
    let case = &fixture.cases[index];
    let config = config(Path::new(&args[0]))?;
    let source = source(case); let mode = mode(case.scene); let dictionary = DictionarySnapshot::default();
    let arms = prepare_arms(case, &config, &source, &mode, &dictionary)?;
    let mut receipt = json!({"tool":"personalization-quality-lab-v1","status":"PLANNED",
        "scope":"EXTENDED_DEADLINE_TEXT_QUALITY","deadlineMs":DEADLINE_MS,
        "productionDeadlineMs":rewrite::initial_wait_ms(&source),"normalProductionLatencyClaim":false,
        "split":fixture.split,"selectedCaseId":case.id,"selectedCaseCount":1,"totalFixtureCases":fixture.cases.len(),
        "fixtureSha256":sha(&bytes),"fixturePin":"EXACT_METADATA_MATCH_TO_DURABLE_PLAN",
        "selectedCaseSha256":hash(case)?,"sourceSha256":hash(&source)?,"sourceTextSha256":sha(case.source.as_bytes()),
        "providerConfigSha256":hash(&config)?,"rewriteProvider":config.rewrite_provider,"rewriteModel":config.rewrite_model,
        "region":config.region,"rewriteEndpointSha256":sha(cloud::rewrite_endpoint(&config)?.as_bytes()),
        "executableSha256":sha(&fs::read(std::env::current_exe()?)?),
        "runnerSourceSha256":sha(include_bytes!("personalization_quality_lab.rs")),
        "pipelineSourceSha256":sha(include_bytes!("../src/cloud/rewrite_minimum_pipeline.rs")),
        "personalizationSourceSha256":sha(include_bytes!("../src/core/personalization.rs")),
        "writingContextSourceSha256":sha(include_bytes!("../src/cloud/rewrite_writing_context.rs")),
        "source":source,"mode":mode,"scene":case.scene,"language":case.language,"inputContext":null,
        "dictionaryLabel":"FIXED_EMPTY_DICTIONARY_SNAPSHOT","dictionary":dictionary,"dictionarySha256":hash(&dictionary)?,
        "category":case.category,"criteriaEvaluatorMetadataOnly":case.criteria,
        "explicitOccupationalBackgroundPresent":arms[2].manifest["explicitOccupationalBackgroundPresent"],
        "occupationContrastApplicable":arms[2].manifest["occupationContrastApplicable"],
        "occupationContrastScope":"Only explicit profession/domain/description fields are ablated; tasks, audience, source and dictionary may retain domain clues",
        "maximumClientStartedRequests":MAX_REQUESTS,"clientStartedRequests":0,"automaticRetries":0,"redirects":0,
        "requestCountBoundary":"Durably authorized pipeline transport starts; conversion/cancellation can precede HTTP send; not billing or provider receipt",
        "transportStartsAreBilling":false,"armOrder":["A","B","C","D"],"plan":arms.iter().map(|arm| &arm.manifest).collect::<Vec<_>>(),
        "actualNativeDelivery":"NOT_RUN","audioRecognition":"NOT_RUN","appContextCapture":"NOT_RUN",
        "qualityEvaluation":"NOT_EVALUATED","evaluationBoundary":"Pipeline acceptance and model review do not establish independent quality",
        "results":arms.iter().map(|arm| not_run(&case.id, arm.name, "plan_only")).collect::<Vec<_>>()});
    let out_path = PathBuf::from(&args[2]);
    let ledger_path = PathBuf::from(format!("{}.jsonl",args[2]));
    let plan_path = PathBuf::from(format!("{}.plan.json",args[2]));
    ensure!(!out_path.exists() && !ledger_path.exists(), "fresh output and ledger required");
    if args[4] == "--plan" {
        let mut plan = OpenOptions::new().write(true).create_new(true).open(&plan_path)?;
        durable_receipt(&mut plan, &receipt)?;
        println!("PLANNED: one selected case, 4 arms, maximum {MAX_REQUESTS} requests; credentials/network NOT_RUN");
        return Ok(());
    }
    let frozen: Value = serde_json::from_slice(&fs::read(&plan_path).context("run --plan with the same NEW_OUT and exact case before --run")?)?;
    ensure!(frozen == receipt, "run metadata differs from the durable plan (fixture/config/source/context/body/executable); create and review a fresh plan");
    let mut output = OpenOptions::new().write(true).create_new(true).open(out_path)?;
    let journal = match OpenOptions::new().write(true).create_new(true).open(ledger_path) {
        Ok(file) => file,
        Err(error) => {
            receipt["status"] = json!("HALTED"); receipt["artifactError"] = json!(error.to_string());
            receipt["results"] = json!(arms.iter().map(|arm| not_run(&case.id, arm.name, "ledger_creation_failure")).collect::<Vec<_>>());
            durable_receipt(&mut output, &receipt)?; return Err(error.into());
        }
    };
    let ledger = Arc::new(Mutex::new(Ledger { file:journal, started:0, arm_started:0, failure:None }));
    let journal_ready = ledger_lock(&ledger).append(&json!({"kind":"plan","receipt":receipt}));
    durable_receipt(&mut output, &receipt)?;
    if !journal_ready {
        receipt["status"] = json!("HALTED"); receipt["ledgerError"] = json!(ledger_lock(&ledger).failure);
        receipt["results"] = json!(arms.iter().map(|arm| not_run(&case.id, arm.name, "ledger_initialization_failure")).collect::<Vec<_>>());
        durable_receipt(&mut output, &receipt)?;
        println!("{}: one selected case; credentials/network NOT_RUN; requests=0/{MAX_REQUESTS}",receipt["status"]);
        anyhow::bail!("durable ledger initialization failed; see receipt");
    }
    // Native Rewrite account only, after both artifacts and initial plan are durable.
    let secret = cloud::credentials::read_for(&config, cloud::credentials::CredentialStage::Rewrite);
    let mut rows = Vec::new();
    let mut halt: Option<String> = None;
    let secret = match secret {
        Ok(value) => Some(value),
        Err(error) => { receipt["credentialError"] = json!(error); halt = Some("credential_failure".into()); None }
    };
    for arm in &arms {
        if let Some(reason) = &halt { rows.push(not_run(&case.id, arm.name, reason)); continue; }
        if !ledger_lock(&ledger).reserve_arm(&case.id, arm.name) {
            halt = Some("ledger_failure_or_budget".into());
            rows.push(not_run(&case.id, arm.name, halt.as_deref().unwrap())); continue;
        }
        // The next API is the sole network path. Callback failure denies dispatch.
        let observed = Arc::clone(&ledger); let case_id = case.id.clone(); let arm_name = arm.name;
        let outcome = rewrite::minimum_pipeline::experiment_personalization_observed(
            &config, secret.as_deref().expect("credential failure halts arms"), &mode, &source, &dictionary,
            &arm.context, Duration::from_millis(DEADLINE_MS), move |event| {
                ledger_lock(&observed).observe(&case_id, arm_name, event)
            }).await;
        let mut row = match outcome {
            Ok(outcome) => {
                let error = outcome.result.as_ref().err();
                if error.is_some_and(fatal) { halt = Some("fatal_provider_or_timeout".into()); }
                json!({"caseId":case.id,"arm":arm.name,"status":"COMPLETED","pipelineAccepted":outcome.result.is_ok(),
                    "acceptedText":outcome.result.as_ref().ok().map(text),"acceptedDraft":outcome.result.as_ref().ok(),
                    "fallbackToRawSource":outcome.result.is_err(),"rawSourceOnFailure":if outcome.result.is_err(){Some(source.text())}else{None},
                    "error":error,"trace":outcome.trace,"actualNativeDelivery":"NOT_RUN","qualityEvaluation":"NOT_EVALUATED"})
            }
            Err(error) => {
                if fatal(&error) { halt = Some("fatal_provider_or_timeout".into()); }
                json!({"caseId":case.id,"arm":arm.name,"status":"COMPLETED","pipelineAccepted":false,
                    "acceptedText":null,"fallbackToRawSource":true,"rawSourceOnFailure":source.text(),
                    "outerApiError":true,"error":error,"actualNativeDelivery":"NOT_RUN","qualityEvaluation":"NOT_EVALUATED"})
            }
        };
        {
            let mut guard = ledger_lock(&ledger);
            row["clientStartedRequests"] = json!(guard.arm_started);
            let total = guard.started;
            guard.append(&json!({"kind":"arm_completed","row":row,"clientStartedRequestsTotal":total}));
            if guard.failure.is_some() { halt = Some("ledger_failure".into()); }
            receipt["clientStartedRequests"] = json!(guard.started);
            receipt["ledgerError"] = json!(guard.failure);
        }
        println!("{} {}: accepted={} starts={}",case.id,arm.name,row["pipelineAccepted"],row["clientStartedRequests"]);
        rows.push(row);
        receipt["status"] = json!("RUNNING"); receipt["results"] = json!(rows);
        // If receipt persistence fails, no later arm is dispatched.
        if let Err(error) = durable_receipt(&mut output, &receipt) {
            receipt["artifactError"] = json!(error.to_string()); halt = Some("receipt_persistence_failure".into());
        }
    }
    receipt["status"] = json!(if halt.is_some(){"HALTED"}else{"COMPLETED"});
    receipt["haltReason"] = json!(halt); receipt["results"] = json!(rows);
    {
        let mut guard = ledger_lock(&ledger);
        guard.append(&json!({"kind":"run_finished","receipt":receipt}));
        receipt["clientStartedRequests"] = json!(guard.started); receipt["ledgerError"] = json!(guard.failure);
        if guard.failure.is_some() { receipt["status"] = json!("HALTED"); receipt["haltReason"] = json!("ledger_failure"); }
    }
    durable_receipt(&mut output, &receipt)?;
    println!("{}: one case, requests={}/{MAX_REQUESTS}; no delivery; quality NOT_EVALUATED",receipt["status"],receipt["clientStartedRequests"]);
    if receipt["status"] == "HALTED" { anyhow::bail!("run halted; all arm outcomes are in the receipt"); }
    Ok(())
}

#[cfg(test)]
#[path = "personalization_quality_lab/tests.rs"]
mod tests;
