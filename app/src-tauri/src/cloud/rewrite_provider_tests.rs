//! Scripted localhost HTTP envelopes verify provider wires, not model quality.
use super::*;
use std::sync::{Arc, Mutex};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener, task::JoinHandle};

const ACCEPT: &str = r#"{"decision":"accept","checks":{"coverage":"pass","facts":"pass","scope":"pass","intent":"pass","cleanup":"pass"},"issues":[]}"#;
const PROVIDERS: [(&str, &str); 6] = [
    ("aliyun", "qwen3.8-flash"), ("openai", "gpt-5.4-mini"),
    ("deepseek", "deepseek-flash"), ("kimi", "kimi-k2.6"),
    ("zhipu", "glm-4.7"), ("custom", "private-text-model"),
];

fn config(provider: &str, model: &str) -> ProviderConfig {
    ProviderConfig { rewrite_provider: provider.into(), rewrite_model: model.into(),
        // Saved Aliyun fields can still be present for the ASR account. They
        // must never become headers or vendor options for another text service.
        region: Some("beijing".into()), workspace: Some("asr-workspace".into()),
        ..ProviderConfig::default() }
}

fn fixture() -> (ModeSnapshot, SourceRevision) {
    (ModeSnapshot { mode_id: "document".into(), name: "文档".into(), base_mode: BaseMode::Auto,
        rules: String::new(), revision: 1 },
     SourceRevision { id: "source".into(), session_id: "session".into(), asr_attempt_id: "attempt".into(),
        generation: 1, provider: "synthetic-asr".into(), model: "fixed-source".into(), adapter_version: "test".into(),
        complete: true, created_at: "now".into(), items: vec![crate::core::SourceItem {
            id: "item-0".into(), provider_item_id: "0".into(), capture_order: 0,
            text: "我感觉这个方案还没有太能做到。预算3500元，先不要发送。".into(),
            is_final: true, start_ms: None, end_ms: None }] })
}

fn envelope(content: impl Into<String>) -> Value {
    json!({"choices":[{"finish_reason":"stop","message":{"content":content.into()}}]})
}

struct Server { endpoint: String, seen: Arc<Mutex<Vec<(String, Value)>>>, task: JoinHandle<()> }
impl Drop for Server { fn drop(&mut self) { self.task.abort(); } }
impl Server {
    async fn new(replies: Vec<(u16, Value)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/scripted", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new())); let captured = Arc::clone(&seen);
        let task = tokio::spawn(async move {
            for (status, reply) in replies {
                let Ok((mut socket, _)) = listener.accept().await else { return; };
                let mut bytes = Vec::new(); let mut chunk = [0; 4096];
                let (header_end, length) = loop {
                    let Ok(size) = socket.read(&mut chunk).await else { return; }; if size == 0 { return; }
                    bytes.extend_from_slice(&chunk[..size]); assert!(bytes.len() <= 2 * 1024 * 1024);
                    if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..index]);
                        let length = header.lines().find_map(|line| line.split_once(':')
                            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>().unwrap())).unwrap();
                        break (index + 4, length);
                    }
                };
                while bytes.len() < header_end + length {
                    let Ok(size) = socket.read(&mut chunk).await else { return; }; if size == 0 { return; }
                    bytes.extend_from_slice(&chunk[..size]);
                }
                let headers = String::from_utf8_lossy(&bytes[..header_end]).to_ascii_lowercase();
                let request = serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                captured.lock().unwrap().push((headers, request));
                let body = reply.to_string();
                let header = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                if socket.write_all(header.as_bytes()).await.is_err() { return; }
                if socket.write_all(body.as_bytes()).await.is_err() { return; }
            }
        });
        Self { endpoint, seen, task }
    }
}

fn assert_wire(provider: &str, model: &str, headers: &str, body: &Value, stage: usize) {
    assert!(headers.contains("authorization: bearer synthetic-provider-key"));
    assert_eq!(headers.contains("x-dashscope-workspace:"), provider == "aliyun");
    assert_eq!(body["model"], model);
    assert_eq!(body["response_format"]["type"], "json_object");
    assert_eq!(body["stream"], false);
    assert!(body.get("tools").is_none());
    if provider == "aliyun" { assert_eq!(body["enable_thinking"], false); }
    else { assert!(body.get("enable_thinking").is_none()); }
    if provider == "openai" {
        assert_eq!(body["reasoning_effort"], "none");
        assert_eq!(body["max_completion_tokens"], if stage == 0 { 8192 } else { 512 });
        assert!(body.get("max_tokens").is_none());
    } else {
        assert!(body.get("reasoning_effort").is_none());
        assert_eq!(body["max_tokens"], if stage == 0 { 8192 } else { 512 });
    }
    if matches!(provider, "deepseek" | "kimi" | "zhipu") {
        assert_eq!(body["thinking"]["type"], "disabled");
    } else { assert!(body.get("thinking").is_none()); }
    if matches!(provider, "openai" | "kimi") { assert!(body.get("temperature").is_none()); }
    let data: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(data["writingContext"]["scene"], "document");
    assert_eq!(data["sourceText"], fixture().1.text());
}

