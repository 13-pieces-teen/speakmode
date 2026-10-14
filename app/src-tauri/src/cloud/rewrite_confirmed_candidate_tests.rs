use super::*;
use crate::core::{BaseMode,SourceCorrection};
use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::TcpListener};

fn fixture()->(ProviderConfig,ModeSnapshot,SourceRevision,ConfirmedContext) {
    let mut config=ProviderConfig::default();config.region=Some("beijing".into());
    let mode=ModeSnapshot{mode_id:"light".into(),name:"light".into(),base_mode:BaseMode::Light,rules:String::new(),revision:1};
    let source=SourceRevision{id:"source".into(),session_id:"session".into(),asr_attempt_id:"attempt".into(),generation:1,provider:"synthetic".into(),model:"fixed".into(),adapter_version:"v1".into(),complete:true,created_at:"now".into(),items:vec![
        SourceItem{id:"a".into(),provider_item_id:"0".into(),capture_order:0,text:"🙂 I work on graph rock. I will graduate in twenty thirty.".into(),is_final:true,start_ms:None,end_ms:None},
        SourceItem{id:"b".into(),provider_item_id:"1".into(),capture_order:1,text:"Three. I have not released it.".into(),is_final:true,start_ms:None,end_ms:None}]};
    let text=source.text();
    let corrections=[("graph rock","GraphRAG"),("twenty thirty.\nThree","2033")].into_iter().map(|(old,new)|{
        let start=text[..text.find(old).unwrap()].chars().count();SourceCorrection{start,end:start+old.chars().count(),expected_text:old.into(),replacement:new.into()}
    }).collect();
    let context=ConfirmedContext{source_revision_id:source.id.clone(),language:DraftLanguage::English,corrections};
    (config,mode,source,context)
}

#[test]
fn exact_cross_fragment_overlay_is_occurrence_bound_and_leaves_source_and_dictionary_unchanged() {
    let (_,_,source,context)=fixture();let frozen=source.clone();
    assert_eq!(context.reference_text(&source).unwrap(),"🙂 I work on GraphRAG. I will graduate in 2033. I have not released it.");
    let reference=reference(&context,&source).unwrap();assert_ne!(reference.id,source.id);assert_eq!(source,frozen);
    let mut repeated=source.clone();repeated.items[0].text.push_str(" Other graph rock remains uncertain.");
    let mut one=context.clone();one.corrections.truncate(1);
    assert!(one.reference_text(&repeated).unwrap().contains("Other graph rock remains uncertain"));
    assert!(!confirmations_present(&context,"GraphRAG, 2030"));
    let mut twice=context.clone();twice.corrections.push(context.corrections[0].clone());
    assert!(!confirmations_present(&twice,"GraphRAG, 2033"));
}

#[test]
fn stale_overlapping_empty_unbounded_or_hidden_corrections_are_rejected_before_requests() {
    let (_,_,source,context)=fixture();
    for index in 0..11 {
        let mut c=context.clone();
        match index {
            0=>c.source_revision_id="foreign".into(),1=>c.corrections[0].expected_text="invented".into(),
            2=>c.corrections[0].start+=1,3=>c.corrections.push(c.corrections[0].clone()),
            4=>c.corrections[0].end=usize::MAX,5=>c.corrections[0].replacement="\nGraphRAG".into(),
            6=>c.corrections[0].replacement="Graph\u{200b}RAG".into(),7=>c.corrections[0].replacement="x".repeat(241),
            8=>c.corrections=vec![c.corrections[0].clone();9],9=>c.corrections[0].replacement="".into(),_=>c.corrections[0].replacement="Graph\u{ad}RAG".into(),
        }
        assert!(validate(&c,&source).is_err(),"{index}");
    }
    let mut incomplete=source;incomplete.complete=false;assert!(validate(&context,&incomplete).is_err());
}

async fn server(contents:Vec<String>,delay:Duration)->(String,tokio::task::JoinHandle<Vec<Value>>) {
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let endpoint=format!("http://{}/test",listener.local_addr().unwrap());
    let task=tokio::spawn(async move {
        let mut requests=Vec::new();
        for content in contents {
            let (mut socket,_)=listener.accept().await.unwrap();let mut bytes=Vec::new();let mut chunk=[0;4096];
            loop {let n=socket.read(&mut chunk).await.unwrap();if n==0{return requests;}bytes.extend_from_slice(&chunk[..n]);
                if let Some(index)=bytes.windows(4).position(|w|w==b"\r\n\r\n") {
                    let header=String::from_utf8_lossy(&bytes[..index]);let length=header.lines().find_map(|line|line.split_once(':').filter(|(k,_)|k.eq_ignore_ascii_case("content-length")).map(|(_,v)|v.trim().parse::<usize>().unwrap())).unwrap();
                    if bytes.len()>=index+4+length{requests.push(serde_json::from_slice(&bytes[index+4..index+4+length]).unwrap());break;}
                }
            }
            tokio::time::sleep(delay).await;
            let body=json!({"choices":[{"finish_reason":"stop","message":{"content":content}}]}).to_string();
            let response=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
            let _=socket.write_all(response.as_bytes()).await;
        }
        requests
    });(endpoint,task)
}
fn draft(text:&str)->String{json!({"resolvedMode":"light","text":text}).to_string()}
fn verdict(accept:bool)->String{json!({"decision":if accept{"accept"}else{"reject"},"checks":{"coverage":"pass","facts":if accept{"pass"}else{"uncertain"},"scope":"pass","intent":"pass","cleanup":"pass"},"issues":[]}).to_string()}

