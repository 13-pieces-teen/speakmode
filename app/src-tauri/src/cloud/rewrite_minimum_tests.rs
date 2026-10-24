use super::*;
use reqwest::Client;
use crate::core::{BaseMode, SourceItem};
use serde_json::{json,Value};
use std::sync::{Arc, Mutex};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener, sync::Notify, task::JoinHandle};
#[path="rewrite_language_tests.rs"]
mod language_tests;
#[path="rewrite_writing_context_tests.rs"]
mod writing_context_tests;
#[path="rewrite_personalization_lab_tests.rs"]
mod personalization_lab_tests;
#[path="rewrite_personalization_quality_regression_tests.rs"]
mod personalization_quality_regression_tests;

const ACCEPT: &str = r#"{"decision":"accept","checks":{"coverage":"pass","facts":"pass","scope":"pass","intent":"pass","cleanup":"pass"},"issues":[]}"#;

fn fixture(text: &str) -> (ProviderConfig, ModeSnapshot, SourceRevision) {
    let mut config = ProviderConfig::default(); config.region = Some("beijing".into());
    (config, ModeSnapshot { mode_id: "auto".into(), name: "自动".into(), base_mode: BaseMode::Auto, rules: String::new(), revision: 1 },
        SourceRevision { id: "source".into(), session_id: "session".into(), asr_attempt_id: "attempt".into(),
            generation: 1, provider: "synthetic".into(), model: "fixed".into(), adapter_version: "test".into(),
            complete: true, created_at: "now".into(), items: vec![SourceItem { id: "item-0".into(), provider_item_id: "0".into(),
                capture_order: 0, text: text.into(), is_final: true, start_ms: None, end_ms: None }] })
}

#[derive(Clone)]
pub(crate) struct Reply { content: String, finish: &'static str, status: u16, delay: Duration }
impl Reply {
    pub(crate) fn new(content: impl Into<String>) -> Self { Self { content: content.into(), finish: "stop", status: 200, delay: Duration::ZERO } }
    fn draft(text: &str) -> Self { Self::new(json!({"resolvedMode":"light","text":text}).to_string()) }
    pub(crate) fn after(mut self, millis: u64) -> Self { self.delay = Duration::from_millis(millis); self }
}
pub(crate) struct Server { pub(crate) endpoint: String, requests: Arc<Mutex<Vec<Value>>>, arrived: Arc<Notify>, task: JoinHandle<()> }
impl Drop for Server { fn drop(&mut self) { self.task.abort(); } }
impl Server {
    pub(crate) async fn new(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/test", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new())); let seen = requests.clone();
        let arrived = Arc::new(Notify::new()); let signal = arrived.clone();
        let task = tokio::spawn(async move {
            for reply in replies {
                let Ok((mut socket, _)) = listener.accept().await else { return; };
                let mut bytes = Vec::new(); let mut chunk = [0; 4096];
                let (header_end, length) = loop {
                    let Ok(size) = socket.read(&mut chunk).await else { return; }; if size == 0 { return; }
                    bytes.extend_from_slice(&chunk[..size]); assert!(bytes.len() <= 2 * 1024 * 1024);
                    if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..index]);
                        let length = header.lines().find_map(|line| line.split_once(':')
                            .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>().unwrap())).unwrap();
                        break (index + 4, length);
                    }
                };
                while bytes.len() < header_end + length {
                    let Ok(size) = socket.read(&mut chunk).await else { return; }; if size == 0 { return; }
                    bytes.extend_from_slice(&chunk[..size]);
                }
                let body: Value = serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                let response_content = if reply.content == "__ECHO_GUARDED_SOURCE__" {
                    let data:Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
                    json!({"resolvedMode":"light","text":data["sourceText"]}).to_string()
                } else { reply.content.clone() };
                seen.lock().unwrap().push(body); signal.notify_one();
                tokio::time::sleep(reply.delay).await;
                let body = json!({"choices":[{"finish_reason":reply.finish,"message":{"content":response_content}}]}).to_string();
                let header = format!("HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.status, body.len());
                if socket.write_all(header.as_bytes()).await.is_err() { return; }
                if socket.write_all(body.as_bytes()).await.is_err() { return; }
            }
        });
        Self { endpoint, requests, arrived, task }
    }
    pub(crate) fn count(&self) -> usize { self.requests.lock().unwrap().len() }
    async fn wait_for(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.count() < count { self.arrived.notified().await; }
        }).await.expect("local request should arrive");
    }
}

