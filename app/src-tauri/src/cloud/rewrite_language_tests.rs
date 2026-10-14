use super::*;

#[tokio::test]
async fn english_cleanup_and_review_use_all_fragments_without_changing_source_or_refs() {
    let parts=["Uh I I have worked with L L M and R A G systems，呃，", "I am still learning. And I would not deploy this without a review."];
    let (config,mode,mut source)=fixture(parts[0]);
    let mut second=source.items[0].clone();second.id="item-1".into();second.provider_item_id="1".into();second.capture_order=1;second.text=parts[1].into();source.items.push(second);
    let before=serde_json::to_value(&source).unwrap();
    let clean="I have worked with LLM and RAG systems. I am still learning, and I would not deploy this without a review.";
    let server=Server::new(vec![Reply::draft(clean),Reply::new(ACCEPT)]).await;
    let out=execute(&client(),&config,"synthetic-secret",&mode,&source,&DictionarySnapshot::default(),CancellationToken::new(),&server.endpoint,Duration::from_secs(4)).await;
    let draft=out.result.unwrap();assert_eq!(draft.blocks[0].text,clean);assert_eq!(draft.blocks[0].source_refs,["item-0","item-1"]);
    assert_eq!(serde_json::to_value(&source).unwrap(),before);assert_eq!(server.count(),2);
    for request in server.requests.lock().unwrap().iter() {
        let system=request["messages"][0]["content"].as_str().unwrap();assert_eq!(system.contains("english-dictation-v2"),cfg!(feature="experimental-english-dictation"));
        let data:Value=serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(data["sourceText"],source.text());
        if cfg!(feature="experimental-english-dictation") { assert_eq!(data["sourceFragments"],json!(parts)); }
        else { assert!(data.get("sourceFragments").is_none()); }
    }
}

#[test]
fn language_guidance_cannot_override_explicit_layout_or_stale_bindings() {
    let (config,mut mode,source)=fixture("I have worked with this team and I am preparing a careful introduction of my work.");
    mode.base_mode=BaseMode::Structure;mode.rules="Keep a numbered list".into();
    let request=minimum::prepare(&config,&mode,&source,&DictionarySnapshot::default(),true).unwrap();
    let data:Value=serde_json::from_str(request.body["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(data["baseMode"],"structure");assert_eq!(data["formatRules"],mode.rules);
    assert!(minimum::parse(&json!({"resolvedMode":"light","text":source.text()}).to_string(),&mode,&source,&request).is_err());
    let mut changed=source.clone();changed.items[0].text.push_str(" Another detail.");
    assert_eq!(minimum::parse(&json!({"resolvedMode":"structure","text":source.text()}).to_string(),&mode,&changed,&request).unwrap_err().code,"stale_rewrite");
    let (_,mode,source)=fixture("我使用 Python 开发，目前仍在学习，不承诺已经完成部署。");
    let request=minimum::prepare(&config,&mode,&source,&DictionarySnapshot::default(),true).unwrap();
    assert!(!request.body["messages"][0]["content"].as_str().unwrap().contains("english-dictation-v2"));
}

#[tokio::test]
async fn english_guidance_does_not_waive_numbers_negation_quotes_or_code_checks() {
    let input="I have not approved this. The budget is 30元 and I will preserve \"um, well\" and `UserID` exactly.";
    for changed in [input.replace("30元","40元"), input.replace("have not approved","have approved"),
        input.replace("um, well","well"), input.replace("UserID","userID")] {
        let server=Server::new(vec![Reply::draft(&changed),Reply::new(ACCEPT)]).await;
        let out=test_run(&server,input,Duration::from_secs(4)).await;
        assert!(out.result.is_err(),"unexpected acceptance: {changed}");assert_eq!(server.count(),1,"must fail before model review");
    }
}

#[tokio::test]
async fn fact_rejection_is_not_overridden_by_a_fluent_english_draft() {
    let input="I have worked with the Kelm team and I am preparing a summary of my research.";
    let changed=input.replace("Kelm","Kelvin");
    let server=Server::new(vec![Reply::draft(&changed),Reply::new(rejection("Kelm","Kelvin","facts"))]).await;
    let out=test_run(&server,input,Duration::from_secs(4)).await;
    assert_eq!(out.result.unwrap_err().code,"rewrite_semantic_rejected");assert_eq!(server.count(),2);
    assert!(!out.trace.repair_attempted);
}

#[tokio::test]
async fn recorded_english_year_error_is_blocked_before_an_accepting_model_review() {
    let case_fixture:Value=serde_json::from_str(include_str!("../../../../docs/experiments/fixtures/bilingual-dictation-v2.holdout.json")).unwrap();
    let receipt:Value=serde_json::from_str(include_str!("../../../../docs/experiments/fixtures/bilingual-dictation-v2.reviewed-error.json")).unwrap();
    let (config,mode,mut source)=fixture("");
    let template=source.items[0].clone();source.items.clear();
    for (index,text) in case_fixture["cases"][0]["sourceItems"].as_array().unwrap().iter().enumerate() {
        let mut item=template.clone();item.id=format!("item-{index}");item.provider_item_id=index.to_string();item.capture_order=index as u64;
        item.text=text.as_str().unwrap().into();source.items.push(item);
    }
    assert_eq!(source.text(),receipt["sourceText"]);assert_eq!(receipt["decision"],"accept");
    let before=source.clone();
    let server=Server::new(vec![Reply::draft(receipt["draftText"].as_str().unwrap()),Reply::new(ACCEPT)]).await;
    let outcome=execute(&client(),&config,"synthetic-secret",&mode,&source,&DictionarySnapshot::default(),CancellationToken::new(),&server.endpoint,Duration::from_secs(4)).await;
    assert_eq!(outcome.result.unwrap_err().code,"rewrite_admission_rejected");assert_eq!(server.count(),1);
    assert_eq!(source,before);assert!(outcome.trace.review.is_none());
}
