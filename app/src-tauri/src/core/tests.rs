use super::*;
use std::{fs, path::PathBuf};
use uuid::Uuid;

fn seed(session:&str,policy:StoragePolicy) -> SessionSeed {
    SessionSeed {id:session.into(),generation:7,started_at:"2026-10-04T10:00:00+08:00".into(),ended_at:"2026-10-04T10:00:12+08:00".into(),capture_duration_ms:12_000,end_local_date:"2026-10-04".into(),time_zone:"Asia/Shanghai".into(),storage_policy:policy,mode_snapshot:ModeSnapshot{mode_id:"auto".into(),name:"自动".into(),base_mode:BaseMode::Auto,rules:"".into(),revision:1}}
}
fn source(session:&str) -> SourceRevision {
    SourceRevision{id:format!("source-{session}"),session_id:session.into(),asr_attempt_id:format!("attempt-{session}"),generation:7,provider:"aliyun".into(),model:"candidate-model".into(),adapter_version:"test-protocol-v1".into(),complete:true,items:vec![SourceItem{id:format!("item-{session}-1"),provider_item_id:"provider-1".into(),capture_order:1,text:"如果会议未确认，不要订机票。Hello 12 🙂".into(),is_final:true,start_ms:Some(0),end_ms:Some(7000)},SourceItem{id:format!("item-{session}-2"),provider_item_id:"provider-2".into(),capture_order:2,text:"先保留行程。".into(),is_final:true,start_ms:Some(7000),end_ms:Some(12000)}],created_at:"2026-10-04T10:00:12+08:00".into()}
}
fn output(session:&str) -> OutputRevision {
    OutputRevision{id:format!("output-{session}"),session_id:session.into(),parent_revision_id:None,base_revision_id:None,source_revision_id:format!("source-{session}"),mode_snapshot:seed(session,StoragePolicy::History).mode_snapshot,resolved_mode:BaseMode::Light,blocks:vec![OutputBlock{id:"block-1".into(),text:"会议未确认前，不要订机票。Hello 12 🙂".into(),source_refs:vec![format!("item-{session}-1")]},OutputBlock{id:"block-2".into(),text:"先保留行程。".into(),source_refs:vec![format!("item-{session}-2")]}],kind:RevisionKind::Initial,creation_reason:CreationReason::NormalInitial,created_at:"2026-10-04T10:00:13+08:00".into()}
}
fn ready(store:&mut Store,session:&str,policy:StoragePolicy) -> HistoryDetail {
    store.checkpoint_source(seed(session,policy),source(session)).unwrap();
    store.commit_ready(output(session)).unwrap()
}
fn path() -> PathBuf {
    let source_file=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(file!());
    let source_file=fs::canonicalize(source_file).unwrap();
    let base=source_file.parent().unwrap().join("harness/test-data");fs::create_dir_all(&base).unwrap();base.join(format!("{}.sqlite",Uuid::new_v4()))
}
fn draft(name:&str) -> ModeDraft {ModeDraft{id:None,name:name.into(),base_mode:BaseMode::Light,rules:"保留条件，用简洁段落。".into(),expected_revision:None}}

fn term_patch()->CorrectionPatch {CorrectionPatch{block_id:"block-1".into(),start:0,end:2,expected_text:"会议".into(),replacement:"SpeakMode".into()}}

#[test]
fn correction_memory_only_persists_only_explicit_word_and_rolls_back_memory_on_sql_failure() {
    let file=path();
    {
        let mut store=Store::open(&file).unwrap();let before=ready(&mut store,"private-correction",StoragePolicy::MemoryOnly);
        store.test_connection().execute_batch("CREATE TRIGGER reject_word BEFORE INSERT ON dictionary_entries BEGIN SELECT RAISE(ABORT,'synthetic'); END;").unwrap();
        assert!(store.correct_term("private-correction","output-private-correction",term_patch(),Some(1),"now").is_err());
        assert_eq!(store.get_result("private-correction").unwrap(),before);assert_eq!(store.dictionary().unwrap().version,1);
        store.test_connection().execute_batch("DROP TRIGGER reject_word;").unwrap();
        let result=store.correct_term("private-correction","output-private-correction",term_patch(),Some(1),"now").unwrap();
        assert_eq!(result.dictionary_status,CorrectionDictionaryStatus::Added);assert_eq!(result.detail.source,before.source);assert!(result.detail.can_undo);
        assert_eq!(store.history_list("",0,20).unwrap().total,0);
        for table in ["sessions","sources","source_items","outputs"] {assert_eq!(store.test_connection().query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|row|row.get::<_,u64>(0)).unwrap(),0);}
        let undone=store.undo_output("private-correction",&result.detail.output.unwrap().id,"undo").unwrap();assert_eq!(undone.output.unwrap().blocks,before.output.unwrap().blocks);
    }
    let store=Store::open(&file).unwrap();assert_eq!(store.dictionary().unwrap().entries[0].term,"SpeakMode");assert!(matches!(store.get_result("private-correction"),Err(StoreError::NotFound)));
    drop(store);fs::remove_file(file).unwrap();
}

#[test]
fn correction_cancelled_or_deleted_commit_guard_cannot_publish_either_change() {
    for policy in [StoragePolicy::History,StoragePolicy::MemoryOnly] {
        for deleted in [false,true] {
            let mut store=Store::in_memory().unwrap();let before=ready(&mut store,"cancel-correction",policy);
            let parent=std::sync::Arc::new(CommitGuard::new("cancel-correction".into(),7));parent.observe_ready().unwrap();let guard=parent.operation();
            if deleted{parent.delete();}else{assert_eq!(guard.cancel(),CancelDecision::Accepted);}
            let result=store.with_commit_guard(guard,|store|store.correct_term("cancel-correction","output-cancel-correction",term_patch(),Some(1),"now"));
            assert!(matches!(result,Err(StoreError::Cancelled)));assert_eq!(store.get_result("cancel-correction").unwrap(),before);assert_eq!(store.dictionary().unwrap(),DictionarySnapshot::default());
        }
    }
}