pub(crate) fn rejection(source:&str,draft:&str,dimension:&str)->String {
    let mut value:Value=serde_json::from_str(ACCEPT).unwrap();value["decision"]=json!("reject");value["checks"][dimension]=json!("fail");
    value["issues"]=json!([{"dimension":dimension,"sourceQuote":source,"draftQuote":draft,"reason":"局部搭配不自然"}]);value.to_string()
}
fn client()->Client{Client::builder().redirect(reqwest::redirect::Policy::none()).no_proxy().build().unwrap()}
async fn test_run(server:&Server,input:&str,wait:Duration)->Outcome{
    let(config,mode,source)=fixture(input);
    execute(&client(),&config,"synthetic-secret",&mode,&source,&DictionarySnapshot::default(),CancellationToken::new(),&server.endpoint,wait).await
}
#[tokio::test]
async fn ordered_review_runs_through_real_http_without_bypassing_review(){
    let input="请先检查麦克风，暂时不要发布。";
    let ordered=r#"{"decision":"accept","checks":["pass","pass","pass","pass","pass"],"issues":[]}"#;
    let server=Server::new(vec![Reply::draft(input),Reply::new(ordered)]).await;
    let(config,mode,source)=fixture(input);let before=source.clone();
    let out=execute_until_with_review_format(&client(),&config,"synthetic-key",&mode,&source,&DictionarySnapshot::default(),
        CancellationToken::new(),&server.endpoint,RewriteExecution::for_duration(Duration::from_secs(4)),None,None,Some(semantic_review::MinimumReviewFormat::Ordered)).await;
    assert!(out.result.is_ok());assert_eq!(out.trace.calls,2);assert_eq!(server.count(),2);assert_eq!(source,before);
    let review=out.trace.review.unwrap();
    assert_eq!(review.protocol_version,semantic_review::ORDERED_REVIEW_VERSION);
    assert_eq!(review.reviewer_wire_protocol_version,semantic_review::ORDERED_REVIEW_VERSION);
}
#[tokio::test]
async fn writing_contract_label_does_not_hide_named_or_ordered_reviewer_wire_metadata(){
    let input="请先检查麦克风，暂时不要发布。";
    for format in [semantic_review::MinimumReviewFormat::Named,semantic_review::MinimumReviewFormat::Ordered] {
        let expected=if format==semantic_review::MinimumReviewFormat::Ordered {
            semantic_review::ORDERED_REVIEW_VERSION
        }else{semantic_review::MINIMUM_REVIEW_VERSION};
        for accept in [true,false] {
            let mut value:Value=serde_json::from_str(ACCEPT).unwrap();
            let scope=if accept {"pass"}else{"fail"};
            value["decision"]=json!(if accept {"accept"}else{"reject"});value["checks"]["scope"]=json!(scope);
            if format==semantic_review::MinimumReviewFormat::Ordered {
                value["checks"]=json!(["pass","pass",scope,"pass","pass"]);
            }
            let server=Server::new(vec![Reply::draft(input),Reply::new(value.to_string())]).await;
            let (config,mut mode,source)=fixture(input);mode.mode_id="email".into();
            let context=super::super::writing_context::for_mode(&mode).unwrap();
            let before=source.clone();
            let out=execute_until_with_review_format(&client(),&config,"synthetic-key",&mode,&source,&DictionarySnapshot::default(),
                CancellationToken::new(),&server.endpoint,RewriteExecution::for_duration(Duration::from_secs(4)),None,Some(&context),Some(format)).await;
            assert_eq!(out.result.is_ok(),accept);assert_eq!(out.trace.calls,2);assert_eq!(server.count(),2);
            assert!(!out.trace.repair_attempted);assert_eq!(source,before);
            if !accept {assert_eq!(out.result.as_ref().err().unwrap().code,"rewrite_semantic_rejected");}
            let serialized=serde_json::to_value(&out.trace).unwrap();
            assert_eq!(serialized["review"]["protocolVersion"],super::super::writing_context::VERSION);
            assert_eq!(serialized["review"]["reviewerWireProtocolVersion"],expected);
            let requests=server.requests.lock().unwrap();
            assert!(requests[1]["messages"][0]["content"].as_str().unwrap().contains(expected));
        }
    }
}
#[tokio::test]
async fn malformed_ordered_review_never_authorizes_a_draft(){
    let input="请先检查麦克风，暂时不要发布。";
    for content in [r#"{"decision":"accept","checks":["pass","pass","pass","pass"],"issues":[]}"#,
        r#"{"decision":"accept","checks":["pass","fail","pass","pass","pass"],"issues":[]}"#] {
        let server=Server::new(vec![Reply::draft(input),Reply::new(content)]).await;let(config,mode,source)=fixture(input);
        let out=execute_until_with_review_format(&client(),&config,"synthetic-key",&mode,&source,&DictionarySnapshot::default(),
            CancellationToken::new(),&server.endpoint,RewriteExecution::for_duration(Duration::from_secs(4)),None,None,Some(semantic_review::MinimumReviewFormat::Ordered)).await;
        assert!(out.result.is_err());assert_eq!(out.trace.calls,2);assert_eq!(server.count(),2);assert!(!out.trace.repair_attempted);
    }
}
#[test]
fn all_modes_share_cleanup_and_explicit_layout_cannot_be_rerouted(){
    let(config,mut mode,source)=fixture("嗯，第一点网络，呃，第二点麦克风。");
    for base in [crate::core::BaseMode::Auto,crate::core::BaseMode::Light,crate::core::BaseMode::Structure]{
        mode.base_mode=base;mode.rules="短句，不新增信息".into();
        let request=minimum::prepare(&config,&mode,&source,&DictionarySnapshot::default(),true).unwrap();
        let data:Value=serde_json::from_str(request.body["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(data["baseMode"],json!(base));assert_eq!(data["formatRules"],mode.rules);
        let output=json!({"resolvedMode":if base==crate::core::BaseMode::Auto{crate::core::BaseMode::Light}else{base},"text":"检查网络和麦克风。"});
        let draft=minimum::parse(&output.to_string(),&mode,&source,&request).unwrap();assert_eq!(draft.blocks[0].source_refs,["item-0"]);
        let mut changed=mode.clone();changed.revision+=1;
        assert_eq!(minimum::parse(&output.to_string(),&changed,&source,&request).err().unwrap().code,"stale_rewrite");
        if base!=crate::core::BaseMode::Auto{let mut wrong=output.clone();wrong["resolvedMode"]=json!(if base==crate::core::BaseMode::Light{"structure"}else{"light"});assert!(minimum::parse(&wrong.to_string(),&mode,&source,&request).is_err());}
    }
}
#[tokio::test]
async fn successful_local_repair_changes_only_selected_sentence_and_requires_fresh_full_review(){
    let input="我感觉这个方案还没有太能做到。预算3500元，先不要发送。";
    let bad="我感觉这个方案还没有太能做到。";
    let repaired="我感觉这个方案还没有很好地做到。";
    let server=Server::new(vec![Reply::draft(input),Reply::new(rejection(bad,bad,"cleanup")),
        Reply::new(json!({"replacements":[{"id":"r0","text":repaired}]}).to_string()),Reply::new(ACCEPT)]).await;
    let out=test_run(&server,input,Duration::from_secs(4)).await;
    assert_eq!(server.count(),4);assert!(out.trace.repair_attempted);
    assert_eq!(out.result.unwrap().blocks[0].text,format!("{repaired}预算3500元，先不要发送。"));
    let calls=server.requests.lock().unwrap();
    let final_data:Value=serde_json::from_str(calls[3]["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(final_data["sourceText"],input);assert!(final_data["draftText"].as_str().unwrap().starts_with(repaired));
}
#[tokio::test]
async fn fidelity_rejection_and_insufficient_budget_never_start_repair(){
    let input="我感觉方案还没有太能做到。";
    for(dim,wait)in[("facts",Duration::from_secs(4)),("cleanup",Duration::from_secs(1))]{
        let server=Server::new(vec![Reply::draft(input),Reply::new(rejection(input,input,dim))]).await;
        let out=test_run(&server,input,wait).await;assert!(out.result.is_err());assert_eq!(server.count(),2);assert!(!out.trace.repair_attempted);
    }
}
#[test]
fn repair_rejects_stale_objects_unknown_ids_and_ambiguous_ranges(){
    let(config,mode,source)=fixture("方案没有太能做到。预算3500元。");
    let request=minimum::prepare(&config,&mode,&source,&DictionarySnapshot::default(),true).unwrap();
    let draft=minimum::parse(&json!({"resolvedMode":"light","text":source.text()}).to_string(),&mode,&source,&request).unwrap();
    let request=semantic_review::prepare_minimum(&config,&mode,&source,&draft).unwrap();
    let review=semantic_review::parse_minimum(&rejection("方案没有太能做到。","方案没有太能做到。","cleanup"),&request,&mode,&source,&draft).unwrap();
    let repair=local_repair::prepare(&config,&mode,&source,&draft,&review).unwrap().unwrap();
    assert!(repair.body["messages"][0]["content"].as_str().unwrap().to_ascii_lowercase().contains("json"),"provider JSON-mode requires the keyword in messages");
    for bad in ["{}",r#"{"replacements":[{"id":"r1","text":"x"}]}"#,r#"{"replacements":[{"id":"r0","text":"x"},{"id":"r0","text":"y"}]}"#]{assert!(local_repair::apply(bad,&repair,&mode,&source,&draft).is_err());}
    let wire=json!({"replacements":[{"id":"r0","text":"方案还没有很好地做到。"}]}).to_string();
    let mut changed=draft.clone();changed.blocks[0].text.push('。');assert!(local_repair::apply(&wire,&repair,&mode,&source,&changed).is_err());
    let mut changed=mode.clone();changed.revision+=1;assert!(local_repair::apply(&wire,&repair,&changed,&source,&draft).is_err());
    assert!(semantic_review::parse_minimum(ACCEPT,&request,&changed,&source,&draft).is_err());
    let mut repeated=draft.clone();repeated.blocks[0].text.push_str("方案没有太能做到。");
    let request=semantic_review::prepare_minimum(&config,&mode,&source,&repeated).unwrap();
    let review=semantic_review::parse_minimum(&rejection("方案没有太能做到。","方案没有太能做到。","cleanup"),&request,&mode,&source,&repeated).unwrap();
    assert!(local_repair::prepare(&config,&mode,&source,&repeated,&review).unwrap().is_none());
}
#[tokio::test]
async fn repair_cannot_change_literal_or_number_and_rejection_never_retries(){
    let input="方案没有太能做到，预算3500元。";
    for change in ["方案还没有很好地做到，预算5000元。","方案还没有很好地做到，预算3500元。"]{
        let server=Server::new(vec![Reply::draft(input),Reply::new(rejection(input,input,"cleanup")),
            Reply::new(json!({"replacements":[{"id":"r0","text":change}]}).to_string()),Reply::new(rejection(input,change,"cleanup"))]).await;
        let out=test_run(&server,input,Duration::from_secs(4)).await;assert!(out.result.is_err());
        assert_eq!(server.count(),if change.contains("5000"){3}else{4});
    }
}
#[tokio::test]
async fn absolute_deadline_and_cancellation_cover_every_stage(){
    let input="方案没有太能做到。";
    let server=Server::new(vec![Reply::draft(input).after(350),Reply::new(ACCEPT).after(350)]).await;
    let began=Instant::now();let out=test_run(&server,input,Duration::from_millis(600)).await;
    assert_eq!(out.result.err().unwrap().code,"rewrite_timeout");assert!(began.elapsed()<Duration::from_millis(1100));
    for stop_after in [1,2,3,4]{
        let server=Server::new(vec![Reply::draft(input).after(if stop_after==1{2000}else{0}),
            Reply::new(rejection(input,input,"cleanup")).after(if stop_after==2{2000}else{0}),
            Reply::new(json!({"replacements":[{"id":"r0","text":"方案还没有很好地做到。"}]}).to_string()).after(if stop_after==3{2000}else{0}),
            Reply::new(ACCEPT).after(2000)]).await;
        let(config,mode,source)=fixture(input);let endpoint=server.endpoint.clone();let cancel=CancellationToken::new();let stop=cancel.clone();
        let handle=tokio::spawn(async move{execute(&client(),&config,"secret",&mode,&source,&DictionarySnapshot::default(),stop,&endpoint,Duration::from_secs(5)).await});
        server.wait_for(stop_after).await;cancel.cancel();let out=handle.await.unwrap();
        assert_eq!(out.result.err().unwrap().code,"cancelled");assert_eq!(server.count(),stop_after);
    }
}

#[tokio::test]
async fn shared_long_request_restores_protected_literals_before_full_review(){
    let input=format!("请逐字保留‘嗯，这个，等我想一下’及 `userID`。S V I D。{}","描述当前流程。".repeat(110));
    let server=Server::new(vec![Reply::new("__ECHO_GUARDED_SOURCE__"),Reply::new(ACCEPT)]).await;
    let out=test_run(&server,&input,Duration::from_secs(4)).await;assert_eq!(out.result.unwrap().blocks[0].text,input);
    assert_eq!(server.count(),2);let requests=server.requests.lock().unwrap();
    let reviewed:Value=serde_json::from_str(requests[1]["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(reviewed["draftText"],input);assert!(!reviewed["draftText"].as_str().unwrap().contains("⟪SM"));
}

#[tokio::test]
async fn accepted_reviewer_cannot_skip_clear_stutter_hint_or_authorize_unchanged_repair(){
    let input="嗯嗯，呃呃，就是，就是我我觉得这个方案布局太松了。预算3500元，先不要发送。";
    for replacement in ["我觉得这个方案布局太松了。","嗯嗯，呃呃，就是，就是我我觉得这个方案布局太松了。"]{
        let server=Server::new(vec![Reply::draft(input),Reply::new(ACCEPT),Reply::new(json!({"replacements":[{"id":"r0","text":replacement}]}).to_string()),Reply::new(ACCEPT)]).await;
        let out=test_run(&server,input,Duration::from_secs(4)).await;assert!(out.trace.repair_attempted);
        if replacement.starts_with("嗯嗯"){assert!(out.result.is_err());assert_eq!(server.count(),3);}
        else{assert_eq!(out.result.unwrap().blocks[0].text,"我觉得这个方案布局太松了。预算3500元，先不要发送。");assert_eq!(server.count(),3);assert!(out.trace.native_deletion_verified);}
    }
}
#[test]
fn native_residue_hint_never_edits_quotes_discussed_words_or_ambiguous_source(){
    for input in ["“嗯嗯，呃呃，就是，就是我我觉得这个方案布局太松了。”", "嗯嗯，呃呃，我我觉得这个词必须逐字保留。", "嗯，我觉得方案有点松。"]{
        let(config,mode,source)=fixture(input);let request=minimum::prepare(&config,&mode,&source,&DictionarySnapshot::default(),false).unwrap();
        let draft=minimum::parse(&json!({"resolvedMode":"light","text":input}).to_string(),&mode,&source,&request).unwrap();
        assert!(local_repair::residue_hint(&source,&draft).is_none());
    }
}
#[test]
fn native_deletion_cannot_accept_facts_opinion_or_bytes_outside_the_known_prelude_changes(){
    let input="嗯嗯，呃呃，就是，就是我我觉得这个方案布局太松了。预算3500元，先不要发送。";
    let(config,mode,source)=fixture(input);let request=minimum::prepare(&config,&mode,&source,&DictionarySnapshot::default(),false).unwrap();
    let draft=minimum::parse(&json!({"resolvedMode":"light","text":input}).to_string(),&mode,&source,&request).unwrap();
    let request=semantic_review::prepare_minimum(&config,&mode,&source,&draft).unwrap();
    let review=semantic_review::parse_minimum(ACCEPT,&request,&mode,&source,&draft).unwrap();
    for text in ["我觉得这个方案布局太松了。预算3500元，先不要发送。","这个方案布局太松了。预算3500元，先不要发送。","我觉得这个方案布局太松了。预算5000元，先不要发送。","我觉得这个方案布局太松了。预算3500元，先发送。"]{
        let mut repaired=draft.clone();repaired.blocks[0].text=text.into();
        assert_eq!(local_repair::native_deletion_verified(&source,&draft,&repaired,&review),text.starts_with("我觉得")&&text.contains("3500")&&text.contains("不要"));
    }
    let mut repaired=draft.clone();repaired.blocks[0].text="我觉得这个方案布局太松了。预算3500元，先不要发送。".into();
    repaired.blocks[0].id="other".into();assert!(!local_repair::native_deletion_verified(&source,&draft,&repaired,&review));
}