#[tokio::test]
async fn every_provider_adapts_generation_review_repair_and_final_review_at_http_boundary() {
    for (provider, model) in PROVIDERS {
        let (mode, source) = fixture(); let original = source.clone(); let config = config(provider, model);
        let bad = "我感觉这个方案还没有太能做到。";
        let repaired = "我感觉这个方案还没有很好地做到。";
        let rejection = json!({"decision":"reject","checks":{"coverage":"pass","facts":"pass","scope":"pass","intent":"pass","cleanup":"fail"},
            "issues":[{"dimension":"cleanup","sourceQuote":bad,"draftQuote":bad,"reason":"局部搭配不自然"}]}).to_string();
        let server = Server::new(vec![
            (200, envelope(json!({"resolvedMode":"light","text":source.text()}).to_string())),
            (200, envelope(rejection)),
            (200, envelope(json!({"replacements":[{"id":"r0","text":repaired}]}).to_string())),
            (200, envelope(ACCEPT)),
        ]).await;
        let client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let result = rewrite_with_client_execution(&client, &config, "synthetic-provider-key", &mode, &source,
            &DictionarySnapshot::default(), CancellationToken::new(), &server.endpoint,
            RewriteExecution::for_duration(Duration::from_secs(4))).await;
        assert_eq!(result.unwrap().blocks[0].text, format!("{repaired}预算3500元，先不要发送。"), "{provider}");
        assert_eq!(source, original);
        let requests = server.seen.lock().unwrap(); assert_eq!(requests.len(), 4, "{provider}");
        for (stage, (headers, body)) in requests.iter().enumerate() { assert_wire(provider, model, headers, body, stage); }
    }
}

#[tokio::test]
async fn provider_refusal_empty_truncation_and_http_errors_keep_static_recoverable_errors() {
    for (provider, model) in PROVIDERS {
        let config = config(provider, model);
        let mut refused = envelope("SENSITIVE_PROVIDER_TEXT");
        refused["choices"][0]["message"]["refusal"] = json!("SENSITIVE_REFUSAL");
        let mut truncated = envelope("SENSITIVE_PROVIDER_TEXT");
        truncated["choices"][0]["finish_reason"] = json!("length");
        for (status, reply, expected) in [
            (200, refused, "rewrite_refused"), (200, envelope(" \n "), "empty_output"),
            (200, truncated, "truncated_output"),
            (429, json!({"error":"SENSITIVE_RATE_LIMIT"}), "rate_limited"),
        ] {
            let server = Server::new(vec![(status, reply)]).await;
            let body = json!({"model":model,"messages":[{"role":"system","content":"Return JSON"}],
                "response_format":{"type":"json_object"},"enable_thinking":false,"temperature":0,"max_tokens":512,"stream":false});
            let client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
            let error = fluent_pipeline::request_content(&client, &config, "synthetic-provider-key", &server.endpoint,
                &body, &CancellationToken::new(), tokio::time::Instant::now() + Duration::from_secs(2)).await.unwrap_err();
            assert_eq!(error.code, expected, "{provider}"); assert!(error.message.contains("识别稿") || error.code == "empty_output" || error.code == "truncated_output");
            assert!(!error.message.contains("SENSITIVE")); assert_eq!(server.seen.lock().unwrap().len(), 1);
        }
    }
}

#[test]
fn aliyun_rollouts_cannot_be_selected_by_another_provider_with_a_qwen_model_name() {
    let (mut mode, source) = fixture(); mode.mode_id = "auto".into();
    for provider in ["openai", "deepseek", "kimi", "zhipu", "custom"] {
        let config = config(provider, "qwen3.8-flash");
        assert!(!uses_minimum_contract(&config, &mode));
        assert!(!uses_fluent_contract(&config, &mode));
        assert!(!uses_automatic_contract(&config, &mode));
        assert_eq!(production_protocol_version(&config, &mode), REWRITE_PROMPT_VERSION);
        assert!(semantic_review::minimum_format(&config, &source) == semantic_review::MinimumReviewFormat::Named);
    }
    let mut config = config("aliyun", "qwen3.8-flash"); config.provider = "google".into();
    assert!(uses_minimum_contract(&config, &mode), "ASR selection does not change the Aliyun rewrite rollout");
}

#[tokio::test]
async fn legacy_layouts_on_new_providers_still_reject_changed_source_facts() {
    for (provider, model) in PROVIDERS.into_iter().filter(|(provider, _)| *provider != "aliyun") {
        let (mut mode, source) = fixture(); let before = source.clone();
        mode.mode_id = "light".into(); mode.base_mode = BaseMode::Light;
        let config = config(provider, model);
        assert_eq!(production_protocol_version(&config, &mode), REWRITE_PROMPT_VERSION);
        let content = json!({"resolvedMode":"light","blocks":[{
            "text":source.text().replace("3500", "5000"),"sourceRefs":["item-0"]}]}).to_string();
        let server = Server::new(vec![(200, envelope(content))]).await;
        let client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let error = rewrite_with_client_execution(&client, &config, "synthetic-provider-key", &mode, &source,
            &DictionarySnapshot::default(), CancellationToken::new(), &server.endpoint,
            RewriteExecution::for_duration(Duration::from_secs(2))).await.unwrap_err();
        assert_eq!(error.code, "rewrite_meaning_risk", "{provider}");
        assert_eq!(server.seen.lock().unwrap().len(), 1, "legacy protocols do not silently gain a model-review rollout");
        assert_eq!(source, before);
    }
}