#[test]
fn correction_context_limit_and_unicode_single_occurrence_are_enforced() {
    let mut store=Store::in_memory().unwrap();let before=ready(&mut store,"bounded-correction",StoragePolicy::History);
    let terms=(0..16).map(|index|format!("{index:02}{}","词".repeat(78))).collect();
    let dictionary=store.import_dictionary(terms,1).unwrap();
    let mut patch=term_patch();patch.replacement="长".repeat(80);
    assert!(matches!(store.correct_term("bounded-correction","output-bounded-correction",patch,Some(dictionary.version),"now"),Err(StoreError::Invalid(reason)) if reason=="dictionary_context_limit"));
    assert_eq!(store.get_result("bounded-correction").unwrap(),before);assert_eq!(store.dictionary().unwrap(),dictionary);
    let mut blocks=before.output.as_ref().unwrap().blocks.clone();blocks[0].text="🙂飞格玛和飞格玛".into();
    let edited=store.edit_output("bounded-correction","output-bounded-correction",blocks,"edit").unwrap();
    let result=store.correct_term("bounded-correction",&edited.output.unwrap().id,CorrectionPatch{block_id:"block-1".into(),start:1,end:4,expected_text:"飞格玛".into(),replacement:"Figma".into()},None,"correct").unwrap();
    let blocks=result.detail.output.unwrap().blocks;assert_eq!(blocks[0].text,"🙂Figma和飞格玛");assert_eq!(blocks[1],before.output.unwrap().blocks[1]);
}

#[test]
fn dictionary_v2_migration_preserves_v1_preferences_modes_history_sources_and_usage() {
    let file=path();
    let expected_detail;let expected_preferences;let expected_mode;
    {
        let mut store=Store::open(&file).unwrap();
        expected_mode=store.save_mode(draft("迁移保留的 Mode")).unwrap();
        let mut preferences=store.preferences().unwrap();preferences.language="zh".into();preferences.default_mode_id=expected_mode.id.clone();
        expected_preferences=store.save_preferences(preferences,1).unwrap();
        expected_detail=ready(&mut store,"v1-preserved",StoragePolicy::History);
        // This is precisely the pre-dictionary v1 schema and existing bodies.
        store.test_connection().execute_batch("DROP TABLE dictionary_entries;DROP TABLE dictionary_meta;PRAGMA user_version=1;").unwrap();
    }
    let store=Store::open(&file).unwrap();
    assert_eq!(store.preferences().unwrap(),expected_preferences);
    assert!(store.list_modes().unwrap().contains(&expected_mode));
    assert_eq!(store.get_result("v1-preserved").unwrap(),expected_detail);
    assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);
    assert_eq!(store.dictionary().unwrap(),DictionarySnapshot::default());
    assert_eq!(store.test_connection().pragma_query_value(None,"user_version",|row|row.get::<_,u32>(0)).unwrap(),2);
    assert!(store.test_connection().execute("UPDATE sources SET data='changed'",[]).is_err());
    drop(store);fs::remove_file(file).unwrap();
}

#[test]
fn dictionary_survives_reopen_edit_and_delete_keep_stable_identity() {
    let file=path();let first;
    {
        let mut store=Store::open(&file).unwrap();
        let saved=store.import_dictionary(vec!["SpeakMode".into(),"PolyU".into()],1).unwrap();
        first=saved.entries.iter().find(|entry|entry.term=="SpeakMode").unwrap().clone();
    }
    let mut store=Store::open(&file).unwrap();let saved=store.dictionary().unwrap();assert_eq!(saved.version,2);
    let edited=store.save_dictionary_entry(DictionaryDraft{id:Some(first.id.clone()),term:"Speak Mode".into()},saved.version).unwrap();
    let edited_entry=edited.entries.iter().find(|entry|entry.id==first.id).unwrap();
    assert_eq!(edited_entry.created_at,first.created_at);assert_eq!(edited_entry.term,"Speak Mode");assert_eq!(edited.version,3);
    let deleted=store.delete_dictionary_entry(&first.id,edited.version).unwrap();assert_eq!(deleted.version,4);
    assert_eq!(deleted.entries.iter().map(|entry|entry.term.as_str()).collect::<Vec<_>>(),["PolyU"]);
    drop(store);assert_eq!(Store::open(&file).unwrap().dictionary().unwrap(),deleted);fs::remove_file(file).unwrap();
}

#[test]
fn dictionary_nfkc_casefold_deduplicates_without_replacing_the_first_spelling() {
    let mut store=Store::in_memory().unwrap();
    let terms=["Ｆｏｏ","foo","Straße","STRASSE","é","e\u{301}"];
    let saved=store.import_dictionary(terms.iter().map(|term|term.to_string()).collect(),1).unwrap();
    assert_eq!(saved.entries.len(),3);assert!(saved.entries.iter().any(|entry|entry.term=="Ｆｏｏ"));assert!(saved.entries.iter().any(|entry|entry.term=="Straße"));
    let duplicate=store.import_dictionary(vec!["FOO".into(),"strasse".into()],saved.version).unwrap();assert_eq!(duplicate,saved);
    let entry=saved.entries.iter().find(|entry|entry.term=="Straße").unwrap();
    assert_eq!(store.save_dictionary_entry(DictionaryDraft{id:Some(entry.id.clone()),term:entry.term.clone()},saved.version).unwrap(),saved);
    assert!(matches!(store.save_dictionary_entry(DictionaryDraft{id:None,term:"strasse".into()},saved.version),Err(StoreError::Invalid(reason)) if reason=="dictionary_duplicate"));
}

#[test]
fn dictionary_invalid_batch_and_capacity_fail_without_partial_commit() {
    let mut store=Store::in_memory().unwrap();let original=store.dictionary().unwrap();
    for bad in ["", " \t ", "bad\nword", "hidden\u{200b}word", "separator\u{2028}word"] {
        assert!(store.import_dictionary(vec!["ValidFirst".into(),bad.into()],original.version).is_err());assert_eq!(store.dictionary().unwrap(),original);
    }
    assert!(store.import_dictionary(vec!["x".repeat(81)],original.version).is_err());
    let multibyte=(0..20).map(|index|format!("{}-{index}","术".repeat(70))).collect::<Vec<_>>();
    assert!(matches!(store.import_dictionary(multibyte,original.version),Err(StoreError::Invalid(reason)) if reason=="dictionary_context_limit"));
    assert_eq!(store.dictionary().unwrap(),original);
    let full=store.import_dictionary((0..200).map(|index|format!("term{index:03}")).collect(),original.version).unwrap();
    assert!(full.asr_context().unwrap().len()<=4000);
    assert!(matches!(store.save_dictionary_entry(DictionaryDraft{id:None,term:"one-more".into()},full.version),Err(StoreError::Invalid(reason)) if reason=="dictionary_entry_limit"));
    assert_eq!(store.dictionary().unwrap(),full);
}

