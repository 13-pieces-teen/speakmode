//! Real loopback WebSocket traffic; synthetic PCM only, no cloud or microphone.
use super::*;
use futures_util::{SinkExt,StreamExt};
use serde_json::{Value,json};
use std::{future::Future,time::Duration,sync::{Mutex,atomic::{AtomicUsize,Ordering}}};
use tokio::{net::{TcpListener,TcpStream},task::JoinHandle};
use tokio_tungstenite::{WebSocketStream,accept_async,tungstenite::Message};

async fn server<F,Fut>(script:F)->(String,JoinHandle<()>)
where F:FnOnce(WebSocketStream<TcpStream>)->Fut+Send+'static,Fut:Future<Output=()>+Send+'static {
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let task=tokio::spawn(async move{let(socket,_)=listener.accept().await.unwrap();script(accept_async(socket).await.unwrap()).await;});
    (format!("ws://{address}/test-only"),task)
}
async fn receive(socket:&mut WebSocketStream<TcpStream>)->Value {
    let message=tokio::time::timeout(Duration::from_secs(3),socket.next()).await.unwrap().unwrap().unwrap();
    if let Message::Text(text)=message {serde_json::from_str(&text).unwrap()} else {panic!("expected protocol JSON")}
}
async fn emit(socket:&mut WebSocketStream<TcpStream>,id:&str,event:&str,payload:Value) {
    socket.send(Message::Text(json!({"header":{"task_id":id,"event":event},"payload":payload}).to_string())).await.unwrap();
}
fn config()->ProviderConfig {let mut c=ProviderConfig::default();c.asr_model="qwen-audio-3.1-asr-flash-streaming".into();c.adapter_version="qwen-task-streaming-v1".into();c}
fn budgets()->Timeouts {Timeouts{establishment:Duration::from_secs(2),send:Duration::from_secs(1),finish:Duration::from_secs(1)}}
fn ignored()->Arc<dyn Fn(AsrProgress)+Send+Sync>{Arc::new(|_|{})}
fn sentence(id:u64,text:&str,is_final:bool,start:u64,end:Option<u64>)->Value {
    json!({"output":{"sentence":{"sentence_id":id,"text":text,"sentence_end":is_final,"begin_time":start,"end_time":end}}})
}
async fn run_local(url:String,audio:mpsc::Receiver<AsrInput>,cancel:CancellationToken,callback:Arc<dyn Fn(AsrProgress)+Send+Sync>,timeouts:Timeouts)->Result<Vec<SourceItem>,CloudError> {
    run_at(config(),"synthetic-key".into(),"auto".into(),DictionarySnapshot::default(),"task-attempt".into(),audio,cancel,callback,url,timeouts).await
}

#[tokio::test]
async fn task_ack_gates_binary_pcm_and_dictionary_then_final_receipt_creates_source() {
    let mut store=crate::core::Store::in_memory().unwrap();
    let dictionary=store.import_dictionary(vec!["SpeakMode".into(),"百炼".into()],1).unwrap();
    let(url,peer)=server(|mut socket|async move {
        let start=receive(&mut socket).await;
        assert_eq!(start["header"]["action"],"run-task");assert_eq!(start["header"]["streaming"],"duplex");
        assert_eq!(start["payload"]["model"],"qwen-audio-3.1-asr-flash-streaming");
        let parameters=&start["payload"]["parameters"];
        assert_eq!(parameters["format"],"pcm");assert_eq!(parameters["sample_rate"],16000);
        assert_eq!(parameters["language_hints"],json!(["en"]));assert_eq!(parameters["heartbeat"],true);
        assert_eq!(parameters["vocabulary"],json!({"SpeakMode":4,"百炼":4}));
        let id=start["header"]["task_id"].as_str().unwrap();assert!(uuid::Uuid::parse_str(id).is_ok());
        assert!(tokio::time::timeout(Duration::from_millis(30),socket.next()).await.is_err());
        emit(&mut socket,id,"task-started",json!({})).await;
        let frame=socket.next().await.unwrap().unwrap();assert_eq!(frame,Message::Binary(vec![1,0,255,127,0,128]));
        let finish=receive(&mut socket).await;assert_eq!(finish["header"]["action"],"finish-task");assert_eq!(finish["header"]["task_id"],id);
        emit(&mut socket,id,"result-generated",sentence(1,"保留原意。",true,0,Some(200))).await;
        emit(&mut socket,id,"task-finished",json!({})).await;
    }).await;
    let(tx,rx)=mpsc::channel(2);tx.send(AsrInput::Frame{sequence:0,pcm16:vec![1,i16::MAX,i16::MIN],padding_samples:0}).await.unwrap();
    tx.send(AsrInput::Finish{next_sequence:1}).await.unwrap();
    let rows=run_at(config(),"synthetic-key".into(),"en".into(),dictionary,"task-attempt".into(),rx,CancellationToken::new(),ignored(),url,budgets()).await.unwrap();
    assert_eq!(rows.len(),1);assert_eq!(rows[0].text,"保留原意。");assert_eq!(rows[0].id,"task-attempt:sentence-1");assert_eq!(rows[0].end_ms,Some(200));peer.await.unwrap();
}

