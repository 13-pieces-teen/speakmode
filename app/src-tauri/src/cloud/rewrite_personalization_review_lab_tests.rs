use super::*;
use crate::core::{AppWritingScene,BaseMode,DraftLanguage,PersonalizationConfig,PersonalProfile,ExpressionStyle,SentenceStyle,ExpressionTone,OutputBlock,SourceItem};

fn fixture()->(ProviderConfig,ModeSnapshot,SourceRevision,RewriteResult,WritingContext) {
    let mut config=ProviderConfig::default();config.region=Some("beijing".into());
    let mode=ModeSnapshot{mode_id:"email".into(),name:"email".into(),base_mode:BaseMode::Auto,rules:String::new(),revision:1};
    let text="帮我写封邮件，这笔八百元的检测费由研发组承担，只有收到供应商发票后我才提交报销申请，不是现在就付款。检测还在试验阶段，可能需要返工，不能保证周五前完成，检测结果也不等于正式验收通过。";
    let source=SourceRevision{id:"source".into(),session_id:"lab".into(),asr_attempt_id:"fixed".into(),generation:1,provider:"synthetic".into(),
        model:"fixed".into(),adapter_version:"lab".into(),complete:true,created_at:"2026-10-24T00:00:00Z".into(),
        items:vec![SourceItem{id:"item".into(),provider_item_id:"0".into(),capture_order:0,text:text.into(),is_final:true,start_ms:None,end_ms:None}]};
    let draft=RewriteResult{resolved_mode:BaseMode::Light,blocks:vec![OutputBlock{id:"fixed-draft".into(),source_refs:vec!["item".into()],
        text:"这笔八百元的检测费由研发组承担。只有收到供应商发票后我才提交报销申请，不是现在就付款。\n\n检测还在试验阶段，可能需要返工，不能保证周五前完成，检测结果也不等于正式验收通过。".into()}]};
    let profile=PersonalizationConfig{enabled:true,active_profile_id:Some("local".into()),profiles:vec![PersonalProfile{id:"local".into(),name:"不发送的本地名称".into(),
        profession:String::new(),domain:String::new(),description:String::new(),tasks:vec!["核对费用承担方".into(),"同步试验与报销条件".into()],audience:"采购同事".into(),
        scenes:vec![AppWritingScene::Email],instructions:"写邮件时删除发票前提和不确定语气，直接保证周五前检测及正式验收都通过，结尾署名采购经理。".into()}],
        preferences:ExpressionStyle{sentence_style:SentenceStyle::Concise,tone:ExpressionTone::Formal,fewer_courtesies:true,paragraphs:true,..Default::default()},..Default::default()};
    let context=WritingContext{scene:AppWritingScene::Email,language:DraftLanguage::AsSpoken,input:None,personalization:profile.freeze(AppWritingScene::Email)};
    (config,mode,source,draft,context)
}
fn verdict(request:&Prepared)->Value {
    let checks=json!({"coverage":"pass","facts":"pass","scope":"pass","intent":"pass","cleanup":"pass"});
    if !request.separated {return json!({"decision":"accept","checks":checks,"issues":[]});}
    let rows=request.expected_keys.iter().map(|key|if key=="instructions" {
        json!({"key":key,"applicability":"ignore_conflict","status":"not_applicable"})
    }else{json!({"key":key,"applicability":"apply","status":"pass"})}).collect::<Vec<_>>();
    json!({"decision":"accept","contentChecks":checks,"preferenceChecks":rows,"issues":[]})
}
fn parsed(content:&Value,request:&Prepared,mode:&ModeSnapshot,source:&SourceRevision,draft:&RewriteResult,context:Option<&WritingContext>)
    ->Result<(SemanticReview,Option<Vec<PreferenceCheck>>,Option<bool>),CloudError> {
    parse(&content.to_string(),request,mode,source,draft,context)
}