#[test]
fn dictionary_edit_collision_and_stale_writes_never_overwrite_newer_words() {
    let mut store=Store::in_memory().unwrap();let saved=store.import_dictionary(vec!["Alpha".into(),"Beta".into()],1).unwrap();
    let alpha=saved.entries.iter().find(|entry|entry.term=="Alpha").unwrap();
    assert!(matches!(store.save_dictionary_entry(DictionaryDraft{id:Some(alpha.id.clone()),term:"ＢＥＴＡ".into()},saved.version),Err(StoreError::Invalid(reason)) if reason=="dictionary_duplicate"));
    assert!(matches!(store.save_dictionary_entry(DictionaryDraft{id:Some(alpha.id.clone()),term:"Changed".into()},1),Err(StoreError::Conflict{..})));
    assert!(matches!(store.import_dictionary(vec!["Gamma".into()],1),Err(StoreError::Conflict{..})));
    assert!(matches!(store.delete_dictionary_entry(&alpha.id,1),Err(StoreError::Conflict{..})));
    assert_eq!(store.dictionary().unwrap(),saved);
}

#[test]
fn cancelled_dictionary_transaction_rolls_back_version_and_entire_batch() {
    let mut store=Store::in_memory().unwrap();let saved=store.import_dictionary(vec!["Existing".into()],1).unwrap();
    let guard=std::sync::Arc::new(CommitGuard::new("dictionary-cancel".into(),0)).operation();assert_eq!(guard.cancel(),CancelDecision::Accepted);
    assert!(matches!(store.with_commit_guard(guard,|store|store.import_dictionary(vec!["NeverCommitted".into()],saved.version)),Err(StoreError::Cancelled)));
    assert_eq!(store.dictionary().unwrap(),saved);
    assert!(store.test_connection().is_autocommit());
}