#[tokio::test]
async fn interim_revisions_replace_text_and_final_sentence_identity_survives_reordering() {
    let captions=Arc::new(Mutex::new(Vec::new()));let seen=captions.clone();
    let configured=Arc::new(AtomicUsize::new(0));let count=configured.clone();
    let telemetry=Arc::new(Mutex::new(None));let diagnostic=telemetry.clone();
    let(url,peer)=server(|mut socket|async move {
        let start=receive(&mut socket).await;let id=start["header"]["task_id"].as_str().unwrap();
        assert!(start["payload"]["parameters"].get("language_hints").is_none());
        assert!(start["payload"]["parameters"].get("vocabulary").is_none());
        for _ in 0..2 {emit(&mut socket,id,"task-started",json!({})).await;}
        assert_eq!(receive(&mut socket).await["header"]["action"],"finish-task");
        emit(&mut socket,id,"result-generated",json!({"output":{"sentence":{"sentence_id":0,"heartbeat":true}}})).await;
        emit(&mut socket,id,"result-generated",sentence(1,"今天可以",false,0,None)).await;
        emit(&mut socket,id,"result-generated",sentence(1,"今天不可以",false,20,Some(250))).await;
        for _ in 0..2 {emit(&mut socket,id,"result-generated",sentence(2,"明天再看。",true,1000,Some(1500))).await;}
        emit(&mut socket,id,"result-generated",sentence(1,"今天不可以。",true,40,Some(800))).await;
        emit(&mut socket,id,"task-finished",json!({})).await;
    }).await;
    let(tx,rx)=mpsc::channel(1);tx.send(AsrInput::Finish{next_sequence:0}).await.unwrap();
    let rows=run_local(url,rx,CancellationToken::new(),Arc::new(move|progress|match progress {
        AsrProgress::Configured=>{count.fetch_add(1,Ordering::SeqCst);},
        AsrProgress::Caption(items)=>seen.lock().unwrap().push(items),
        AsrProgress::Diagnostics(value)=>*diagnostic.lock().unwrap()=Some(value),_=>{},
    }),budgets()).await.unwrap();
    assert_eq!(configured.load(Ordering::SeqCst),1);assert_eq!(rows.iter().map(|row|row.text.as_str()).collect::<Vec<_>>(),["今天不可以。","明天再看。"]);
    assert_eq!(rows[0].start_ms,Some(40));assert_eq!(rows[0].end_ms,Some(800));assert_eq!(rows[1].capture_order,1);
    let updates=captions.lock().unwrap();assert_eq!(updates.len(),4);assert_eq!(updates[1][0].text,"今天不可以");
    assert_eq!(updates[3][0].revision,3);assert!(updates[3][0].is_final);assert_eq!(updates[3][0].id,"task-attempt:sentence-1");
    let trace=telemetry.lock().unwrap();let trace=trace.as_ref().unwrap();assert!(trace.terminated);assert_eq!(trace.pending_items,0);assert_eq!(trace.final_items,2);
    assert!(trace.finish_written_at.is_some());assert!(trace.session_finished_received_at>=trace.finish_written_at);
    peer.await.unwrap();
}

