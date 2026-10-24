//! Explicit offline CPU microbenchmark. No network client, credentials, user
//! database, model response, microphone, persistence API or host delivery.
//! Public production preparation APIs only; post-response validation is excluded.
#![recursion_limit = "256"]
use anyhow::{ensure,Context};
use serde::Serialize;
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
use speakmode_lib::{cloud::rewrite::{self,minimum,semantic_review},core::{AppWritingScene,BaseMode,DictionarySnapshot,
    DraftLanguage,ExpressionStyle,ExpressionTone,ModeSnapshot,OutputBlock,PersonalProfile,PersonalizationConfig,
    ProviderConfig,SentenceStyle,SourceItem,SourceRevision,PERSONALIZATION_PROVIDER_MAX_CHARS,writing_context::WritingContext}};
use std::{fs::{self,OpenOptions},hint::black_box,io::Write,path::Path,time::Instant};

const LENGTHS:[usize;5]=[400,800,3000,8000,80_000];
const WARMUP_PAIRS:usize=20;
const SAMPLE_PAIRS:usize=200;
const ADDED_PREPARATION_LIMIT_MS:f64=30.;
const SOURCE_PATTERN:&str="项目状态仍需核对，方案尚处试验阶段，可能需要返工。请先检查流程，再同步进度。";
#[derive(Clone)]
struct ProfileCase {label:&'static str,config:PersonalizationConfig}
#[derive(Debug,Clone,Copy,Serialize)]
#[serde(rename_all="camelCase")]
struct Durations {
    freeze_ns:u64,generation_prepare_ns:u64,review_prepare_ns:u64,
    /// Diagnostic only: prepares already project the profile internally.
    projection_only_ns:u64,
    /// Sum of the three preparation windows, not a wall-clock end-to-end time.
    preparation_total_ns:u64,
}
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
struct Pair {index:usize,order:&'static str,without_profile:Durations,with_profile:Durations,signed_preparation_delta_ns:i64}
fn sha(bytes:&[u8])->String{format!("{:x}",Sha256::digest(bytes))}
fn hash(value:&impl Serialize)->anyhow::Result<String>{Ok(sha(&serde_json::to_vec(value)?))}
fn text(pattern:&str,count:usize)->String{pattern.chars().cycle().take(count).collect()}
fn elapsed(start:Instant)->u64{start.elapsed().as_nanos().min(u64::MAX as u128) as u64}
fn percentile(values:&[f64],numerator:usize)->f64{
    assert!(!values.is_empty());let mut values=values.to_vec();values.sort_by(f64::total_cmp);
    values[((values.len()*numerator+99)/100).saturating_sub(1)]
}
fn distribution(values:&[f64])->Value{
    json!({"samples":values.len(),"p50Ms":percentile(values,50),"p95Ms":percentile(values,95),
        "minimumMs":values.iter().copied().fold(f64::INFINITY,f64::min),
        "maximumMs":values.iter().copied().fold(f64::NEG_INFINITY,f64::max),"quantile":"nearest-rank"})
}
fn ms(value:u64)->f64{value as f64/1_000_000.}

fn maximum_config(instruction_chars:usize,escaped:bool)->PersonalizationConfig{
    let pattern=if escaped{"资料🙂\"\\\n\t"}else{"沟通资料🙂"};
    let profiles=(0..3).map(|index|PersonalProfile{id:format!("LOCAL_ONLY_CPU_PROFILE_{index}"),name:format!("本地合成画像{index}"),
        profession:text(pattern,80),domain:text(pattern,120),description:text(pattern,300),audience:text(pattern,120),
        tasks:(0..8).map(|_|text(pattern,80)).collect(),scenes:vec![AppWritingScene::Technical],instructions:text(pattern,instruction_chars)}).collect();
    PersonalizationConfig{enabled:true,active_profile_id:Some("LOCAL_ONLY_CPU_PROFILE_0".into()),profiles,
        preferences:ExpressionStyle{sentence_style:SentenceStyle::Concise,tone:ExpressionTone::Formal,
            keep_english_terms:true,fewer_courtesies:true,paragraphs:true,lists:true},..Default::default()}
}
fn maximum_case(escaped:bool)->anyhow::Result<ProfileCase>{
    // Search is fixture setup, outside all timed windows. Freeze remains the
    // exact production whole-field budget selector; no payload is hand-filled.
    let mut best=None;let mut best_size=0;
    for length in 0..=500 {
        let config=maximum_config(length,escaped);config.validate().map_err(anyhow::Error::msg)?;
        let frozen=config.freeze(AppWritingScene::Technical).context("synthetic maximum profile missing")?;
        let size=frozen.provider_data().to_string().chars().count();
        if size>best_size {best_size=size;best=Some(config);}
        if size==PERSONALIZATION_PROVIDER_MAX_CHARS {break;}
    }
    ensure!(best_size>=PERSONALIZATION_PROVIDER_MAX_CHARS-10,"maximum profile fixture is not near the actual serialized character bound");
    Ok(ProfileCase{label:if escaped{"maxEscapedUtf8"}else{"maxPlainUtf8"},config:best.context("maximum profile missing")?})
}
fn profile_cases()->anyhow::Result<Vec<ProfileCase>>{
    let typical=PersonalizationConfig{enabled:true,active_profile_id:Some("LOCAL_ONLY_TYPICAL".into()),
        profiles:vec![PersonalProfile{id:"LOCAL_ONLY_TYPICAL".into(),name:"本地合成开发画像".into(),profession:"软件开发".into(),domain:"后端与智能体系统".into(),
            description:"需要向同事同步项目进度，保留具体条件与不确定性。".into(),tasks:vec!["技术说明".into(),"同步工作进度".into()],audience:"项目同事".into(),
            scenes:vec![AppWritingScene::Technical],instructions:"沿用源文明确出现的技术术语。".into()}],
        preferences:ExpressionStyle{sentence_style:SentenceStyle::Concise,keep_english_terms:true,paragraphs:true,..Default::default()},..Default::default()};
    typical.validate().map_err(anyhow::Error::msg)?;
    Ok(vec![ProfileCase{label:"syntheticTypical",config:typical},maximum_case(false)?,maximum_case(true)?])
}
fn mode()->ModeSnapshot{ModeSnapshot{mode_id:"technical".into(),name:"technical".into(),base_mode:BaseMode::Light,rules:String::new(),revision:1}}
fn source(chars:usize)->SourceRevision{
    SourceRevision{id:format!("synthetic-source-{chars}"),session_id:"offline-cpu-lab".into(),asr_attempt_id:"fixed".into(),generation:1,
        provider:"synthetic-text".into(),model:"fixed".into(),adapter_version:"local-cpu-lab-v1".into(),complete:true,created_at:"2026-10-24T00:00:00Z".into(),
        items:vec![SourceItem{id:"item".into(),provider_item_id:"0".into(),capture_order:0,text:text(SOURCE_PATTERN,chars),is_final:true,start_ms:None,end_ms:None}]}
}
fn passthrough(source:&SourceRevision)->rewrite::RewriteResult{
    rewrite::RewriteResult{resolved_mode:BaseMode::Light,blocks:vec![OutputBlock{id:"fixed-passthrough-draft".into(),text:source.text(),source_refs:vec!["item".into()]}]}
}
fn context(config:&PersonalizationConfig)->WritingContext{
    WritingContext{scene:AppWritingScene::Technical,language:DraftLanguage::AsSpoken,input:None,personalization:config.freeze(AppWritingScene::Technical)}
}
fn validate_payload(body:&Value,source:&SourceRevision,writing:&WritingContext,review:bool)->anyhow::Result<()> {
    let data:Value=serde_json::from_str(body["messages"][1]["content"].as_str().context("request user data missing")?)?;
    ensure!(data["sourceText"]==json!(source.text()),"full synthetic source changed");
    if review {ensure!(data["draftText"]==json!(source.text()),"fixed passthrough draft changed");}
    ensure!(data["writingContext"]["scene"]=="technical" && data["writingContext"]["language"]=="asSpoken","explicit context changed");
    ensure!(data["writingContext"].get("personalization").is_none(),"profile duplicated in context");
    if let Some(profile)=&writing.personalization {
        let expected=profile.provider_data();ensure!(data["personalization"]==expected,"profile projection changed");
        let fields=data["personalization"].as_object().context("profile projection is not an object")?;
        ensure!(fields.keys().all(|key|matches!(key.as_str(),"scene"|"rules"|"background")),"local identity escaped provider projection");
    }else{ensure!(data.get("personalization").is_none(),"disabled profile unexpectedly sent");}
    Ok(())
}

fn validate_case(provider:&ProviderConfig,mode:&ModeSnapshot,source:&SourceRevision,draft:&rewrite::RewriteResult,
    config:&PersonalizationConfig)->anyhow::Result<Value>{
    let writing=context(config);let dictionary=DictionarySnapshot::default();
    let generation=minimum::prepare_with_writing_context(provider,mode,source,&dictionary,true,Some(&writing))?;
    let review=semantic_review::prepare_minimum_with_writing_context(provider,mode,source,draft,Some(&writing))?;
    validate_payload(&generation.body,source,&writing,false)?;validate_payload(&review.body,source,&writing,true)?;
    let projection=writing.personalization.as_ref().map(|profile|profile.provider_data().to_string());
    Ok(json!({"configSha256":hash(config)?,"contextSha256":hash(&writing)?,
        "generationBodySha256":hash(&generation.body)?,"reviewBodySha256":hash(&review.body)?,
        "profileProviderDataSha256":projection.as_ref().map(|value|sha(value.as_bytes())),
        "profileSerializedUnicodeChars":projection.as_ref().map_or(0,|value|value.chars().count()),
        "profileSerializedUtf8Bytes":projection.as_ref().map_or(0,String::len),
        "profileConfigCount":config.profiles.len(),
        "profileOmittedFields":writing.personalization.as_ref().map(|profile|&profile.omitted),
        "profileRuleKeys":writing.personalization.as_ref().map(|profile|profile.rules.iter().map(|rule|&rule.key).collect::<Vec<_>>())}))
}

fn measure(provider:&ProviderConfig,mode:&ModeSnapshot,source:&SourceRevision,draft:&rewrite::RewriteResult,
    dictionary:&DictionarySnapshot,config:&PersonalizationConfig)->anyhow::Result<Durations>{
    let started=Instant::now();let frozen=black_box(config).freeze(AppWritingScene::Technical);let freeze_ns=elapsed(started);
    let writing=WritingContext{scene:AppWritingScene::Technical,language:DraftLanguage::AsSpoken,input:None,personalization:frozen};
    let started=Instant::now();
    let generation=minimum::prepare_with_writing_context(black_box(provider),black_box(mode),black_box(source),black_box(dictionary),true,Some(black_box(&writing)))?;
    black_box(&generation.body);let generation_prepare_ns=elapsed(started);
    let started=Instant::now();
    let review=semantic_review::prepare_minimum_with_writing_context(black_box(provider),black_box(mode),black_box(source),black_box(draft),Some(black_box(&writing)))?;
    black_box(&review.body);let review_prepare_ns=elapsed(started);
    // A diagnostic projection runs AFTER the core windows, so it cannot warm
    // the profile-only preparation path first. Never add it to the core total.
    let projection_only_ns=if let Some(profile)=&writing.personalization {
        let started=Instant::now();let projected=black_box(profile).provider_data().to_string();black_box(&projected);elapsed(started)
    }else{0};
    black_box((&generation,&review,&writing));
    Ok(Durations{freeze_ns,generation_prepare_ns,review_prepare_ns,projection_only_ns,
        preparation_total_ns:freeze_ns.saturating_add(generation_prepare_ns).saturating_add(review_prepare_ns)})
}
fn pair(provider:&ProviderConfig,mode:&ModeSnapshot,source:&SourceRevision,draft:&rewrite::RewriteResult,dictionary:&DictionarySnapshot,
    off:&PersonalizationConfig,on:&PersonalizationConfig,index:usize,on_first:bool)->anyhow::Result<Pair>{
    let(without_profile,with_profile)=if on_first {
        let on_sample=measure(provider,mode,source,draft,dictionary,on)?;
        (measure(provider,mode,source,draft,dictionary,off)?,on_sample)
    }else{
        let off_sample=measure(provider,mode,source,draft,dictionary,off)?;
        (off_sample,measure(provider,mode,source,draft,dictionary,on)?)
    };
    let delta=(with_profile.preparation_total_ns as i128-without_profile.preparation_total_ns as i128).clamp(i64::MIN as i128,i64::MAX as i128) as i64;
    Ok(Pair{index,order:if on_first{"onThenOff"}else{"offThenOn"},without_profile,with_profile,signed_preparation_delta_ns:delta})
}
fn summarize(samples:&[Pair])->Value{
    let off=samples.iter().map(|p|ms(p.without_profile.preparation_total_ns)).collect::<Vec<_>>();
    let on=samples.iter().map(|p|ms(p.with_profile.preparation_total_ns)).collect::<Vec<_>>();
    let delta=samples.iter().map(|p|p.signed_preparation_delta_ns as f64/1_000_000.).collect::<Vec<_>>();
    let stages=[("freeze",0),("generationPrepare",1),("reviewPrepare",2),("projectionOnlyDiagnostic",3)].into_iter().map(|(name,index)|{
        let get=|d:Durations|match index{0=>d.freeze_ns,1=>d.generation_prepare_ns,2=>d.review_prepare_ns,_=>d.projection_only_ns};
        let a=samples.iter().map(|p|ms(get(p.without_profile))).collect::<Vec<_>>();
        let b=samples.iter().map(|p|ms(get(p.with_profile))).collect::<Vec<_>>();
        (name.to_owned(),json!({"withoutProfile":distribution(&a),"withProfile":distribution(&b)}))
    }).collect::<serde_json::Map<_,_>>();
    json!({"withoutProfile":distribution(&off),"withProfile":distribution(&on),"signedPairedDelta":distribution(&delta),
        "differenceOfArmP95MsDescriptiveOnly":percentile(&on,95)-percentile(&off,95),"stages":stages,
        "stageScopedPreparationDeltaGatePassed":percentile(&delta,95)<=ADDED_PREPARATION_LIMIT_MS})
}

fn main()->anyhow::Result<()> {
    let args=std::env::args().skip(1).collect::<Vec<_>>();
    if args==["--help"] {
        println!("ABSOLUTE_NEW_OUTPUT_JSON --plan|--run\nOffline public-API preparation benchmark; no network/credentials/user database. --run requires release. 5 lengths (400/800/3000/8000/80000 Unicode chars) x3 profile fixtures x200 counterbalanced pairs,20 warmup pairs each. Projection diagnostic is excluded from total. No post-response validation, UI or online latency claim.");return Ok(());
    }
    ensure!(args.len()==2 && ["--plan","--run"].contains(&args[1].as_str()),"use --help");
    let output=Path::new(&args[0]);ensure!(output.is_absolute(),"absolute output path required");
    ensure!(!output.exists(),"fresh output required");ensure!(output.parent().is_some_and(Path::is_dir),"output parent must exist");
    let run=args[1]=="--run";ensure!(!run || !cfg!(debug_assertions),"measurement requires a release build");
    let profiles=profile_cases()?;let mode=mode();let dictionary=DictionarySnapshot::default();
    let mut provider=ProviderConfig::default();provider.rewrite_provider="aliyun".into();provider.rewrite_model="qwen3.8-flash".into();provider.region=Some("beijing".into());
    let mut plan=Vec::new();
    for chars in LENGTHS {let source=source(chars);let draft=passthrough(&source);
        for profile in &profiles {let mut off=profile.config.clone();off.enabled=false;
            plan.push(json!({"sourceUnicodeChars":chars,"sourceUtf8Bytes":source.text().len(),"sourceSha256":hash(&source)?,"draftSha256":hash(&draft)?,
                "profileFixture":profile.label,"withoutProfile":validate_case(&provider,&mode,&source,&draft,&off)?,
                "withProfile":validate_case(&provider,&mode,&source,&draft,&profile.config)?}));
        }
    }
    let mut result=json!({"tool":"personalization-local-cpu-lab-v1","status":if run{"RUNNING"}else{"PLANNED"},
        "scope":"SYNTHETIC_PUBLIC_API_PREPARATION_CPU_ONLY","sourceLengthsUnicodeChars":LENGTHS,"profileFixtures":profiles.iter().map(|p|p.label).collect::<Vec<_>>(),
        "warmupPairsPerCase":WARMUP_PAIRS,"measuredPairsPerCase":SAMPLE_PAIRS,"cases":plan.len(),"plannedMeasuredPairs":plan.len()*SAMPLE_PAIRS,
        "pairedOrder":"strictly alternated within each case, parity shifted by case index","rawSignedDeltasPreserved":true,
        "clock":"Instant elapsed wall clock for CPU-bound preparation; includes scheduling, not process CPU cycles",
        "preparationTotalDefinition":"sum(freeze,generationPrepare,reviewPrepare); separate Instant windows; excludes projectionOnlyDiagnostic and fixture/response construction",
        "projectionBoundary":"provider_data plus JSON serialization measured separately; excluded from total to avoid double counting projection already inside production prepares",
        "bindingBoundary":"production prepare APIs include internal source/mode/draft/profile/message binding hashes; private post-response binding validation is NOT_MEASURED",
        "draftBoundary":"fixed passthrough of synthetic source; no actual model reply, semantic acceptance or quality claim",
        "profileBudgetSerializedUnicodeChars":PERSONALIZATION_PROVIDER_MAX_CHARS,"addedPreparationP95DeltaLimitMs":ADDED_PREPARATION_LIMIT_MS,
        "gateBoundary":"empirical per-case p95(on-off) for these three CPU preparation windows only; not difference of arm p95s and not the complete PRD local-processing gate",
        "normalProductionLatencyClaim":false,"onlineLatency":"NOT_RUN","userQuality":"NOT_RUN","UI":"NOT_RUN","audio":"NOT_RUN","deliveryAuthorized":false,
        "network":"NOT_RUN_NO_CLIENT_CONSTRUCTED","credentials":"NOT_READ","userDatabase":"NOT_OPENED","postResponseValidation":"NOT_MEASURED",
        "automaticRetries":0,"requestCount":0,"hardwareLabel":null,"os":std::env::consts::OS,"architecture":std::env::consts::ARCH,
        "debugAssertions":cfg!(debug_assertions),"packageVersion":env!("CARGO_PKG_VERSION"),
        "executableSha256":sha(&fs::read(std::env::current_exe()?)?),"runnerSourceSha256":sha(include_bytes!("personalization_local_cpu_lab.rs")),
        "generationPreparationSourceSha256":sha(include_bytes!("../src/cloud/rewrite_minimum.rs")),"reviewPreparationSourceSha256":sha(include_bytes!("../src/cloud/rewrite_semantic_review.rs")),
        "personalizationSourceSha256":sha(include_bytes!("../src/core/personalization.rs")),"writingContextSourceSha256":sha(include_bytes!("../src/cloud/rewrite_writing_context.rs")),
        "sourcePatternSha256":sha(SOURCE_PATTERN.as_bytes()),"providerConfigSha256":hash(&provider)?,"modeSha256":hash(&mode)?,
        "hashBasis":"SHA256 exact typed serde_json UTF8 serialization; source/module/executable hashes use raw bytes","plan":plan,"rows":[]});
    let mut out=OpenOptions::new().write(true).create_new(true).open(output)?;
    if run {
        let mut rows=Vec::new();let mut case_index=0;
        for chars in LENGTHS {let source=source(chars);let draft=passthrough(&source);
            for profile in &profiles {let mut off=profile.config.clone();off.enabled=false;
                for index in 0..WARMUP_PAIRS {black_box(pair(&provider,&mode,&source,&draft,&dictionary,&off,&profile.config,index,(index+case_index)%2==0)?);}
                let mut samples=Vec::with_capacity(SAMPLE_PAIRS);
                for index in 0..SAMPLE_PAIRS {samples.push(pair(&provider,&mode,&source,&draft,&dictionary,&off,&profile.config,index,(index+case_index)%2==0)?);}
                let summary=summarize(&samples);
                println!("{} chars {}: paired p95 delta {:.6}ms",chars,profile.label,summary["signedPairedDelta"]["p95Ms"].as_f64().unwrap());
                rows.push(json!({"sourceUnicodeChars":chars,"profileFixture":profile.label,"warmupPairs":WARMUP_PAIRS,"measuredPairs":samples.len(),"summary":summary,"samples":samples}));case_index+=1;
            }
        }
        result["status"]=json!("COMPLETED");result["stageScopedPreparationDeltaGatePassed"]=json!(rows.iter().all(|row|row["summary"]["stageScopedPreparationDeltaGatePassed"]==true));
        result["rows"]=json!(rows);
    }
    serde_json::to_writer_pretty(&mut out,&result)?;out.write_all(b"\n")?;out.flush()?;out.sync_all()?;
    println!("{}: {} cases; no network or model calls",if run{"COMPLETED"}else{"PLANNED"},profiles.len()*LENGTHS.len());Ok(())
}