#[test]
fn migrations_and_preferences_survive_reopen_without_mode_or_key_fields() {
    let path=path();
    {let mut store=Store::open(&path).unwrap();let mut prefs=store.preferences().unwrap();prefs.keep_history=false;prefs.language="zh".into();let saved=store.save_preferences(prefs,1).unwrap();assert_eq!(saved.version,2);assert_eq!(store.list_modes().unwrap().len(),3);}
    let store=Store::open(&path).unwrap();assert_eq!(store.preferences().unwrap().version,2);assert!(!store.preferences().unwrap().keep_history);assert_eq!(store.test_connection().pragma_query_value(None,"user_version",|r|r.get::<_,u32>(0)).unwrap(),2);
    drop(store);fs::remove_file(path).unwrap();
}
#[test]
fn preference_cas_does_not_overwrite_a_newer_setting() {
    let mut store=Store::in_memory().unwrap();let original=store.preferences().unwrap();let mut first=original.clone();first.keep_history=false;store.save_preferences(first,1).unwrap();
    assert!(matches!(store.save_preferences(original,1),Err(StoreError::Conflict{..})));assert!(!store.preferences().unwrap().keep_history);
}
#[test]
fn provider_configuration_rejects_url_secret_fields_and_invalid_regions() {
    assert!(serde_json::from_str::<ProviderConfig>(r#"{"version":1,"provider":"aliyun","region":"beijing","workspace":null,"asrModel":"m","rewriteModel":"r","adapterVersion":"v","apiKey":"never-store"}"#).is_err());
    let mut store=Store::in_memory().unwrap();let mut config=store.provider_config().unwrap();config.region=Some("arbitrary-domain".into());assert!(store.save_provider_config(config,1).is_err());assert_eq!(store.provider_config().unwrap().version,1);
}
#[test]
fn unicode_casefold_and_compatibility_normalization_reject_duplicate_mode_names() {
    let mut store=Store::in_memory().unwrap();store.save_mode(draft("Straße")).unwrap();assert!(store.save_mode(draft("STRASSE")).is_err());
    store.save_mode(draft("ＡＢＣ")).unwrap();assert!(store.save_mode(draft("abc")).is_err());assert!(store.save_mode(draft("自动")).is_err());
}
#[test]
fn mode_revisions_are_frozen_and_deleting_default_resets_only_the_default() {
    let mut store=Store::in_memory().unwrap();let original=store.save_mode(draft("简报")).unwrap();store.set_default_mode(&original.id,1).unwrap();
    let edited=store.save_mode(ModeDraft{id:Some(original.id.clone()),name:"简报新版".into(),base_mode:BaseMode::Structure,rules:"分点保留条件。".into(),expected_revision:Some(1)}).unwrap();
    assert_eq!(store.mode_snapshot(&original.id,Some(1)).unwrap().rules,original.rules);assert_eq!(edited.revision,2);
    store.delete_mode(&original.id,2).unwrap();assert_eq!(store.preferences().unwrap().default_mode_id,"auto");assert!(store.mode_snapshot(&original.id,None).is_err());assert_eq!(store.mode_snapshot(&original.id,Some(1)).unwrap().name,"简报");
}
#[test]
fn mode_edit_requires_the_current_revision_and_builtin_modes_are_protected() {
    let mut store=Store::in_memory().unwrap();let original=store.save_mode(draft("消息")).unwrap();let stale=ModeDraft{id:Some(original.id),name:"消息".into(),base_mode:BaseMode::Light,rules:"保留全部信息".into(),expected_revision:Some(0)};
    assert!(matches!(store.save_mode(stale),Err(StoreError::Conflict{..})));assert!(store.delete_mode("light",1).is_err());
}
#[test]
fn checkpoint_has_no_usage_or_output_and_is_immutable() {
    let mut store=Store::in_memory().unwrap();let saved=store.checkpoint_source(seed("s1",StoragePolicy::History),source("s1")).unwrap();assert!(saved.output.is_none());assert_eq!(store.usage_today("2026-10-04").unwrap().count,0);
    assert_eq!(store.checkpoint_source(seed("s1",StoragePolicy::History),source("s1")).unwrap(),saved);
    let mut changed=source("s1");changed.items[0].text="听错后覆盖原稿".into();assert!(matches!(store.checkpoint_source(seed("s1",StoragePolicy::History),changed),Err(StoreError::ImmutableSource)));
    assert!(store.test_connection().execute("UPDATE sources SET data='{}'",[]).is_err());assert_eq!(store.get_source("source-s1").unwrap(),source("s1"));
}
#[test]
fn incomplete_partial_duplicate_and_out_of_order_sources_cannot_checkpoint() {
    for mutation in 0..4 {
        let mut store=Store::in_memory().unwrap();let mut src=source("s1");match mutation {0=>src.complete=false,1=>src.items[0].is_final=false,2=>src.items[1].provider_item_id=src.items[0].provider_item_id.clone(),_=>src.items.swap(0,1)};
        assert!(store.checkpoint_source(seed("s1",StoragePolicy::History),src).is_err());assert_eq!(store.history_list("",0,20).unwrap().total,0);
    }
}
#[test]
fn source_checkpoint_failure_rolls_back_session_and_all_items() {
    let mut store=Store::in_memory().unwrap();store.test_connection().execute_batch("CREATE TRIGGER fail_item BEFORE INSERT ON source_items BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();
    assert!(store.checkpoint_source(seed("s1",StoragePolicy::History),source("s1")).is_err());assert_eq!(store.history_list("",0,20).unwrap().total,0);assert!(store.get_source("source-s1").is_err());
    store.test_connection().execute_batch("DROP TRIGGER fail_item;").unwrap();assert!(store.checkpoint_source(seed("s1",StoragePolicy::History),source("s1")).is_ok());
}
#[test]
fn ready_transaction_failure_preserves_source_but_rolls_back_output_and_usage() {
    let mut store=Store::in_memory().unwrap();store.checkpoint_source(seed("s1",StoragePolicy::History),source("s1")).unwrap();
    store.test_connection().execute_batch("CREATE TRIGGER fail_usage BEFORE INSERT ON usage_events BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();assert!(store.commit_ready(output("s1")).is_err());
    assert!(store.get_output("output-s1").is_err());assert!(store.get_result("s1").unwrap().output.is_none());assert_eq!(store.get_source("source-s1").unwrap(),source("s1"));assert_eq!(store.usage_today("2026-10-04").unwrap().count,0);
    store.test_connection().execute_batch("DROP TRIGGER fail_usage;").unwrap();store.commit_ready(output("s1")).unwrap();assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);
}
#[test]
fn ready_is_idempotent_and_statistics_use_source_not_output() {
    let mut store=Store::in_memory().unwrap();ready(&mut store,"s1",StoragePolicy::History);store.commit_ready(output("s1")).unwrap();
    let usage=store.usage_today("2026-10-04").unwrap();assert_eq!(usage.count,1);assert_eq!(usage.source_char_count,super::store::source_character_count(&source("s1").text()));assert_eq!(usage.capture_duration_ms,12_000);
    assert!(store.cancel_unfinished("s1").is_err());assert_eq!(store.usage_today("2026-10-04").unwrap(),usage);
}
#[test]
fn output_references_cannot_cross_source_revision_or_reference_partial_items() {
    let mut store=Store::in_memory().unwrap();store.checkpoint_source(seed("s1",StoragePolicy::History),source("s1")).unwrap();store.checkpoint_source(seed("s2",StoragePolicy::History),source("s2")).unwrap();
    let mut wrong=output("s1");wrong.blocks[0].source_refs=vec!["item-s2-1".into()];assert!(store.commit_ready(wrong).is_err());
    let mut src=source("s1");src.items[0].is_final=false;assert!(super::store::validate_output(&output("s1"),&src).is_err());assert_eq!(store.usage_today("2026-10-04").unwrap().count,0);
}
#[test]
fn cancel_removes_unadopted_source_and_tombstone_rejects_late_commit() {
    let mut store=Store::in_memory().unwrap();store.checkpoint_source(seed("s1",StoragePolicy::History),source("s1")).unwrap();store.cancel_unfinished("s1").unwrap();
    assert!(store.get_source("source-s1").is_err());assert!(matches!(store.checkpoint_source(seed("s1",StoragePolicy::History),source("s1")),Err(StoreError::Cancelled)));assert!(matches!(store.commit_ready(output("s1")),Err(StoreError::Cancelled)));assert_eq!(store.usage_today("2026-10-04").unwrap().count,0);
}
#[test]
fn history_off_keeps_source_and_versions_out_of_every_body_table() {
    let mut store=Store::in_memory().unwrap();ready(&mut store,"private",StoragePolicy::MemoryOnly);
    for table in ["sessions","sources","source_items","outputs"] {let count:u64=store.test_connection().query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|r|r.get(0)).unwrap();assert_eq!(count,0,"{table}");}
    assert_eq!(store.history_list("",0,20).unwrap().total,0);assert!(store.history_get("private").is_err());assert_eq!(store.latest_temporary_result().unwrap().unwrap().source,source("private"));assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);
}
#[test]
fn temporary_ready_is_retained_during_next_recording_and_replaced_only_on_completion() {
    let mut store=Store::in_memory().unwrap();ready(&mut store,"p1",StoragePolicy::MemoryOnly);store.checkpoint_source(seed("p2",StoragePolicy::MemoryOnly),source("p2")).unwrap();assert_eq!(store.latest_temporary_result().unwrap().unwrap().session.seed.id,"p1");
    store.commit_ready(output("p2")).unwrap();assert_eq!(store.latest_temporary_result().unwrap().unwrap().session.seed.id,"p2");assert!(store.get_result("p1").is_err());store.clear_temporary_results();assert!(store.latest_temporary_result().unwrap().is_none());assert_eq!(store.usage_today("2026-10-04").unwrap().count,2);
}
#[test]
fn manual_edit_cas_rejects_stale_draft_and_does_not_modify_source_or_usage() {
    let mut store=Store::in_memory().unwrap();let detail=ready(&mut store,"s1",StoragePolicy::History);let initial=detail.output.unwrap();let mut blocks=initial.blocks.clone();blocks[0].text="手动修正，不要订机票。".into();
    let edited=store.edit_output("s1",&initial.id,blocks.clone(),"now").unwrap();assert!(matches!(store.edit_output("s1",&initial.id,blocks,"later"),Err(StoreError::Conflict{..})));assert_eq!(store.get_source("source-s1").unwrap(),source("s1"));assert!(edited.can_undo);assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);
}
#[test]
fn candidate_adoption_requires_frozen_base_and_does_not_auto_deliver() {
    let mut store=Store::in_memory().unwrap();let initial=ready(&mut store,"s1",StoragePolicy::History).output.unwrap();let mut candidate=initial.clone();candidate.id="candidate".into();candidate.base_revision_id=Some(initial.id.clone());candidate.blocks[0].text="候选整理，保留否定。".into();
    let adopted=store.adopt_candidate("s1",&initial.id,candidate.clone()).unwrap().output.unwrap();assert_eq!(adopted.creation_reason,CreationReason::History);assert!(matches!(store.adopt_candidate("s1",&initial.id,candidate),Err(StoreError::Conflict{..})));assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);
}
#[test]
fn restoring_unicode_scalar_range_only_changes_selected_block_and_retains_other_text() {
    let mut store=Store::in_memory().unwrap();let initial=ready(&mut store,"s1",StoragePolicy::History).output.unwrap();let mut blocks=initial.blocks.clone();blocks[0].text="🙂可以订机票，Hello。".into();let edited=store.edit_output("s1",&initial.id,blocks,"now").unwrap().output.unwrap();
    // Source scalar 8..13 is 不要订机票; output scalar 1..6 is 可以订机票.
    let src=source("s1");let original=&src.items[0].text;let start=original.chars().position(|c|c=='不').unwrap();
    let detail=store.restore_output("s1",&edited.id,RestorePatch{block_id:"block-1".into(),start:1,end:6,source_selections:vec![SourceSelection{item_id:"item-s1-1".into(),start,end:start+5}]},"later").unwrap();let restored=detail.output.unwrap();assert_eq!(restored.blocks[0].text,"🙂不要订机票，Hello。");assert_eq!(restored.blocks[1],initial.blocks[1]);assert_eq!(detail.source,src);
    assert!(store.restore_output("s1",&restored.id,RestorePatch{block_id:"block-1".into(),start:0,end:999,source_selections:vec![SourceSelection{item_id:"item-s1-1".into(),start:0,end:1}]},"later").is_err());
}
#[test]
fn persistent_one_level_undo_survives_restart_and_cannot_undo_twice() {
    let path=path();let edited_id;
    {let mut store=Store::open(&path).unwrap();let initial=ready(&mut store,"s1",StoragePolicy::History).output.unwrap();let mut blocks=initial.blocks.clone();blocks[0].text="手改".into();edited_id=store.edit_output("s1",&initial.id,blocks,"now").unwrap().output.unwrap().id;}
    let mut store=Store::open(&path).unwrap();let undone=store.undo_output("s1",&edited_id,"later").unwrap();assert_eq!(undone.output.as_ref().unwrap().blocks,output("s1").blocks);assert!(!undone.can_undo);assert!(matches!(store.undo_output("s1",&undone.output.unwrap().id,"later"),Err(StoreError::UndoUnavailable)));drop(store);fs::remove_file(path).unwrap();
}
#[test]
fn search_is_literal_paginated_and_includes_source_without_injecting_sql() {
    let mut store=Store::in_memory().unwrap();for name in ["s1","s2","s3"] {ready(&mut store,name,StoragePolicy::History);}
    let page=store.history_list("HELLO",0,2).unwrap();assert_eq!(page.total,3);assert_eq!(page.items.len(),2);assert_eq!(page.next_cursor,Some(2));assert_eq!(store.history_list("",2,2).unwrap().items.len(),1);assert_eq!(store.history_list("%' OR 1=1 --",0,50).unwrap().total,0);
}
#[test]
fn delete_history_keeps_usage_and_delivery_and_prevents_late_recreation() {
    let mut store=Store::in_memory().unwrap();ready(&mut store,"s1",StoragePolicy::History);store.prepare_delivery(delivery("s1")).unwrap();store.delete_history("s1").unwrap();assert_eq!(store.history_list("",0,20).unwrap().total,0);assert!(store.get_source("source-s1").is_err());assert!(store.get_output("output-s1").is_err());assert!(store.delivery_attempt("delivery-s1").is_ok());assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);assert!(matches!(store.checkpoint_source(seed("s1",StoragePolicy::History),source("s1")),Err(StoreError::Cancelled)));
}
fn delivery(session:&str) -> DeliveryAttempt {DeliveryAttempt{id:format!("delivery-{session}"),session_id:session.into(),generation:7,output_revision_id:format!("output-{session}"),target_fingerprint:"hwnd:42-pid:24-control:abc".into(),claimed_at:"now".into(),state:DeliveryState::Prepared,reason:None}}
#[test]
fn delivery_permission_is_once_and_works_without_persisted_body_rows() {
    let mut store=Store::in_memory().unwrap();ready(&mut store,"p1",StoragePolicy::MemoryOnly);store.prepare_delivery(delivery("p1")).unwrap();assert!(matches!(store.prepare_delivery(delivery("p1")),Err(StoreError::DeliveryConsumed)));
    store.transition_delivery("delivery-p1",DeliveryState::Prepared,DeliveryState::Dispatched,None).unwrap();store.transition_delivery("delivery-p1",DeliveryState::Dispatched,DeliveryState::Unconfirmed,Some("readbackUnavailable".into())).unwrap();assert!(store.transition_delivery("delivery-p1",DeliveryState::Unconfirmed,DeliveryState::Prepared,None).is_err());
}
#[test]
fn retry_use_source_and_history_outputs_never_get_automatic_delivery_permission() {
    let mut store=Store::in_memory().unwrap();store.checkpoint_source(seed("s1",StoragePolicy::History),source("s1")).unwrap();let mut result=output("s1");result.creation_reason=CreationReason::Retry;store.commit_ready(result).unwrap();assert!(store.prepare_delivery(delivery("s1")).is_err());
    let mut store=Store::in_memory().unwrap();store.checkpoint_source(seed("s1",StoragePolicy::History),source("s1")).unwrap();let used=store.use_source("s1",7,"now").unwrap().output.unwrap();let mut attempt=delivery("s1");attempt.output_revision_id=used.id;assert!(store.prepare_delivery(attempt).is_err());
}
#[test]
fn restarting_marks_checkpoint_interrupted_and_dispatched_unconfirmed_without_replay() {
    let mut store=Store::in_memory().unwrap();store.checkpoint_source(seed("waiting",StoragePolicy::History),source("waiting")).unwrap();ready(&mut store,"sent",StoragePolicy::History);store.prepare_delivery(delivery("sent")).unwrap();store.transition_delivery("delivery-sent",DeliveryState::Prepared,DeliveryState::Dispatched,None).unwrap();store.recover_after_restart().unwrap();assert_eq!(store.get_result("waiting").unwrap().session.status,StoredSessionStatus::Interrupted);assert_eq!(store.delivery_attempt("delivery-sent").unwrap().state,DeliveryState::Unconfirmed);assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);
}
#[test]
fn end_date_is_frozen_and_characters_exclude_punctuation_symbols_and_whitespace() {
    assert_eq!(super::store::source_character_count("中 文, A-b 12🙂\n"),6);let mut store=Store::in_memory().unwrap();let mut metadata=seed("s1",StoragePolicy::History);metadata.end_local_date="2026-10-05".into();store.checkpoint_source(metadata,source("s1")).unwrap();store.commit_ready(output("s1")).unwrap();assert_eq!(store.usage_today("2026-10-04").unwrap().count,0);assert_eq!(store.usage_today("2026-10-05").unwrap().count,1);
}
#[test]
fn dto_uses_camel_case_and_no_secret_or_external_path() {
    let data=serde_json::to_value(output("s1")).unwrap();assert!(data.get("sourceRevisionId").is_some());assert!(data.get("creationReason").is_some());assert!(data.get("source_revision_id").is_none());let data=serde_json::to_value(source("s1")).unwrap();assert_eq!(data["items"][0]["final"],true);
}

