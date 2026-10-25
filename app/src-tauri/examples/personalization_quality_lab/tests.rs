use super::*;
use speakmode_lib::core::{ExpressionStyle, ExpressionTone, PersonalProfile, SentenceStyle};

fn case() -> Case {
    Case {
        id: "synthetic-case".into(), scene: AppWritingScene::Technical,
        language: DraftLanguage::AsSpoken, source: "如果检查通过，周五再讨论 beta；现在还没有完成。".into(),
        profile: PersonalizationConfig {
            enabled: true, active_profile_id: Some("local-id-sentinel".into()),
            profiles: vec![PersonalProfile {
                id: "local-id-sentinel".into(), name: "local-name-sentinel".into(),
                profession: "软件开发".into(), domain: "数据处理".into(), description: "研发背景".into(),
                tasks: vec!["接口沟通".into()], audience: "协作同事".into(),
                scenes: vec![AppWritingScene::Technical], instructions: "保留条件。".into(),
            }],
            preferences: ExpressionStyle { paragraphs: true, fewer_courtesies: true, ..Default::default() },
            ..Default::default()
        },
        criteria: Criteria { facts: vec!["if the check passes".into()], preferences: vec!["paragraphs".into()],
            risks: vec!["do not mark the work completed".into()] }, category: "synthetic".into(),
    }
}
fn profile(context: &WritingContext) -> &EffectivePersonalization { context.personalization.as_ref().unwrap() }
fn assert_isolation(case: &Case) {
    let [a, b, c, d] = contexts(case).unwrap();
    assert!(a.personalization.is_none());
    for value in [&a, &b, &c, &d] {
        assert_eq!(value.scene, case.scene); assert_eq!(value.language, case.language); assert!(value.input.is_none());
        assert!(value.validate().is_ok());
    }
    let c = profile(&c); let b = b.personalization.as_ref(); let d = d.personalization.as_ref();
    assert_eq!(Some(c.clone()), case.profile.freeze(case.scene));
    if let Some(b) = b { assert!(b.rules.is_empty()); assert_eq!(b.omitted,c.omitted); assert_eq!(b.profile_id,c.profile_id); }
    if let Some(d) = d { assert_eq!(d.rules,c.rules); assert_eq!(d.omitted,c.omitted); assert_eq!(d.profile_id,c.profile_id); }
    else { assert!(c.rules.is_empty()); }
    let b_background = b.and_then(|value|value.background.as_ref());
    let d_background = d.and_then(|value|value.background.as_ref());
    if let Some(original) = &c.background {
        if let Some(retained) = b_background {
            assert_eq!(retained.profession, original.profession); assert_eq!(retained.domain, original.domain);
            assert_eq!(retained.description, original.description); assert!(retained.tasks.is_empty()); assert!(retained.audience.is_empty());
        } else { assert!(original.profession.is_empty() && original.domain.is_empty() && original.description.is_empty()); }
        if let Some(retained) = d_background {
            assert!(retained.profession.is_empty()); assert!(retained.domain.is_empty()); assert!(retained.description.is_empty());
            assert_eq!(retained.tasks, original.tasks); assert_eq!(retained.audience, original.audience);
        } else { assert!(original.tasks.is_empty() && original.audience.is_empty()); }
    } else { assert!(b_background.is_none()); assert!(d_background.is_none()); }
}
#[test]
fn arms_only_remove_their_assigned_fields_from_one_frozen_c() { assert_isolation(&case()); }

#[test]
fn near_provider_budget_ablations_never_refill_omitted_fields() {
    let mut case = case();
    case.profile.preferences = ExpressionStyle { sentence_style: SentenceStyle::Concise,
        tone: ExpressionTone::Formal, keep_english_terms: true, fewer_courtesies: true, paragraphs: true, lists: true };
    let p = &mut case.profile.profiles[0];
    p.profession = "工".repeat(80); p.domain = "域".repeat(120); p.description = "背".repeat(300);
    p.audience = "众".repeat(120); p.instructions = "保".repeat(500); p.tasks = (0..8).map(|_| "任".repeat(80)).collect();
    case.profile.validate().unwrap();
    let frozen = case.profile.freeze(case.scene).unwrap();
    let count = frozen.provider_data().to_string().chars().count();
    assert!((1000..=1200).contains(&count), "exercise a nearly full projection, got {count}");
    assert!(!frozen.omitted.is_empty());
    assert_isolation(&case);
    // Refreezing B from the reduced config could import the formerly omitted description/tasks.
    let [_, b, _, d] = contexts(&case).unwrap();
    if frozen.background.as_ref().is_none_or(|b| b.description.is_empty()) {
        assert!(b.personalization.as_ref().and_then(|p|p.background.as_ref()).is_none_or(|b| b.description.is_empty()));
    }
    if frozen.background.as_ref().is_none_or(|b| b.tasks.is_empty()) {
        assert!(d.personalization.as_ref().and_then(|p|p.background.as_ref()).is_none_or(|b| b.tasks.is_empty()));
    }
}

