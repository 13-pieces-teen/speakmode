//! Google Interactions speech transcription. Receives ordered capture frames,
//! sends one complete WAV after Finish, and never enables smart transcription:
//! the immutable source must remain distinct from the chosen rewriting service.
use super::{CloudError, asr::{AsrInput, AsrProgress, AudioSequence, CaptionItem, WireDiagnostics, AsrEventKind}};
use crate::core::{DictionarySnapshot, ProviderConfig, SourceItem};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value,json};
use std::{sync::{Arc,OnceLock},time::{Duration,Instant}};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const MAX_RESPONSE:usize=1024*1024;
const MAX_REQUEST:usize=20_000_000;

fn invalid()->CloudError {CloudError::new("invalid_asr_event","asr","Google 未返回完整的语音转写结果")}
fn client()->Result<&'static reqwest::Client,CloudError> {
    static CLIENT:OnceLock<Result<reqwest::Client,reqwest::Error>>=OnceLock::new();
    CLIENT.get_or_init(||reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8)).build()).as_ref()
        .map_err(|_|CloudError::new("asr_transport","asr","无法初始化语音服务连接"))
}

fn request_body(config:&ProviderConfig,language:&str,dictionary:&DictionarySnapshot,pcm:&[i16])->Result<Value,CloudError> {
    let model=super::models::validate_recognition(config,language)?;
    if pcm.is_empty() {return Err(CloudError::new("empty_audio","audio","未收到有效录音"));}
    if pcm.len() as u64>model.product_audio_limit_seconds*16000 {return Err(CloudError::new("audio_frame_limit","audio","Google 单次录音请控制在7分钟以内"));}
    crate::core::store::validate_dictionary(dictionary).map_err(|_|CloudError::new("invalid_dictionary","configuration","词典快照无效"))?;
    let bytes_len=(pcm.len()*2) as u32;
    let mut wav=Vec::with_capacity(44+bytes_len as usize);
    wav.extend_from_slice(b"RIFF");wav.extend_from_slice(&(36+bytes_len).to_le_bytes());wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());wav.extend_from_slice(&1u16.to_le_bytes());wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16000u32.to_le_bytes());wav.extend_from_slice(&32000u32.to_le_bytes());wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());wav.extend_from_slice(b"data");wav.extend_from_slice(&bytes_len.to_le_bytes());
    for sample in pcm {wav.extend_from_slice(&sample.to_le_bytes());}
    let languages=match language {"zh"=>vec!["zh-CN"],"en"=>vec!["en-US"],_=>vec![]};
    let mut transcription=json!({"mode":{"type":"verbatim"},"language_codes":languages});
    if !dictionary.entries.is_empty() {transcription["custom_vocabulary"]=json!(dictionary.entries.iter().map(|e|e.term.as_str()).take(1000).collect::<Vec<_>>());}
    Ok(json!({"model":config.asr_model,"input":[{"type":"audio","mime_type":"audio/wav","data":STANDARD.encode(wav)}],
        "generation_config":{"transcription_config":transcription},"store":false,"stream":false}))
}

fn parse(value:&Value,attempt:&str)->Result<Vec<SourceItem>,CloudError> {
    if value["status"]!="completed" {return Err(invalid());}
    let steps=value["steps"].as_array().filter(|s|!s.is_empty()).ok_or_else(invalid)?;
    let mut text=String::new();
    for step in steps {
        if step["type"]!="model_output" {return Err(invalid());}
        for content in step["content"].as_array().filter(|s|!s.is_empty()).ok_or_else(invalid)? {
            if content["type"]!="text" {return Err(invalid());}
            let part=content["text"].as_str().ok_or_else(invalid)?;
            if !text.is_empty() {text.push('\n');}text.push_str(part);
        }
    }
    let text=text.trim();
    if text.is_empty() {return Err(CloudError::new("empty_transcript","asr","没有识别到清晰语音，请再说一次"));}
    if text.len()>200_000||text.chars().any(|c|c.is_control()&&!matches!(c,'\n'|'\r'|'\t')) {return Err(invalid());}
    Ok(vec![SourceItem{id:format!("{attempt}:google-final"),provider_item_id:"google-final".into(),capture_order:0,
        text:text.into(),is_final:true,start_ms:None,end_ms:None}])
}

