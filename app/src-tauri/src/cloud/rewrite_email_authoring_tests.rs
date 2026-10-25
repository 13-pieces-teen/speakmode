//! Synthetic, local-only contract regressions. Scripted reviewer verdicts do
//! not establish model fidelity or real voice usefulness.
use super::*;
use crate::cloud::rewrite::{self, email_authoring, writing_context};
use crate::core::{AppWritingScene, ConfirmedContext, DraftLanguage, SourceCorrection, writing_context::WritingContext};

fn email() -> WritingContext {
    WritingContext {scene:AppWritingScene::Email,language:DraftLanguage::AsSpoken,input:None,personalization:None}
}
fn payload(body:&Value)->Value {serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap()}
fn draft(source:&SourceRevision,text:&str)->rewrite::RewriteResult {
    rewrite::RewriteResult {resolved_mode:BaseMode::Light,blocks:vec![crate::core::OutputBlock {
        id:"draft".into(),text:text.into(),source_refs:source.items.iter().map(|item|item.id.clone()).collect(),
    }]}
}
fn admission(input:&str,output:&str)->bool {
    let (_,mode,source)=fixture(input);
    rewrite::review_admission_with_writing_context(&mode,&source,&draft(&source,output),Some(&email())).is_ok()
}
async fn run_email(server:&Server,input:&str)->Outcome {
    let (config,mode,source)=fixture(input);
    execute_until_with_writing_context(&client(),&config,"synthetic-secret",&mode,&source,&DictionarySnapshot::default(),
        CancellationToken::new(),&server.endpoint,RewriteExecution::for_duration(Duration::from_secs(5)),None,Some(&email())).await
}

#[test]
fn only_exact_leading_unquoted_nonnegated_complete_wrapper_clauses_are_authorized() {
    for prefix in ["能不能帮我分段回邮件","帮我分段回邮件","能不能帮我回邮件","帮我回邮件","帮我写封邮件","帮我回一下"] {
        for separator in ["，","：","？"] {
            let text=format!("  {prefix}{separator}收到地址证明后继续核对。");
            let (_,mode,source)=fixture(&text);
            let plan=email_authoring::omission(&mode,&source,Some(&email())).unwrap();
            assert_eq!(&source.items[0].text[plan.start..plan.end],prefix);
            assert_eq!(plan.source_id,"item-0");assert_eq!(plan.body(&source.text()),"收到地址证明后继续核对。");
        }
    }
    for text in [
        "不要帮我回邮件，收到地址证明后继续核对。", "能不能不帮我回邮件，收到地址证明后继续核对。",
        "‘能不能帮我回邮件’，收到地址证明后继续核对。", "`帮我分段回邮件`，收到地址证明后继续核对。",
        "客户说能不能帮我回邮件，收到地址证明后继续核对。", "收到地址证明，能不能帮我回邮件。",
        "帮我回一下客户的问题，收到地址证明后继续核对。", "帮我分段回邮件给客户，收到地址证明后继续核对。",
        "帮我回邮件。", "帮我回邮件，", "帮我回邮件收到地址证明后继续核对。",
        "能不能帮我回邮件？不是让你回邮件，而是保留这个问题。",
        "帮我写封关于地址证明的邮件，收到后继续核对。",
        "能不能帮我回邮件，这是客户刚才问我的问题。", "帮我分段回邮件，这是他让我转述的原话。",
    ] {
        let (_,mode,source)=fixture(text);
        assert!(email_authoring::omission(&mode,&source,Some(&email())).is_none(),"{text}");
    }
    let (_,mode,mut source)=fixture("能不能帮我");
    let mut second=source.items[0].clone();second.id="item-1".into();second.capture_order=1;
    second.text="回邮件，收到地址证明后继续核对。".into();source.items.push(second);
    assert!(email_authoring::omission(&mode,&source,Some(&email())).is_none());
}