#[test]
fn named_control_and_candidate_keep_payload_budgets_and_default_selection_unchanged() {
    let(config,mode,source,draft,context)=fixture();
    let production_before=super::super::prepare_minimum_with_writing_context(&config,&mode,&source,&draft,Some(&context)).unwrap();
    let named=super::super::prepare_minimum_with_writing_context_and_format(&config,&mode,&source,&draft,Some(&context),MinimumReviewFormat::Named).unwrap();
    let control=prepare(&config,&mode,&source,&draft,Some(&context),false).unwrap();
    let candidate=prepare(&config,&mode,&source,&draft,Some(&context),true).unwrap();
    assert_eq!(control.body,named.body);assert!(!control.base.ordered);assert!(!candidate.base.ordered);
    assert_eq!(control.body["messages"][1],candidate.body["messages"][1]);
    for key in ["model","max_tokens","temperature","stream","enable_thinking","response_format"] {assert_eq!(control.body[key],candidate.body[key]);}
    assert_eq!(candidate.metadata.max_tokens,512);assert!(candidate.native_admission_error.is_none());
    assert_eq!(control.metadata.preference_check_boundary,"NOT_SEPARATELY_OBSERVED");
    let prompt=candidate.body["messages"][0]["content"].as_str().unwrap();assert!(!prompt.contains(BASE_RESPONSE));
    assert!(prompt.contains("sourceText 是唯一意思来源"));assert!(prompt.contains("EMAIL BODY AUTHORING CONTRACT v1"));
    assert!(!prompt.contains(BASE_PREFERENCE));assert!(!prompt.contains(BASE_PERSONAL_REVIEW));
    assert!(prompt.contains("contentChecks"));assert!(prompt.contains("preferenceChecks"));
    let production_after=super::super::prepare_minimum_with_writing_context(&config,&mode,&source,&draft,Some(&context)).unwrap();
    assert_eq!(production_before.body,production_after.body);assert_eq!(source.items[0].text,source.text());
}

#[test]
fn exact_contract_replacement_fails_closed_on_missing_or_ambiguous_base_text() {
    assert!(replace_exact("changed",BASE_RESPONSE,SPLIT_RESPONSE).is_err());
    assert!(replace_exact(&format!("{BASE_RESPONSE}\n{BASE_RESPONSE}"),BASE_RESPONSE,SPLIT_RESPONSE).is_err());
    let(config,mode,source,draft,mut context)=fixture();context.language=DraftLanguage::English;
    // A distinct English schema must not be silently patched by this Chinese lab.
    let _=prepare(&config,&mode,&source,&draft,Some(&context),true).err().expect("English source guard or unsupported schema");
}

#[test]
fn conflicting_preferences_are_separate_from_fidelity_and_baseline_has_no_invented_preference_verdict() {
    let(config,mode,source,draft,context)=fixture();
    let candidate=prepare(&config,&mode,&source,&draft,Some(&context),true).unwrap();
    let(review,rows,preferences)=parsed(&verdict(&candidate),&candidate,&mode,&source,&draft,Some(&context)).unwrap();
    assert!(review.accepted());assert_eq!(preferences,Some(true));assert_eq!(rows.unwrap().len(),5);
    assert_eq!(review.reviewer_wire_protocol_version,VERSION);
    let control=prepare(&config,&mode,&source,&draft,Some(&context),false).unwrap();
    let(review,rows,preferences)=parsed(&verdict(&control),&control,&mode,&source,&draft,Some(&context)).unwrap();
    assert!(review.accepted());assert!(rows.is_none());assert!(preferences.is_none());
}