#[test]
fn empty_ablated_backgrounds_are_none_and_remain_valid() {
    let mut occupation_only = case();
    let p = &mut occupation_only.profile.profiles[0]; p.tasks.clear(); p.audience.clear();
    let values = contexts(&occupation_only).unwrap(); assert!(profile(&values[3]).background.is_none());
    let mut preference_only = case();
    let p = &mut preference_only.profile.profiles[0]; p.profession.clear(); p.domain.clear(); p.description.clear();
    let values = contexts(&preference_only).unwrap(); assert!(values[1].personalization.is_none());
    assert_isolation(&occupation_only); assert_isolation(&preference_only);
    occupation_only.profile.preferences = Default::default(); occupation_only.profile.profiles[0].instructions.clear();
    let values = contexts(&occupation_only).unwrap();
    assert!(values[3].personalization.is_none()); assert!(values[2].personalization.is_some());
    assert_isolation(&occupation_only);
}

#[test]
fn occupation_contrast_reports_only_explicit_occupation_fields() {
    let mut case = case(); let config = ProviderConfig {region:Some("beijing".into()),..Default::default()};
    let arms = prepare_arms(&case,&config,&source(&case),&mode(case.scene),&DictionarySnapshot::default()).unwrap();
    assert_eq!(arms.iter().map(|arm|arm.manifest["explicitOccupationalBackgroundPresent"].clone()).collect::<Vec<_>>(),
        vec![json!(false),json!(true),json!(true),json!(false)]);
    assert!(arms.iter().all(|arm|arm.manifest["occupationContrastApplicable"] == true));
    let p = &mut case.profile.profiles[0]; p.profession.clear(); p.domain.clear(); p.description.clear();
    p.audience = "接口架构工程师".into(); // A domain clue deliberately retained in C and D.
    let arms = prepare_arms(&case,&config,&source(&case),&mode(case.scene),&DictionarySnapshot::default()).unwrap();
    assert!(arms[1].context.personalization.is_none());
    assert!(arms.iter().all(|arm|arm.manifest["explicitOccupationalBackgroundPresent"] == false
        && arm.manifest["occupationContrastApplicable"] == false));
    assert_eq!(profile(&arms[2].context).background.as_ref().unwrap().audience,"接口架构工程师");
    assert_eq!(profile(&arms[3].context).background.as_ref().unwrap().audience,"接口架构工程师");
}