#[test]
fn explicit_scene_overrides_host_and_other_scenes_keep_existing_guards() {
    let (_,mut mode,source)=fixture("能不能帮我回邮件，收到地址证明后继续核对。");
    let output=draft(&source,"收到地址证明后继续核对。");
    for scene in [AppWritingScene::General,AppWritingScene::Chat,AppWritingScene::Document,AppWritingScene::Technical] {
        let mut context=email();context.scene=scene;
        assert!(rewrite::review_admission_with_writing_context(&mode,&source,&output,Some(&context)).is_err());
    }
    mode.mode_id="chat".into();
    assert!(rewrite::review_admission_with_writing_context(&mode,&source,&output,Some(&email())).is_err());
    mode.mode_id="email".into();let mut context=email();context.scene=AppWritingScene::Chat;
    assert!(rewrite::review_admission_with_writing_context(&mode,&source,&output,Some(&context)).is_ok());
    assert!(rewrite::review_admission_with_writing_context(&mode,&source,&output,None).is_ok());
    assert!(rewrite::review_admission(&source,&output).is_err());
}

#[test]
fn authorized_prefix_never_waives_customer_questions_conditions_negation_numbers_or_literals() {
    assert!(admission("能不能帮我回邮件？收到地址证明后继续核对。","收到地址证明后继续核对。"));
    assert!(admission("能不能帮我回邮件，客户问补交地址证明后是不是当天有结果？暂时不能承诺。",
        "客户问补交地址证明后是不是当天有结果？暂时不能承诺。"));
    for (input,output) in [
        ("能不能帮我回邮件，客户问能不能当天有结果？收到地址证明后继续核对。","收到地址证明后继续核对。"),
        ("能不能帮我回邮件，客户问是不是当天有结果？收到地址证明后继续核对。","收到地址证明后继续核对。"),
        ("能不能帮我回邮件，如果收到地址证明，再继续核对。","收到地址证明后继续核对。"),
        ("能不能帮我回邮件，暂时不要承诺当天有结果。","当天有结果。"),
        ("能不能帮我回邮件，金额500元。","金额600元。"),
        ("能不能帮我回邮件，保留字段 `userID`。","保留字段 `userId`。"),
        ("帮我分段回邮件，请补交地址证明，如果金额是500元，暂时不要承诺当天有结果。","请补交材料，如果金额是500元，暂时不要承诺当天有结果。"),
        ("帮我分段回邮件，请补交地址证明，如果金额是500元，暂时不要承诺当天有结果。","地址证明，如果金额是500元，暂时不要承诺当天有结果。"),
        ("帮我分段回邮件，客户问是不是当天有结果？收到地址证明后继续核对。","收到地址证明后继续核对。"),
    ] {assert!(!admission(input,output),"{input} -> {output}");}
    let (_,mode,mut source)=fixture("能不能帮我回邮件，收到地址证明后继续核对。");
    let mut second=source.items[0].clone();second.id="later".into();second.capture_order=1;
    second.text="客户的原话是能不能帮我回邮件？".into();source.items.push(second);
    assert!(rewrite::review_admission_with_writing_context(&mode,&source,&draft(&source,"收到地址证明后继续核对。"),Some(&email())).is_err());
}

#[test]
fn contextual_attachment_replaces_generic_anchors_and_is_idempotent_without_touching_source() {
    let input="帮我分段回邮件，收到地址证明后继续核对。";
    let (config,mode,source)=fixture(input);let frozen=source.clone();
    let mut request=minimum::prepare_with_writing_context(&config,&mode,&source,&DictionarySnapshot::default(),false,None).unwrap();
    assert_eq!(payload(&request.body)["sourceRetentionAnchors"],json!([input]));
    for _ in 0..2 {
        writing_context::attach(&mut request.body,&mode,&source,&email(),writing_context::Stage::Generation).unwrap();
        let data=payload(&request.body);assert_eq!(data["sourceText"],input);
        assert_eq!(data["sourceRetentionAnchors"],json!(["收到地址证明后继续核对。"]));
        assert_eq!(request.body["messages"][0]["content"].as_str().unwrap().matches("SOURCE RETENTION ANCHORS:").count(),1);
    }
    let output=json!({"resolvedMode":"light","text":"收到地址证明后继续核对。"}).to_string();
    let parsed=minimum::parse(&output,&mode,&source,&request).unwrap();assert_eq!(parsed.blocks[0].source_refs,["item-0"]);
    let mut changed=source.clone();changed.items[0].text.push_str("改动。");
    assert_eq!(minimum::parse(&output,&mode,&changed,&request).unwrap_err().code,"stale_rewrite");assert_eq!(source,frozen);
}

