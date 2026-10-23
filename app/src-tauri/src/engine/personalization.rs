//! Explicit profile snapshots and real, non-delivering paired trials.
use super::*;
pub(super) use crate::core::personalization::*;
use crate::core::writing_context::{WritingContext, WritingReceipt, selected_scene};
use std::time::{Duration, Instant};

pub(super) fn context_for(mode:&ModeSnapshot, language:DraftLanguage, config:&PersonalizationConfig)->Option<WritingContext> {
    let scene=selected_scene(mode).unwrap_or_default();
    let personalization=config.freeze(scene);
    if personalization.is_none() && selected_scene(mode).is_none() && mode.mode_id!="auto" && language==DraftLanguage::AsSpoken { return None; }
    Some(WritingContext {scene,language,input:None,personalization})
}
pub(super) fn receipt(context:Option<&WritingContext>)->Option<WritingReceipt> {
    context.map(|c|WritingReceipt{scene:c.scene,language:c.language,context_used:c.input.is_some(),contract_version:rewrite::writing_context::VERSION.into(),personalization:c.personalization.clone()})
}
pub(super) fn task_event(id:&str,kind:PersonalizationEventKind,context:Option<&WritingContext>,status:PersonalizationEventStatus,duration_ms:u64)->PersonalizationEvent {
    let reference=context.and_then(|c|c.personalization.as_ref());
    PersonalizationEvent {task_id:id.into(),kind,status,scene:context.map_or(AppWritingScene::General,|c|c.scene),profile_id:reference.and_then(|p|p.profile_id.clone()),profile_version:reference.map_or(0,|p|p.version),personalized:reference.is_some(),created_at:now(),duration_ms,feedback:None}
}
pub(super) fn finished(event:&PersonalizationEvent,status:PersonalizationEventStatus,duration_ms:u64)->PersonalizationEvent {
    let mut event=event.clone();event.status=status;event.duration_ms=duration_ms;event
}
pub(super) fn revoked(inner:&Inner, reference:&EffectivePersonalization)->bool {
    inner.personalization_revocations.get("*").is_some_and(|version|reference.version<*version)
        || reference.profile_id.as_ref().is_some_and(|id|inner.personalization_revocations.get(id).is_some_and(|version|reference.version<*version))
}