#[test]
fn memory_only_body_is_absent_from_actual_sqlite_file_and_does_not_return_after_restart() {
    let path=path();let sentinel="PRIVATE_BODY_NEVER_PERSISTED_9bf87";
    {let mut store=Store::open(&path).unwrap();let mut src=source("private");src.items[0].text=sentinel.into();store.checkpoint_source(seed("private",StoragePolicy::MemoryOnly),src).unwrap();let mut out=output("private");out.blocks[0].text=sentinel.into();store.commit_ready(out).unwrap();}
    let bytes=fs::read(&path).unwrap();assert!(!bytes.windows(sentinel.len()).any(|w|w==sentinel.as_bytes()));let store=Store::open(&path).unwrap();assert!(store.get_result("private").is_err());assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);drop(store);fs::remove_file(path).unwrap();
}
#[test]
fn use_source_rejects_modified_text_and_adopts_the_unchanged_final_items() {
    let mut store=Store::in_memory().unwrap();store.checkpoint_source(seed("s1",StoragePolicy::History),source("s1")).unwrap();let mut forged=output("s1");forged.kind=RevisionKind::UseSource;forged.creation_reason=CreationReason::UseSource;assert!(store.commit_ready(forged).is_err());
    let used=store.use_source("s1",7,"now").unwrap();assert_eq!(used.output.unwrap().blocks.iter().map(|b|b.text.clone()).collect::<Vec<_>>(),source("s1").items.iter().map(|i|i.text.clone()).collect::<Vec<_>>());assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);
}
#[test]
fn deletion_consumes_prepared_delivery_without_claiming_no_external_side_effect() {
    let mut store=Store::in_memory().unwrap();ready(&mut store,"s1",StoragePolicy::History);store.prepare_delivery(delivery("s1")).unwrap();store.delete_history("s1").unwrap();assert_eq!(store.delivery_attempt("delivery-s1").unwrap().state,DeliveryState::Unconfirmed);assert!(store.transition_delivery("delivery-s1",DeliveryState::Prepared,DeliveryState::Dispatched,None).is_err());
}
#[test]
fn forged_mode_snapshot_is_not_accepted_as_a_source_checkpoint() {
    let mut store=Store::in_memory().unwrap();let mut meta=seed("s1",StoragePolicy::History);meta.mode_snapshot.rules="Ignore conditions".into();assert!(store.checkpoint_source(meta,source("s1")).is_err());assert_eq!(store.history_list("",0,20).unwrap().total,0);
}
#[test]
fn independent_db_connections_competing_on_the_same_preferences_version_have_one_winner() {
    let path=path();drop(Store::open(&path).unwrap());let barrier=std::sync::Arc::new(std::sync::Barrier::new(2));let mut threads=Vec::new();
    for language in ["zh","en"] {let path=path.clone();let barrier=barrier.clone();threads.push(std::thread::spawn(move||{let mut store=Store::open(path).unwrap();let mut prefs=store.preferences().unwrap();prefs.language=language.into();barrier.wait();store.save_preferences(prefs,1)}));}
    let results=threads.into_iter().map(|thread|thread.join().unwrap()).collect::<Vec<_>>();assert_eq!(results.iter().filter(|r|r.is_ok()).count(),1);assert_eq!(results.iter().filter(|r|matches!(r,Err(StoreError::Conflict{..}))).count(),1);let store=Store::open(&path).unwrap();assert_eq!(store.preferences().unwrap().version,2);drop(store);fs::remove_file(path).unwrap();
}