pub(super) async fn run_at(config:ProviderConfig,secret:String,language:String,dictionary:DictionarySnapshot,attempt:String,
    mut audio:mpsc::Receiver<AsrInput>,cancel:CancellationToken,callback:Arc<dyn Fn(AsrProgress)+Send+Sync>,url:String,timeout:Duration)->Result<Vec<SourceItem>,CloudError> {
    let model=super::models::validate_recognition(&config,&language)?;
    let mut diagnostics=WireDiagnostics::new(callback.clone());
    diagnostics.dictionary(&config,&dictionary,dictionary.entries.len(),0)?;
    let mut sequence=AudioSequence::default();let mut pcm=Vec::new();
    // Ready to consume capture; this batch adapter has no remote live session.
    callback(AsrProgress::Configured);
    let capture_deadline=tokio::time::Instant::now()+Duration::from_secs(model.product_audio_limit_seconds+15);
    loop {
        let input=tokio::select! {biased;_=cancel.cancelled()=>return Err(super::asr::cancelled()),
            _=tokio::time::sleep_until(capture_deadline)=>return Err(CloudError::new("asr_finish_timeout","asr","录音队列未及时结束")),
            input=audio.recv()=>input};
        match input {
            Some(AsrInput::Frame{sequence:ordinal,pcm16,padding_samples})=>{
                sequence.frame(ordinal,&pcm16,padding_samples)?;
                let real=pcm16.len()-padding_samples;
                if pcm.len() as u64+real as u64>model.product_audio_limit_seconds*16000 {return Err(CloudError::new("audio_frame_limit","audio","Google 单次录音请控制在7分钟以内"));}
                pcm.extend_from_slice(&pcm16[..real]);diagnostics.metadata.dequeued.add_frame(real,padding_samples);diagnostics.publish();
            },
            Some(AsrInput::Finish{next_sequence})=>{sequence.finish(next_sequence)?;
                diagnostics.metadata.finish_dequeued_at=Some(Instant::now());diagnostics.metadata.finish_expected_frames=Some(next_sequence);break;},
            None=>return Err(CloudError::new("audio_disconnected","asr","采音队列未正常收尾")),
        }
    }
    let deadline=tokio::time::Instant::now()+timeout;
    let body=serde_json::to_vec(&request_body(&config,&language,&dictionary,&pcm)?).map_err(|_|invalid())?;
    if body.len()>MAX_REQUEST {return Err(CloudError::new("audio_frame_limit","audio","Google 音频请求超过本次大小限制"));}
    let request=client()?.post(url).header("x-goog-api-key",secret).header("Content-Type","application/json").body(body);
    let work=async {
        let mut response=request.send().await.map_err(|_|CloudError::new("asr_connection_failed","asr","无法连接 Google 语音服务"))?;
        // reqwest exposes response headers, not the instant its body reached the
        // socket. Leave WebSocket-only write counters/timestamps unobserved;
        // never count stripped capture padding as uploaded audio.
        diagnostics.publish();
        let status=response.status();
        if !status.is_success() {return Err(match status.as_u16() {
            401|403=>CloudError::new("asr_auth_failed","asr","Google API Key 无效或没有模型权限"),
            429=>CloudError::new("asr_rate_limited","asr","Google 语音服务暂时限流，请稍后重试"),
            _=>CloudError::new("asr_service_unavailable","asr","Google 语音服务未完成识别"),
        });}
        if response.content_length().is_some_and(|n|n>MAX_RESPONSE as u64) {return Err(invalid());}
        let mut bytes=Vec::new();
        while let Some(chunk)=response.chunk().await.map_err(|_|invalid())? {if bytes.len()+chunk.len()>MAX_RESPONSE{return Err(invalid());}bytes.extend_from_slice(&chunk);}
        let items=parse(&serde_json::from_slice::<Value>(&bytes).map_err(|_|invalid())?,&attempt)?;
        diagnostics.event_summary(AsrEventKind::Final,0,items.len(),Some(0),None);
        diagnostics.event_summary(AsrEventKind::SessionFinished,0,items.len(),Some(0),None);
        Ok(items)
    };
    let items=tokio::select! {biased;_=cancel.cancelled()=>return Err(super::asr::cancelled()),
        result=tokio::time::timeout_at(deadline,work)=>result.map_err(|_|CloudError::new("asr_finish_timeout","asr","Google 语音识别等待超时"))??};
    if cancel.is_cancelled(){return Err(super::asr::cancelled());}
    callback(AsrProgress::Caption(items.iter().map(|item|CaptionItem{id:item.id.clone(),text:item.text.clone(),revision:1,is_final:true,capture_order:item.capture_order}).collect()));
    Ok(items)
}

#[cfg(test)]
#[path="asr_google_tests.rs"]
mod tests;
