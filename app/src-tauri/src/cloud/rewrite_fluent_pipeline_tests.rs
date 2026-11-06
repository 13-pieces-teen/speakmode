use super::*;
use crate::core::{BaseMode, SourceItem};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener, sync::Notify, task::JoinHandle};

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
struct Reply { content: String, finish: &'static str, status: u16, delay: Duration }
impl Reply {
    fn new(content: impl Into<String>) -> Self { Self { content: content.into(), finish: "stop", status: 200, delay: Duration::ZERO } }
    fn draft(text: &str) -> Self { Self::new(json!({"resolvedMode":"light","text":text}).to_string()) }
    fn after(mut self, millis: u64) -> Self { self.delay = Duration::from_millis(millis); self }
}
struct Server { endpoint: String, requests: Arc<Mutex<Vec<Value>>>, arrived: Arc<Notify>, task: JoinHandle<()> }
impl Drop for Server { fn drop(&mut self) { self.task.abort(); } }
impl Server {
    async fn new(replies: Vec<Reply>) -> Self {
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
    fn count(&self) -> usize { self.requests.lock().unwrap().len() }
    async fn wait_for(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.count() < count { self.arrived.notified().await; }
        }).await.expect("local request should arrive");
    }
}

#[tokio::test]
async fn long_pipeline_restores_literal_and_terms_before_frozen_review() {
    let input = format!("请逐字保留‘嗯，这个，等我想一下’及 `userID`。S V I D。{}", "描述当前流程。".repeat(110));
    let server = Server::new(vec![Reply::new("__ECHO_GUARDED_SOURCE__"),Reply::new(ACCEPT)]).await;
    let (config,mode,source) = fixture(&input); let original = source.clone();
    let output = run(&client(), &config, "synthetic-secret", &mode, &source, &DictionarySnapshot::default(),
        CancellationToken::new(), &server.endpoint, Duration::from_secs(3)).await.unwrap();
    assert_eq!(source,original); assert_eq!(server.count(),2); assert_eq!(output.blocks[0].text,input);
    let requests=server.requests.lock().unwrap();
    assert!(requests[0]["messages"][0]["content"].as_str().unwrap().contains("protected-long-v3"));
    let review:Value=serde_json::from_str(requests[1]["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(review["draftText"],input); assert!(!review["draftText"].as_str().unwrap().contains("⟪SM"));
}

#[tokio::test]
async fn local_literal_guard_cannot_be_overridden_by_an_accepting_reviewer() {
    let server = Server::new(vec![Reply::draft("引用‘等我想一下’。"),Reply::new(ACCEPT)]).await;
    let error = execute(&server,"引用‘嗯，这个，等我想一下’。",Duration::from_secs(3)).await.unwrap_err();
    assert_eq!(error.code,"rewrite_literal_changed"); assert_eq!(server.count(),1);
}

#[tokio::test]
async fn missing_long_protection_is_rejected_before_review_and_never_leaks_markers() {
    let source=format!("使用 SVID。{}", "检查流程。".repeat(170));
    let server=Server::new(vec![Reply::draft("检查流程。"),Reply::new(ACCEPT)]).await;
    let error=execute(&server,&source,Duration::from_secs(3)).await.unwrap_err();
    assert_eq!(error.code,"protected_terms_missing"); assert_eq!(server.count(),1);
    assert!(!error.message.contains("SVID")); assert!(!error.message.contains("⟪SM"));
}

#[tokio::test]
async fn short_dictionary_normalization_is_frozen_before_review_and_excludes_quoted_terms() {
    let input="S V I D 来自 spiffe，原句是‘S V I D’。";
    let server=Server::new(vec![Reply::draft(input),Reply::new(ACCEPT)]).await;
    let (config,mode,source)=fixture(input);
    let dictionary=DictionarySnapshot {version:8,entries:["SVID","SPIFFE"].into_iter().enumerate().map(|(i,term)|crate::core::DictionaryEntry {
        id:i.to_string(),term:term.into(),created_at:String::new(),updated_at:String::new()}).collect()};
    let output=run(&client(),&config,"synthetic-secret",&mode,&source,&dictionary,CancellationToken::new(),&server.endpoint,Duration::from_secs(3)).await.unwrap();
    assert_eq!(output.blocks[0].text,"SVID 来自 SPIFFE，原句是‘S V I D’。");
    assert_eq!(source.text(),input);
    let requests=server.requests.lock().unwrap();
    let review:Value=serde_json::from_str(requests[1]["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(review["draftText"],output.blocks[0].text);
}

#[test]
fn longer_english_prose_remains_editable_and_unicode_budget_boundaries_are_explicit() {
    let (_,_,mut source)=fixture(&"I would like to explain the current flow. ".repeat(30));
    assert!(!uses_guarded_long(&source));
    source.items[0].text="字".repeat(400);assert!(!uses_guarded_long(&source));assert_eq!(super::super::initial_wait_ms(&source),5000);
    source.items[0].text.push('字');assert!(uses_guarded_long(&source));assert_eq!(super::super::initial_wait_ms(&source),8000);
    source.items[0].text="字".repeat(801);assert_eq!(super::super::initial_wait_ms(&source),20000);
}
fn client() -> Client { Client::builder().redirect(reqwest::redirect::Policy::none()).no_proxy().build().unwrap() }
async fn execute(server: &Server, source: &str, timeout: Duration) -> Result<RewriteResult, CloudError> {
    let (config, mode, source) = fixture(source);
    run(&client(), &config, "synthetic-secret", &mode, &source, &DictionarySnapshot::default(),
        CancellationToken::new(), &server.endpoint, timeout).await
}

#[tokio::test]
async fn fluent_pipeline_accepts_only_after_two_calls_and_preserves_the_frozen_draft() {
    let server = Server::new(vec![Reply::draft("测试工具的运行形式。"), Reply::new(ACCEPT)]).await;
    let (config, mode, source) = fixture("嗯，测试一个这样的一个工具的运行形式。 ");
    let before = source.clone();
    let output = run(&client(), &config, "synthetic-secret", &mode, &source, &DictionarySnapshot::default(),
        CancellationToken::new(), &server.endpoint, Duration::from_secs(3)).await.unwrap();
    assert_eq!(server.count(), 2); assert_eq!(source, before);
    assert_eq!(output.blocks[0].text, "测试工具的运行形式。");
    assert_eq!(output.blocks[0].source_refs, vec!["item-0"]);
    let requests = server.requests.lock().unwrap();
    let review_data: Value = serde_json::from_str(requests[1]["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(review_data["sourceText"], source.text());
    assert_eq!(review_data["draftText"], output.blocks[0].text);
    assert!(!review_data.to_string().contains("sha256"));
}

#[tokio::test]
async fn fluent_pipeline_shape_or_hard_admission_failure_never_calls_review() {
    for (source, reply, code) in [
        ("测试工具。", Reply::new("{}"), "invalid_output"),
        ("预算3000元。", Reply::draft("预算5000元。"), "rewrite_admission_rejected"),
        ("不要发送邀请。", Reply::draft("发送邀请。"), "rewrite_admission_rejected"),
    ] {
        let server = Server::new(vec![reply]).await;
        let error = execute(&server, source, Duration::from_secs(3)).await.err().unwrap();
        assert_eq!(error.code, code); assert_eq!(error.stage, "rewrite"); assert_eq!(server.count(), 1);
        assert!(!error.message.contains(source));
        assert!(!error.message.contains("3000")); assert!(!error.message.contains("5000"));
    }
}

#[tokio::test]
async fn fluent_pipeline_review_rejection_and_invalid_review_map_to_existing_rewrite_failures() {
    let mut rejected: Value = serde_json::from_str(ACCEPT).unwrap();
    rejected["decision"] = json!("reject"); rejected["checks"]["coverage"] = json!("fail");
    for (review, code) in [(rejected.to_string(), "rewrite_semantic_rejected"), ("{}".into(), "invalid_output")] {
        let server = Server::new(vec![Reply::draft("测试工具。"), Reply::new(review)]).await;
        let error = execute(&server, "嗯，测试工具。", Duration::from_secs(3)).await.err().unwrap();
        assert_eq!(error.code, code); assert_eq!(error.stage, "rewrite"); assert_eq!(server.count(), 2);
        assert!(!error.message.contains("测试工具"));
    }
}

#[tokio::test]
async fn fluent_pipeline_total_deadline_is_not_restarted_for_review() {
    let server = Server::new(vec![Reply::draft("测试工具。").after(350), Reply::new(ACCEPT).after(350)]).await;
    let began = Instant::now();
    let error = execute(&server, "嗯，测试工具。", Duration::from_millis(600)).await.err().unwrap();
    assert_eq!(error.code, "rewrite_timeout"); assert_eq!(error.stage, "rewrite");
    assert_eq!(server.count(), 2);
    assert!(began.elapsed() < Duration::from_millis(1000), "one deadline covers both stages");
}

#[tokio::test]
async fn fluent_pipeline_cancellation_stops_either_stage_without_retry() {
    for stop_after in [1, 2] {
        let server = Server::new(vec![Reply::draft("测试工具。").after(if stop_after == 1 { 1500 } else { 0 }),
            Reply::new(ACCEPT).after(1500)]).await;
        let (config, mode, source) = fixture("嗯，测试工具。");
        let endpoint = server.endpoint.clone(); let cancel = CancellationToken::new(); let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { run(&client(), &config, "synthetic-secret", &mode, &source,
            &DictionarySnapshot::default(), task_cancel, &endpoint, Duration::from_secs(3)).await });
        server.wait_for(stop_after).await; cancel.cancel();
        let error = task.await.unwrap().err().unwrap();
        assert_eq!(error.code, "cancelled"); assert_eq!(server.count(), stop_after);
    }
}

#[tokio::test]
async fn fluent_pipeline_rejects_non_auto_custom_or_incomplete_source_before_network() {
    let server = Server::new(vec![]).await;
    for variant in 0..3 {
        let (config, mut mode, mut source) = fixture("测试工具。");
        let expected = match variant { 0 => { mode.base_mode = BaseMode::Light; "invalid_mode" },
            1 => { mode.rules = "自定义".into(); "invalid_mode" }, _ => { source.complete = false; "source_incomplete" } };
        let error = run(&client(), &config, "synthetic-secret", &mode, &source, &DictionarySnapshot::default(),
            CancellationToken::new(), &server.endpoint, Duration::from_secs(3)).await.err().unwrap();
        assert_eq!(error.code, expected); assert_eq!(server.count(), 0);
    }
}

#[tokio::test]
async fn fluent_pipeline_rejects_truncated_oversized_or_service_error_in_either_stage() {
    for second_stage in [false, true] {
        for (reply, code) in [
            (Reply { finish: "length", ..Reply::new(ACCEPT) }, "truncated_output"),
            (Reply::new("x".repeat(1024 * 1024 + 1)), "response_too_large"),
            (Reply { status: 503, ..Reply::new("private provider response") }, "rewrite_service_error"),
        ] {
            let replies = if second_stage { vec![Reply::draft("测试工具。"), reply] } else { vec![reply] };
            let server = Server::new(replies).await;
            let error = execute(&server, "测试工具。", Duration::from_secs(3)).await.err().unwrap();
            assert_eq!(error.code, code); assert_eq!(error.stage, "rewrite");
            assert_eq!(server.count(), if second_stage { 2 } else { 1 });
            assert!(!error.message.contains("private provider response"));
        }
    }
}

#[test]
fn fluent_pipeline_checkpoint_prioritizes_cancel_and_rejects_synchronous_late_work() {
    let token = CancellationToken::new(); let expired = Instant::now() - Duration::from_millis(1);
    assert_eq!(checkpoint(&token, expired).unwrap_err().code, "rewrite_timeout");
    token.cancel();
    assert_eq!(checkpoint(&token, expired).unwrap_err().code, "cancelled");
}

#[tokio::test]
async fn fluent_pipeline_production_dispatcher_generates_then_reviews_the_same_draft() {
    let server = Server::new(vec![Reply::draft("用 Typeless 测试工具。"), Reply::new(ACCEPT)]).await;
    let (mut config, mode, source) = fixture("嗯，用 typeless 测试工具。");
    config.region = Some("beijing".into());
    config.rewrite_model = "qwen3.8-flash".into();
    let original = source.clone();
    let dictionary = DictionarySnapshot { version: 1, entries: vec![crate::core::DictionaryEntry {
        id: "term-1".into(), term: "Typeless".into(), created_at: "now".into(), updated_at: "now".into(),
    }] };
    let output = super::super::rewrite_with_client_dictionary(&client(), &config, "synthetic-secret",
        &mode, &source, &dictionary, CancellationToken::new(), &server.endpoint, Duration::from_secs(3))
        .await.unwrap();
    assert_eq!(server.count(), 2);
    assert_eq!(source, original);
    assert_eq!(output.blocks[0].text, "用 Typeless 测试工具。");
    assert_eq!(output.blocks[0].source_refs, vec!["item-0"]);
    assert_eq!(super::super::production_protocol_version(&config, &mode), super::super::MINIMUM_PIPELINE_VERSION);
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests[0], super::super::minimum::prepare(&config, &mode, &source, &dictionary,true).unwrap().body);
    assert_eq!(requests[0]["model"], "qwen3.8-flash");
    assert_eq!(requests[1]["model"], "qwen3.8-flash");
    let review_data: Value = serde_json::from_str(requests[1]["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(review_data["sourceText"], source.text());
    assert_eq!(review_data["draftText"], output.blocks[0].text);
    assert_eq!(requests[1], semantic_review::prepare_minimum(&config, &mode, &source, &output).unwrap().body);
}

fn dispatch_body(personalized: bool) -> Value {
    let mut data=json!({"sourceText":"A literal personalization key is not profile data."});
    if personalized {data["personalization"]=json!({"scene":"general","rules":[]});}
    json!({"model":"synthetic","messages":[{"role":"system","content":"Return JSON"},
        {"role":"user","content":data.to_string()}],"max_tokens":64,"enable_thinking":false})
}

#[test]
fn dispatch_profile_flag_requires_a_top_level_object_in_user_wire_data() {
    assert!(wire_has_personalization(&dispatch_body(true)));
    for content in ["not-json",r#"{"personalization":null}"#,r#"{"personalization":true}"#,
        r#"{"sourceText":"personalization"}"#,r#"{"writingContext":{"personalization":{}}}"#] {
        assert!(!wire_has_personalization(&json!({"messages":[{"role":"user","content":content}]})));
    }
    assert!(!wire_has_personalization(&json!({"messages":[{"role":"system","content":"{\"personalization\":{}}"}]})));
}

#[tokio::test]
async fn dispatch_observers_compose_without_adding_or_replacing_timing_events() {
    use super::super::execution::{DispatchEvent,RequestEvent,RequestOutcome};
    let server=Server::new(vec![Reply::new("accepted");4]).await;
    let (config,_,_)=fixture("测试。");
    let timing=Arc::new(Mutex::new(Vec::<RequestEvent>::new()));let timed=timing.clone();
    let first=Arc::new(Mutex::new(Vec::<DispatchEvent>::new()));let first_seen=first.clone();
    let second=Arc::new(Mutex::new(Vec::<DispatchEvent>::new()));let second_seen=second.clone();
    let deadline=Instant::now()+Duration::from_secs(3);
    let execution=RewriteExecution::observed(deadline,move|event|timed.lock().unwrap().push(event))
        .with_dispatch_observer(move|event|first_seen.lock().unwrap().push(event))
        .with_dispatch_observer(move|event|second_seen.lock().unwrap().push(event));
    assert_eq!(execution.deadline,deadline);
    let stages=[RequestStage::Generation,RequestStage::Review,RequestStage::Repair,RequestStage::FinalReview];
    for stage in stages {
        assert_eq!(request_observed(&client(),&config,"synthetic",&server.endpoint,&dispatch_body(true),
            &CancellationToken::new(),&execution,stage).await.unwrap(),"accepted");
    }
    assert_eq!(server.count(),4);
    let dispatches=first.lock().unwrap();assert_eq!(*dispatches,*second.lock().unwrap());
    assert_eq!(dispatches.iter().map(|event|event.stage).collect::<Vec<_>>(),stages);
    assert!(dispatches.iter().all(|event|event.personalization));
    let timed=timing.lock().unwrap();assert_eq!(timed.len(),8);
    for (pair,stage) in timed.chunks_exact(2).zip(stages) {
        assert_eq!(pair[0].stage,stage);assert_eq!(pair[1].stage,stage);
        assert!(pair[0].finished_at.is_none());assert!(pair[0].outcome.is_none());
        assert_eq!(pair[1].started_at,pair[0].started_at);assert!(pair[1].finished_at.is_some());
        assert_eq!(pair[1].outcome,Some(RequestOutcome::ResponseReceived));
    }
}

#[tokio::test]
async fn dispatch_profile_flag_uses_the_adapted_wire_body_for_all_six_providers() {
    for provider in ["aliyun","openai","deepseek","kimi","zhipu","custom"] {
        let server=Server::new(vec![Reply::new("accepted");2]).await;
        let (mut config,_,_)=fixture("测试。");config.rewrite_provider=provider.into();
        let dispatches=Arc::new(Mutex::new(Vec::new()));let seen=dispatches.clone();
        let execution=RewriteExecution::for_duration(Duration::from_secs(3))
            .with_dispatch_observer(move|event|seen.lock().unwrap().push(event));
        for personalized in [false,true] {
            request_observed(&client(),&config,"synthetic",&server.endpoint,&dispatch_body(personalized),
                &CancellationToken::new(),&execution,RequestStage::Generation).await.unwrap();
        }
        assert_eq!(server.count(),2,"{provider}");
        assert_eq!(dispatches.lock().unwrap().iter().map(|event|event.personalization).collect::<Vec<_>>(),[false,true],"{provider}");
        for (body,personalized) in server.requests.lock().unwrap().iter().zip([false,true]) {
            assert_eq!(wire_has_personalization(body),personalized,"{provider}");
        }
    }
}

#[tokio::test]
async fn dispatch_never_counts_local_conversion_url_or_header_build_failures() {
    use super::super::execution::{RequestEvent,RequestOutcome};
    for variant in 0..5 {
        let server=Server::new(vec![Reply::new("must not arrive")]).await;
        let (mut config,_,_)=fixture("测试。");
        let mut body=dispatch_body(true);let mut secret="synthetic";let mut endpoint=server.endpoint.as_str();
        match variant {
            0=>config.rewrite_provider="unknown".into(),1=>body=json!([]),
            2=>secret="invalid\nheader",3=>config.workspace=Some("invalid\nworkspace".into()),
            _=>endpoint="invalid url",
        }
        let dispatches=Arc::new(Mutex::new(Vec::new()));let seen=dispatches.clone();
        let timing=Arc::new(Mutex::new(Vec::<RequestEvent>::new()));let timed=timing.clone();
        let execution=RewriteExecution::observed(Instant::now()+Duration::from_secs(2),move|event|timed.lock().unwrap().push(event))
            .with_dispatch_observer(move|event|seen.lock().unwrap().push(event));
        assert!(request_observed(&client(),&config,secret,endpoint,&body,&CancellationToken::new(),&execution,RequestStage::Generation).await.is_err());
        assert_eq!(server.count(),0,"variant {variant}");assert!(dispatches.lock().unwrap().is_empty(),"variant {variant}");
        let timing=timing.lock().unwrap();assert_eq!(timing.len(),2);
        assert!(timing[0].finished_at.is_none());assert_eq!(timing[1].outcome,Some(RequestOutcome::Failed));
    }
}

#[tokio::test]
async fn dispatch_pre_cancel_and_expired_deadline_never_enter_the_send_future() {
    for (cancelled,expired) in [(true,false),(false,true),(true,true)] {
        let server=Server::new(vec![Reply::new("must not arrive")]).await;
        let (config,_,_)=fixture("测试。");let cancel=CancellationToken::new();if cancelled{cancel.cancel();}
        let dispatches=Arc::new(Mutex::new(Vec::new()));let seen=dispatches.clone();
        let deadline=if expired{Instant::now()-Duration::from_millis(1)}else{Instant::now()+Duration::from_secs(2)};
        let execution=RewriteExecution::until(deadline).with_dispatch_observer(move|event|seen.lock().unwrap().push(event));
        let error=request_observed(&client(),&config,"synthetic",&server.endpoint,&dispatch_body(true),&cancel,&execution,RequestStage::Generation).await.unwrap_err();
        assert_eq!(error.code,if cancelled{"cancelled"}else{"rewrite_timeout"});
        assert!(dispatches.lock().unwrap().is_empty());assert_eq!(server.count(),0);
    }
    // A timing callback may cancel after its early start event. It is still not
    // a send attempt; transport checks again before polling the send future.
    let server=Server::new(vec![Reply::new("must not arrive")]).await;
    let (config,_,_)=fixture("测试。");let cancel=CancellationToken::new();let stop=cancel.clone();
    let count=Arc::new(std::sync::atomic::AtomicUsize::new(0));let seen=count.clone();
    let execution=RewriteExecution::observed(Instant::now()+Duration::from_secs(2),move|event|if event.finished_at.is_none(){stop.cancel();})
        .with_dispatch_observer(move|_|{seen.fetch_add(1,std::sync::atomic::Ordering::SeqCst);});
    assert_eq!(request_observed(&client(),&config,"synthetic",&server.endpoint,&dispatch_body(true),&cancel,&execution,RequestStage::Generation).await.unwrap_err().code,"cancelled");
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst),0);assert_eq!(server.count(),0);
}

#[tokio::test]
async fn dispatch_is_recorded_once_when_http_fails_or_is_cancelled_after_arrival() {
    for status in [401,503] {
        let server=Server::new(vec![Reply{status,..Reply::new("private response")}]).await;
        let (config,_,_)=fixture("测试。");let count=Arc::new(std::sync::atomic::AtomicUsize::new(0));let seen=count.clone();
        let execution=RewriteExecution::for_duration(Duration::from_secs(2)).with_dispatch_observer(move|_|{seen.fetch_add(1,std::sync::atomic::Ordering::SeqCst);});
        let error=request_observed(&client(),&config,"synthetic",&server.endpoint,&dispatch_body(true),&CancellationToken::new(),&execution,RequestStage::Generation).await.unwrap_err();
        assert_eq!(error.code,if status==401{"authentication_failed"}else{"rewrite_service_error"});
        assert_eq!(server.count(),1);assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst),1);
    }
    let server=Server::new(vec![Reply::new("private response").after(1500)]).await;
    let (config,_,_)=fixture("测试。");let cancel=CancellationToken::new();let stop=cancel.clone();let endpoint=server.endpoint.clone();
    let count=Arc::new(std::sync::atomic::AtomicUsize::new(0));let seen=count.clone();
    let execution=RewriteExecution::for_duration(Duration::from_secs(3)).with_dispatch_observer(move|_|{seen.fetch_add(1,std::sync::atomic::Ordering::SeqCst);});
    let task=tokio::spawn(async move{request_observed(&client(),&config,"synthetic",&endpoint,&dispatch_body(true),&stop,&execution,RequestStage::Generation).await});
    server.wait_for(1).await;cancel.cancel();assert_eq!(task.await.unwrap().unwrap_err().code,"cancelled");
    assert_eq!(server.count(),1);assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst),1);
}