#[tokio::test]
async fn a_nonempty_next_sentence_stash_is_partial_until_its_own_final_arrives() {
    for finalize_stash in [false,true] {
        let(url,peer)=server(move|mut socket|async move {
            let start=receive(&mut socket).await;let id=start["header"]["task_id"].as_str().unwrap();
            emit(&mut socket,id,"task-started",json!({})).await;receive(&mut socket).await;
            let mut first=sentence(1,"设备预算3500元。",true,0,Some(500));
            first["output"]["sentence"]["stash"]=json!({"sentence_id":2,"text":"差旅","begin_time":500});
            emit(&mut socket,id,"result-generated",first).await;
            if finalize_stash {emit(&mut socket,id,"result-generated",sentence(2,"差旅另算。",true,500,Some(800))).await;}
            emit(&mut socket,id,"task-finished",json!({})).await;
        }).await;
        let(tx,rx)=mpsc::channel(1);tx.send(AsrInput::Finish{next_sequence:0}).await.unwrap();
        let result=run_local(url,rx,CancellationToken::new(),ignored(),budgets()).await;
        if finalize_stash {assert_eq!(result.unwrap()[1].text,"差旅另算。");}else{assert_eq!(result.unwrap_err().code,"asr_incomplete");}
        peer.await.unwrap();
    }
}

#[tokio::test]
async fn partial_missing_sentence_and_conflicting_final_never_become_complete_source() {
    for (scenario,expected) in [("partial","asr_incomplete"),("gap","missing_predecessor"),("conflict","conflicting_final"),("time","conflicting_order")] {
        let(url,peer)=server(move|mut socket|async move {
            let start=receive(&mut socket).await;let id=start["header"]["task_id"].as_str().unwrap();
            emit(&mut socket,id,"task-started",json!({})).await;receive(&mut socket).await;
            let events=match scenario {
                "partial"=>vec![sentence(1,"还没说完",false,0,None)],
                "gap"=>vec![sentence(2,"第二句。",true,100,Some(200))],
                "conflict"=>vec![sentence(1,"不要发。",true,0,Some(100)),sentence(1,"可以发。",true,0,Some(100))],
                _=>vec![sentence(1,"先说的。",true,100,Some(200)),sentence(2,"后说的。",true,0,Some(50))],
            };
            for event in events {emit(&mut socket,id,"result-generated",event).await;}
            // Conflicting final fails immediately; no further server send is needed.
            if scenario!="conflict" {emit(&mut socket,id,"task-finished",json!({})).await;}
        }).await;
        let(tx,rx)=mpsc::channel(1);tx.send(AsrInput::Finish{next_sequence:0}).await.unwrap();
        assert_eq!(run_local(url,rx,CancellationToken::new(),ignored(),budgets()).await.unwrap_err().code,expected);peer.await.unwrap();
    }
}

#[tokio::test]
async fn foreign_task_and_service_errors_are_rejected_without_provider_text_leaking() {
    for foreign in [false,true] {
        let(url,peer)=server(move|mut socket|async move {
            let start=receive(&mut socket).await;let id=start["header"]["task_id"].as_str().unwrap();
            socket.send(Message::Text(json!({"header":{"event":"task-failed","task_id":if foreign{"other-task"}else{id},
                "error_code":"provider-arbitrary","error_message":"synthetic-sensitive-text"},"payload":{}}).to_string())).await.unwrap();
        }).await;
        let(_tx,rx)=mpsc::channel(1);let error=run_local(url,rx,CancellationToken::new(),ignored(),budgets()).await.unwrap_err();
        assert_eq!(error.code,if foreign{"invalid_asr_event"}else{"asr_service_error"});assert!(!serde_json::to_string(&error).unwrap().contains("synthetic-sensitive-text"));peer.await.unwrap();
    }
}

#[tokio::test]
async fn premature_task_finish_cannot_skip_the_local_capture_finish_boundary() {
    let(url,peer)=server(|mut socket|async move {
        let start=receive(&mut socket).await;let id=start["header"]["task_id"].as_str().unwrap();
        emit(&mut socket,id,"task-started",json!({})).await;emit(&mut socket,id,"task-finished",json!({})).await;
    }).await;
    let(_tx,rx)=mpsc::channel(1);assert_eq!(run_local(url,rx,CancellationToken::new(),ignored(),budgets()).await.unwrap_err().code,"unexpected_finish");peer.await.unwrap();
}

