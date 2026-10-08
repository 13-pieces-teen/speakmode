//! Explicit ignored cloud experiment through AppHub::retry_asr, not a substitute feeder.
//! Only the previously registered synthetic B/C WAV hashes are accepted. No microphone,
//! hotkey/UI hooks, native clipboard/input, user history, prefs, modes or dictionary are read.
use super::*;
use rusqlite::{Connection,OpenFlags};
use serde::Deserialize;
use sha2::{Digest,Sha256};
use std::{fs::{self,File,OpenOptions},io::{Seek,SeekFrom,Write},path::{Path,PathBuf}};

const MANIFEST_ENV:&str="SPEAKMODE_RETRY_EXPERIMENT_MANIFEST";
const SCOPE:&str="synthetic-fixed-PCM-only-production-retry-no-auto-input";
const MAX_RUNS:usize=4;
const RUN_TIMEOUT_SECONDS:u64=75;
const CLEANUP_TIMEOUT_SECONDS:u64=10;
const B_HASH:&str="6692ed05e4eceb2809535361de70bff15be7d076af2836812eab43733ac41700";
const C_HASH:&str="1abb6a60c678755d4e281bfb2e1a64a4ace32855ec62b17ea2e1b3c320f6785e";
type ExperimentResult<T>=std::result::Result<T,&'static str>;