fn dispatch_writing_context() -> crate::core::writing_context::WritingContext {
    use crate::core::{AppWritingScene,DraftLanguage};
    use crate::core::personalization::{PersonalizationConfig,ExpressionStyle};
    let config=PersonalizationConfig{enabled:true,
        preferences:ExpressionStyle{keep_english_terms:true,..Default::default()},..Default::default()};
    crate::core::writing_context::WritingContext{scene:AppWritingScene::General,language:DraftLanguage::AsSpoken,
        input:None,personalization:config.freeze(AppWritingScene::General)}
}

#[tokio::test]
async fn dispatch_tracking_survives_manual_and_confirmed_preview_paths_with_and_without_context() {
    use super::super::{manual_candidate,confirmed_candidate};
    use crate::core::{ConfirmedContext,DraftLanguage};
    for personalized in [false,true] {
        for confirmed in [false,true] {
            let input="I have not released the report.";
            let (config,mode,source)=fixture(input);let context=dispatch_writing_context();
            assert!(context.personalization.is_some());
            let writing=personalized.then_some(&context);
            let expected_calls=if confirmed||personalized{2}else{1};
            let mut replies=vec![Reply::draft(input)];if expected_calls==2{replies.push(Reply::new(ACCEPT));}
            let server=Server::new(replies).await;
            let dispatches=Arc::new(Mutex::new(Vec::new()));let seen=dispatches.clone();
            let execution=RewriteExecution::for_duration(Duration::from_secs(3))
                .with_dispatch_observer(move|event|seen.lock().unwrap().push(event));
            let result=if confirmed {
                let confirmed_context=ConfirmedContext{source_revision_id:source.id.clone(),language:DraftLanguage::AsSpoken,corrections:Vec::new()};
                confirmed_candidate::run_until_with_writing_context(&client(),&config,"synthetic",&mode,&source,
                    &DictionarySnapshot::default(),&confirmed_context,CancellationToken::new(),&server.endpoint,execution,writing).await.result
            } else {
                manual_candidate::run_until_with_writing_context(&client(),&config,"synthetic",&mode,&source,
                    &DictionarySnapshot::default(),CancellationToken::new(),&server.endpoint,writing,execution).await
            };
            let candidate=result.unwrap();assert!(candidate.review_issues.is_empty());
            assert_eq!(candidate.draft.blocks[0].text,input);assert_eq!(source.text(),input);
            assert_eq!(server.count(),expected_calls,"confirmed={confirmed} personalized={personalized}");
            let observed=dispatches.lock().unwrap();assert_eq!(observed.len(),expected_calls);
            assert!(observed.iter().all(|event|event.personalization==personalized));
            assert_eq!(observed[0].stage,RequestStage::Generation);
            if expected_calls==2{assert_eq!(observed[1].stage,RequestStage::Review);}
            for body in server.requests.lock().unwrap().iter(){assert_eq!(wire_has_personalization(body),personalized);}
        }
    }
}