#[derive(Deserialize)]
#[serde(tag="kind",rename_all="camelCase",deny_unknown_fields)]
enum ComparisonSource {
    Text {text:String},
    History {#[serde(rename="sessionId")]session_id:String,#[serde(rename="sourceRevisionId")]source_revision_id:String},
}
#[derive(Clone,Serialize)]
#[serde(rename_all="camelCase")]
struct ComparisonArm {status:&'static str,text:Option<String>,error_code:Option<String>}
impl ComparisonArm {
    fn from_result(result:std::result::Result<rewrite::RewriteResult,CloudError>)->Self {
        match result {
            Ok(draft)=>Self{status:"success",text:Some(draft.blocks.iter().map(|b|b.text.as_str()).collect::<Vec<_>>().join("\n\n")),error_code:None},
            Err(error)=>Self{status:if error.code=="cancelled"{"cancelled"}else if error.code=="rewrite_timeout"{"timeout"}else{"failed"},text:None,error_code:Some(error.code)},
        }
    }
}
/// Each arm gets the ordinary total budget. Cancellation drops the actual
/// request before the lease, and never starts the second arm after cancellation.
async fn paired_trial<F,Fut>(baseline:WritingContext,personalized:WritingContext,cancel:CancellationToken,mut call:F)->(ComparisonArm,ComparisonArm)
where F:FnMut(WritingContext,CancellationToken)->Fut,Fut:Future<Output=std::result::Result<rewrite::RewriteResult,CloudError>> {
    let mut arms=Vec::new();
    for context in [baseline,personalized] {
        let result=if cancel.is_cancelled(){Err(preview_cancelled())}else{
            tokio::select!{biased;_=cancel.cancelled()=>Err(preview_cancelled()),result=call(context,cancel.child_token())=>result}
        };
        arms.push(ComparisonArm::from_result(if cancel.is_cancelled(){Err(preview_cancelled())}else{result}));
    }
    let personalized=arms.pop().unwrap();(arms.pop().unwrap(),personalized)
}

impl AppHub {
    pub(super) fn personalization_get(&self)->Result<Value> {
        let mut value=wire(self.db.snapshot().personalization)?;
        value["capability"]=json!({"supported":true,"protocol":"personalization-v1","modelQuality":"notEvaluated"});
        Ok(value)
    }
    pub(super) async fn personalization_save(self:&Arc<Self>,p:&Value)->Result<Value> {
        let _settings=self.reserve_settings()?;
        let config:PersonalizationConfig=parse(p["config"].clone())?;
        config.validate().map_err(|code|ApiError::new(code,"personalization","画像字段无效或超出上限，请检查后保存"))?;
        let expected_version=number(p,"expectedVersion")?;
        let old=self.db.snapshot().personalization;
        if old.version!=expected_version{return Err(ApiError::new("revisionConflict","personalization","画像已变化，当前草稿仍保留，请重新读取"));}
        let value=self.db_write(DbCommand::SavePersonalization{config,expected_version},self.operation_guard(),DbPriority::Normal).await?;
        let DbValue::Personalization(saved)=value else{return Err(ApiError::new("storage_protocol","personalization","画像保存回执无效"));};
        let deleted:Vec<String>=old.profiles.iter().filter(|profile|!saved.profiles.iter().any(|p|p.id==profile.id)).map(|p|p.id.clone()).collect();
        let stopped=old.enabled&&!saved.enabled;
        {
            let mut inner=self.inner.lock().unwrap();
            if stopped {inner.personalization_revocations.insert("*".into(),saved.version);}
            for id in &deleted {inner.personalization_revocations.insert(id.clone(),saved.version);}
            let revoked_profile=|profile:Option<&EffectivePersonalization>|profile.is_some_and(|profile|stopped||profile.profile_id.as_ref().is_some_and(|id|deleted.contains(id)));
            for (operation,profile) in &inner.personalization_workers {if revoked_profile(Some(profile)){if let Some(token)=inner.auxiliary_workers.get(operation){token.cancel();}}}
            if inner.candidate.as_ref().is_some_and(|candidate|revoked_profile(candidate.personalization.as_ref())) {
                if let Some(candidate)=inner.candidate.take(){candidate.cancel.cancel();if let Some(guard)=candidate.adopting{guard.cancel();}}
            }
            if let Some(session)=inner.active.as_mut(){session.revoke_personalization(stopped,&deleted);}
            if let Some(session)=inner.recovery.as_mut(){session.revoke_personalization(stopped,&deleted);}
        }
        self.publish_session();
        wire(saved)
    }
    pub(super) fn personalization_cancel(&self,p:&Value)->Result<Value> {
        let id=operation_id(p,"operationId")?;let operation=format!("personalization:{id}");
        let mut inner=self.inner.lock().unwrap();cancel_operation(&mut inner,&operation)?;
        if let Some(token)=inner.auxiliary_workers.get(&operation){token.cancel();}
        Ok(json!({"cancelled":true,"operationId":id}))
    }
    pub(super) async fn personalization_feedback(&self,p:&Value)->Result<Value> {
        let task_id=operation_id(p,"comparisonId")?.into();let feedback:PersonalizationFeedback=parse(p["verdict"].clone())?;
        self.db_write(DbCommand::SetPersonalizationFeedback{task_id,feedback},self.operation_guard(),DbPriority::Normal).await?;
        Ok(json!({"saved":true}))
    }
    pub(super) async fn record_personalization_event(&self,event:PersonalizationEvent)->Result<()> {
        self.db_write(DbCommand::RecordPersonalizationEvent{event},self.operation_guard(),DbPriority::Normal).await?;Ok(())
    }
    pub(super) async fn personalization_stats(&self,p:&Value)->Result<Value> {
        let window=p.get("window").and_then(Value::as_str).unwrap_or("week");
        if !matches!(window,"week"|"all"){return Err(ApiError::new("invalid_payload","personalization","统计时间范围无效"));}
        let since=(window=="week").then(||(chrono::Utc::now()-chrono::Duration::days(7)).to_rfc3339());
        let DbValue::PersonalizationEvents(events)=self.db_read(DbCommand::GetPersonalizationEvents{since:since.clone()}).await? else{return Err(ApiError::new("storage_protocol","personalization","统计回执无效"));};
        Ok(statistics(window,since,events))
    }
    pub(super) async fn personalization_compare(self:&Arc<Self>,p:&Value)->Result<Value> {
        let id=operation_id(p,"operationId")?.to_owned();let operation=format!("personalization:{id}");
        let source_input:ComparisonSource=parse(p["source"].clone())?;
        let source_input=match source_input {
            ComparisonSource::Text{text}=>preview_source_input(&json!({"text":text}))?,
            ComparisonSource::History{session_id,source_revision_id}=>preview_source_input(&json!({"source":{"sessionId":session_id,"sourceRevisionId":source_revision_id}}))?,
        };
        let (snapshot,epoch)={let mut inner=self.inner.lock().unwrap();self.auxiliary_available()?;claim_operation(&mut inner,&operation)?;(self.db.snapshot(),self.lifecycle_epoch.load(std::sync::atomic::Ordering::SeqCst))};
        let mode:ModeSnapshot=snapshot.modes.iter().find(|mode|Some(mode.id.as_str())==p.get("modeId").and_then(Value::as_str).or(Some(snapshot.preferences.default_mode_id.as_str()))).cloned().ok_or_else(||ApiError::new("notFound","personalization","整理场合不存在"))?.into();
        let personalized=context_for(&mode,snapshot.preferences.app_writing.output_language,&snapshot.personalization)
            .filter(|context|context.personalization.is_some()).ok_or_else(||ApiError::new("personalization_not_applicable","personalization","请启用画像，并为当前场合设置背景或表达偏好"))?;
        let reference=personalized.personalization.clone().unwrap();let mut baseline=personalized.clone();baseline.personalization=None;
        let source=self.load_preview_source(source_input,&id).await?;rewrite::validate_source(&source.revision)?;
        cloud::rewrite_endpoint(&snapshot.provider_config)?;
        let secret=credentials::read_for(&snapshot.provider_config,CredentialStage::Rewrite)?;
        self.validate_preview_source(&source).await?;
        let cancel=CancellationToken::new();
        let lease={let mut inner=self.inner.lock().unwrap();self.auxiliary_available()?;
            if self.lifecycle_epoch.load(std::sync::atomic::Ordering::SeqCst)!=epoch||inner.cancelled_operation_ids.contains(&operation)||revoked(&inner,&reference){return Err(ApiError::new("cancelled","personalization","对照已取消，请重新开始"));}
            if inner.shortcut_capture.is_some(){return Err(ApiError::new("capture_busy","personalization","请先结束快捷键录制"));}
            if source.kind=="history"&&inner.guards.get(&source.revision.session_id).is_some_and(|guard|guard.is_cancelled()){return Err(ApiError::new("cancelled","personalization","来源已删除"));}
            if !inner.auxiliary_workers.is_empty(){return Err(ApiError::new("processing_busy","personalization","请等待当前文字处理完成"));}
            inner.auxiliary_workers.insert(operation.clone(),cancel.clone());inner.personalization_workers.insert(operation.clone(),reference.clone());
            if source.kind=="history"{inner.preview_sources.insert(operation.clone(),source.revision.session_id.clone());}
            AuxiliaryLease{hub:Arc::clone(self),key:operation}
        };
        let started=Instant::now();let event=PersonalizationEvent{task_id:id.clone(),kind:PersonalizationEventKind::Comparison,status:PersonalizationEventStatus::Started,scene:personalized.scene,profile_id:reference.profile_id.clone(),profile_version:reference.version,personalized:true,created_at:now(),duration_ms:0,feedback:None};
        self.record_personalization_event(event.clone()).await?;
        let work=async {
            let config=&snapshot.provider_config;let dictionary=&snapshot.dictionary;let secret=&secret;let source=&source.revision;let mode=&mode;
            paired_trial(baseline,personalized,cancel.clone(),|context,token|async move {
                let execution=rewrite::execution::RewriteExecution::for_duration(Duration::from_secs(20));
                rewrite::rewrite_with_writing_context(config,secret,mode,source,dictionary,token,execution,&context).await
            }).await
        };
        let (baseline,personalized)=(AuxiliaryFuture{future:Some(Box::pin(work)),lease:Some(lease)}).await;
        let source_validation=self.validate_preview_source(&source).await;
        let cancelled=cancel.is_cancelled()||revoked(&self.inner.lock().unwrap(),&reference);
        let valid=!cancelled&&source_validation.is_ok()&&baseline.status=="success"&&personalized.status=="success";
        let mut finished=event;finished.duration_ms=started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        finished.status=if cancelled{PersonalizationEventStatus::Cancelled}else if valid{PersonalizationEventStatus::Success}else{PersonalizationEventStatus::Failed};
        self.record_personalization_event(finished.clone()).await?;
        let revoked_after_save={let inner=self.inner.lock().unwrap();revoked(&inner,&reference)||inner.cancelled_operation_ids.contains(&format!("personalization:{id}"))};
        if cancelled||cancel.is_cancelled()||revoked_after_save {
            if !cancelled{self.record_personalization_event(super::personalization::finished(&finished,PersonalizationEventStatus::Cancelled,finished.duration_ms)).await?;}
            return Err(ApiError::new("cancelled","personalization","对照已取消，迟到结果已丢弃"));
        }
        source_validation?;
        Ok(json!({"id":id,"comparisonId":id,"sourceKind":source.kind,"sourceRevisionId":source.revision.id,"modeSnapshot":mode,"scene":reference.scene,"valid":valid,"identical":valid&&baseline.text==personalized.text,"baseline":baseline,"personalized":personalized,"reference":reference,"provider":snapshot.provider_config.rewrite_provider,"model":snapshot.provider_config.rewrite_model}))
    }
}

fn statistics(window:&str,since:Option<String>,events:Vec<PersonalizationEvent>)->Value {
    let mut result=json!({"window":window,"since":since,"normal":{"total":0,"personalized":0},"preview":{"total":0,"personalized":0},"comparisons":{"total":0,"valid":0,"failed":0,"fallback":0,"cancelled":0,"pending":0,"feedback":{"better":0,"same":0,"worse":0,"total":0}},"scenes":[],"records":events});
    let mut scenes:HashMap<String,(u64,u64)>=HashMap::new();
    let increment=|result:&mut Value,path:&str|{let value=result.pointer(path).and_then(Value::as_u64).unwrap_or(0);*result.pointer_mut(path).unwrap()=json!(value+1);};
    for event in &events {
        match event.kind {
            PersonalizationEventKind::Normal|PersonalizationEventKind::Preview=>{
                let key=if event.kind==PersonalizationEventKind::Normal{"normal"}else{"preview"};increment(&mut result,&format!("/{key}/total"));
                if event.personalized{increment(&mut result,&format!("/{key}/personalized"));}
                if key=="normal"{let key=serde_json::to_value(event.scene).unwrap().as_str().unwrap().to_owned();let count=scenes.entry(key).or_default();count.0+=1;if event.personalized{count.1+=1;}}
            },
            PersonalizationEventKind::Comparison=>{
                increment(&mut result,"/comparisons/total");
                let key=match event.status{PersonalizationEventStatus::Started=>"pending",PersonalizationEventStatus::Success=>"valid",PersonalizationEventStatus::Fallback=>"fallback",PersonalizationEventStatus::Failed=>"failed",PersonalizationEventStatus::Cancelled=>"cancelled"};
                increment(&mut result,&format!("/comparisons/{key}"));
                if event.status==PersonalizationEventStatus::Success{if let Some(feedback)=event.feedback{let key=match feedback{PersonalizationFeedback::Better=>"better",PersonalizationFeedback::Same=>"same",PersonalizationFeedback::Worse=>"worse"};increment(&mut result,"/comparisons/feedback/total");increment(&mut result,&format!("/comparisons/feedback/{key}"));}}
            },
        }
    }
    let mut rows:Vec<_>=scenes.into_iter().map(|(scene,(count,personalized))|json!({"scene":scene,"count":count,"personalized":personalized})).collect();rows.sort_by_key(|v|v["scene"].as_str().unwrap().to_owned());result["scenes"]=json!(rows);result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn contexts()->(WritingContext,WritingContext) {
        let config=PersonalizationConfig{enabled:true,preferences:ExpressionStyle{paragraphs:true,..Default::default()},..Default::default()};
        let mode=ModeSnapshot{mode_id:"technical".into(),name:"技术".into(),base_mode:BaseMode::Light,rules:"保留条件".into(),revision:2};
        let personalized=context_for(&mode,DraftLanguage::English,&config).unwrap();
        let mut baseline=personalized.clone();baseline.personalization=None;(baseline,personalized)
    }
    fn draft(text:&str)->rewrite::RewriteResult {rewrite::RewriteResult{resolved_mode:BaseMode::Light,blocks:vec![OutputBlock{id:"fixture".into(),text:text.into(),source_refs:vec!["original-source".into()]}]}}
    #[tokio::test]
    async fn paired_calls_change_only_new_profile_and_keep_language_scene_and_input() {
        let (baseline,personalized)=contexts();let expected=baseline.clone();let observed=Arc::new(Mutex::new(Vec::new()));let calls=observed.clone();
        let (a,b)=paired_trial(baseline,personalized,CancellationToken::new(),move |context,_|{let calls=calls.clone();async move {calls.lock().unwrap().push(context);Ok(draft("原有事实"))}}).await;
        let calls=observed.lock().unwrap();assert_eq!(calls.len(),2);assert_eq!(calls[0],expected);let mut stripped=calls[1].clone();assert!(stripped.personalization.take().is_some());assert_eq!(stripped,expected);
        assert_eq!(a.status,"success");assert_eq!(b.status,"success");assert_eq!(a.text,b.text,"same output is not a claimed improvement");
    }
    #[tokio::test]
    async fn paired_failure_is_exposed_without_substituting_fixture_or_source_success() {
        let (baseline,personalized)=contexts();let (a,b)=paired_trial(baseline,personalized,CancellationToken::new(),|context,_|async move {
            if context.personalization.is_none(){Err(CloudError::new("rewrite_timeout","rewrite","timeout"))}else{Ok(draft("真实第二组"))}
        }).await;
        assert_eq!(a.status,"timeout");assert!(a.text.is_none());assert_eq!(b.status,"success");assert_eq!(b.text.as_deref(),Some("真实第二组"));
    }
    #[tokio::test]
    async fn cancellation_discards_late_success_and_never_starts_other_arm() {
        let (baseline,personalized)=contexts();let cancel=CancellationToken::new();let worker=cancel.clone();let calls=Arc::new(Mutex::new(0));let count=calls.clone();
        let (a,b)=paired_trial(baseline,personalized,cancel,move |_,_|{let worker=worker.clone();let count=count.clone();async move{*count.lock().unwrap()+=1;worker.cancel();Ok(draft("late"))}}).await;
        assert_eq!(*calls.lock().unwrap(),1);assert_eq!(a.status,"cancelled");assert_eq!(b.status,"cancelled");assert!(a.text.is_none());assert!(b.text.is_none());
    }
}