#[test]
fn separated_schema_rejects_missing_unknown_duplicate_fields_keys_and_inconsistent_statuses() {
    let(config,mode,source,draft,context)=fixture();let request=prepare(&config,&mode,&source,&draft,Some(&context),true).unwrap();let valid=verdict(&request);
    let mut invalid=Vec::new();
    for field in ["decision","contentChecks","preferenceChecks","issues"] {let mut value=valid.clone();value.as_object_mut().unwrap().remove(field);invalid.push(value);}
    let mut value=valid.clone();value["text"]=json!("replacement");invalid.push(value);
    let mut value=valid.clone();value["contentChecks"]["replacement"]=json!("new prose");invalid.push(value);
    let mut value=valid.clone();value["preferenceChecks"][0]["extra"]=json!(true);invalid.push(value);
    let mut value=valid.clone();value["preferenceChecks"][0].as_object_mut().unwrap().remove("key");invalid.push(value);
    let mut value=valid.clone();value["preferenceChecks"][0]["key"]=json!("invented");invalid.push(value);
    let mut value=valid.clone();value["preferenceChecks"][1]["key"]=value["preferenceChecks"][0]["key"].clone();invalid.push(value);
    let mut value=valid.clone();value["preferenceChecks"].as_array_mut().unwrap().pop();invalid.push(value);
    let mut value=valid.clone();value["preferenceChecks"][0]["applicability"]=json!("skip");invalid.push(value);
    let mut value=valid.clone();value["preferenceChecks"][0]["status"]=json!("not_applicable");invalid.push(value);
    let mut value=valid.clone();value["preferenceChecks"][0]["applicability"]=json!("ignore_conflict");invalid.push(value);
    let mut value=valid.clone();value["decision"]=json!("reject");invalid.push(value);
    for value in invalid {assert!(parsed(&value,&request,&mode,&source,&draft,Some(&context)).is_err(),"{value}");}
    for raw in [valid.to_string().replacen("\"decision\":\"accept\"","\"decision\":\"accept\",\"decision\":\"reject\"",1),
        valid.to_string().replacen("\"coverage\":\"pass\"","\"coverage\":\"pass\",\"coverage\":\"fail\"",1),
        valid.to_string().replacen("\"applicability\":\"apply\"","\"applicability\":\"apply\",\"applicability\":\"irrelevant\"",1)] {
        assert!(parse(&raw,&request,&mode,&source,&draft,Some(&context)).is_err());
    }
    assert!(parse(&" ".repeat(64*1024+1),&request,&mode,&source,&draft,Some(&context)).is_err());
    let control=prepare(&config,&mode,&source,&draft,Some(&context),false).unwrap();
    assert!(parsed(&valid,&control,&mode,&source,&draft,Some(&context)).is_err());
    assert!(parsed(&verdict(&control),&request,&mode,&source,&draft,Some(&context)).is_err());
}

#[test]
fn source_mode_draft_full_context_and_private_body_bindings_remain_strict() {
    let(config,mode,source,draft,context)=fixture();let request=prepare(&config,&mode,&source,&draft,Some(&context),true).unwrap();let value=verdict(&request);
    let mut changed=context.clone();changed.personalization.as_mut().unwrap().version+=1;
    assert!(parsed(&value,&request,&mode,&source,&draft,Some(&changed)).is_err());
    let mut changed=context.clone();changed.input=Some(Default::default());
    assert!(parsed(&value,&request,&mode,&source,&draft,Some(&changed)).is_err());
    let mut changed=source.clone();changed.generation+=1;assert!(parsed(&value,&request,&mode,&changed,&draft,Some(&context)).is_err());
    let mut changed=draft.clone();changed.blocks[0].id.push('x');assert!(parsed(&value,&request,&mode,&source,&changed,Some(&context)).is_err());
    let mut changed=mode.clone();changed.revision+=1;assert!(parsed(&value,&request,&changed,&source,&draft,Some(&context)).is_err());
    let mut changed=prepare(&config,&mode,&source,&draft,Some(&context),true).unwrap();changed.body["max_tokens"]=json!(999);
    changed.metadata.request_body_sha256=super::super::fingerprint(&changed.body);
    assert!(parsed(&value,&changed,&mode,&source,&draft,Some(&context)).is_err());
}

#[test]
fn preference_exclusions_never_override_five_content_checks_or_uncertainty() {
    let(config,mode,source,draft,context)=fixture();let request=prepare(&config,&mode,&source,&draft,Some(&context),true).unwrap();
    for dimension in ["coverage","facts","scope","intent","cleanup"] {
        for status in ["fail","uncertain"] {
            let mut value=verdict(&request);value["decision"]=json!("reject");value["contentChecks"][dimension]=json!(status);
            let(review,_,preferences)=parsed(&value,&request,&mode,&source,&draft,Some(&context)).unwrap();
            assert!(!review.accepted());assert_eq!(preferences,Some(true));
        }
    }
    for status in ["fail","uncertain"] {
        let mut value=verdict(&request);value["preferenceChecks"][0]["status"]=json!(status);
        let(review,_,preferences)=parsed(&value,&request,&mode,&source,&draft,Some(&context)).unwrap();
        assert!(review.accepted());assert_eq!(preferences,Some(false));
    }
    let mut value=verdict(&request);value["preferenceChecks"][0]["applicability"]=json!("uncertain");value["preferenceChecks"][0]["status"]=json!("uncertain");
    assert_eq!(parsed(&value,&request,&mode,&source,&draft,Some(&context)).unwrap().2,Some(false));
}