#[test]
fn preflight_has_no_local_identity_provenance_or_evaluator_data_in_prompt() {
    let case = case(); let source = source(&case); let mode = mode(case.scene);
    let config = ProviderConfig { region: Some("beijing".into()), ..Default::default() };
    let dictionary = DictionarySnapshot::default();
    let arms = prepare_arms(&case, &config, &source, &mode, &dictionary).unwrap();
    assert_eq!(arms.iter().map(|a| a.name).collect::<Vec<_>>(), ["A", "B", "C", "D"]);
    for arm in arms {
        let request = rewrite::minimum::prepare_with_writing_context(&config,&mode,&source,&dictionary,true,Some(&arm.context)).unwrap();
        let body = request.body.to_string();
        for sentinel in ["local-id-sentinel", "local-name-sentinel", "do not mark the work completed"] {
            assert!(!body.contains(sentinel), "unexpected prompt metadata {sentinel}");
        }
        let data: Value = serde_json::from_str(request.body["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert!(data.get("criteria").is_none());
        if let Some(personalization) = data.get("personalization") {
            let object = personalization.as_object().unwrap();
            assert!(object.keys().all(|key| ["scene", "rules", "background"].contains(&key.as_str())));
            for rule in object["rules"].as_array().unwrap() { assert!(rule.get("origin").is_none()); }
        }
        assert_eq!(source.text(),case.source); assert!(dictionary.entries.is_empty());
    }
}

#[test]
fn request_reservation_and_fatal_decisions_are_bounded() {
    assert!(budget_allows(0,4)); assert!(budget_allows(12,4));
    assert!(!budget_allows(13,4)); assert!(!budget_allows(usize::MAX,4)); assert!(!budget_allows(0,3));
    for code in ["authentication_failed","rate_limited","rewrite_timeout","lab_ledger_failed","key_required"] {
        assert!(fatal(&CloudError::new(code,"rewrite","synthetic")));
    }
    for code in ["rewrite_semantic_rejected","invalid_output","rewrite_admission_rejected"] {
        assert!(!fatal(&CloudError::new(code,"rewrite","synthetic")));
    }
}
fn event(kind: &'static str) -> PersonalizationLabEvent {
    PersonalizationLabEvent { stage:"generation", event:kind, duration_ms:None, remaining_budget_ms:20_000, outcome:None }
}
#[test]
fn durable_start_events_enforce_four_per_arm_and_sixteen_total() {
    let path = std::env::temp_dir().join(format!("personalization-ledger-{}.jsonl",uuid::Uuid::new_v4()));
    {
        let file = OpenOptions::new().write(true).create_new(true).open(&path).unwrap();
        let mut ledger = Ledger {file,started:0,arm_started:0,failure:None};
        for arm in ["A","B","C","D"] {
            assert!(ledger.reserve_arm("synthetic-case",arm));
            for _ in 0..4 { assert!(ledger.observe("synthetic-case",arm,event("started"))); }
        }
        assert_eq!(ledger.started,16); assert!(!ledger.observe("synthetic-case","D",event("started")));
        assert!(ledger.failure.is_some()); assert_eq!(ledger.started,16);
    }
    let rows = fs::read_to_string(&path).unwrap();
    assert_eq!(rows.lines().filter(|line| line.contains("request_event")).count(),16);
    fs::remove_file(path).unwrap();
}
#[test]
fn unwritable_ledger_cancels_before_any_transport_start_is_authorized() {
    let path = std::env::temp_dir().join(format!("personalization-readonly-{}",uuid::Uuid::new_v4()));
    fs::write(&path,b"seed").unwrap();
    {
        let mut ledger = Ledger { file:File::open(&path).unwrap(), started:0, arm_started:0, failure:None };
        assert!(!ledger.observe("synthetic-case","A",event("started")));
        assert_eq!(ledger.started,0); assert!(ledger.failure.is_some());
        assert!(!ledger.observe("synthetic-case","A",event("started"))); assert_eq!(ledger.started,0);
    }
    assert_eq!(fs::read(&path).unwrap(),b"seed"); fs::remove_file(path).unwrap();
}

#[test]
fn scene_mode_ids_match_production_selection() {
    for (scene,id) in [(AppWritingScene::General,"auto"),(AppWritingScene::Chat,"chat"),
        (AppWritingScene::Document,"document"),(AppWritingScene::Email,"email"),(AppWritingScene::Technical,"technical")] {
        let mode = mode(scene); assert_eq!(mode.mode_id,id); assert_eq!(mode.base_mode,BaseMode::Auto);
    }
}

#[test]
fn guarded_long_source_plan_hashes_ignore_only_generated_nonce() {
    let mut case = case();
    case.source = format!("{} beta 仍是未完成的原始名词。", "这是保持原有条件与事实的合成口述片段。".repeat(30));
    let source = source(&case); let mode = mode(case.scene);
    let config = ProviderConfig {region:Some("beijing".into()),..Default::default()};
    let dictionary = DictionarySnapshot::default(); let context = &contexts(&case).unwrap()[2];
    let a = rewrite::minimum::prepare_with_writing_context(&config,&mode,&source,&dictionary,true,Some(context)).unwrap();
    let b = rewrite::minimum::prepare_with_writing_context(&config,&mode,&source,&dictionary,true,Some(context)).unwrap();
    assert_eq!(stable_generation_body(&a.body,&source).unwrap(),stable_generation_body(&b.body,&source).unwrap());
    let original = json!({"messages":[{}, {"content":json!({"sourceText":"⟪SMabcd_0⟫"}).to_string()}]});
    let mut authored_source = source.clone(); authored_source.items[0].text = "⟪SMabcd_0⟫".into();
    assert_eq!(stable_generation_body(&original,&authored_source).unwrap(),original);
    let mut changed = b.body.clone(); changed["messages"][0]["content"] = json!("changed policy");
    assert_ne!(stable_generation_body(&a.body,&source).unwrap(),stable_generation_body(&changed,&source).unwrap());
}