#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Manifest {
    schema_version:u8,acknowledgement:String,config_db:PathBuf,receipt_path:PathBuf,
    text_evidence_path:PathBuf,runs:Vec<RunSpec>,
}
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct RunSpec {
    run_id:String,fixture_id:String,wav_path:PathBuf,wav_sha256:String,audio_samples:u64,
    base_mode:BaseMode,dictionary_terms:Vec<String>,
}
struct PreparedRun { spec:RunSpec,pcm:Vec<i16>,dictionary:DictionarySnapshot }
struct Prepared { manifest_sha256:String,config:ProviderConfig,receipt:PathBuf,text_evidence:PathBuf,runs:Vec<PreparedRun> }
fn static_error<T>(result:std::result::Result<T,impl std::fmt::Debug>,message:&'static str)->ExperimentResult<T> {result.map_err(|_|message)}
fn valid_label(value:&str)->bool { !value.is_empty()&&value.len()<=64&&value.bytes().all(|byte|byte.is_ascii_alphanumeric()||b"-_".contains(&byte)) }
fn fixed_identity(spec:&RunSpec)->ExperimentResult<()> {
    let expected=match spec.fixture_id.as_str(){"B"=>(B_HASH,302160),"C"=>(C_HASH,665760),_=>return Err("only frozen synthetic B/C fixtures are permitted")};
    if spec.wav_sha256!=expected.0||spec.audio_samples!=expected.1{return Err("fixture hash/sample registration differs from frozen B/C evidence");}
    if !valid_label(&spec.run_id)||spec.base_mode==BaseMode::Auto{return Err("run ID or fixed mode is invalid");}Ok(())
}
fn evidence_root()->ExperimentResult<PathBuf> {static_error(PathBuf::from("D:/Codex_Project/SpeakMode_Dev/evidence").canonicalize(),"owned evidence directory unavailable")}
fn owned_input(path:&Path,root:&Path)->ExperimentResult<PathBuf> {
    if !path.is_absolute(){return Err("experiment paths must be absolute");}
    let resolved=static_error(path.canonicalize(),"experiment input unavailable")?;
    if !resolved.starts_with(root){return Err("synthetic inputs must stay inside the owned evidence directory");}Ok(resolved)
}
fn owned_output(path:&Path,root:&Path)->ExperimentResult<PathBuf> {
    if !path.is_absolute(){return Err("experiment output must be absolute");}
    let parent=static_error(path.parent().ok_or("output parent required")?.canonicalize(),"output parent must already exist")?;
    if !parent.starts_with(root){return Err("output must stay inside the owned evidence directory");}
    let name=path.file_name().ok_or("output filename required")?;let resolved=parent.join(name);
    if resolved.exists(){return Err("output exists; evidence must never be replaced");}Ok(resolved)
}
fn dictionary(terms:&[String])->ExperimentResult<DictionarySnapshot> {
    if terms.len()>DICTIONARY_MAX_ENTRIES{return Err("synthetic dictionary exceeds entry limit");}
    let snapshot=DictionarySnapshot{version:1,entries:terms.iter().enumerate().map(|(index,term)|DictionaryEntry{
        id:format!("retry-fixture-term-{index}"),term:term.clone(),created_at:"2026-10-08T00:00:00Z".into(),updated_at:"2026-10-08T00:00:00Z".into()}).collect()};
    static_error(crate::core::store::validate_dictionary(&snapshot),"synthetic dictionary invalid")?;Ok(snapshot)
}
fn prepare(manifest_path:&Path)->ExperimentResult<Prepared> {
    let root=evidence_root()?;let path=owned_input(manifest_path,&root)?;
    if static_error(fs::metadata(&path),"manifest unavailable")?.len()>65536{return Err("manifest exceeds bound");}
    let bytes=static_error(fs::read(&path),"manifest unavailable")?;
    let manifest:Manifest=static_error(serde_json::from_slice(&bytes),"invalid explicit retry experiment manifest")?;
    if manifest.schema_version!=1||manifest.acknowledgement!=SCOPE||manifest.runs.is_empty()||manifest.runs.len()>MAX_RUNS{return Err("explicit scope or run bound invalid");}
    let receipt=owned_output(&manifest.receipt_path,&root)?;let text_evidence=owned_output(&manifest.text_evidence_path,&root)?;
    if receipt==text_evidence||receipt.extension().is_none_or(|ext|ext!="json")||text_evidence.extension().is_none_or(|ext|ext!="jsonl"){return Err("separate .json metadata and .jsonl synthetic text outputs required");}
    let mut ids=HashSet::new();let mut runs=Vec::new();
    for spec in manifest.runs {
        fixed_identity(&spec)?;if !ids.insert(spec.run_id.clone()){return Err("duplicate experiment run ID");}
        let wav=owned_input(&spec.wav_path,&root)?;
        if static_error(fs::metadata(&wav),"WAV unavailable")?.len()>16_000*120*2+1024*1024{return Err("WAV byte bound exceeded");}
        let bytes=static_error(fs::read(&wav),"WAV unavailable")?;
        if format!("{:x}",Sha256::digest(&bytes))!=spec.wav_sha256{return Err("WAV hash mismatch before credential access");}
        let mut reader=static_error(hound::WavReader::new(std::io::Cursor::new(&bytes)),"invalid synthetic WAV")?;let format=reader.spec();
        if format.channels!=1||format.sample_rate!=16000||format.bits_per_sample!=16||format.sample_format!=hound::SampleFormat::Int{return Err("synthetic WAV must be mono PCM16 16kHz");}
        let pcm=static_error(reader.samples::<i16>().collect::<std::result::Result<Vec<_>,_>>(),"invalid WAV samples")?;
        if pcm.len() as u64!=spec.audio_samples{return Err("WAV sample count mismatch");}
        let dictionary=dictionary(&spec.dictionary_terms)?;runs.push(PreparedRun{spec,pcm,dictionary});
    }
    if !manifest.config_db.is_absolute(){return Err("configDb must be absolute");}
    // Read exactly one configuration row; never initialize/open the user's Store.
    let connection=static_error(Connection::open_with_flags(&manifest.config_db,OpenFlags::SQLITE_OPEN_READ_ONLY|OpenFlags::SQLITE_OPEN_NO_MUTEX),"provider configuration cannot be read-only opened")?;
    let text:String=static_error(connection.query_row("SELECT data FROM provider_config WHERE singleton=1 AND length(data)<=16384",[],|row|row.get(0)),"provider configuration unavailable")?;
    if text.len()>16384{return Err("provider configuration exceeds bound");}
    let config:ProviderConfig=static_error(serde_json::from_str(&text),"provider configuration invalid")?;
    static_error(cloud::endpoints(&config),"provider endpoint selection invalid")?;
    Ok(Prepared{manifest_sha256:format!("{:x}",Sha256::digest(&bytes)),config,receipt,text_evidence,runs})
}
fn checkpoint(file:&mut File,value:&Value)->ExperimentResult<()> {
    static_error(file.seek(SeekFrom::Start(0)),"receipt seek failed")?;static_error(file.set_len(0),"receipt truncate failed")?;
    static_error(serde_json::to_writer_pretty(&mut *file,value),"metadata receipt write failed")?;
    static_error(file.flush(),"receipt flush failed")?;static_error(file.sync_data(),"receipt persistence failed")?;Ok(())
}
fn append_text(file:&mut File,value:&Value)->ExperimentResult<()> {
    static_error(serde_json::to_writer(&mut *file,value),"synthetic text evidence write failed")?;
    static_error(file.write_all(b"\n"),"synthetic text evidence newline failed")?;
    static_error(file.flush(),"synthetic text evidence flush failed")?;static_error(file.sync_data(),"synthetic text evidence persistence failed")?;Ok(())
}
fn safe_code(code:&str)->String {if code.len()<=80&&code.bytes().all(|byte|byte.is_ascii_alphanumeric()||b"-_".contains(&byte)){code.into()}else{"invalid_error_code".into()}}

async fn isolated_read(hub:&Arc<AppHub>,command:DbCommand)->ExperimentResult<DbValue> {
    let ticket=static_error(hub.db.submit(command,None,DbPriority::Normal),"isolated fixture read admission failed")?;
    let result=static_error(tokio::time::timeout(Duration::from_secs(3),ticket.wait()).await,"isolated fixture read deadline exceeded")?;
    static_error(result,"isolated fixture read failed")
}
async fn fixed_fixture_hub(id:&str,base_mode:BaseMode)->ExperimentResult<Arc<AppHub>> {
    let mode_id=match base_mode {BaseMode::Light=>"light",BaseMode::Structure=>"structure",BaseMode::Auto=>return Err("fixed fixture must not use automatic mode")};
    let hub=super::tests::hub();super::tests::install(&hub,id,SessionPhase::Incomplete,true);
    let mode=match isolated_read(&hub,DbCommand::ModeSnapshot{id:mode_id.into(),revision:Some(1)}).await? {
        DbValue::ModeSnapshot(mode)=>mode,_=>return Err("isolated fixture mode protocol mismatch")
    };
    if mode.base_mode!=base_mode{return Err("isolated fixture mode differs from registration");}
    {let mut inner=hub.inner.lock().unwrap();let session=inner.active.as_mut().unwrap();
        session.prefs=Preferences{keep_history:false,delivery_mode:DeliveryMode::Preview,language:"auto".into(),..Preferences::default()};
        // Store validates the entire frozen revision, not only its ID. Never
        // fabricate a ModeSnapshot that has no corresponding isolated revision.
        session.mode=mode;}
    Ok(hub)
}
fn checkpoint_fixture(hub:&Arc<AppHub>,id:&str)->SourceRevision {
    let attempt=format!("preflight-attempt-{id}");
    {let mut inner=hub.inner.lock().unwrap();let session=inner.active.as_mut().unwrap();
        // Match the generation/reason transition made by the actual ASR retry,
        // without calling retry_asr (which would read normal credentials).
        session.generation=8;session.phase=SessionPhase::FinalizingAsr;session.reason=CreationReason::Retry;
        session.asr_attempt_id=Some(attempt.clone());inner.guards.insert(id.into(),Arc::new(CommitGuard::new(id.into(),8)));}
    SourceRevision{id:format!("preflight-source-{id}"),session_id:id.into(),asr_attempt_id:attempt,generation:8,
        provider:"aliyun".into(),model:"qwen3-asr-flash-realtime".into(),adapter_version:"qwen-realtime-v1".into(),
        complete:true,created_at:"2026-10-04T10:00:12+08:00".into(),items:vec![SourceItem{
            id:format!("preflight-item-{id}"),provider_item_id:"synthetic-preflight-item".into(),capture_order:0,
            text:"如果会议时间未确认，先不要发邀请。".into(),is_final:true,start_ms:Some(0),end_ms:Some(12000)}]}
}
async fn release_fixture(hub:&Arc<AppHub>)->ExperimentResult<()> {
    let result=static_error(tokio::time::timeout(Duration::from_secs(3),hub.db.shutdown()).await,"isolated fixture shutdown deadline exceeded")?;
    static_error(result,"isolated fixture shutdown failed")?;
    if !hub.db.owner_released(){return Err("isolated fixture owner not released");}Ok(())
}
async fn preflight_fixture_modes()->ExperimentResult<()> {
    for (id,base_mode) in [("retry-preflight-light",BaseMode::Light),("retry-preflight-structure",BaseMode::Structure)] {
        let hub=fixed_fixture_hub(id,base_mode).await?;let source=checkpoint_fixture(&hub,id);
        let checked=async {
            let result=static_error(tokio::time::timeout(Duration::from_secs(3),hub.checkpoint_async(id,8,source.clone())).await,"isolated checkpoint preflight deadline exceeded")?;
            static_error(result,"isolated checkpoint preflight failed")?;
            let detail=match isolated_read(&hub,DbCommand::GetResult{session_id:id.into()}).await? {
                DbValue::Detail(detail)=>detail,_=>return Err("isolated checkpoint receipt protocol mismatch")
            };
            let session=hub.inner.lock().unwrap().active.as_ref().unwrap().view();
            if detail.source!=source||detail.session.seed.mode_snapshot!=session.mode_snapshot||detail.session.seed.storage_policy!=StoragePolicy::MemoryOnly||detail.output.is_some()||session.state!=SessionPhase::Rewriting {
                return Err("isolated checkpoint receipt differs from frozen fixture");
            }
            if hub.db.snapshot().usage.count!=0{return Err("checkpoint preflight must not count usage");}
            match isolated_read(&hub,DbCommand::TestQueryCount{sql:"SELECT COUNT(*) FROM delivery_attempts".into()}).await? {
                DbValue::Count(0)=>Ok(()),_=>Err("checkpoint preflight must not prepare native delivery")
            }
        }.await;
        release_fixture(&hub).await?;checked?;
    }Ok(())
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
#[ignore="explicit frozen B/C cloud calls; requires an absolute SPEAKMODE_RETRY_EXPERIMENT_MANIFEST; no default execution"]
async fn actual_retry_asr_fixed_pcm_experiment() {
    assert!(cfg!(windows),"normal Windows credential store is required");
    let path=PathBuf::from(std::env::var_os(MANIFEST_ENV).expect("explicit retry experiment manifest environment variable required"));
    // Check both permitted fixed Modes through the real checkpoint path before
    // even opening the user's read-only provider config, and before any Key/cloud.
    preflight_fixture_modes().await.expect("isolated fixture preflight failed before provider configuration/credentials");
    let prepared=prepare(&path).expect("retry experiment preflight failed before cloud/credentials");
    let mut metadata=OpenOptions::new().write(true).create_new(true).open(&prepared.receipt).expect("new metadata output required");
    let mut text=OpenOptions::new().write(true).create_new(true).open(&prepared.text_evidence).expect("new synthetic text output required");
    let mut receipt=json!({"schemaVersion":1,"tool":"speakmode-actual-retry-asr-experiment","appVersion":env!("CARGO_PKG_VERSION"),
        "manifestSha256":prepared.manifest_sha256,"startedAt":now(),"status":"running","maximumAsrCalls":prepared.runs.len(),
        "maximumRewriteCalls":prepared.runs.len(),"maximumRunSeconds":RUN_TIMEOUT_SECONDS,"maximumCleanupSecondsPerRun":CLEANUP_TIMEOUT_SECONDS,
        "configuration":{"region":prepared.config.region,"asrModel":prepared.config.asr_model,"rewriteModel":prepared.config.rewrite_model,"adapterVersion":prepared.config.adapter_version},
        "automaticRetries":0,"microphoneOpened":false,"nativeInputEnabled":false,"userHistoryRead":false,
        "fixturePreflight":{"passed":true,"modeIds":["light","structure"],"completeSourceCheckpoints":2,"cloudCalls":0,"usageCount":0,"deliveryAttempts":0},
        "preregisteredRuns":prepared.runs.iter().map(|run|json!({"runId":run.spec.run_id,"fixtureId":run.spec.fixture_id,"wavSha256":run.spec.wav_sha256,
            "audioSamples":run.spec.audio_samples,"baseMode":run.spec.base_mode,"dictionaryTermCount":run.dictionary.entries.len()})).collect::<Vec<_>>(),"results":[],
        "timingNotes":["Uses the real AppHub::retry_asr producer unchanged; no substitute pacing and no automatic retry.",
            "socketWritten is client write completion, not a provider ACK; synthetic text is stored separately.",
            "Ready means a saved synthetic output, not semantic accuracy, microphone acceptance, UI latency or native insertion."]});
    checkpoint(&mut metadata,&receipt).unwrap();let mut halted=None::<String>;let mut failures=0;
    for run in prepared.runs {
        if let Some(reason)=&halted {receipt["results"].as_array_mut().unwrap().push(json!({"runId":run.spec.run_id,"status":"skipped","reason":reason}));checkpoint(&mut metadata,&receipt).unwrap();continue;}
        let id=format!("fixed-retry-{}",run.spec.run_id);let hub=fixed_fixture_hub(&id,run.spec.base_mode).await.expect("legal isolated Mode fixture required");
        {let mut inner=hub.inner.lock().unwrap();let session=inner.active.as_mut().unwrap();
            session.config=prepared.config.clone();
            session.dictionary=run.dictionary;session.cache=Some(run.pcm);session.cache_complete=true;session.cache_created=Some(Instant::now());
            session.capture_duration_ms=run.spec.audio_samples*1000/16000;session.error=Some(ApiError::new("asr_incomplete","asr","synthetic failed cache fixture"));}
        let request=json!({"sessionId":id,"expectedGeneration":7,"failedAttemptId":format!("attempt-{id}")});let started=Instant::now();
        let outcome=tokio::time::timeout(Duration::from_secs(RUN_TIMEOUT_SECONDS),hub.retry_asr(&request)).await;
        let command_error=match outcome {Ok(Ok(_))=>None,Ok(Err(error))=>Some(safe_code(&error.code)),Err(_)=>{
            let generation=hub.inner.lock().unwrap().active.as_ref().unwrap().generation;
            let _=tokio::time::timeout(Duration::from_secs(3),hub.cancel_session_async(&json!({"sessionId":id,"expectedGeneration":generation}))).await;
            halted=Some("experiment_timeout".into());Some("experiment_timeout".into())}};
        let elapsed_ms=started.elapsed().as_millis();
        let cleanup_deadline=Instant::now()+Duration::from_secs(CLEANUP_TIMEOUT_SECONDS);
        loop {let released=hub.inner.lock().unwrap().active.as_ref().unwrap().view().worker_released;
            if released&&hub.db.outstanding()==0{break;}if Instant::now()>=cleanup_deadline{halted=Some("experiment_cleanup_unreleased".into());break;}
            tokio::time::sleep(Duration::from_millis(10)).await;}
        let (live,text_row)={let inner=hub.inner.lock().unwrap();let session=inner.active.as_ref().unwrap();
            (session.view(),json!({"runId":run.spec.run_id,"fixtureId":run.spec.fixture_id,"wavSha256":run.spec.wav_sha256,
                "source":session.source,"output":session.output,"receivedCaptions":session.captions}))};
        append_text(&mut text,&text_row).unwrap();
        let count_ticket=static_error(hub.db.submit(DbCommand::TestQueryCount{sql:"SELECT COUNT(*) FROM delivery_attempts".into()},None,DbPriority::Normal),"isolated delivery count admission failed").unwrap();
        let count_result=static_error(tokio::time::timeout(Duration::from_secs(3),count_ticket.wait()).await,"isolated delivery count deadline exceeded").unwrap();
        let deliveries=match static_error(count_result,"isolated delivery count query failed").unwrap(){DbValue::Count(count)=>count,_=>panic!("isolated delivery count protocol mismatch")};
        assert_eq!(deliveries,0,"retry must not prepare or dispatch native input");
        let code=command_error.or_else(||live.error_code.as_deref().map(safe_code));let passed=live.state==SessionPhase::Ready;
        if !passed{failures+=1;}if code.as_deref().is_some_and(|code|matches!(code,"key_required"|"asr_service_error"|"authentication_failed"|"rate_limited")){halted=Some("service_or_credential_rejection".into());}
        let diagnostics=hub.diagnostics.snapshot().unwrap();let usage=hub.db.snapshot().usage.count;
        let shutdown=tokio::time::timeout(Duration::from_secs(3),hub.db.shutdown()).await;let db_released=matches!(shutdown,Ok(Ok(())))&&hub.db.owner_released();
        if !db_released{halted=Some("isolated_db_shutdown_unreleased".into());}
        receipt["results"].as_array_mut().unwrap().push(json!({"runId":run.spec.run_id,"fixtureId":run.spec.fixture_id,"status":if passed{"ready"}else{"failed"},
            "elapsedMs":elapsed_ms,"state":live.state,"errorCode":code,"sourceComplete":live.source_revision_id.is_some(),"workerReleased":live.worker_released,
            "deliveryAttempts":deliveries,"isolatedUsageCount":usage,"isolatedDbReleased":db_released,"diagnostics":diagnostics}));
        checkpoint(&mut metadata,&receipt).unwrap();
    }
    receipt["endedAt"]=json!(now());receipt["haltReason"]=json!(halted);receipt["failedRuns"]=json!(failures);
    receipt["status"]=json!(if halted.is_some(){"halted"}else if failures>0{"completed_with_failures"}else{"completed"});checkpoint(&mut metadata,&receipt).unwrap();
    assert!(failures==0&&halted.is_none(),"retry experiment retained failures; inspect the preregistered evidence outputs");
}

#[test]
fn retry_experiment_registration_rejects_unfrozen_audio_and_automatic_modes_before_credentials() {
    let mut spec=RunSpec{run_id:"B-retry".into(),fixture_id:"B".into(),wav_path:PathBuf::from("D:/fixed/B.wav"),wav_sha256:B_HASH.into(),audio_samples:302160,base_mode:BaseMode::Structure,dictionary_terms:Vec::new()};
    assert!(fixed_identity(&spec).is_ok());spec.wav_sha256="unregistered".into();assert!(fixed_identity(&spec).is_err());
    spec.wav_sha256=B_HASH.into();spec.audio_samples+=1;assert!(fixed_identity(&spec).is_err());spec.audio_samples-=1;
    spec.base_mode=BaseMode::Auto;assert!(fixed_identity(&spec).is_err());spec.base_mode=BaseMode::Light;
    spec.fixture_id="private-microphone-recording".into();assert!(fixed_identity(&spec).is_err());
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn retry_experiment_fixture_preflight_checks_legal_modes_and_checkpoint_without_credentials() {
    // This is exactly the gate used by the ignored cloud experiment, and never
    // reads the manifest, user provider configuration, Key or cached WAV audio.
    preflight_fixture_modes().await.unwrap();
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn retry_experiment_checkpoint_preflight_rejects_missing_and_mismatched_mode_snapshots() {
    for missing in [true,false] {
        let id=if missing{"preflight-missing-mode"}else{"preflight-mismatched-mode"};
        let hub=fixed_fixture_hub(id,BaseMode::Structure).await.unwrap();let source=checkpoint_fixture(&hub,id);
        {let mut inner=hub.inner.lock().unwrap();let mode=&mut inner.active.as_mut().unwrap().mode;
            if missing {mode.mode_id="synthetic-fixed-mode".into();}else{mode.name="Synthetic fixed mode".into();}}
        let error=tokio::time::timeout(Duration::from_secs(3),hub.checkpoint_async(id,8,source)).await.unwrap().unwrap_err();
        assert_eq!(error.code,if missing{"notFound"}else{"invalidInput"});assert_eq!(error.commit_stage.as_deref(),Some("sourceCheckpoint"));
        let ticket=hub.db.submit(DbCommand::GetResult{session_id:id.into()},None,DbPriority::Normal).unwrap();
        assert!(matches!(tokio::time::timeout(Duration::from_secs(3),ticket.wait()).await.unwrap(),Err(StoreError::NotFound)));
        assert_eq!(hub.db.snapshot().usage.count,0);
        assert!(matches!(isolated_read(&hub,DbCommand::TestQueryCount{sql:"SELECT COUNT(*) FROM delivery_attempts".into()}).await.unwrap(),DbValue::Count(0)));
        release_fixture(&hub).await.unwrap();
    }
}