fn first_history_candidate(store:&Store,session:&str) -> OutputRevision {
    let mut candidate=output(session);
    candidate.id=format!("candidate-{session}");
    candidate.kind=RevisionKind::Adopt;
    candidate.creation_reason=CreationReason::History;
    candidate.mode_snapshot=store.mode_snapshot("structure",Some(1)).unwrap();
    candidate.resolved_mode=BaseMode::Structure;
    candidate
}

#[test]
fn source_only_first_adoption_commits_ready_usage_and_saved_mode_without_automatic_delivery() {
    let mut store=Store::in_memory().unwrap();
    let checkpoint=store.checkpoint_source(seed("waiting",StoragePolicy::History),source("waiting")).unwrap();
    let candidate=first_history_candidate(&store,"waiting");
    assert_eq!(store.usage_today("2026-10-04").unwrap().count,0);
    assert!(store.get_output(&candidate.id).is_err(),"an unadopted candidate is not a saved revision");
    let adopted=store.adopt_candidate("waiting","",candidate.clone()).unwrap();
    let revision=adopted.output.as_ref().unwrap();
    assert_ne!(revision.id,candidate.id);
    assert_eq!(revision.kind,RevisionKind::Adopt);
    assert_eq!(revision.creation_reason,CreationReason::History);
    assert_eq!(revision.mode_snapshot,candidate.mode_snapshot);
    assert_eq!(revision.parent_revision_id,None);
    assert_eq!(revision.base_revision_id,None);
    assert_eq!(adopted.session.status,StoredSessionStatus::Ready);
    assert_eq!(adopted.session.seed,checkpoint.session.seed);
    assert_eq!(adopted.source,checkpoint.source);
    assert!(!adopted.can_undo);
    let usage=store.usage_event("waiting").unwrap().unwrap();
    assert_eq!(usage.source_char_count,super::store::source_character_count(&checkpoint.source.text()));
    assert_eq!(usage.capture_duration_ms,12_000);
    let mut permission=delivery("waiting");permission.output_revision_id=revision.id.clone();
    assert!(store.prepare_delivery(permission).is_err());
    assert!(matches!(store.undo_output("waiting",&revision.id,"later"),Err(StoreError::UndoUnavailable)));
}