#[test]
fn repair_omission_authority_is_bound_to_frozen_context_and_only_selected_target_changes() {
    let body="方案暂时不可行。";let input=format!("能不能帮我回邮件，{body}额外说明保持原文。");
    let (config,mode,source)=fixture(&input);let context=email();let original=draft(&source,&format!("{body}额外说明保持原文。"));
    let request=semantic_review::prepare_minimum_with_writing_context(&config,&mode,&source,&original,Some(&context)).unwrap();
    let review=semantic_review::parse_minimum(&rejection(body,body,"cleanup"),&request,&mode,&source,&original).unwrap();
    let mut repair=local_repair::prepare(&config,&mode,&source,&original,&review).unwrap().unwrap();
    writing_context::attach(&mut repair.body,&mode,&source,&context,writing_context::Stage::Repair).unwrap();
    repair.bind_personalization(Some(&context));
    let wire=json!({"replacements":[{"id":"r0","text":"方案暂时不可行！"}]}).to_string();
    let output=local_repair::apply_with_writing_context(&wire,&repair,&mode,&source,&original,Some(&context)).unwrap();
    assert_eq!(output.blocks[0].text,"方案暂时不可行！额外说明保持原文。");
    let mut changed=context.clone();changed.scene=AppWritingScene::General;
    assert_eq!(local_repair::apply_with_writing_context(&wire,&repair,&mode,&source,&original,Some(&changed)).unwrap_err().code,"stale_rewrite");
    assert_eq!(local_repair::apply(&wire,&repair,&mode,&source,&original).unwrap_err().code,"stale_rewrite");
    let mut tampered=payload(&repair.body);tampered["sourceText"]=json!("替换来源");
    repair.body["messages"][1]["content"]=json!(tampered.to_string());
    assert_eq!(local_repair::apply_with_writing_context(&wire,&repair,&mode,&source,&original,Some(&context)).unwrap_err().code,"stale_rewrite");
}

#[tokio::test]
async fn ordinary_wrapper_omission_still_requires_independent_review_and_keeps_complete_source() {
    for (input,output) in [
        ("帮我分段回邮件，收到地址证明后继续核对。","收到地址证明后继续核对。"),
        ("能不能帮我回邮件？收到地址证明后继续核对。","收到地址证明后继续核对。"),
    ] {
        let server=Server::new(vec![Reply::draft(output),Reply::new(ACCEPT)]).await;
        let outcome=run_email(&server,input).await;let result=outcome.result.unwrap();
        assert!(outcome.trace.review.unwrap().accepted());assert_eq!(server.count(),2);
        assert_eq!(result.blocks[0].text,output);assert_eq!(result.blocks[0].source_refs,["item-0"]);
        for request in server.requests.lock().unwrap().iter(){assert_eq!(payload(request)["sourceText"],input);}
    }
    // Arbitrary material substitutions exceed native finite guards. A scripted
    // coverage failure demonstrates independent review is still a hard gate.
    let input="能不能帮我回邮件，收到地址证明后继续核对。";let output="收到材料后继续核对。";
    assert!(admission(input,output));
    let server=Server::new(vec![Reply::draft(output),Reply::new(rejection("地址证明","材料","coverage"))]).await;
    let outcome=run_email(&server,input).await;
    assert!(outcome.result.is_err());assert_eq!(server.count(),2);assert!(!outcome.trace.repair_attempted);
}

#[tokio::test]
async fn contextual_post_review_repair_omits_wrapper_and_keeps_full_source_and_targets() {
    let body="嗯，呃呃，就是就是，我我觉得方案暂时不可行。";
    let input=format!("能不能帮我回邮件，{body}");let output="我觉得方案暂时不可行。";
    let server=Server::new(vec![Reply::draft(body),Reply::new(ACCEPT),
        Reply::new(json!({"replacements":[{"id":"r0","text":output}]}).to_string()),Reply::new(ACCEPT)]).await;
    let outcome=run_email(&server,&input).await;let result=outcome.result.unwrap();
    assert_eq!(server.count(),4);assert!(outcome.trace.repair_attempted);assert!(!outcome.trace.native_deletion_verified);
    assert!(outcome.trace.final_review.unwrap().accepted());assert_eq!(result.blocks[0].text,output);
    assert_eq!(result.blocks[0].source_refs,["item-0"]);
    for request in server.requests.lock().unwrap().iter(){assert_eq!(payload(request)["sourceText"],input);}
}

