// Preview inputs retain their provenance. Synthetic lines are never ASR items.

#[derive(Debug, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct HistoryPreviewSource { session_id:String, source_revision_id:String }

#[derive(Debug)]
enum PreviewSourceInput { Synthetic(String), History(HistoryPreviewSource) }

#[derive(Debug)]
struct PreviewSource { revision:SourceRevision, kind:&'static str }

fn preview_options(p:&Value)->Result<(bool,Option<ModeSnapshot>)> {
    let compare=match p.get("compareBuiltins") {None=>false,Some(Value::Bool(value))=>*value,_=>return Err(ApiError::new("invalidInput","mode","试用方式无效"))};
    if compare{return Ok((true,None));}
    let draft:ModeDraft=parse(p["draft"].clone())?;
    if draft.name.trim().is_empty()||draft.name.chars().count()>40||draft.rules.trim().is_empty()||draft.rules.chars().count()>4000||draft.base_mode==BaseMode::Auto{return Err(ApiError::new("invalidInput","mode","请填写名称、有效规则与基础方式"));}
    Ok((false,Some(ModeSnapshot{mode_id:"preview".into(),name:draft.name,base_mode:draft.base_mode,rules:draft.rules,revision:0})))
}

fn preview_source_input(p:&Value)->Result<PreviewSourceInput> {
    let sample=p.get("sample");let text=p.get("text");let source=p.get("source");
    if source.is_some() {
        if sample.is_some()||text.is_some(){return Err(ApiError::new("invalidInput","mode","请选择历史来源或试用文本，不能同时提供"));}
        let history:HistoryPreviewSource=parse(source.unwrap().clone())?;
        if [&history.session_id,&history.source_revision_id].iter().any(|v|v.trim().is_empty()||v.len()>4000){return Err(ApiError::new("invalidInput","mode","历史来源身份无效"));}
        return Ok(PreviewSourceInput::History(history));
    }
    if sample.is_some()&&text.is_some(){return Err(ApiError::new("invalidInput","mode","试用文本字段重复"));}
    let sample=sample.or(text).and_then(Value::as_str).ok_or_else(||ApiError::new("invalidInput","mode","请填写试用文本或选择历史来源"))?;
    if sample.trim().is_empty()||sample.chars().count()>80_000||sample.lines().filter(|line|!line.trim().is_empty()).count()>2000{return Err(ApiError::new("invalidInput","mode","试用文本为空或过长，最多 80,000 字和 2,000 行"));}
    Ok(PreviewSourceInput::Synthetic(sample.into()))
}

impl AppHub {
    fn reserve_preview(self:&Arc<Self>,key:&str,source:&PreviewSource,cancel:CancellationToken,epoch:u64)->Result<AuxiliaryLease> {
        let operation=format!("preview:{key}");let mut i=self.inner.lock().unwrap();self.auxiliary_available()?;
        if self.lifecycle_epoch.load(std::sync::atomic::Ordering::SeqCst)!=epoch{return Err(ApiError::new("cancelled","preview","系统状态变化，试用已取消"));}
        if i.cancelled_operation_ids.contains(&operation){return Err(ApiError::new("cancelled","preview","试用已取消"));}
        if i.shortcut_capture.is_some(){return Err(ApiError::new("capture_busy","preview","请先结束快捷键录制"));}
        // A delete may win after the last DB read but before registration. Its
        // record tombstone and this source association are checked/published
        // under the same lock used by deletion's cancellation sweep.
        if source.kind=="history"&&i.guards.get(&source.revision.session_id).is_some_and(|guard|guard.is_cancelled()) {
            return Err(ApiError::new("cancelled","preview","来源记录已删除或取消，请重新选择"));
        }
        if let Some((_,old))=i.preview.take(){old.cancel();}
        i.preview=Some((key.into(),cancel.clone()));
        i.auxiliary_workers.insert(operation.clone(),cancel);
        if source.kind=="history"{i.preview_sources.insert(operation.clone(),source.revision.session_id.clone());}
        Ok(AuxiliaryLease{hub:Arc::clone(self),key:operation})
    }

    fn cancel_previews_for_source_locked(inner:&mut Inner,session_id:&str) {
        for (operation,source_session) in &inner.preview_sources {
            if source_session==session_id {if let Some(token)=inner.auxiliary_workers.get(operation){token.cancel();}}
        }
        if inner.preview.as_ref().is_some_and(|(run,_)|inner.preview_sources.get(&format!("preview:{run}")).is_some_and(|source|source==session_id)) {
            inner.preview.take().unwrap().1.cancel();
        }
        // Keep source associations until their futures actually release leases.
        // Cancellation cannot retract content already sent to the provider.
    }