#[tokio::test]
async fn config_and_final_ack_timeouts_are_distinct_and_cannot_return_partial_source() {
    for accept in [false,true] {
        let(url,peer)=server(move|mut socket|async move {
            let start=receive(&mut socket).await;let id=start["header"]["task_id"].as_str().unwrap();
            if accept {emit(&mut socket,id,"task-started",json!({})).await;assert_eq!(receive(&mut socket).await["header"]["action"],"finish-task");}
            std::future::pending::<()>().await;
        }).await;
        let(tx,rx)=mpsc::channel(1);tx.send(AsrInput::Finish{next_sequence:0}).await.unwrap();
        let mut timeout=budgets();if accept {timeout.finish=Duration::from_millis(80);}else{timeout.establishment=Duration::from_millis(80);}
        assert_eq!(run_local(url,rx,CancellationToken::new(),ignored(),timeout).await.unwrap_err().code,if accept{"asr_finish_timeout"}else{"asr_config_timeout"});peer.abort();
    }
}

#[tokio::test]
async fn disconnected_peer_after_sentence_final_is_not_a_task_final_receipt() {
    let(url,peer)=server(|mut socket|async move {
        let start=receive(&mut socket).await;let id=start["header"]["task_id"].as_str().unwrap();
        emit(&mut socket,id,"task-started",json!({})).await;receive(&mut socket).await;
        emit(&mut socket,id,"result-generated",sentence(1,"已经听到。",true,0,Some(200))).await;socket.close(None).await.unwrap();
    }).await;
    let(tx,rx)=mpsc::channel(1);tx.send(AsrInput::Finish{next_sequence:0}).await.unwrap();
    assert_eq!(run_local(url,rx,CancellationToken::new(),ignored(),budgets()).await.unwrap_err().code,"asr_disconnected");peer.await.unwrap();
}

#[tokio::test]
async fn cancellation_from_caption_prevents_source_even_when_task_final_is_already_queued() {
    let cancel=CancellationToken::new();let on_caption=cancel.clone();
    let(url,peer)=server(|mut socket|async move {
        let start=receive(&mut socket).await;let id=start["header"]["task_id"].as_str().unwrap();
        emit(&mut socket,id,"task-started",json!({})).await;receive(&mut socket).await;
        emit(&mut socket,id,"result-generated",sentence(1,"取消这段。",true,0,Some(200))).await;
        emit(&mut socket,id,"task-finished",json!({})).await;
    }).await;
    let(tx,rx)=mpsc::channel(1);tx.send(AsrInput::Finish{next_sequence:0}).await.unwrap();
    let error=run_local(url,rx,cancel,Arc::new(move|event|if matches!(event,AsrProgress::Caption(_)){on_caption.cancel();}),budgets()).await.unwrap_err();
    assert_eq!(error.code,"cancelled");peer.await.unwrap();
}

#[tokio::test]
async fn cancellation_before_configuration_ack_does_not_wait_for_service_timeout() {
    let cancel=CancellationToken::new();let trigger=cancel.clone();
    let(url,peer)=server(move|mut socket|async move {receive(&mut socket).await;trigger.cancel();std::future::pending::<()>().await;}).await;
    let(_tx,rx)=mpsc::channel(1);
    let result=tokio::time::timeout(Duration::from_millis(500),run_local(url,rx,cancel,ignored(),budgets())).await.unwrap();
    assert_eq!(result.unwrap_err().code,"cancelled");peer.abort();
}

#[tokio::test]
async fn binary_tail_padding_and_telemetry_preserve_exact_sample_counts() {
    let telemetry=Arc::new(Mutex::new(None));let observed=telemetry.clone();
    let(url,peer)=server(|mut socket|async move {
        let start=receive(&mut socket).await;let id=start["header"]["task_id"].as_str().unwrap();
        emit(&mut socket,id,"task-started",json!({})).await;
        if let Message::Binary(bytes)=socket.next().await.unwrap().unwrap(){assert_eq!(bytes.len(),3200);assert_eq!(&bytes[..6],&[1,0,2,0,3,0]);assert!(bytes[6..].iter().all(|n|*n==0));}else{panic!("tail must be binary PCM");}
        assert_eq!(receive(&mut socket).await["header"]["action"],"finish-task");emit(&mut socket,id,"task-finished",json!({})).await;
    }).await;
    let mut pcm=vec![0;1600];pcm[..3].copy_from_slice(&[1,2,3]);let(tx,rx)=mpsc::channel(2);
    tx.send(AsrInput::Frame{sequence:0,pcm16:pcm,padding_samples:1597}).await.unwrap();tx.send(AsrInput::Finish{next_sequence:1}).await.unwrap();
    assert!(run_local(url,rx,CancellationToken::new(),Arc::new(move|event|if let AsrProgress::Diagnostics(value)=event{*observed.lock().unwrap()=Some(value);}),budgets()).await.unwrap().is_empty());
    let trace=telemetry.lock().unwrap();let trace=trace.as_ref().unwrap();assert_eq!(trace.dequeued,trace.socket_written);assert_eq!(trace.socket_written.real_samples,3);assert_eq!(trace.socket_written.padding_samples,1597);assert_eq!(trace.socket_written.frames,1);assert_eq!(trace.finish_expected_frames,Some(1));assert!(trace.terminated);peer.await.unwrap();
}