#[tokio::test]
async fn contextual_repair_cannot_remove_conditions_customer_questions_or_literals() {
    for (body,changed) in [
        ("如果收到地址证明，再继续核对。","收到地址证明后继续核对。"),
        ("客户问是不是当天有结果？","当天有结果。"),
        ("暂时不要承诺当天有结果。","当天有结果。"),
        ("收到500元后继续核对。","收到600元后继续核对。"),
        ("保留字段 `userID`。","保留字段 `userId`。"),
    ] {
        let input=format!("能不能帮我回邮件，{body}");
        let server=Server::new(vec![Reply::draft(body),Reply::new(rejection(body,body,"cleanup")),
            Reply::new(json!({"replacements":[{"id":"r0","text":changed}]}).to_string()),Reply::new(ACCEPT)]).await;
        let outcome=run_email(&server,&input).await;
        assert!(outcome.result.is_err(),"{body} -> {changed}");assert_eq!(server.count(),3);
        assert!(outcome.trace.repair_attempted);assert!(outcome.trace.final_review.is_none());
    }
}

#[tokio::test]
async fn confirmed_candidate_replaces_generic_anchors_and_keeps_original_asr_references() {
    let (config,mode,mut source)=fixture("帮我分段回邮件，收到地址正明后继续核对。");
    let mut second=source.items[0].clone();second.id="item-1".into();second.capture_order=1;
    second.text="暂时不要承诺当天有结果。".into();source.items.push(second);let frozen=source.clone();
    let text=source.text();let start=text[..text.find("地址正明").unwrap()].chars().count();
    let confirmation=ConfirmedContext {source_revision_id:source.id.clone(),language:DraftLanguage::AsSpoken,
        corrections:vec![SourceCorrection {start,end:start+4,expected_text:"地址正明".into(),replacement:"地址证明".into()}]};
    let output="收到地址证明后继续核对。\n暂时不要承诺当天有结果。";
    let server=Server::new(vec![Reply::draft(output),Reply::new(ACCEPT)]).await;
    let outcome=rewrite::confirmed_candidate::run_with_writing_context(&client(),&config,"synthetic-secret",&mode,&source,
        &DictionarySnapshot::default(),&confirmation,CancellationToken::new(),&server.endpoint,Duration::from_secs(5),Some(&email())).await;
    let candidate=outcome.result.unwrap();assert!(candidate.review_issues.is_empty(),"{:?}",candidate.review_issues);
    assert_eq!(server.count(),2);assert_eq!(candidate.draft.blocks[0].source_refs,["item-0","item-1"]);assert_eq!(source,frozen);
    let reference=confirmation.reference_text(&source).unwrap();
    for request in server.requests.lock().unwrap().iter() {
        let data=payload(request);assert_eq!(data["sourceText"],reference);
        assert_eq!(data["sourceRetentionAnchors"],json!(["收到地址证明后继续核对。"]));
    }
}

#[tokio::test]
async fn plain_manual_email_candidate_uses_same_bounded_omission_without_other_scene_changes() {
    let input="能不能帮我回邮件，收到地址证明后继续核对。";let output="收到地址证明后继续核对。";
    let (config,mut mode,source)=fixture(input);mode.mode_id="email".into();
    let server=Server::new(vec![Reply::draft(output),Reply::new(ACCEPT)]).await;
    let candidate=rewrite::manual_candidate::run(&client(),&config,"synthetic-secret",&mode,&source,&DictionarySnapshot::default(),
        CancellationToken::new(),&server.endpoint,Duration::from_secs(5)).await.unwrap();
    assert!(candidate.review_issues.is_empty());assert_eq!(server.count(),2);
    // This manual entry returns a reviewable candidate, never delivery permission.
}
