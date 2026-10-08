//! Qwen wire adapter: final order is based on capture metadata/previous-item chain,
//! never on completion arrival. Partial revisions are for display only.
use super::{CloudError, endpoints};
use crate::core::{DictionarySnapshot, ProviderConfig, SourceItem};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::{BTreeMap, HashSet}, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async_with_config, tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig}};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug)]
pub enum AsrInput {
    Frame { sequence:u64, pcm16:Vec<i16>, padding_samples:usize },
    /// Number of frames emitted by capture after its cutoff/drain boundary.
    /// Counting the receiver's own frames alone cannot detect a dropped tail.
    Finish { next_sequence:u64 },
}

#[derive(Default)]
struct AudioSequence { next:u64, padded_tail:bool, real_samples:u64 }
impl AudioSequence {
    fn frame(&mut self,sequence:u64,pcm:&[i16],padding:usize)->Result<(),CloudError> {
        if pcm.is_empty() || pcm.len()>1600 {
            return Err(CloudError::new("audio_frame_limit","asr","音频帧为空或超过100毫秒限制"));
        }
        if self.padded_tail || sequence!=self.next || padding>=pcm.len()
            || (padding>0 && (pcm.len()!=1600 || pcm[pcm.len()-padding..].iter().any(|sample|*sample!=0))) {
            return Err(CloudError::new("audio_incomplete","audio","音频帧顺序或尾帧无效，不能确认完整录音"));
        }
        let real_samples=self.real_samples+(pcm.len()-padding) as u64;
        if real_samples>16_000*600 {
            return Err(CloudError::new("audio_frame_limit","audio","本次录音超过10分钟限制"));
        }
        self.next=self.next.checked_add(1).ok_or_else(||CloudError::new("audio_incomplete","audio","音频帧计数无效"))?;
        self.real_samples=real_samples;
        self.padded_tail=padding>0;
        Ok(())
    }
    fn finish(&self,next_sequence:u64)->Result<(),CloudError> {
        if next_sequence!=self.next {
            return Err(CloudError::new("audio_incomplete","audio","音频尾帧尚未收齐，不能结束识别"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct CaptionItem { pub id:String, pub text:String, pub revision:u64, #[serde(rename="final")] pub is_final:bool, pub capture_order:u64 }
#[derive(Debug,Clone)]
pub enum AsrProgress { Configured, Caption(Vec<CaptionItem>), SpeechStart }

#[derive(Default,Debug)]
struct Item {
    partial:String, final_text:Option<String>, revision:u64,
    previous:Option<String>, predecessor_known:bool,
    start_ms:Option<u64>, end_ms:Option<u64>, seen:u64,
}
#[derive(Default)]
pub struct Assembler { items:BTreeMap<String,Item>, counter:u64, finished:bool }

impl Assembler {
    fn item(&mut self,id:&str)->&mut Item {
        let seen=self.counter;
        if !self.items.contains_key(id) { self.counter+=1; }
        self.items.entry(id.to_owned()).or_insert_with(||Item {seen,..Default::default()})
    }
    pub fn ingest(&mut self,event:&Value)->Result<bool,CloudError> {
        let kind=event["type"].as_str().ok_or_else(||CloudError::new("invalid_asr_event","asr","语音识别事件缺少类型"))?;
        if self.finished {
            if kind=="session.finished" {return Ok(false);}
            return Err(CloudError::new("late_asr_event","asr","识别已经结束，忽略迟到事件"));
        }
        let mut changed=false;
        match kind {
            "session.finished" => { self.finished=true; return Ok(false); }
            "error" | "conversation.item.input_audio_transcription.failed" => return Err(CloudError::new("asr_service_error","asr","语音服务返回错误，请检查 Key、地域、模型权限或额度")),
            "input_audio_buffer.speech_started" | "input_audio_buffer.speech_stopped" | "input_audio_buffer.committed" | "conversation.item.created" | "conversation.item.input_audio_transcription.text" | "conversation.item.input_audio_transcription.completed" => {
                let id=event["item_id"].as_str().or_else(||event.pointer("/item/id").and_then(Value::as_str)).ok_or_else(||CloudError::new("missing_item_id","asr","语音事件缺少句子身份，保留已有字幕"))?;
                if id.is_empty() || id.len()>128 || id.chars().any(char::is_control) {return Err(CloudError::new("invalid_item_id","asr","语音事件句子身份无效"));}
                if !self.items.contains_key(id) && self.items.len()>=500 {return Err(CloudError::new("asr_limit","asr","单次识别分段过多，请结束后重新口述"));}
                let item=self.item(id);
                if event.get("previous_item_id").is_some() {
                    let previous=match &event["previous_item_id"] {
                        Value::Null=>None,
                        Value::String(value) if value.is_empty()=>None,
                        Value::String(value) if value.len()<=128 && !value.chars().any(char::is_control)=>Some(value.clone()),
                        _=>return Err(CloudError::new("invalid_order","asr","识别段落前序身份无效")),
                    };
                    if item.predecessor_known && item.previous!=previous { return Err(CloudError::new("conflicting_order","asr","识别段落顺序冲突")); }
                    item.previous=previous; item.predecessor_known=true;
                }
                match kind {
                    "input_audio_buffer.speech_started" => set_time(&mut item.start_ms,event.get("audio_start_ms"))?,
                    "input_audio_buffer.speech_stopped" => set_time(&mut item.end_ms,event.get("audio_end_ms"))?,
                    "conversation.item.input_audio_transcription.text" => {
                        if item.final_text.is_none() {
                            let text=event["text"].as_str().ok_or_else(||CloudError::new("invalid_asr_event","asr","字幕事件内容无效"))?;
                            let stash=match event.get("stash") {None=>"",Some(Value::String(stash))=>stash.as_str(),_=>return Err(CloudError::new("invalid_asr_event","asr","字幕修订内容无效"))};
                            if text.len()+stash.len()>240_000 { return Err(CloudError::new("asr_limit","asr","识别段落超过容量限制")); }
                            let next=format!("{text}{stash}");
                            if next!=item.partial {item.partial=next;item.revision+=1;changed=true;}
                        }
                    }
                    "conversation.item.input_audio_transcription.completed" => {
                        let text=event["transcript"].as_str().ok_or_else(||CloudError::new("invalid_final","asr","最终识别结果格式无效"))?;
                        if let Some(old)=&item.final_text {
                            if old!=text { return Err(CloudError::new("conflicting_final","asr","重复最终结果不一致，不能自动交付")); }
                        } else {
                            if text.len()>240_000 { return Err(CloudError::new("asr_limit","asr","识别段落超过容量限制")); }
                            item.final_text=Some(text.to_owned()); item.revision+=1; changed=true;
                        }
                    }
                    _=>{}
                }
            }
            _=>{}
        }
        if self.items.values().map(|item|item.final_text.as_ref().unwrap_or(&item.partial).chars().count()).sum::<usize>()>80_000 {return Err(CloudError::new("asr_limit","asr","识别字幕超过容量限制"));}
        Ok(changed)
    }
    pub fn captions(&self,attempt:&str)->Vec<CaptionItem> {
        let mut rows=self.items.iter().collect::<Vec<_>>();
        rows.sort_by_key(|(_,i)| (i.start_ms.unwrap_or(u64::MAX), i.seen));
        if self.items.values().all(|item|item.predecessor_known) {
            let mut chained=Vec::new();let mut predecessor=None::<&str>;let mut seen=HashSet::new();
            loop {
                let next=self.items.iter().filter(|(id,item)|!seen.contains(id.as_str())&&item.previous.as_deref()==predecessor).collect::<Vec<_>>();
                if next.len()!=1 {break;}
                let (id,item)=next[0];seen.insert(id.as_str());chained.push((id,item));predecessor=Some(id);
            }
            if chained.len()==self.items.len() {rows=chained;}
        }
        rows.into_iter().enumerate().map(|(n,(id,i))|CaptionItem {id:format!("{attempt}:{id}"),text:i.final_text.as_ref().unwrap_or(&i.partial).clone(),revision:i.revision,is_final:i.final_text.is_some(),capture_order:n as u64}).collect()
    }
    pub fn finish(&self,attempt:&str)->Result<Vec<SourceItem>,CloudError> {
        if !self.finished || self.items.values().any(|i|i.final_text.is_none()) {
            return Err(CloudError::new("asr_incomplete","asr","尚未收齐所有最终识别段落，可复制已有字幕"));
        }
        if self.items.is_empty() { return Ok(Vec::new()); }
        // Timestamps must not hide an explicitly reported missing/cyclic predecessor.
        for (item_id,item) in &self.items {
            if let Some(previous)=&item.previous {
                if previous==item_id || !self.items.contains_key(previous) {return Err(CloudError::new("missing_predecessor","asr","识别缺段或顺序循环，保留已有字幕"));}
            }
            if item.start_ms.zip(item.end_ms).is_some_and(|(start,end)|start>end) {return Err(CloudError::new("invalid_order","asr","识别段落时间范围无效"));}
        }
        let mut ordered=Vec::new();
        if self.items.values().all(|i|i.start_ms.is_some()) {
            ordered=self.items.iter().collect();
            ordered.sort_by_key(|(_,i)|i.start_ms.unwrap());
            if ordered.windows(2).any(|w|w[0].1.start_ms==w[1].1.start_ms) {
                return Err(CloudError::new("ambiguous_order","asr","识别段落时间冲突"));
            }
            for (index,(_,item)) in ordered.iter().enumerate() {
                if item.predecessor_known {
                    let previous=index.checked_sub(1).map(|previous|ordered[previous].0.as_str());
                    if item.previous.as_deref()!=previous {return Err(CloudError::new("conflicting_order","asr","识别时间与前序链不一致"));}
                }
            }
        } else if self.items.len()==1 && !self.items.values().next().unwrap().predecessor_known {
            ordered=self.items.iter().collect();
        } else {
            if self.items.values().any(|i|!i.predecessor_known) { return Err(CloudError::new("unknown_order","asr","缺少识别段落顺序，不能自动整理")); }
            let mut predecessor=None::<String>;
            let mut seen=HashSet::new();
            loop {
                let next=self.items.iter().filter(|(id,i)|!seen.contains(id.as_str()) && i.previous==predecessor).collect::<Vec<_>>();
                if next.is_empty(){break;}
                if next.len()!=1 {return Err(CloudError::new("ambiguous_order","asr","识别段落顺序不唯一"));}
                let (id,item)=next[0]; seen.insert(id.as_str()); ordered.push((id,item)); predecessor=Some(id.clone());
            }
            if ordered.len()!=self.items.len() {return Err(CloudError::new("missing_predecessor","asr","识别缺段或顺序循环，保留已有字幕"));}
        }
        let rows=ordered.into_iter().enumerate().filter_map(|(n,(id,i))| {
            let text=i.final_text.as_ref().unwrap();
            (!text.trim().is_empty()).then(||SourceItem{id:format!("{attempt}:{id}"),provider_item_id:id.clone(),capture_order:n as u64,text:text.clone(),is_final:true,start_ms:i.start_ms,end_ms:i.end_ms})
        }).collect::<Vec<_>>();
        if rows.iter().map(|i|i.text.chars().count()).sum::<usize>()>80_000 { return Err(CloudError::new("asr_limit","asr","识别结果超过容量限制")); }
        Ok(rows)
    }
}

fn event(kind:&str)->Value { json!({"type":kind,"event_id":Uuid::new_v4().to_string()}) }

fn session_update(language:&str,dictionary:&DictionarySnapshot)->Result<Value,CloudError> {
    // Validate again at the adapter boundary, before opening a provider socket.
    crate::core::store::validate_dictionary(dictionary).map_err(|_|CloudError::new("dictionary_invalid","configuration","个人词典无效或超过识别上下文容量，请在词典页检查"))?;
    let mut update=event("session.update");
    update["session"]=json!({"input_audio_format":"pcm","sample_rate":16000,"turn_detection":{"type":"server_vad","threshold":0.0,"silence_duration_ms":400},"input_audio_transcription":{}});
    if language=="zh" || language=="en" {update["session"]["input_audio_transcription"]["language"]=json!(language);}
    if let Some(context)=dictionary.asr_context() {update["session"]["input_audio_transcription"]["corpus"]=json!({"text":context});}
    Ok(update)
}

pub async fn run(config:ProviderConfig, secret:String, language:String, dictionary:DictionarySnapshot, attempt:String, audio:mpsc::Receiver<AsrInput>, cancel:CancellationToken, callback:Arc<dyn Fn(AsrProgress)+Send+Sync>)->Result<Vec<SourceItem>,CloudError> {
    let url=endpoints(&config)?.asr;
    run_at(config,secret,language,dictionary,attempt,audio,cancel,callback,url,Timeouts::default()).await
}

#[derive(Clone,Copy)]
struct Timeouts { establishment:Duration,send:Duration,finish:Duration }
impl Default for Timeouts {fn default()->Self {Self{establishment:Duration::from_secs(10),send:Duration::from_secs(5),finish:Duration::from_secs(15)}}}

// Private transport seam: only run() can select a production URL; localhost injection lives in tests.
async fn run_at(config:ProviderConfig, secret:String, language:String, dictionary:DictionarySnapshot, attempt:String, mut audio:mpsc::Receiver<AsrInput>, cancel:CancellationToken, callback:Arc<dyn Fn(AsrProgress)+Send+Sync>, url:String, timeouts:Timeouts)->Result<Vec<SourceItem>,CloudError> {
    let update=session_update(&language,&dictionary)?;
    let connect_deadline=tokio::time::Instant::now()+timeouts.establishment;
    let request=authorized_request(&url,&secret,config.workspace.as_deref())?;
    let mut ws_config=WebSocketConfig::default();
    ws_config.max_message_size=Some(1024*1024); ws_config.max_frame_size=Some(1024*1024);
    let connect=connect_async_with_config(request,Some(ws_config),false);
    let (mut socket,_)=tokio::select! {biased;
        _=cancel.cancelled()=>return Err(CloudError::new("cancelled","asr","已取消识别")),
        r=tokio::time::timeout_at(connect_deadline,connect)=>r.map_err(|_|CloudError::new("asr_connect_timeout","asr","语音服务连接超时"))?.map_err(|_|CloudError::new("asr_connect_failed","asr","语音服务连接失败，请检查地域与 Key"))?,
    };
    send_message(&mut socket,Message::Text(update.to_string()),&cancel,connect_deadline.min(tokio::time::Instant::now()+timeouts.send)).await?;
    let mut assembler=Assembler::default();
    let mut audio_sequence=AudioSequence::default();
    let mut configured=false;
    let mut finishing=false;
    let mut finish_deadline=None;
    loop {
        let deadline=finish_deadline.unwrap_or(connect_deadline);
        if cancel.is_cancelled(){return Err(cancelled());}
        if (!configured||finishing)&&tokio::time::Instant::now()>=deadline {return Err(if finishing {CloudError::new("asr_finish_timeout","asr","语音服务未完成最终确认，保留已有字幕")} else {CloudError::new("asr_config_timeout","asr","语音服务未确认配置，已停止录音")});}
        tokio::select! {
            _=cancel.cancelled()=>return Err(cancelled()),
            _=tokio::time::sleep_until(deadline), if !configured || finishing=>return Err(if finishing {CloudError::new("asr_finish_timeout","asr","语音服务未完成最终确认，保留已有字幕")} else {CloudError::new("asr_config_timeout","asr","语音服务未确认配置，已停止录音")}),
            incoming=socket.next()=>{
                let message=incoming.ok_or_else(||CloudError::new("asr_disconnected","asr","语音服务提前断开，结果不完整"))?.map_err(|_|CloudError::new("asr_disconnected","asr","语音连接中断"))?;
                match message {
                    Message::Text(text)=>{
                        let value:Value=serde_json::from_str(&text).map_err(|_|CloudError::new("invalid_asr_event","asr","语音事件无法解析"))?;
                        if cancel.is_cancelled(){return Err(cancelled());}
                        match value["type"].as_str() {
                            Some("session.updated") if !configured=>{configured=true;callback(AsrProgress::Configured);},
                            Some("input_audio_buffer.speech_started")=>callback(AsrProgress::SpeechStart),
                            Some("session.finished") if !finishing=>return Err(CloudError::new("unexpected_finish","asr","语音服务提前结束，请重新口述")),
                            _=>{}
                        }
                        if assembler.ingest(&value)? {callback(AsrProgress::Caption(assembler.captions(&attempt)));}
                        if assembler.finished {if cancel.is_cancelled(){return Err(cancelled());}return assembler.finish(&attempt);}
                    }
                    Message::Ping(data)=>{let send_deadline=if !configured||finishing {deadline.min(tokio::time::Instant::now()+timeouts.send)} else {tokio::time::Instant::now()+timeouts.send};send_message(&mut socket,Message::Pong(data),&cancel,send_deadline).await?;},
                    Message::Close(_)=>return Err(CloudError::new("asr_disconnected","asr","语音服务未完成最终回执")),
                    _=>{}
                }
            },
            frame=audio.recv(), if configured && !finishing=>{
                let value=match frame {
                    Some(AsrInput::Frame{sequence,pcm16:pcm,padding_samples})=>{
                        audio_sequence.frame(sequence,&pcm,padding_samples)?;
                        let bytes=pcm.into_iter().flat_map(i16::to_le_bytes).collect::<Vec<_>>();
                        let mut value=event("input_audio_buffer.append");value["audio"]=json!(STANDARD.encode(bytes));value
                    }
                    Some(AsrInput::Finish{next_sequence})=>{audio_sequence.finish(next_sequence)?;finishing=true;finish_deadline=Some(tokio::time::Instant::now()+timeouts.finish);event("session.finish")},
                    None=>return Err(CloudError::new("audio_disconnected","asr","采音队列未正常收尾")),
                };
                send_message(&mut socket,Message::Text(value.to_string()),&cancel,finish_deadline.unwrap_or(tokio::time::Instant::now()+timeouts.send).min(tokio::time::Instant::now()+timeouts.send)).await?;
            }
        }
    }
}

fn cancelled()->CloudError {CloudError::new("cancelled","asr","已取消识别")}
fn set_time(slot:&mut Option<u64>,value:Option<&Value>)->Result<(),CloudError> {
    if let Some(value)=value {
        let time=value.as_u64().ok_or_else(||CloudError::new("invalid_order","asr","识别时间无效"))?;
        if slot.is_some_and(|old|old!=time) {return Err(CloudError::new("conflicting_order","asr","重复识别时间不一致"));}
        *slot=Some(time);
    }
    Ok(())
}
async fn send_message(socket:&mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,message:Message,cancel:&CancellationToken,deadline:tokio::time::Instant)->Result<(),CloudError> {
    tokio::select! {biased;
        _=cancel.cancelled()=>Err(cancelled()),
        result=tokio::time::timeout_at(deadline,socket.send(message))=>result.map_err(|_|CloudError::new("asr_send_timeout","asr","语音发送积压，已停止"))?.map_err(|_|CloudError::new("asr_send_failed","asr","语音数据发送失败")),
    }
}

fn authorized_request(url:&str,secret:&str,workspace:Option<&str>)->Result<tokio_tungstenite::tungstenite::http::Request<()>,CloudError> {
    let mut request=url.into_client_request().map_err(|_|CloudError::new("invalid_endpoint","configuration","服务地址无效"))?;
    request.headers_mut().insert("Authorization",format!("Bearer {secret}").parse().map_err(|_|CloudError::new("invalid_key","credentials","Key 格式无效"))?);
    if let Some(workspace)=workspace.filter(|s|!s.is_empty()) {
        request.headers_mut().insert("X-DashScope-WorkSpace",workspace.parse().map_err(|_|CloudError::new("invalid_workspace","configuration","业务空间 ID 无效"))?);
    }
    Ok(request)
}

#[cfg(test)]
#[path="asr_protocol_tests.rs"]
mod protocol_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_revision_does_not_accumulate_and_final_is_immutable() {
        let mut a=Assembler::default();
        a.ingest(&json!({"type":"conversation.item.input_audio_transcription.text","item_id":"1","text":"今天","stash":"应该"})).unwrap();
        a.ingest(&json!({"type":"conversation.item.input_audio_transcription.text","item_id":"1","text":"今天","stash":"不应该发布"})).unwrap();
        assert_eq!(a.captions("attempt")[0].text,"今天不应该发布");
        a.ingest(&json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"1","transcript":"今天不应该发布。"})).unwrap();
        assert!(!a.ingest(&json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"1","transcript":"今天不应该发布。"})).unwrap());
        assert!(a.ingest(&json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"1","transcript":"今天发布。"})).is_err());
    }
    #[test]
    fn final_completion_arrival_does_not_determine_source_order() {
        let mut a=Assembler::default();
        for v in [json!({"type":"input_audio_buffer.committed","item_id":"b","previous_item_id":"a"}),json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"b","transcript":"第二句"}),json!({"type":"input_audio_buffer.committed","item_id":"a","previous_item_id":null}),json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"a","transcript":"第一句"}),json!({"type":"session.finished"})] {a.ingest(&v).unwrap();}
        let rows=a.finish("attempt").unwrap();
        assert_eq!(rows.iter().map(|i|i.text.as_str()).collect::<Vec<_>>(),["第一句","第二句"]);
        assert_eq!(rows[0].id,"attempt:a");
    }
    #[test]
    fn missing_predecessor_or_unfinished_partial_is_not_a_complete_source() {
        let mut a=Assembler::default();
        a.ingest(&json!({"type":"input_audio_buffer.committed","item_id":"b","previous_item_id":"missing"})).unwrap();
        a.ingest(&json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"b","transcript":"第二句"})).unwrap();
        a.ingest(&json!({"type":"session.finished"})).unwrap();
        assert!(a.finish("attempt").is_err());
        let mut a=Assembler::default();
        a.ingest(&json!({"type":"conversation.item.input_audio_transcription.text","item_id":"a","text":"尚未结束","stash":""})).unwrap();
        a.ingest(&json!({"type":"session.finished"})).unwrap();
        assert!(a.finish("attempt").is_err());
    }
}