    async fn load_preview_source(&self,input:PreviewSourceInput,key:&str)->Result<PreviewSource> {
        match input {
            PreviewSourceInput::Synthetic(sample)=>Ok(PreviewSource{kind:"synthetic",revision:SourceRevision{
                id:key.into(),session_id:key.into(),asr_attempt_id:"preview".into(),generation:0,provider:"preview".into(),model:String::new(),adapter_version:"synthetic-lines-v1".into(),complete:true,created_at:now(),
                items:sample.lines().filter(|line|!line.trim().is_empty()).enumerate().map(|(index,line)|SourceItem{id:format!("preview:{index}"),provider_item_id:index.to_string(),capture_order:index as u64,text:line.into(),is_final:true,start_ms:None,end_ms:None}).collect(),
            }}),
            PreviewSourceInput::History(history)=>{
                let detail=db_detail(self.db_read(DbCommand::GetResult{session_id:history.session_id.clone()}).await?)?;
                if detail.source.id!=history.source_revision_id||detail.source.session_id!=history.session_id{return Err(ApiError::new("revisionConflict","preview","识别来源已变化，请重新选择记录"));}
                Ok(PreviewSource{kind:"history",revision:detail.source})
            }
        }
    }

    async fn validate_preview_source(&self,source:&PreviewSource)->Result<()> {
        if source.kind=="history" {
            let current=db_detail(self.db_read(DbCommand::GetResult{session_id:source.revision.session_id.clone()}).await?)?;
            if current.source!=source.revision{return Err(ApiError::new("revisionConflict","preview","识别来源已变化，请重新试用"));}
        }
        Ok(())
    }
}

enum PreviewOutputs { Single(rewrite::RewriteResult), Comparison{light:rewrite::RewriteResult,structure:rewrite::RewriteResult} }

fn preview_cancelled()->CloudError {CloudError::new("cancelled","preview","试用已取消")}

/// One lease encloses this entire sequence. Both requests receive exactly the
/// same immutable source, provider snapshot and cancellation token. Nothing is
/// exposed until both succeed, and cancellation never starts the second call.
async fn run_preview_rewrites<F,Fut>(mode:Option<ModeSnapshot>,source:&SourceRevision,cancel:CancellationToken,mut call:F)->std::result::Result<PreviewOutputs,CloudError>
where F:FnMut(ModeSnapshot,SourceRevision,CancellationToken)->Fut,Fut:Future<Output=std::result::Result<rewrite::RewriteResult,CloudError>> {
    let modes=match mode {
        Some(mode)=>vec![mode],
        None=>vec![
            ModeSnapshot{mode_id:"light".into(),name:"轻整理".into(),base_mode:BaseMode::Light,rules:String::new(),revision:1},
            ModeSnapshot{mode_id:"structure".into(),name:"结构".into(),base_mode:BaseMode::Structure,rules:String::new(),revision:1},
        ],
    };
    let mut results=Vec::with_capacity(modes.len());
    for mode in modes {
        if cancel.is_cancelled(){return Err(preview_cancelled());}
        let result=tokio::select! {biased;
            _=cancel.cancelled()=>return Err(preview_cancelled()),
            result=call(mode,source.clone(),cancel.clone())=>result?,
        };
        if cancel.is_cancelled(){return Err(preview_cancelled());}
        results.push(result);
    }
    if results.len()==1 {Ok(PreviewOutputs::Single(results.remove(0)))}
    else {let structure=results.pop().unwrap();let light=results.pop().unwrap();Ok(PreviewOutputs::Comparison{light,structure})}
}

fn preview_output_value(result:rewrite::RewriteResult)->Value {
    json!({"resolvedMode":result.resolved_mode,"text":result.blocks.iter().map(|b|b.text.as_str()).collect::<Vec<_>>().join("\n\n"),"blocks":result.blocks})
}

fn preview_response(key:&str,source:&PreviewSource,result:PreviewOutputs)->Value {
    let mut value=json!({"id":key,"runId":key,"sourceKind":source.kind,"sourceItemCount":source.revision.items.len(),"sourceRevisionId":source.revision.id,"promptVersion":rewrite::REWRITE_PROMPT_VERSION});
    match result {
        PreviewOutputs::Single(result)=>value.as_object_mut().unwrap().extend(preview_output_value(result).as_object().unwrap().clone()),
        PreviewOutputs::Comparison{light,structure}=>value["comparison"]=json!({"light":preview_output_value(light),"structure":preview_output_value(structure)}),
    }
    value
}