#[test]
fn source_only_history_survives_restart_then_first_adoption_preserves_the_selected_mode_revision() {
    let db_path=path();let candidate;let saved_mode;
    {
        let mut store=Store::open(&db_path).unwrap();
        store.checkpoint_source(seed("waiting",StoragePolicy::History),source("waiting")).unwrap();
        let mode=store.save_mode(draft("会议事项")).unwrap();
        saved_mode=store.mode_snapshot(&mode.id,Some(1)).unwrap();
        candidate={let mut value=first_history_candidate(&store,"waiting");value.mode_snapshot=saved_mode.clone();value.resolved_mode=BaseMode::Light;value};
        store.save_mode(ModeDraft{id:Some(mode.id.clone()),name:"会议事项新版".into(),base_mode:BaseMode::Structure,rules:"分点列出全部信息。".into(),expected_revision:Some(1)}).unwrap();
        store.delete_mode(&mode.id,2).unwrap();
    }
    let adopted_id;
    {
        let mut store=Store::open(&db_path).unwrap();store.recover_after_restart().unwrap();
        let interrupted=store.get_result("waiting").unwrap();
        assert_eq!(interrupted.session.status,StoredSessionStatus::Interrupted);
        assert!(interrupted.output.is_none());assert_eq!(store.usage_today("2026-10-04").unwrap().count,0);
        let adopted=store.adopt_candidate("waiting","",candidate).unwrap();adopted_id=adopted.output.unwrap().id;
    }
    let store=Store::open(&db_path).unwrap();let persisted=store.get_result("waiting").unwrap();
    assert_eq!(persisted.output.as_ref().unwrap().id,adopted_id);
    assert_eq!(persisted.output.unwrap().mode_snapshot,saved_mode);
    assert_eq!(persisted.source,source("waiting"));assert!(!persisted.can_undo);
    assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);
    drop(store);fs::remove_file(db_path).unwrap();
}

#[test]
fn source_only_adoption_usage_failure_rolls_back_every_new_body_and_can_be_retried() {
    let mut store=Store::in_memory().unwrap();
    store.checkpoint_source(seed("waiting",StoragePolicy::History),source("waiting")).unwrap();
    let candidate=first_history_candidate(&store,"waiting");
    store.test_connection().execute_batch("CREATE TRIGGER fail_first_adoption_usage BEFORE INSERT ON usage_events BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();
    assert!(matches!(store.adopt_candidate("waiting","",candidate.clone()),Err(StoreError::Database(_))));
    let checkpoint=store.get_result("waiting").unwrap();assert!(checkpoint.output.is_none());
    assert_eq!(checkpoint.session.status,StoredSessionStatus::Checkpointed);
    assert_eq!(checkpoint.source,source("waiting"));
    assert_eq!(store.test_connection().query_row("SELECT COUNT(*) FROM outputs",[],|row|row.get::<_,u64>(0)).unwrap(),0);
    assert_eq!(store.usage_today("2026-10-04").unwrap().count,0);
    store.test_connection().execute_batch("DROP TRIGGER fail_first_adoption_usage;").unwrap();
    store.adopt_candidate("waiting","",candidate.clone()).unwrap();
    assert!(matches!(store.adopt_candidate("waiting","",candidate),Err(StoreError::Conflict{..})));
    assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);
}

#[test]
fn source_only_adoption_requires_empty_current_and_no_base_or_parent() {
    let mut store=Store::in_memory().unwrap();
    store.checkpoint_source(seed("waiting",StoragePolicy::History),source("waiting")).unwrap();
    let candidate=first_history_candidate(&store,"waiting");
    assert!(matches!(store.adopt_candidate("waiting","nonexistent",candidate.clone()),Err(StoreError::Conflict{current}) if current.is_empty()));
    for mutation in 0..3 {
        let mut wrong=candidate.clone();
        match mutation {0=>wrong.base_revision_id=Some("".into()),1=>wrong.base_revision_id=Some("nonexistent".into()),_=>wrong.parent_revision_id=Some("nonexistent".into())};
        assert!(store.adopt_candidate("waiting","",wrong).is_err());
    }
    assert!(store.get_result("waiting").unwrap().output.is_none());
    assert_eq!(store.usage_today("2026-10-04").unwrap().count,0);
}

#[test]
fn source_only_adoption_rejects_other_session_source_and_cross_source_references() {
    let mut store=Store::in_memory().unwrap();
    for session in ["waiting","other"] {store.checkpoint_source(seed(session,StoragePolicy::History),source(session)).unwrap();}
    let candidate=first_history_candidate(&store,"waiting");
    for mutation in 0..3 {
        let mut wrong=candidate.clone();
        match mutation {0=>wrong.session_id="other".into(),1=>wrong.source_revision_id="source-other".into(),_=>wrong.blocks[0].source_refs=vec!["item-other-1".into()]};
        assert!(store.adopt_candidate("waiting","",wrong).is_err());
    }
    assert_eq!(store.get_source("source-waiting").unwrap(),source("waiting"));
    assert!(store.get_result("waiting").unwrap().output.is_none());
    assert_eq!(store.usage_today("2026-10-04").unwrap().count,0);
}

#[test]
fn source_only_adoption_requires_a_real_saved_mode_snapshot_and_resolved_base() {
    let mut store=Store::in_memory().unwrap();
    store.checkpoint_source(seed("waiting",StoragePolicy::History),source("waiting")).unwrap();
    let candidate=first_history_candidate(&store,"waiting");
    for mutation in 0..4 {
        let mut wrong=candidate.clone();
        match mutation {0=>wrong.mode_snapshot.rules="forged unsaved rules".into(),1=>wrong.mode_snapshot.revision=999,2=>wrong.mode_snapshot.mode_id="unsaved-mode".into(),_=>wrong.resolved_mode=BaseMode::Light};
        assert!(store.adopt_candidate("waiting","",wrong).is_err());
    }
    assert!(store.get_result("waiting").unwrap().output.is_none());
    assert_eq!(store.usage_today("2026-10-04").unwrap().count,0);
}

#[test]
fn public_initial_ready_cannot_use_history_adoption_to_override_the_frozen_capture_mode() {
    let mut store=Store::in_memory().unwrap();
    store.checkpoint_source(seed("waiting",StoragePolicy::History),source("waiting")).unwrap();
    let candidate=first_history_candidate(&store,"waiting");
    for (kind,reason) in [(RevisionKind::Adopt,CreationReason::History),(RevisionKind::Initial,CreationReason::NormalInitial),(RevisionKind::Initial,CreationReason::Retry),(RevisionKind::UseSource,CreationReason::UseSource)] {
        let mut wrong=candidate.clone();wrong.kind=kind;wrong.creation_reason=reason;
        assert!(store.commit_ready(wrong).is_err());
    }
    assert!(store.get_result("waiting").unwrap().output.is_none());
    assert_eq!(store.usage_today("2026-10-04").unwrap().count,0);
}