#[tokio::test]
async fn dispatch_tracking_records_profile_payload_in_generation_review_repair_and_final_review() {
    let input="嗯，呃呃，就是就是，我我觉得方案暂时不可行。";
    let server=Server::new(vec![Reply::draft(input),Reply::new(ACCEPT),
        Reply::new(json!({"replacements":[{"id":"r0","text":"我觉得方案暂时不可行。"}]}).to_string()),Reply::new(ACCEPT)]).await;
    let (config,mode,source)=fixture(input);let context=dispatch_writing_context();
    let dispatches=Arc::new(Mutex::new(Vec::new()));let seen=dispatches.clone();
    let execution=RewriteExecution::for_duration(Duration::from_secs(3))
        .with_dispatch_observer(move|event|seen.lock().unwrap().push(event));
    let outcome=super::super::minimum_pipeline::execute_until_with_writing_context(&client(),&config,"synthetic",&mode,
        &source,&DictionarySnapshot::default(),CancellationToken::new(),&server.endpoint,execution,None,Some(&context)).await;
    assert!(outcome.result.is_ok(),"{:?}",outcome.result.err());assert_eq!(server.count(),4);
    let observed=dispatches.lock().unwrap();
    assert_eq!(observed.iter().map(|event|event.stage).collect::<Vec<_>>(),
        [RequestStage::Generation,RequestStage::Review,RequestStage::Repair,RequestStage::FinalReview]);
    assert!(observed.iter().all(|event|event.personalization));
    for body in server.requests.lock().unwrap().iter(){assert!(wire_has_personalization(body));}
}

#[tokio::test]
async fn dispatch_selected_profile_with_local_english_guard_failure_is_not_a_send() {
    let server=Server::new(vec![Reply::new("must not arrive")]).await;
    let (config,mode,source)=fixture("请把这两个约束分开列点。");let mut context=dispatch_writing_context();
    context.language=crate::core::DraftLanguage::English;assert!(context.personalization.is_some());
    let count=Arc::new(std::sync::atomic::AtomicUsize::new(0));let seen=count.clone();
    let execution=RewriteExecution::for_duration(Duration::from_secs(3)).with_dispatch_observer(move|_|{seen.fetch_add(1,std::sync::atomic::Ordering::SeqCst);});
    let result=super::super::manual_candidate::run_until_with_writing_context(&client(),&config,"synthetic",&mode,
        &source,&DictionarySnapshot::default(),CancellationToken::new(),&server.endpoint,Some(&context),execution).await;
    assert!(result.is_err());assert_eq!(server.count(),0);assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst),0);
}
