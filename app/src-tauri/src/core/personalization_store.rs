// Included within Store's module. Image/profile content lives in a deletable side table,
// while immutable output revisions retain only the frozen identity/version marker.
impl Store {
    fn ensure_personalization_live(&self,output:&OutputRevision)->StoreResult<()> {
        if let Some(snapshot)=output.writing_receipt.as_ref().and_then(|receipt|receipt.personalization.as_ref()).filter(|snapshot|!snapshot.reference_cleared) {
            if let Some(profile_id)=&snapshot.profile_id {
                let removed:bool=self.connection.query_row("SELECT EXISTS(SELECT 1 FROM personalization_deleted_profiles WHERE profile_id=?1)",[profile_id],|row|row.get(0))?;
                if removed {return Err(StoreError::Cancelled);}
            }
        }
        Ok(())
    }
    pub fn personalization(&self) -> StoreResult<PersonalizationConfig> {
        let (version,data):(u64,String)=self.connection.query_row("SELECT version,data FROM personalization_config WHERE singleton=1",[],|row|Ok((row.get(0)?,row.get(1)?)))?;
        let config:PersonalizationConfig=decode(data)?;
        config.validate().map_err(invalid)?;
        if config.version!=version {return Err(invalid("personalization_stored_version"));}
        Ok(config)
    }
    pub fn save_personalization(&mut self,mut config:PersonalizationConfig,expected_version:u64)->StoreResult<PersonalizationConfig> {
        config.validate().map_err(invalid)?;
        let current=self.personalization()?;
        if current.version!=expected_version {return Err(StoreError::Conflict{current:current.version.to_string()});}
        config.version=expected_version.checked_add(1).ok_or_else(||invalid("version overflow"))?;
        config.validate().map_err(invalid)?;
        let deleted:Vec<String>=current.profiles.iter().filter(|profile|!config.profiles.iter().any(|next|next.id==profile.id)).map(|profile|profile.id.clone()).collect();
        let guard=self.commit_guard.clone();let tx=CheckedTransaction::new(&mut self.connection,self.health.clone())?;
        for profile in &config.profiles {
            let removed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM personalization_deleted_profiles WHERE profile_id=?1)",[&profile.id],|row|row.get(0))?;
            if removed {return Err(invalid("personalization_deleted_profile_id"));}
        }
        let changed=tx.execute("UPDATE personalization_config SET version=?1,data=?2 WHERE singleton=1 AND version=?3",params![i64_checked(config.version)?,json(&config)?,i64_checked(expected_version)?])?;
        if changed!=1 {let version:u64=tx.query_row("SELECT version FROM personalization_config WHERE singleton=1",[],|row|row.get(0))?;return Err(StoreError::Conflict{current:version.to_string()});}
        for profile_id in &deleted {
            tx.execute("INSERT OR IGNORE INTO personalization_deleted_profiles(profile_id) VALUES(?1)",[profile_id])?;
            tx.execute("DELETE FROM personalization_references WHERE profile_id=?1",[profile_id])?;
        }
        tx.commit_guarded(guard.as_deref(),CommitKind::Revision,||{
            for memory in self.memory_sessions.values_mut() {
                for output in memory.outputs.values_mut() {
                    if let Some(snapshot)=output.writing_receipt.as_mut().and_then(|receipt|receipt.personalization.as_mut()) {
                        if snapshot.profile_id.as_ref().is_some_and(|id|deleted.contains(id)) {snapshot.clear_reference();}
                    }
                }
            }
        })?;
        Ok(config)
    }
    pub fn record_personalization_event(&mut self,mut event:PersonalizationEvent)->StoreResult<()> {
        event.validate().map_err(invalid)?;
        let guard=self.commit_guard.clone();let tx=CheckedTransaction::new(&mut self.connection,self.health.clone())?;
        let old:Option<String>=tx.query_row("SELECT data FROM personalization_events WHERE task_id=?1",[&event.task_id],|row|row.get(0)).optional()?;
        if let Some(old)=old {
            let old:PersonalizationEvent=decode(old)?;
            if old.kind!=event.kind || old.scene!=event.scene || old.profile_id!=event.profile_id || old.profile_version!=event.profile_version || old.created_at!=event.created_at
                || (old.personalized&&!event.personalized) || (old.status!=PersonalizationEventStatus::Started&&old.status!=event.status&&event.status!=PersonalizationEventStatus::Cancelled) {
                return Err(StoreError::Conflict{current:"personalization_event_identity_or_terminal_state".into()});
            }
            // A retried completion receipt is not an explicit feedback edit.
            // Preserve the user's newer evaluation when an older clone is replayed.
            // Revocation is the exception: cancelled comparisons are no longer valid.
            if event.feedback.is_none()&&event.status!=PersonalizationEventStatus::Cancelled {event.feedback=old.feedback;}
        }
        tx.execute("INSERT INTO personalization_events(task_id,created_at,data) VALUES(?1,?2,?3) ON CONFLICT(task_id) DO UPDATE SET data=excluded.data",params![event.task_id,event.created_at,json(&event)?])?;
        tx.commit_guarded(guard.as_deref(),CommitKind::Revision,||{})?;
        Ok(())
    }
    pub fn personalization_events(&self,since:Option<&str>)->StoreResult<Vec<PersonalizationEvent>> {
        let threshold=since.map(|since|chrono::DateTime::parse_from_rfc3339(since).map_err(|_|invalid("personalization_event_time"))).transpose()?;
        let mut statement=self.connection.prepare("SELECT data FROM personalization_events ORDER BY created_at,task_id")?;
        let serialized=statement.query_map([],|row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut events=Vec::with_capacity(serialized.len());
        for data in serialized {
            let event:PersonalizationEvent=decode(data)?;event.validate().map_err(invalid)?;
            if threshold.as_ref().is_none_or(|threshold|chrono::DateTime::parse_from_rfc3339(&event.created_at).is_ok_and(|time|time>=*threshold)) {events.push(event);}
        }
        Ok(events)
    }
    pub fn set_personalization_feedback(&mut self,task_id:&str,feedback:PersonalizationFeedback)->StoreResult<()> {
        let data:Option<String>=self.connection.query_row("SELECT data FROM personalization_events WHERE task_id=?1",[task_id],|row|row.get(0)).optional()?;
        let mut event:PersonalizationEvent=decode(data.ok_or(StoreError::NotFound)?)?;
        event.feedback=Some(feedback);self.record_personalization_event(event)
    }
    fn restore_personalization_reference(&self,output:&mut OutputRevision)->StoreResult<()> {
        let Some(marker)=output.writing_receipt.as_ref().and_then(|receipt|receipt.personalization.as_ref()) else{return Ok(());};
        let data:Option<String>=self.connection.query_row("SELECT data FROM personalization_references WHERE output_id=?1",[&output.id],|row|row.get(0)).optional()?;
        let snapshot=if let Some(data)=data {
            let snapshot:EffectivePersonalization=decode(data)?;snapshot.validate().map_err(invalid)?;
            if snapshot.version!=marker.version || snapshot.profile_id!=marker.profile_id || snapshot.scene!=marker.scene {return Err(invalid("personalization_reference_identity"));}
            snapshot
        }else{let mut cleared=marker.clone();cleared.clear_reference();cleared};
        output.writing_receipt.as_mut().expect("checked receipt").personalization=Some(snapshot);
        Ok(())
    }
}