#[tokio::test]
async fn generation_and_review_share_corrected_reference_but_output_keeps_real_source_refs() {
    let (config,mode,source,context)=fixture();let text=context.reference_text(&source).unwrap();let frozen=source.clone();
    let (endpoint,task)=server(vec![draft(&text),verdict(true)],Duration::ZERO).await;
    let outcome=run(shared_client().unwrap(),&config,"synthetic",&mode,&source,&DictionarySnapshot::default(),&context,CancellationToken::new(),&endpoint,Duration::from_secs(2)).await;
    let candidate=outcome.result.unwrap();assert!(candidate.review_issues.is_empty());assert_eq!(outcome.trace.calls,2);
    assert_eq!(candidate.draft.blocks[0].source_refs,vec!["a","b"]);assert_eq!(source,frozen);
    for request in task.await.unwrap(){let data:Value=serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();assert_eq!(data["sourceText"],text);assert_eq!(data["confirmedSpellings"],json!(["GraphRAG","2033"]));}
}

#[tokio::test]
async fn reviewer_accept_cannot_hide_a_detected_tense_change() {
    let (config,mode,source,context)=fixture();let text=context.reference_text(&source).unwrap().replace("I work on","I worked on");
    let (endpoint,task)=server(vec![draft(&text),verdict(true)],Duration::ZERO).await;
    let outcome=run(shared_client().unwrap(),&config,"synthetic",&mode,&source,&DictionarySnapshot::default(),&context,CancellationToken::new(),&endpoint,Duration::from_secs(2)).await;
    assert_eq!(outcome.result.unwrap().review_issues,vec![ManualReviewIssue::TenseChanged]);assert_eq!(outcome.trace.calls,2);assert!(outcome.trace.review.unwrap().accepted());task.await.unwrap();
}

#[tokio::test]
async fn equivalent_spoken_year_and_money_reach_full_review() {
    let (config,mode,mut source,mut context)=fixture();source.items.truncate(1);
    source.items[0].text="I expect to graduate in twenty twenty seven. My budget is thirty dollars, not thirteen dollars.".into();
    context.corrections.clear();
    let text="I expect to graduate in 2027. My budget is 30 dollars, not 13 dollars.";
    let (endpoint,task)=server(vec![draft(text),verdict(true)],Duration::ZERO).await;
    let outcome=run(shared_client().unwrap(),&config,"synthetic",&mode,&source,&DictionarySnapshot::default(),&context,CancellationToken::new(),&endpoint,Duration::from_secs(2)).await;
    assert!(outcome.result.unwrap().review_issues.is_empty());assert_eq!(outcome.trace.calls,2);task.await.unwrap();
}

#[tokio::test]
async fn unchanged_good_prose_survives_reviewer_rejection_and_unknown_review_shape() {
    for review in [verdict(false),"{}".into()] {
        let (config,mode,source,context)=fixture();let text=context.reference_text(&source).unwrap();
        let (endpoint,task)=server(vec![draft(&text),review],Duration::ZERO).await;
        let outcome=run(shared_client().unwrap(),&config,"synthetic",&mode,&source,&DictionarySnapshot::default(),&context,CancellationToken::new(),&endpoint,Duration::from_secs(2)).await;
        let candidate=outcome.result.unwrap();assert_eq!(candidate.draft.blocks[0].text,text);assert_eq!(candidate.review_issues.len(),1);assert_eq!(outcome.trace.calls,2);task.await.unwrap();
    }
}

#[tokio::test]
async fn changed_confirmation_and_negation_never_get_a_clean_verdict() {
    for wrong in ["🙂 I work on GraphRAG. I will graduate in 2030. I have not released it.","🙂 I work on GraphRAG. I will graduate in 2033. I have released it."] {
        let (config,mode,source,context)=fixture();let (endpoint,task)=server(vec![draft(wrong)],Duration::ZERO).await;
        let outcome=run(shared_client().unwrap(),&config,"synthetic",&mode,&source,&DictionarySnapshot::default(),&context,CancellationToken::new(),&endpoint,Duration::from_secs(2)).await;
        assert!(!outcome.result.unwrap().review_issues.is_empty());assert_eq!(outcome.trace.calls,1);task.await.unwrap();
    }
}

#[tokio::test]
async fn cancellation_stale_context_and_deadline_return_no_candidate() {
    let (config,mode,source,mut context)=fixture();
    context.source_revision_id="old".into();
    let result=run(shared_client().unwrap(),&config,"synthetic",&mode,&source,&DictionarySnapshot::default(),&context,CancellationToken::new(),"http://127.0.0.1:1",Duration::from_secs(1)).await;
    assert_eq!(result.trace.calls,0);assert!(result.result.is_err());context.source_revision_id=source.id.clone();
    let cancel=CancellationToken::new();cancel.cancel();
    let result=run(shared_client().unwrap(),&config,"synthetic",&mode,&source,&DictionarySnapshot::default(),&context,cancel,"http://127.0.0.1:1",Duration::from_secs(1)).await;
    assert_eq!(result.result.err().unwrap().code,"cancelled");assert_eq!(result.trace.calls,0);
    let (endpoint,task)=server(vec![draft(&context.reference_text(&source).unwrap())],Duration::from_millis(100)).await;
    let result=run(shared_client().unwrap(),&config,"synthetic",&mode,&source,&DictionarySnapshot::default(),&context,CancellationToken::new(),&endpoint,Duration::from_millis(20)).await;
    assert_eq!(result.result.err().unwrap().code,"rewrite_timeout");task.abort();
}