#[tokio::test]
async fn audio_gaps_oversized_frames_and_incomplete_drain_cannot_send_a_finish() {
    for (input,expected) in [(AsrInput::Frame{sequence:1,pcm16:vec![1],padding_samples:0},"audio_incomplete"),
        (AsrInput::Frame{sequence:0,pcm16:vec![1;1601],padding_samples:0},"audio_frame_limit"),
        (AsrInput::Finish{next_sequence:1},"audio_incomplete")] {
        let(url,peer)=server(|mut socket|async move {
            let start=receive(&mut socket).await;let id=start["header"]["task_id"].as_str().unwrap();emit(&mut socket,id,"task-started",json!({})).await;
            if let Some(Ok(message))=socket.next().await {assert!(matches!(message,Message::Close(_)),"invalid audio was transmitted");}
        }).await;
        let(tx,rx)=mpsc::channel(1);tx.send(input).await.unwrap();
        assert_eq!(run_local(url,rx,CancellationToken::new(),ignored(),budgets()).await.unwrap_err().code,expected);peer.await.unwrap();
    }
}

#[tokio::test]
async fn longer_dictionary_entries_use_bounded_context_without_truncation() {
    let terms=(0..18).map(|i|format!("这是一个超过十五字但需要完整保留的产品领域术语编号{i}")).chain(["SpeakMode".into()]).collect::<Vec<_>>();
    let mut store=crate::core::Store::in_memory().unwrap();let dictionary=store.import_dictionary(terms.clone(),1).unwrap();
    let(url,peer)=server(move|mut socket|async move {
        let start=receive(&mut socket).await;let id=start["header"]["task_id"].as_str().unwrap();
        assert_eq!(start["payload"]["parameters"]["vocabulary"],json!({"SpeakMode":4}));
        let context=start["payload"]["input"]["context"].as_array().unwrap();assert!(context.len()>1&&context.len()<=5);
        let mut entries=Vec::new();for message in context {assert_eq!(message["role"],"user");assert_eq!(message["content"][0]["type"],"input_text");let text=message["content"][0]["text"].as_str().unwrap();assert!(text.chars().count()<=400);entries.extend(text.lines().map(str::to_owned));}
        for term in terms.iter().filter(|term|term.as_str()!="SpeakMode"){assert!(entries.contains(term));}assert_eq!(entries.len(),18);
        emit(&mut socket,id,"task-started",json!({})).await;receive(&mut socket).await;emit(&mut socket,id,"task-finished",json!({})).await;
    }).await;
    let(tx,rx)=mpsc::channel(1);tx.send(AsrInput::Finish{next_sequence:0}).await.unwrap();
    run_at(config(),"synthetic-key".into(),"zh".into(),dictionary,"long-term-attempt".into(),rx,CancellationToken::new(),ignored(),url,budgets()).await.unwrap();peer.await.unwrap();
}

#[tokio::test]
async fn oversized_domain_context_and_unsupported_language_fail_before_connection() {
    let mut store=crate::core::Store::in_memory().unwrap();
    let dictionary=store.import_dictionary((0..40).map(|i|format!("one two three four five six seven eight nine long term entry {i}")).collect(),1).unwrap();
    let(_tx,rx)=mpsc::channel(1);
    let error=run_at(config(),"synthetic-key".into(),"auto".into(),dictionary,"capacity".into(),rx,CancellationToken::new(),ignored(),"not-a-websocket-url".into(),budgets()).await.unwrap_err();
    assert_eq!(error.code,"dictionary_invalid");assert_eq!(error.stage,"configuration");assert!(error.message.contains("上下文容量"));
    let(_tx,rx)=mpsc::channel(1);
    assert_eq!(run_at(config(),"synthetic-key".into(),"ja".into(),DictionarySnapshot::default(),"language".into(),rx,CancellationToken::new(),ignored(),"not-a-websocket-url".into(),budgets()).await.unwrap_err().code,"unsupported_asr_language");
}