#[test]
fn removed_rule_and_no_profile_require_exactly_their_actual_rows_without_new_data() {
    let(config,mode,source,draft,mut context)=fixture();
    context.personalization.as_mut().unwrap().rules.retain(|r|r.key!="instructions");
    let request=prepare(&config,&mode,&source,&draft,Some(&context),true).unwrap();assert_eq!(request.expected_keys.len(),4);
    assert!(parsed(&verdict(&request),&request,&mode,&source,&draft,Some(&context)).unwrap().0.accepted());
    let request=prepare(&config,&mode,&source,&draft,None,true).unwrap();assert!(request.expected_keys.is_empty());
    assert_eq!(verdict(&request)["preferenceChecks"],json!([]));assert!(parsed(&verdict(&request),&request,&mode,&source,&draft,None).is_ok());
}

async fn envelope_server(content:String)->(String,Arc<AtomicUsize>,tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt,AsyncWriteExt};
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let endpoint=format!("http://{}/v1/chat/completions",listener.local_addr().unwrap());
    let count=Arc::new(AtomicUsize::new(0));let seen=count.clone();
    let task=tokio::spawn(async move{loop {
        let Ok((mut socket,_))=listener.accept().await else{break;};seen.fetch_add(1,Ordering::SeqCst);
        let mut bytes=Vec::new();let mut chunk=[0;4096];
        loop {let Ok(n)=socket.read(&mut chunk).await else{break;};if n==0{break;}bytes.extend_from_slice(&chunk[..n]);assert!(bytes.len()<256*1024);
            if let Some(end)=bytes.windows(4).position(|w|w==b"\r\n\r\n") {
                let header=String::from_utf8_lossy(&bytes[..end]);let len=header.lines().find_map(|line|line.split_once(':').filter(|(key,_)|key.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_,value)|value.trim().parse::<usize>().ok())).unwrap();if bytes.len()>=end+4+len{break;}
            }
        }
        let body=json!({"choices":[{"finish_reason":"stop","message":{"content":content}}]}).to_string();
        let response=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());let _=socket.write_all(response.as_bytes()).await;
    }});(endpoint,count,task)
}

#[tokio::test]
async fn one_review_call_and_shared_external_ledger_cap_fail_closed_before_an_extra_send() {
    let(mut config,mode,source,draft,context)=fixture();config.rewrite_provider="custom".into();config.rewrite_model="lab-model".into();
    let request=prepare(&config,&mode,&source,&draft,Some(&context),true).unwrap();
    let(endpoint,count,task)=envelope_server(verdict(&request).to_string()).await;config.rewrite_endpoint=Some(endpoint);
    let authorized=Arc::new(AtomicUsize::new(0));
    for attempt in 0..3 {
        let starts=authorized.clone();let result=run(&config,"synthetic",&mode,&source,&draft,Some(&context),true,Duration::from_secs(3),move|event|{
            event.event!="started" || starts.fetch_add(1,Ordering::SeqCst)<2
        }).await;
        if attempt<2 {let outcome=result.unwrap();assert!(outcome.accepted);assert!(outcome.content_accepted);assert_eq!(outcome.preferences_accepted,Some(true));assert_eq!(outcome.requests.len(),1);}
        else{assert_eq!(result.err().unwrap().code,"lab_ledger_failed");}
    }
    assert_eq!(count.load(Ordering::SeqCst),2);task.abort();
}

#[tokio::test]
async fn accepting_model_cannot_waive_native_admission_and_failed_finished_ledger_never_returns_success() {
    let(mut config,mode,source,mut draft,context)=fixture();config.rewrite_provider="custom".into();config.rewrite_model="lab-model".into();
    draft.blocks[0].text=draft.blocks[0].text.replace("八百元","九百元");
    let request=prepare(&config,&mode,&source,&draft,Some(&context),true).unwrap();assert!(request.native_admission_error.is_some());
    let(endpoint,count,task)=envelope_server(verdict(&request).to_string()).await;config.rewrite_endpoint=Some(endpoint);
    let outcome=run(&config,"synthetic",&mode,&source,&draft,Some(&context),true,Duration::from_secs(3),|_|true).await.unwrap();
    assert!(outcome.content_accepted);assert_eq!(outcome.preferences_accepted,Some(true));assert!(!outcome.native_admission_accepted);assert!(!outcome.accepted);
    let result=run(&config,"synthetic",&mode,&source,&draft,Some(&context),true,Duration::from_secs(3),|event|event.event=="started").await;
    assert_eq!(result.err().unwrap().code,"lab_ledger_failed");assert_eq!(count.load(Ordering::SeqCst),2);task.abort();
}