#[test]
fn first_history_candidate_cannot_claim_normal_initial_use_source_retry_or_preview_origin() {
    let mut store=Store::in_memory().unwrap();
    store.checkpoint_source(seed("waiting",StoragePolicy::History),source("waiting")).unwrap();
    let candidate=first_history_candidate(&store,"waiting");
    for (kind,reason) in [(RevisionKind::Initial,CreationReason::NormalInitial),(RevisionKind::UseSource,CreationReason::UseSource),(RevisionKind::Adopt,CreationReason::NormalInitial),(RevisionKind::Adopt,CreationReason::Retry),(RevisionKind::Adopt,CreationReason::Preview)] {
        let mut wrong=candidate.clone();wrong.kind=kind;wrong.creation_reason=reason;
        assert!(store.adopt_candidate("waiting","",wrong).is_err());
    }
    assert!(store.get_result("waiting").unwrap().output.is_none());
    assert_eq!(store.usage_today("2026-10-04").unwrap().count,0);
}

#[test]
fn first_adoption_revalidates_stored_source_completeness_order_and_capture_identity() {
    for mutation in 0..5 {
        let mut store=Store::in_memory().unwrap();
        store.checkpoint_source(seed("waiting",StoragePolicy::History),source("waiting")).unwrap();
        let candidate=first_history_candidate(&store,"waiting");let mut corrupt=source("waiting");
        match mutation {0=>corrupt.complete=false,1=>corrupt.items[1].is_final=false,2=>corrupt.items.swap(0,1),3=>corrupt.generation+=1,_=>corrupt.session_id="other".into()};
        // Deliberate database-corruption fixture, not a production mutation API.
        store.test_connection().execute_batch("DROP TRIGGER source_no_update;").unwrap();
        store.test_connection().execute("UPDATE sources SET data=?1 WHERE id='source-waiting'",[serde_json::to_string(&corrupt).unwrap()]).unwrap();
        assert!(store.adopt_candidate("waiting","",candidate).is_err());
        assert!(store.get_result("waiting").unwrap().output.is_none());
        assert_eq!(store.usage_today("2026-10-04").unwrap().count,0);
    }
}

#[test]
fn cancelled_or_deleted_source_only_history_rejects_late_candidate_without_usage_or_recreation() {
    for cancel in [true,false] {
        let mut store=Store::in_memory().unwrap();
        store.checkpoint_source(seed("waiting",StoragePolicy::History),source("waiting")).unwrap();
        let candidate=first_history_candidate(&store,"waiting");
        if cancel {store.cancel_unfinished("waiting").unwrap();} else {store.delete_history("waiting").unwrap();}
        assert!(matches!(store.adopt_candidate("waiting","",candidate),Err(StoreError::Cancelled)));
        assert!(store.get_source("source-waiting").is_err());
        assert_eq!(store.history_list("",0,20).unwrap().total,0);
        assert_eq!(store.test_connection().query_row("SELECT COUNT(*) FROM outputs",[],|row|row.get::<_,u64>(0)).unwrap(),0);
        assert_eq!(store.usage_today("2026-10-04").unwrap().count,0);
    }
}

#[test]
fn independent_connections_competing_for_first_adoption_have_one_ready_and_one_usage_event() {
    let db_path=path();
    {let mut store=Store::open(&db_path).unwrap();store.checkpoint_source(seed("waiting",StoragePolicy::History),source("waiting")).unwrap();}
    let barrier=std::sync::Arc::new(std::sync::Barrier::new(2));let mut threads=Vec::new();
    for index in 0..2 {
        let db_path=db_path.clone();let barrier=barrier.clone();
        threads.push(std::thread::spawn(move||{
            let mut store=Store::open(db_path).unwrap();let mut candidate=first_history_candidate(&store,"waiting");candidate.blocks[0].text=format!("候选 {index}：保留条件和否定。");
            barrier.wait();store.adopt_candidate("waiting","",candidate)
        }));
    }
    let results=threads.into_iter().map(|thread|thread.join().unwrap()).collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result|result.is_ok()).count(),1);
    assert_eq!(results.iter().filter(|result|matches!(result,Err(StoreError::Conflict{..}))).count(),1);
    let store=Store::open(&db_path).unwrap();
    assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);
    assert_eq!(store.test_connection().query_row("SELECT COUNT(*) FROM outputs",[],|row|row.get::<_,u64>(0)).unwrap(),1);
    assert_eq!(store.get_source("source-waiting").unwrap(),source("waiting"));
    drop(store);fs::remove_file(db_path).unwrap();
}

#[test]
fn first_adoption_then_edit_and_one_level_undo_keep_the_original_source_and_once_usage() {
    let mut store=Store::in_memory().unwrap();
    store.checkpoint_source(seed("waiting",StoragePolicy::History),source("waiting")).unwrap();
    let candidate=first_history_candidate(&store,"waiting");
    let first=store.adopt_candidate("waiting","",candidate.clone()).unwrap().output.unwrap();
    assert!(matches!(store.adopt_candidate("waiting","",candidate),Err(StoreError::Conflict{..})));
    assert!(matches!(store.cancel_unfinished("waiting"),Err(StoreError::AlreadyCompleted)));
    let mut blocks=first.blocks.clone();blocks[0].text="手动修正，保留否定。".into();
    let edited=store.edit_output("waiting",&first.id,blocks,"later").unwrap();assert!(edited.can_undo);
    let undone=store.undo_output("waiting",&edited.output.unwrap().id,"later").unwrap();
    assert_eq!(undone.output.unwrap().blocks,first.blocks);assert!(!undone.can_undo);
    assert_eq!(undone.source,source("waiting"));assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);
    store.delete_history("waiting").unwrap();assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);
}

#[test]
fn source_only_memory_adoption_keeps_body_out_of_sqlite_and_uses_the_same_first_ready_boundary() {
    let mut store=Store::in_memory().unwrap();
    store.checkpoint_source(seed("private",StoragePolicy::MemoryOnly),source("private")).unwrap();
    let candidate=first_history_candidate(&store,"private");
    let adopted=store.adopt_candidate("private","",candidate).unwrap();assert!(!adopted.can_undo);
    assert_eq!(store.latest_temporary_result().unwrap().unwrap(),adopted);
    for table in ["sessions","sources","source_items","outputs"] {
        assert_eq!(store.test_connection().query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|row|row.get::<_,u64>(0)).unwrap(),0);
    }
    assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);
    store.clear_temporary_results();assert!(store.get_result("private").is_err());
    assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);
}
