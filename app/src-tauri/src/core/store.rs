use super::types::*;
use super::OnboardingProgress;
use super::personalization::*;
use super::{CommitGuard,commit_guard::CommitKind};
use std::sync::{Arc,Mutex};
use std::ops::Deref;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{de::DeserializeOwned, Serialize};
use std::{collections::{HashMap, HashSet}, path::Path};
use thiserror::Error;
use unicode_casefold::UnicodeCaseFold;
use unicode_categories::UnicodeCategories;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

pub type StoreResult<T> = Result<T, StoreError>;
#[derive(Debug, Clone, Error)]
pub enum StoreError {
    #[error("database is busy")] Busy,
    #[error("database worker has stopped")] WorkerStopped,
    #[error("database rollback/connection state is unconfirmed: {0}")] Unhealthy(String),
    #[error("database operation failed: {0}")] Database(String),
    #[error("stored data could not be decoded: {0}")] Serialization(String),
    #[error("record not found")] NotFound,
    #[error("version conflict; current version is {current}")] Conflict { current: String },
    #[error("invalid input: {0}")] Invalid(String),
    #[error("source revisions are immutable")] ImmutableSource,
    #[error("the session was deleted or cancelled")] Cancelled,
    #[error("the session is already complete")] AlreadyCompleted,
    #[error("delivery permission has already been consumed")] DeliveryConsumed,
    #[error("no undo is available")] UndoUnavailable,
}
impl StoreError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Busy => "db_busy", Self::WorkerStopped => "db_stopped", Self::Unhealthy(_) => "db_unhealthy",
            Self::Database(_) => "storageFailed", Self::Serialization(_) => "storageCorrupt",
            Self::NotFound => "notFound", Self::Conflict { .. } => "revisionConflict",
            Self::Invalid(_) => "invalidInput", Self::ImmutableSource => "immutableSource",
            Self::Cancelled => "cancelled", Self::AlreadyCompleted => "alreadyCompleted",
            Self::DeliveryConsumed => "deliveryConsumed", Self::UndoUnavailable => "undoUnavailable",
        }
    }
}
impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self { if matches!(&e,rusqlite::Error::SqliteFailure(code,_) if matches!(code.code,rusqlite::ErrorCode::DatabaseBusy|rusqlite::ErrorCode::DatabaseLocked)) {Self::Busy} else {Self::Database(e.to_string())} }
}
impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self { Self::Serialization(e.to_string()) }
}
fn json<T: Serialize>(v: &T) -> StoreResult<String> { Ok(serde_json::to_string(v)?) }
fn decode<T: DeserializeOwned>(s: String) -> StoreResult<T> { Ok(serde_json::from_str(&s)?) }
fn id() -> String { Uuid::new_v4().to_string() }
fn invalid(message: &str) -> StoreError { StoreError::Invalid(message.into()) }
fn i64_checked(value: u64) -> StoreResult<i64> { i64::try_from(value).map_err(|_| invalid("integer exceeds SQLite range")) }
fn folded(text: &str) -> String { text.nfkc().case_fold().collect() }
fn dictionary_term(raw: &str) -> StoreResult<String> {
    // Check before trimming: pasted newlines/tabs must not become silent word edits.
    if raw.chars().any(|c| c.is_control() || c.is_other_format() || c=='\u{2028}' || c=='\u{2029}') { return Err(invalid("dictionary_control")); }
    let term=raw.trim().to_owned();
    if term.is_empty() { return Err(invalid("dictionary_empty")); }
    if term.chars().count()>DICTIONARY_MAX_TERM_CHARS { return Err(invalid("dictionary_term_limit")); }
    Ok(term)
}
pub(crate) fn validate_dictionary(snapshot: &DictionarySnapshot) -> StoreResult<()> {
    if snapshot.version==0 { return Err(invalid("dictionary_version")); }
    if snapshot.entries.len()>DICTIONARY_MAX_ENTRIES { return Err(invalid("dictionary_entry_limit")); }
    let mut terms=HashSet::new();let mut ids=HashSet::new();
    for entry in &snapshot.entries {
        valid_id(&entry.id)?;
        if dictionary_term(&entry.term)?!=entry.term { return Err(invalid("dictionary_whitespace")); }
        if !terms.insert(folded(&entry.term)) || !ids.insert(entry.id.as_str()) { return Err(invalid("dictionary_duplicate")); }
    }
    if snapshot.asr_context().is_some_and(|context|context.len()>DICTIONARY_MAX_CONTEXT_BYTES) { return Err(invalid("dictionary_context_limit")); }
    Ok(())
}
fn valid_id(value: &str) -> StoreResult<()> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) { Err(invalid("invalid opaque ID")) } else { Ok(()) }
}
pub fn source_character_count(text: &str) -> u64 {
    // Count letters/numbers/marks; exclude whitespace, punctuation, symbols and controls.
    text.chars().filter(|c| c.is_alphanumeric() || c.is_mark()).count() as u64
}

const MIGRATION_V1: &str = r#"
CREATE TABLE preferences (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL, data TEXT NOT NULL);
CREATE TABLE provider_config (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL, data TEXT NOT NULL);
CREATE TABLE modes (id TEXT PRIMARY KEY, normalized_name TEXT NOT NULL, current_revision INTEGER NOT NULL, deleted INTEGER NOT NULL DEFAULT 0);
CREATE UNIQUE INDEX modes_active_name ON modes(normalized_name) WHERE deleted=0;
CREATE TABLE mode_revisions (mode_id TEXT NOT NULL REFERENCES modes(id), revision INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(mode_id,revision));
CREATE TABLE sessions (id TEXT PRIMARY KEY, seed TEXT NOT NULL, status TEXT NOT NULL, source_id TEXT NOT NULL, output_id TEXT, undo_id TEXT, ended_at TEXT NOT NULL, search_text TEXT NOT NULL);
CREATE TABLE sources (id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE, attempt_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(session_id,attempt_id));
CREATE TABLE source_items (source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE, item_id TEXT NOT NULL, provider_item_id TEXT NOT NULL, capture_order INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(source_id,item_id), UNIQUE(source_id,provider_item_id), UNIQUE(source_id,capture_order));
CREATE TABLE outputs (id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE, source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE, data TEXT NOT NULL);
CREATE TABLE usage_events (session_id TEXT PRIMARY KEY, source_char_count INTEGER NOT NULL CHECK(source_char_count>0), capture_duration_ms INTEGER NOT NULL CHECK(capture_duration_ms>=0), ended_at TEXT NOT NULL, end_local_date TEXT NOT NULL, time_zone TEXT NOT NULL);
CREATE INDEX usage_day ON usage_events(end_local_date);
CREATE TABLE delivery_attempts (id TEXT PRIMARY KEY, session_id TEXT NOT NULL UNIQUE, generation INTEGER NOT NULL, output_revision_id TEXT NOT NULL, target_fingerprint TEXT NOT NULL, claimed_at TEXT NOT NULL, state TEXT NOT NULL, reason TEXT);
CREATE TABLE content_tombstones (session_id TEXT PRIMARY KEY);
CREATE TRIGGER source_no_update BEFORE UPDATE ON sources BEGIN SELECT RAISE(ABORT,'immutable source'); END;
CREATE TRIGGER source_item_no_update BEFORE UPDATE ON source_items BEGIN SELECT RAISE(ABORT,'immutable source item'); END;
CREATE TRIGGER output_no_update BEFORE UPDATE ON outputs BEGIN SELECT RAISE(ABORT,'immutable output'); END;
CREATE TRIGGER mode_revision_no_update BEFORE UPDATE ON mode_revisions BEGIN SELECT RAISE(ABORT,'immutable mode revision'); END;
CREATE TRIGGER usage_no_update BEFORE UPDATE ON usage_events BEGIN SELECT RAISE(ABORT,'immutable usage'); END;
PRAGMA user_version=1;
"#;

const MIGRATION_V2: &str = r#"
CREATE TABLE dictionary_meta (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL CHECK(version>0));
INSERT INTO dictionary_meta(singleton,version) VALUES(1,1);
CREATE TABLE dictionary_entries (id TEXT PRIMARY KEY, normalized_term TEXT NOT NULL UNIQUE, term TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
PRAGMA user_version=2;
"#;

const MIGRATION_V3: &str = r#"
CREATE TABLE onboarding (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL CHECK(version>0), data TEXT NOT NULL);
PRAGMA user_version=3;
"#;

include!("onboarding_store.rs");
include!("personalization_store.rs");

const MIGRATION_V4: &str = r#"
CREATE TABLE personalization_config (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL CHECK(version>0), data TEXT NOT NULL);
CREATE TABLE personalization_events (task_id TEXT PRIMARY KEY, created_at TEXT NOT NULL, data TEXT NOT NULL);
CREATE INDEX personalization_event_time ON personalization_events(created_at);
CREATE TABLE personalization_deleted_profiles (profile_id TEXT PRIMARY KEY);
CREATE TABLE personalization_references (output_id TEXT PRIMARY KEY REFERENCES outputs(id) ON DELETE CASCADE, profile_id TEXT, data TEXT NOT NULL);
CREATE INDEX personalization_reference_profile ON personalization_references(profile_id);
PRAGMA user_version=4;
"#;

#[derive(Clone)]
struct MemorySession {
    record: SessionRecord,
    source: SourceRevision,
    outputs: HashMap<String, OutputRevision>,
}

/// Synchronous store, intended to live on exactly one DB worker. No network or native actions here.
/// Production writes use with_commit_guard; only final COMMIT and memory publication hold its gate.
pub struct Store {
    connection: Connection,
    memory_sessions: HashMap<String, MemorySession>,
    latest_memory_ready: Option<String>,
    commit_guard:Option<Arc<CommitGuard>>,
    health:Arc<Mutex<Option<StoreError>>>,
}
impl Store {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> { Self::from_connection(Connection::open(path)?) }
    pub fn new(path: impl AsRef<Path>) -> StoreResult<Self> { Self::open(path) }
    pub fn in_memory() -> StoreResult<Self> { Self::from_connection(Connection::open_in_memory()?) }
    fn from_connection(mut connection: Connection) -> StoreResult<Self> {
        let health=Arc::new(Mutex::new(None));
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(2))?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > 4 { return Err(invalid("database schema is newer than this application")); }
        if version == 0 {
            let tx = CheckedTransaction::new(&mut connection,health.clone())?;
            tx.execute_batch(MIGRATION_V1)?;
            tx.execute("INSERT INTO preferences(singleton,version,data) VALUES(1,1,?1)", [json(&Preferences::default())?])?;
            tx.execute("INSERT INTO provider_config(singleton,version,data) VALUES(1,1,?1)", [json(&ProviderConfig::default())?])?;
            tx.commit()?;
        }
        if version < 2 {
            let tx = CheckedTransaction::new(&mut connection,health.clone())?;
            tx.execute_batch(MIGRATION_V2)?;
            tx.commit()?;
        }
        if version < 3 {
            let tx = CheckedTransaction::new(&mut connection,health.clone())?;
            tx.execute_batch(MIGRATION_V3)?;
            tx.execute("INSERT INTO onboarding(singleton,version,data) VALUES(1,1,?1)", [json(&OnboardingProgress::new_install(version == 0))?])?;
            tx.commit()?;
        }
        if version < 4 {
            let tx = CheckedTransaction::new(&mut connection,health.clone())?;
            tx.execute_batch(MIGRATION_V4)?;
            tx.execute("INSERT INTO personalization_config(singleton,version,data) VALUES(1,1,?1)", [json(&PersonalizationConfig::default())?])?;
            tx.commit()?;
        }
        Ok(Self { connection, memory_sessions: HashMap::new(), latest_memory_ready: None,commit_guard:None,health })
    }

    pub fn with_commit_guard<T>(&mut self,guard:Arc<CommitGuard>,operation:impl FnOnce(&mut Self)->StoreResult<T>)->StoreResult<T> {
        if let Some(error)=self.health_error(){return Err(error);}
        let previous=self.commit_guard.replace(guard);
        let result=operation(self);
        self.commit_guard=previous;
        if let Some(error)=self.health_error(){return Err(error);}
        result
    }
    pub fn health_error(&self)->Option<StoreError>{self.health.lock().unwrap_or_else(|p|p.into_inner()).clone()}
    /// One consistent SQLite read view for related DTOs and full snapshots.
    pub(crate) fn read_consistent<T>(&self,read:impl FnOnce(&Self)->StoreResult<T>)->StoreResult<T>{
        let tx=CheckedTransaction{tx:Some(self.connection.unchecked_transaction()?),health:self.health.clone()};
        let result=read(self)?;tx.commit()?;Ok(result)
    }
    pub fn preferences(&self) -> StoreResult<Preferences> {
        decode(self.connection.query_row("SELECT data FROM preferences WHERE singleton=1", [], |r| r.get(0))?)
    }
    /// Caller completes hook/autostart registration first, then persists these already-applied values.
    pub fn save_preferences(&mut self, mut next: Preferences, expected_version: u64) -> StoreResult<Preferences> {
        next.app_writing.validate().map_err(invalid)?;
        if !matches!(next.language.as_str(), "auto" | "zh" | "en") { return Err(invalid("unsupported recognition language")); }
        if next.shortcut.is_empty() || next.shortcut.len()>128 || !next.shortcut.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c,'_'|'+'|'-')) { return Err(invalid("invalid canonical shortcut")); }
        self.mode_snapshot(&next.default_mode_id, None)?;
        next.version = expected_version.checked_add(1).ok_or_else(|| invalid("version overflow"))?;
        let guard=self.commit_guard.clone();let tx=CheckedTransaction::new(&mut self.connection,self.health.clone())?;
        let changed=tx.execute("UPDATE preferences SET version=?1,data=?2 WHERE singleton=1 AND version=?3",params![i64_checked(next.version)?,json(&next)?,i64_checked(expected_version)?])?;
        if changed==0{let current:u64=tx.query_row("SELECT version FROM preferences WHERE singleton=1",[],|r|r.get(0))?;return Err(StoreError::Conflict{current:current.to_string()});}
        tx.commit_guarded(guard.as_deref(),CommitKind::Revision,||{})?;

        Ok(next)
    }
    pub fn reset_preferences(&mut self, expected_version: u64) -> StoreResult<Preferences> { self.save_preferences(Preferences::default(), expected_version) }

    pub fn dictionary(&self) -> StoreResult<DictionarySnapshot> {
        let version = self.connection.query_row("SELECT version FROM dictionary_meta WHERE singleton=1", [], |row| row.get(0))?;
        let mut statement = self.connection.prepare("SELECT id,term,created_at,updated_at FROM dictionary_entries ORDER BY normalized_term,id")?;
        let entries = statement.query_map([], |row| Ok(DictionaryEntry { id: row.get(0)?, term: row.get(1)?, created_at: row.get(2)?, updated_at: row.get(3)? }))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let snapshot = DictionarySnapshot { version, entries };
        validate_dictionary(&snapshot)?;
        Ok(snapshot)
    }
    fn checked_dictionary(&self, expected_version: u64) -> StoreResult<DictionarySnapshot> {
        let snapshot = self.dictionary()?;
        if snapshot.version != expected_version { return Err(StoreError::Conflict { current: snapshot.version.to_string() }); }
        Ok(snapshot)
    }
    pub fn save_dictionary_entry(&mut self, draft: DictionaryDraft, expected_version: u64) -> StoreResult<DictionarySnapshot> {
        let mut next = self.checked_dictionary(expected_version)?;
        let term = dictionary_term(&draft.term)?;
        let normalized = folded(&term);
        if next.entries.iter().any(|entry| folded(&entry.term)==normalized && draft.id.as_deref()!=Some(entry.id.as_str())) {
            return Err(invalid("dictionary_duplicate"));
        }
        let timestamp = chrono::Utc::now().to_rfc3339();
        if let Some(entry_id) = draft.id {
            valid_id(&entry_id)?;
            let entry = next.entries.iter_mut().find(|entry| entry.id==entry_id).ok_or(StoreError::NotFound)?;
            if entry.term==term { return self.publish_dictionary(next,expected_version,false); }
            entry.term=term;entry.updated_at=timestamp;
        } else {
            next.entries.push(DictionaryEntry { id: id(), term, created_at: timestamp.clone(), updated_at: timestamp });
        }
        self.publish_dictionary(next,expected_version,true)
    }
    /// Atomic append: normalize for deduplication, preserve the first user spelling.
    pub fn import_dictionary(&mut self, terms: Vec<String>, expected_version: u64) -> StoreResult<DictionarySnapshot> {
        if terms.is_empty() || terms.len()>DICTIONARY_MAX_ENTRIES { return Err(invalid("dictionary_import_limit")); }
        let mut next = self.checked_dictionary(expected_version)?;
        let mut normalized = next.entries.iter().map(|entry| folded(&entry.term)).collect::<HashSet<_>>();
        let timestamp = chrono::Utc::now().to_rfc3339();
        let mut changed = false;
        for raw in terms {
            let term = dictionary_term(&raw)?;
            if normalized.insert(folded(&term)) {
                next.entries.push(DictionaryEntry { id:id(),term,created_at:timestamp.clone(),updated_at:timestamp.clone() });changed=true;
            }
        }
        self.publish_dictionary(next,expected_version,changed)
    }
    pub fn delete_dictionary_entry(&mut self, entry_id: &str, expected_version: u64) -> StoreResult<DictionarySnapshot> {
        valid_id(entry_id)?;
        let mut next=self.checked_dictionary(expected_version)?;
        let position=next.entries.iter().position(|entry|entry.id==entry_id).ok_or(StoreError::NotFound)?;
        next.entries.remove(position);
        self.publish_dictionary(next,expected_version,true)
    }
    fn publish_dictionary(&mut self, mut next: DictionarySnapshot, expected_version: u64, changed: bool) -> StoreResult<DictionarySnapshot> {
        next.entries.sort_by_cached_key(|entry|(folded(&entry.term),entry.id.clone()));
        validate_dictionary(&next)?;
        let guard=self.commit_guard.clone();
        let tx=CheckedTransaction::new(&mut self.connection,self.health.clone())?;
        if changed { next.version=expected_version.checked_add(1).ok_or_else(||invalid("version overflow"))?; }
        // Acquire the writer directly instead of upgrading a read transaction,
        // which SQLite can reject immediately while another writer is waiting.
        let updated=tx.execute("UPDATE dictionary_meta SET version=?1 WHERE singleton=1 AND version=?2",params![i64_checked(next.version)?,i64_checked(expected_version)?])?;
        if updated!=1 { let current:u64=tx.query_row("SELECT version FROM dictionary_meta WHERE singleton=1",[],|row|row.get(0))?;return Err(StoreError::Conflict{current:current.to_string()}); }
        if changed {
            tx.execute("DELETE FROM dictionary_entries",[])?;
            for entry in &next.entries {
                tx.execute("INSERT INTO dictionary_entries(id,normalized_term,term,created_at,updated_at) VALUES(?1,?2,?3,?4,?5)",params![entry.id,folded(&entry.term),entry.term,entry.created_at,entry.updated_at])?;
            }
        }
        // A no-op is still arbitrated at completion, so accepted cancellation cannot
        // masquerade as a successful dictionary operation.
        tx.commit_guarded(guard.as_deref(),CommitKind::Revision,||{})?;
        Ok(next)
    }
    pub fn provider_config(&self) -> StoreResult<ProviderConfig> {
        decode(self.connection.query_row("SELECT data FROM provider_config WHERE singleton=1", [], |r| r.get(0))?)
    }
    /// Product upgrades run only during application bootstrap, never when a tool opens Store.
    /// This additive table stays on schema v2. JSON configuration compatibility
    /// is separate: older binaries may reject newly added provider fields.
    pub(crate) fn apply_app_upgrades(&mut self) -> StoreResult<()> {
        const UPGRADE: &str = "rewrite-model-0.2.11";
        let tx = CheckedTransaction {
            tx: Some(self.connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?),
            health: self.health.clone(),
        };
        tx.execute_batch("CREATE TABLE IF NOT EXISTS app_upgrades (key TEXT PRIMARY KEY NOT NULL);")?;
        let applied = tx.query_row("SELECT 1 FROM app_upgrades WHERE key=?1", [UPGRADE], |_| Ok(())).optional()?.is_some();
        if !applied {
            let (version, data): (u64, String) = tx.query_row("SELECT version,data FROM provider_config WHERE singleton=1", [], |row| Ok((row.get(0)?, row.get(1)?)))?;
            let mut config: ProviderConfig = decode(data)?;
            if config.version != version { return Err(StoreError::Serialization("provider configuration version does not match its row".into())); }
            if config.provider == "aliyun" && config.rewrite_provider == "aliyun" && config.region.as_deref() == Some("beijing") && config.rewrite_model == "qwen-flash" {
                config.rewrite_model = "qwen3.8-flash".into();
                config.version = version.checked_add(1).ok_or_else(|| invalid("version overflow"))?;
                tx.execute("UPDATE provider_config SET version=?1,data=?2 WHERE singleton=1", params![i64_checked(config.version)?, json(&config)?])?;
            }
            // Mark skipped profiles too: future user choices must not become upgrade candidates.
            tx.execute("INSERT INTO app_upgrades(key) VALUES(?1)", [UPGRADE])?;
        }
        tx.commit()
    }
    pub fn save_provider_config(&mut self, mut config: ProviderConfig, expected_version: u64) -> StoreResult<ProviderConfig> {
        if !matches!(config.provider.as_str(), "aliyun" | "google")
            || !matches!(config.rewrite_provider.as_str(), "aliyun" | "openai" | "deepseek" | "kimi" | "zhipu" | "custom")
            || !matches!(config.region.as_deref(), None | Some("beijing") | Some("singapore")) { return Err(invalid("unsupported provider or region")); }
        if config.rewrite_provider == "custom" {
            crate::cloud::custom_rewrite_endpoint(config.rewrite_endpoint.as_deref()).map_err(|_|invalid("invalid compatible endpoint"))?;
        } else if config.rewrite_endpoint.is_some() { return Err(invalid("preset provider cannot override its endpoint")); }
        for value in [&config.asr_model, &config.rewrite_model, &config.adapter_version] { valid_id(value)?; }
        if let Some(workspace) = &config.workspace { valid_id(workspace)?; }
        config.version = expected_version.checked_add(1).ok_or_else(|| invalid("version overflow"))?;
        let guard=self.commit_guard.clone();let tx=CheckedTransaction::new(&mut self.connection,self.health.clone())?;
        let changed=tx.execute("UPDATE provider_config SET version=?1,data=?2 WHERE singleton=1 AND version=?3",params![i64_checked(config.version)?,json(&config)?,i64_checked(expected_version)?])?;
        if changed==0{let current:u64=tx.query_row("SELECT version FROM provider_config WHERE singleton=1",[],|r|r.get(0))?;return Err(StoreError::Conflict{current:current.to_string()});}
        tx.commit_guarded(guard.as_deref(),CommitKind::Revision,||{})?;

        Ok(config)
    }

    pub fn list_modes(&self) -> StoreResult<Vec<ModeRecord>> {
        let mut modes = builtin_modes();
        let mut stmt = self.connection.prepare("SELECT r.data FROM modes m JOIN mode_revisions r ON r.mode_id=m.id AND r.revision=m.current_revision WHERE m.deleted=0 ORDER BY m.normalized_name,m.id")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? { modes.push(decode(row?)?); }
        Ok(modes)
    }
    pub fn mode_snapshot(&self, mode_id: &str, revision: Option<u64>) -> StoreResult<ModeSnapshot> {
        if let Some(mode) = builtin_modes().into_iter().find(|m| m.id==mode_id) {
            if revision.is_some_and(|r| r!=1) { return Err(StoreError::NotFound); }
            return Ok(mode.into());
        }
        let data: Option<String> = if let Some(rev) = revision {
            // Frozen revisions remain accessible after soft deletion.
            self.connection.query_row("SELECT data FROM mode_revisions WHERE mode_id=?1 AND revision=?2", params![mode_id,i64_checked(rev)?], |r|r.get(0)).optional()?
        } else {
            self.connection.query_row("SELECT r.data FROM modes m JOIN mode_revisions r ON r.mode_id=m.id AND r.revision=m.current_revision WHERE m.id=?1 AND m.deleted=0", [mode_id], |r|r.get(0)).optional()?
        };
        Ok(decode::<ModeRecord>(data.ok_or(StoreError::NotFound)?)?.into())
    }
    pub fn save_mode(&mut self, draft: ModeDraft) -> StoreResult<ModeRecord> {
        if draft.base_mode == BaseMode::Auto { return Err(invalid("personal Mode needs light or structure base")); }
        let name = draft.name.trim().to_string();
        let rules = draft.rules.trim().to_string();
        if name.chars().count()==0 || name.chars().count()>40 || name.chars().any(char::is_control) { return Err(invalid("Mode name needs 1–40 printable characters")); }
        if rules.is_empty() || rules.chars().count()>4000 { return Err(invalid("Mode rules need 1–4000 characters")); }
        let normalized = folded(&name);
        let reserved=["auto","automatic","light","structure","自动","轻整理","结构","chat","document","email","technical","聊天消息","文档写作","邮件沟通","技术沟通"].iter().any(|reserved| folded(reserved)==normalized);
        let retained_personal_name=draft.id.as_ref().filter(|id|!builtin_modes().iter().any(|mode|&mode.id==*id))
            .and_then(|id|self.mode_snapshot(id,None).ok()).is_some_and(|current|folded(&current.name)==normalized);
        if reserved && !retained_personal_name { return Err(invalid("Mode name conflicts with a built-in Mode")); }
        let mode_id = draft.id.clone().unwrap_or_else(id);
        if builtin_modes().iter().any(|m|m.id==mode_id) { return Err(invalid("built-in Modes cannot be modified")); }
        let current: Option<u64> = self.connection.query_row("SELECT current_revision FROM modes WHERE id=?1 AND deleted=0", [&mode_id], |r|r.get(0)).optional()?;
        let revision = match (draft.id.is_some(),current,draft.expected_revision) {
            (false,None,None) => 1,
            (true,Some(current),Some(expected)) if current==expected => current.checked_add(1).ok_or_else(||invalid("version overflow"))?,
            (true,Some(current),_) => return Err(StoreError::Conflict {current:current.to_string()}),
            _ => return Err(StoreError::NotFound),
        };
        let collision: bool = self.connection.query_row("SELECT EXISTS(SELECT 1 FROM modes WHERE normalized_name=?1 AND deleted=0 AND id!=?2)", params![normalized,mode_id], |r|r.get(0))?;
        if collision { return Err(invalid("a Mode with this normalized name already exists")); }
        let result = ModeRecord { id: mode_id, name, base_mode:draft.base_mode, rules, revision, builtin:false };
        let guard=self.commit_guard.clone();
        let tx = CheckedTransaction::new(&mut self.connection,self.health.clone())?;
        if current.is_none() { tx.execute("INSERT INTO modes(id,normalized_name,current_revision) VALUES(?1,?2,?3)", params![result.id,normalized,i64_checked(revision)?])?; }
        else {
            let changed=tx.execute("UPDATE modes SET normalized_name=?1,current_revision=?2 WHERE id=?3 AND current_revision=?4 AND deleted=0",params![normalized,i64_checked(revision)?,result.id,i64_checked(current.unwrap())?])?;
            if changed!=1 {return Err(StoreError::Conflict{current:"Mode changed during save".into()});}
        }
        tx.execute("INSERT INTO mode_revisions(mode_id,revision,data) VALUES(?1,?2,?3)",params![result.id,i64_checked(revision)?,json(&result)?])?;
        tx.commit_guarded(guard.as_deref(),CommitKind::Revision,||{})?;
        Ok(result)
    }
    pub fn delete_mode(&mut self, mode_id: &str, expected_revision: u64) -> StoreResult<()> {
        if builtin_modes().iter().any(|m|m.id==mode_id) { return Err(invalid("built-in Modes cannot be deleted")); }
        let current = self.mode_snapshot(mode_id,None)?;
        if current.revision != expected_revision { return Err(StoreError::Conflict {current:current.revision.to_string()}); }
        let mut prefs = self.preferences()?;
        let guard=self.commit_guard.clone();
        let tx = CheckedTransaction::new(&mut self.connection,self.health.clone())?;
        let changed=tx.execute("UPDATE modes SET deleted=1 WHERE id=?1 AND current_revision=?2 AND deleted=0",params![mode_id,i64_checked(expected_revision)?])?;
        if changed!=1 {return Err(StoreError::Conflict{current:"Mode changed during delete".into()});}
        if prefs.default_mode_id==mode_id {
            prefs.default_mode_id="auto".into(); prefs.version=prefs.version.checked_add(1).ok_or_else(||invalid("version overflow"))?;
            tx.execute("UPDATE preferences SET version=?1,data=?2 WHERE singleton=1",params![i64_checked(prefs.version)?,json(&prefs)?])?;
        }
        tx.commit_guarded(guard.as_deref(),CommitKind::Revision,||{})?;
        Ok(())
    }
    pub fn set_default_mode(&mut self, mode_id: &str, expected_preferences_version: u64) -> StoreResult<Preferences> {
        let mut prefs=self.preferences()?; prefs.default_mode_id=mode_id.into(); self.save_preferences(prefs,expected_preferences_version)
    }

    pub fn checkpoint_source(&mut self,seed:SessionSeed,source:SourceRevision)->StoreResult<HistoryDetail> {
        validate_source(&seed,&source)?;self.validate_mode_snapshot(&seed.mode_snapshot)?;self.ensure_not_tombstoned(&seed.id)?;
        match self.get_result(&seed.id){Ok(existing)=>{if existing.session.seed==seed&&existing.source==source{return Ok(existing);}return Err(StoreError::ImmutableSource);},Err(StoreError::NotFound)=>{},Err(e)=>return Err(e)}
        match self.get_source(&source.id){Ok(_)=>return Err(StoreError::ImmutableSource),Err(StoreError::NotFound)=>{},Err(e)=>return Err(e)}
        let record=SessionRecord{seed:seed.clone(),status:StoredSessionStatus::Checkpointed,current_source_id:source.id.clone(),current_output_id:None,undo_target_id:None};
        let result=HistoryDetail{session:record.clone(),source:source.clone(),output:None,can_undo:false};
        let guard=self.commit_guard.clone();
        if seed.storage_policy==StoragePolicy::MemoryOnly {
            let memory=MemorySession{record,source,outputs:HashMap::new()};
            finalize_memory(guard.as_deref(),CommitKind::Checkpoint,||{self.memory_sessions.insert(seed.id.clone(),memory);})?;
        } else {
            let tx=CheckedTransaction::new(&mut self.connection,self.health.clone())?;
            tx.execute("INSERT INTO sessions(id,seed,status,source_id,ended_at,search_text) VALUES(?1,?2,'checkpointed',?3,?4,?5)",params![seed.id,json(&seed)?,source.id,seed.ended_at,folded(&source.text())])?;
            tx.execute("INSERT INTO sources(id,session_id,attempt_id,data) VALUES(?1,?2,?3,?4)",params![source.id,seed.id,source.asr_attempt_id,json(&source)?])?;
            for item in &source.items{tx.execute("INSERT INTO source_items(source_id,item_id,provider_item_id,capture_order,data) VALUES(?1,?2,?3,?4,?5)",params![source.id,item.id,item.provider_item_id,i64_checked(item.capture_order)?,json(item)?])?;}
            tx.commit_guarded(guard.as_deref(),CommitKind::Checkpoint,||{})?;
        }
        Ok(result)
    }

    /// Second phase: initial output + current pointer + exactly one independent UsageEvent.
    pub fn commit_ready(&mut self, output: OutputRevision) -> StoreResult<HistoryDetail> {
        self.commit_first_ready(output,false)
    }

    /// Only adopt_candidate may opt into the first historical candidate path.
    /// A new historical Mode never changes the immutable capture Mode snapshot.
    fn commit_first_ready(&mut self, output: OutputRevision, history_candidate:bool) -> StoreResult<HistoryDetail> {
        self.ensure_personalization_live(&output)?;
        self.ensure_not_tombstoned(&output.session_id)?;
        let detail=self.get_result(&output.session_id)?;
        if let Some(current)=&detail.output {
            if current==&output { return Ok(detail); }
            return Err(StoreError::Conflict {current:current.id.clone()});
        }
        if output.parent_revision_id.is_some() || output.base_revision_id.is_some() {return Err(invalid("first Ready revision cannot have a parent or base"));}
        if history_candidate {
            if output.kind!=RevisionKind::Adopt || output.creation_reason!=CreationReason::History {return Err(invalid("first historical candidate must be an explicit History adoption"));}
            self.validate_mode_snapshot(&output.mode_snapshot)?;
        } else {
            if !matches!(output.kind,RevisionKind::Initial|RevisionKind::UseSource) || !matches!(output.creation_reason,CreationReason::NormalInitial|CreationReason::Retry|CreationReason::UseSource) {return Err(invalid("invalid initial Ready revision"));}
            if output.mode_snapshot!=detail.session.seed.mode_snapshot { return Err(invalid("initial Mode snapshot must match the frozen session")); }
            if (output.kind==RevisionKind::UseSource)!=(output.creation_reason==CreationReason::UseSource) { return Err(invalid("useSource kind and reason must agree")); }
        }
        validate_source(&detail.session.seed,&detail.source)?;
        validate_output(&output,&detail.source)?;
        let usage=usage_for(&detail.session.seed,&detail.source);
        let mut result=detail.clone();result.session.status=StoredSessionStatus::Ready;result.session.current_output_id=Some(output.id.clone());result.session.undo_target_id=None;result.output=Some(output.clone());result.can_undo=false;
        let memory=if detail.session.seed.storage_policy==StoragePolicy::MemoryOnly{let mut memory=self.memory_sessions.get(&output.session_id).ok_or(StoreError::NotFound)?.clone();memory.record=result.session.clone();memory.outputs.insert(output.id.clone(),output.clone());Some(memory)}else{None};
        let guard=self.commit_guard.clone();let tx=CheckedTransaction::new(&mut self.connection,self.health.clone())?;
        if detail.session.seed.storage_policy==StoragePolicy::History {
            insert_output(&tx,&output)?;
            let changed=tx.execute("UPDATE sessions SET status='ready',output_id=?1,undo_id=NULL,search_text=?2 WHERE id=?3 AND output_id IS NULL AND source_id=?4",params![output.id,folded(&format!("{}\n{}",detail.source.text(),output.text())),output.session_id,output.source_revision_id])?;
            if changed!=1{return Err(StoreError::Conflict{current:"initial result already committed or deleted".into()});}
        }
        insert_usage(&tx,&usage)?;
        let mut discarded=Vec::new();
        tx.commit_guarded(guard.as_deref(),CommitKind::Ready,||{
            if let Some(memory)=memory{if let Some(old)=self.memory_sessions.insert(output.session_id.clone(),memory){discarded.push(old);}
                if let Some(previous)=self.latest_memory_ready.replace(output.session_id.clone()){if previous!=output.session_id{if let Some(old)=self.memory_sessions.remove(&previous){discarded.push(old);}}}
            }
        })?;
        drop(discarded);
        Ok(result)
    }

    /// Explicit adoption of unchanged complete ASR, never an automatic rewrite fallback.
    pub fn use_source(&mut self,session_id:&str,expected_generation:u64,created_at:&str) -> StoreResult<HistoryDetail> {
        let detail=self.get_result(session_id)?;
        if detail.session.seed.generation!=expected_generation {return Err(StoreError::Conflict{current:detail.session.seed.generation.to_string()});}
        if detail.output.is_some() {return Err(StoreError::AlreadyCompleted);}
        let mode=detail.session.seed.mode_snapshot;
        let resolved=if mode.base_mode==BaseMode::Auto {BaseMode::Light} else {mode.base_mode};
        let output=OutputRevision{writing_receipt:None, confirmed_context:None, id:id(),session_id:session_id.into(),parent_revision_id:None,base_revision_id:None,source_revision_id:detail.source.id,mode_snapshot:mode,resolved_mode:resolved,rewrite_fallback:None,
            blocks:detail.source.items.into_iter().map(|item|OutputBlock{id:id(),text:item.text,source_refs:vec![item.id]}).collect(),kind:RevisionKind::UseSource,creation_reason:CreationReason::UseSource,created_at:created_at.into()};
        self.commit_ready(output)
    }

    pub fn get_source(&self, source_id: &str) -> StoreResult<SourceRevision> {
        for memory in self.memory_sessions.values() { if memory.source.id==source_id { return Ok(memory.source.clone()); } }
        let data:Option<String>=self.connection.query_row("SELECT data FROM sources WHERE id=?1",[source_id],|r|r.get(0)).optional()?;
        decode(data.ok_or(StoreError::NotFound)?)
    }
    pub fn get_output(&self, revision_id: &str) -> StoreResult<OutputRevision> {
        for memory in self.memory_sessions.values() { if let Some(output)=memory.outputs.get(revision_id) { return Ok(output.clone()); } }
        let data:Option<String>=self.connection.query_row("SELECT data FROM outputs WHERE id=?1",[revision_id],|r|r.get(0)).optional()?;
        let mut output=decode(data.ok_or(StoreError::NotFound)?)?;
        self.restore_personalization_reference(&mut output)?;
        Ok(output)
    }
    pub fn get_result(&self, session_id: &str) -> StoreResult<HistoryDetail> {
        if let Some(memory)=self.memory_sessions.get(session_id) {
            let output=memory.record.current_output_id.as_ref().and_then(|i|memory.outputs.get(i)).cloned();
            return Ok(HistoryDetail {session:memory.record.clone(),source:memory.source.clone(),output,can_undo:memory.record.undo_target_id.is_some()});
        }
        let row:Option<(String,String,String,Option<String>,Option<String>)>=self.connection.query_row("SELECT seed,status,source_id,output_id,undo_id FROM sessions WHERE id=?1",[session_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        let (seed,status,source_id,output_id,undo_id)=row.ok_or(StoreError::NotFound)?;
        let source=self.get_source(&source_id)?;
        let output=output_id.as_ref().map(|id|self.get_output(id)).transpose()?;
        Ok(HistoryDetail {session:SessionRecord {seed:decode(seed)?,status:decode(json_string(&status))?,current_source_id:source_id,current_output_id:output_id,undo_target_id:undo_id.clone()},source,output,can_undo:undo_id.is_some()})
    }
    pub fn latest_temporary_result(&self) -> StoreResult<Option<HistoryDetail>> {
        self.latest_memory_ready.as_ref().map(|id|self.get_result(id)).transpose()
    }

    pub fn history_list(&self, query: &str, cursor: u64, limit: u64) -> StoreResult<HistoryPage> {
        if query.chars().count()>4000 { return Err(invalid("search query is too long")); }
        let query=folded(query.trim()); let limit=limit.clamp(1,50);
        let total:u64=self.connection.query_row("SELECT COUNT(*) FROM sessions WHERE instr(search_text,?1)>0",[&query],|r|r.get(0))?;
        let mut stmt=self.connection.prepare("SELECT id FROM sessions WHERE instr(search_text,?1)>0 ORDER BY ended_at DESC,id DESC LIMIT ?2 OFFSET ?3")?;
        let ids=stmt.query_map(params![query,i64_checked(limit)?,i64_checked(cursor)?],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
        let mut items=Vec::with_capacity(ids.len());
        for session_id in ids { items.push(summary(&self.get_result(&session_id)?)); }
        let next=cursor.saturating_add(items.len() as u64);
        Ok(HistoryPage {items,total,next_cursor:if next<total {Some(next)} else {None}})
    }
    pub fn history_get(&self, session_id: &str) -> StoreResult<HistoryDetail> {
        let detail=self.get_result(session_id)?;
        if detail.session.seed.storage_policy!=StoragePolicy::History { return Err(StoreError::NotFound); }
        Ok(detail)
    }
    pub fn delete_history(&mut self,session_id:&str)->StoreResult<()> {
        let detail=self.history_get(session_id)?;if detail.session.seed.storage_policy!=StoragePolicy::History{return Err(StoreError::NotFound);}
        let guard=self.commit_guard.clone();let tx=CheckedTransaction::new(&mut self.connection,self.health.clone())?;
        tx.execute("INSERT OR IGNORE INTO content_tombstones(session_id) VALUES(?1)",[session_id])?;
        tx.execute("DELETE FROM sessions WHERE id=?1",[session_id])?;
        tx.execute("UPDATE delivery_attempts SET state='unconfirmed',reason='deletedBeforeReceipt' WHERE session_id=?1 AND state='prepared'",[session_id])?;
        tx.commit_guarded(guard.as_deref(),CommitKind::Cleanup,||{})?;Ok(())
    }
    /// Cancelling a rewrite run must never call this on completed history.
    pub fn cancel_unfinished(&mut self,session_id:&str)->StoreResult<()> {
        if self.usage_event(session_id)?.is_some(){return Err(StoreError::AlreadyCompleted);}
        let guard=self.commit_guard.clone();let tx=CheckedTransaction::new(&mut self.connection,self.health.clone())?;
        tx.execute("INSERT OR IGNORE INTO content_tombstones(session_id) VALUES(?1)",[session_id])?;
        tx.execute("DELETE FROM sessions WHERE id=?1",[session_id])?;
        tx.execute("UPDATE delivery_attempts SET state='skipped',reason='cancelled' WHERE session_id=?1 AND state='prepared'",[session_id])?;
        let mut discarded=None;tx.commit_guarded(guard.as_deref(),CommitKind::Cleanup,||{discarded=self.memory_sessions.remove(session_id);if self.latest_memory_ready.as_deref()==Some(session_id){self.latest_memory_ready=None;}})?;
        drop(discarded);Ok(())
    }
    fn ensure_not_tombstoned(&self, session_id:&str) -> StoreResult<()> {
        let deleted:bool=self.connection.query_row("SELECT EXISTS(SELECT 1 FROM content_tombstones WHERE session_id=?1)",[session_id],|r|r.get(0))?;
        if deleted { Err(StoreError::Cancelled) } else { Ok(()) }
    }

    pub fn adopt_candidate(&mut self, session_id:&str, expected_current_revision_id:&str, mut candidate:OutputRevision) -> StoreResult<HistoryDetail> {
        self.ensure_not_tombstoned(session_id)?;
        let detail=self.get_result(session_id)?;
        let current=detail.session.current_output_id.as_deref().unwrap_or("");
        if current!=expected_current_revision_id {return Err(StoreError::Conflict{current:current.into()});}
        let expected_base=if expected_current_revision_id.is_empty(){None}else{Some(expected_current_revision_id)};
        if candidate.session_id!=session_id || candidate.base_revision_id.as_deref()!=expected_base { return Err(invalid("candidate does not bind this session/base revision")); }
        if expected_current_revision_id.is_empty() {
            if candidate.kind!=RevisionKind::Adopt || candidate.creation_reason!=CreationReason::History || candidate.parent_revision_id.is_some() {return Err(invalid("first candidate must be an explicit History adoption without a parent"));}
            candidate.id=id();
            return self.commit_first_ready(candidate,true);
        }
        candidate.id=id(); candidate.kind=RevisionKind::Adopt; candidate.creation_reason=CreationReason::History;
        self.commit_revision(session_id,expected_current_revision_id,candidate,false)
    }
    pub fn edit_output(&mut self, session_id:&str, expected_current_revision_id:&str, blocks:Vec<OutputBlock>, created_at:&str) -> StoreResult<HistoryDetail> {
        let mut output=self.current_for_edit(session_id,expected_current_revision_id)?;
        output.id=id(); output.blocks=blocks; output.kind=RevisionKind::Edit; output.creation_reason=CreationReason::History; output.created_at=created_at.into();
        output.clear_rewrite_fallback();
        self.commit_revision(session_id,expected_current_revision_id,output,false)
    }
    pub fn restore_output(&mut self, session_id:&str, expected_current_revision_id:&str, patch:RestorePatch, created_at:&str) -> StoreResult<HistoryDetail> {
        let detail=self.get_result(session_id)?;
        let mut output=self.current_for_edit(session_id,expected_current_revision_id)?;
        if patch.source_selections.is_empty() { return Err(invalid("restore needs explicit source ranges")); }
        let mut fragments=Vec::new(); let mut refs=Vec::new();
        for selection in &patch.source_selections {
            let item=detail.source.items.iter().find(|i|i.id==selection.item_id && i.is_final).ok_or_else(||invalid("restore source does not belong to the frozen revision"))?;
            if selection.start==selection.end { return Err(invalid("empty source range")); }
            let (start,end)=scalar_range(&item.text,selection.start,selection.end)?;
            fragments.push(item.text[start..end].to_string()); refs.push(item.id.clone());
        }
        let block=output.blocks.iter_mut().find(|b|b.id==patch.block_id).ok_or_else(||invalid("restore target block not found"))?;
        let (start,end)=scalar_range(&block.text,patch.start,patch.end)?;
        block.text.replace_range(start..end,&fragments.join("\n"));
        for source_ref in refs { if !block.source_refs.contains(&source_ref) {block.source_refs.push(source_ref);} }
        output.id=id(); output.kind=RevisionKind::Restore; output.creation_reason=CreationReason::History; output.created_at=created_at.into();
        output.clear_rewrite_fallback();
        self.commit_revision(session_id,expected_current_revision_id,output,false)
    }
    /// The explicit correction and optional dictionary append share one guarded
    /// commit. Neither a stale dictionary nor a failed word insert edits text.
    pub fn correct_term(&mut self, session_id:&str, expected_current_revision_id:&str, patch:CorrectionPatch, expected_dictionary_version:Option<u64>, created_at:&str) -> StoreResult<CorrectionReceipt> {
        let mut output=self.current_for_edit(session_id,expected_current_revision_id)?;
        if patch.end<=patch.start || patch.end-patch.start>DICTIONARY_MAX_TERM_CHARS || patch.expected_text.chars().count()>DICTIONARY_MAX_TERM_CHARS {return Err(invalid("correction_selection_invalid"));}
        let term=dictionary_term(&patch.replacement)?;
        let block=output.blocks.iter_mut().find(|block|block.id==patch.block_id).ok_or_else(||invalid("correction_selection_invalid"))?;
        let (start,end)=scalar_range(&block.text,patch.start,patch.end)?;
        if block.text[start..end]!=patch.expected_text {return Err(invalid("correction_selection_mismatch"));}
        if patch.expected_text==term {return Err(invalid("correction_unchanged"));}
        block.text.replace_range(start..end,&term);
        output.id=id();output.kind=RevisionKind::Edit;output.creation_reason=CreationReason::History;output.created_at=created_at.into();
        output.clear_rewrite_fallback();
        let mut status=CorrectionDictionaryStatus::NotRequested;
        let dictionary=if let Some(expected)=expected_dictionary_version {
            let mut next=self.checked_dictionary(expected)?;
            if next.entries.iter().any(|entry|folded(&entry.term)==folded(&term)) {
                status=CorrectionDictionaryStatus::Existing;
            }else{
                next.entries.push(DictionaryEntry{id:id(),term,created_at:created_at.into(),updated_at:created_at.into()});
                next.entries.sort_by_cached_key(|entry|(folded(&entry.term),entry.id.clone()));
                next.version=expected.checked_add(1).ok_or_else(||invalid("dictionary_version"))?;
                status=CorrectionDictionaryStatus::Added;
            }
            validate_dictionary(&next)?;Some(next)
        }else{None};
        let mutation=dictionary.as_ref().zip(expected_dictionary_version).map(|(next,expected)|(next,expected,status==CorrectionDictionaryStatus::Added));
        let detail=self.commit_revision_with_dictionary(session_id,expected_current_revision_id,output,false,mutation)?;
        Ok(CorrectionReceipt{detail,dictionary,dictionary_status:status})
    }
    pub fn undo_output(&mut self, session_id:&str, expected_current_revision_id:&str, created_at:&str) -> StoreResult<HistoryDetail> {
        let detail=self.get_result(session_id)?;
        self.current_for_edit(session_id,expected_current_revision_id)?;
        let target=detail.session.undo_target_id.ok_or(StoreError::UndoUnavailable)?;
        let mut output=self.get_output(&target)?;
        output.id=id(); output.kind=RevisionKind::Undo; output.creation_reason=CreationReason::History; output.created_at=created_at.into();
        self.commit_revision(session_id,expected_current_revision_id,output,true)
    }
    fn current_for_edit(&self, session_id:&str, expected:&str) -> StoreResult<OutputRevision> {
        self.ensure_not_tombstoned(session_id)?;
        let detail=self.get_result(session_id)?;
        let current=detail.output.ok_or(StoreError::NotFound)?;
        if current.id!=expected { return Err(StoreError::Conflict {current:current.id}); }
        Ok(current)
    }
    fn commit_revision(&mut self, session_id:&str, expected:&str, output:OutputRevision, clear_undo:bool) -> StoreResult<HistoryDetail> {
        self.commit_revision_with_dictionary(session_id,expected,output,clear_undo,None)
    }
    fn commit_revision_with_dictionary(&mut self, session_id:&str, expected:&str, mut output:OutputRevision, clear_undo:bool, dictionary:Option<(&DictionarySnapshot,u64,bool)>) -> StoreResult<HistoryDetail> {
        self.ensure_personalization_live(&output)?;
        let detail=self.get_result(session_id)?;
        self.current_for_edit(session_id,expected)?;
        if output.session_id!=session_id || output.source_revision_id!=detail.session.current_source_id { return Err(invalid("revision source/session mismatch")); }
        validate_output(&output,&detail.source)?;
        self.validate_mode_snapshot(&output.mode_snapshot)?;
        output.parent_revision_id=Some(expected.into()); output.base_revision_id=Some(expected.into());
        let undo=if clear_undo{None}else{Some(expected.to_string())};
        let mut result=detail.clone();result.session.current_output_id=Some(output.id.clone());result.session.undo_target_id=undo.clone();result.output=Some(output.clone());result.can_undo=undo.is_some();
        let guard=self.commit_guard.clone();
        if detail.session.seed.storage_policy==StoragePolicy::MemoryOnly{
            let mut memory=self.memory_sessions.get(session_id).ok_or(StoreError::NotFound)?.clone();memory.record=result.session.clone();memory.outputs.insert(output.id.clone(),output.clone());
            let current=memory.record.current_output_id.as_deref();let previous=memory.record.undo_target_id.as_deref();memory.outputs.retain(|key,_|Some(key.as_str())==current||Some(key.as_str())==previous);
            let mut discarded=None;
            if let Some((next,version,changed))=dictionary {
                let tx=CheckedTransaction::new(&mut self.connection,self.health.clone())?;
                write_dictionary_transaction(&tx,next,version,changed)?;
                // Only the explicitly remembered word is durable. The corrected
                // memory-only body publishes after the SQL COMMIT under one guard.
                tx.commit_guarded(guard.as_deref(),CommitKind::Revision,||{discarded=self.memory_sessions.insert(session_id.into(),memory);})?;
            }else{finalize_memory(guard.as_deref(),CommitKind::Revision,||{discarded=self.memory_sessions.insert(session_id.into(),memory);})?;}
            drop(discarded);
        }else{
            let tx=CheckedTransaction::new(&mut self.connection,self.health.clone())?;insert_output(&tx,&output)?;
            let changed=tx.execute("UPDATE sessions SET output_id=?1,undo_id=?2,search_text=?3 WHERE id=?4 AND output_id=?5",params![output.id,undo,folded(&format!("{}\n{}",detail.source.text(),output.text())),session_id,expected])?;
            if changed!=1{return Err(StoreError::Conflict{current:detail.session.current_output_id.unwrap_or_default()});}
            if let Some((next,version,changed))=dictionary {write_dictionary_transaction(&tx,next,version,changed)?;}
            tx.commit_guarded(guard.as_deref(),CommitKind::Revision,||{})?;
        }
        Ok(result)
    }

    pub fn usage_event(&self, session_id:&str) -> StoreResult<Option<UsageEvent>> {
        Ok(self.connection.query_row("SELECT session_id,source_char_count,capture_duration_ms,ended_at,end_local_date,time_zone FROM usage_events WHERE session_id=?1",[session_id],|r|Ok(UsageEvent{session_id:r.get(0)?,source_char_count:r.get(1)?,capture_duration_ms:r.get(2)?,ended_at:r.get(3)?,end_local_date:r.get(4)?,time_zone:r.get(5)?})).optional()?)
    }
    pub fn usage_today(&self,date:&str) -> StoreResult<DailyUsage> {
        let (count,chars,duration):(u64,u64,u64)=self.connection.query_row("SELECT COUNT(*),COALESCE(SUM(source_char_count),0),COALESCE(SUM(capture_duration_ms),0) FROM usage_events WHERE end_local_date=?1",[date],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        Ok(DailyUsage {date:date.into(),count,source_char_count:chars,capture_duration_ms:duration})
    }

    pub fn usage_totals(&self) -> StoreResult<UsageTotals> {
        Ok(self.connection.query_row("SELECT COUNT(*),COALESCE(SUM(source_char_count),0),COALESCE(SUM(capture_duration_ms),0) FROM usage_events",[],|r|Ok(UsageTotals{count:r.get(0)?,source_char_count:r.get(1)?,capture_duration_ms:r.get(2)?}))?)
    }

    /// Only grants the database permission. Target/native readiness checks remain the caller's job.
    pub fn prepare_delivery(&mut self, attempt:DeliveryAttempt) -> StoreResult<DeliveryAttempt> {
        if attempt.state!=DeliveryState::Prepared || attempt.reason.is_some() { return Err(invalid("new delivery must be Prepared")); }
        if attempt.target_fingerprint.is_empty() || attempt.target_fingerprint.len()>512 || !attempt.target_fingerprint.chars().all(|c|c.is_ascii_alphanumeric()||matches!(c,':'|'_'|'-'|'.'|'/')) { return Err(invalid("target fingerprint must be opaque ASCII")); }
        self.ensure_not_tombstoned(&attempt.session_id)?;
        let detail=self.get_result(&attempt.session_id)?;
        let output=detail.output.ok_or(StoreError::NotFound)?;
        if detail.session.seed.generation!=attempt.generation || output.id!=attempt.output_revision_id || output.creation_reason!=CreationReason::NormalInitial || detail.session.status!=StoredSessionStatus::Ready || self.usage_event(&attempt.session_id)?.is_none() { return Err(invalid("output is not eligible for initial delivery")); }
        let consumed:bool=self.connection.query_row("SELECT EXISTS(SELECT 1 FROM delivery_attempts WHERE session_id=?1)",[&attempt.session_id],|r|r.get(0))?;
        if consumed {return Err(StoreError::DeliveryConsumed);}
        let guard=self.commit_guard.clone();let tx=CheckedTransaction::new(&mut self.connection,self.health.clone())?;
        tx.execute("INSERT INTO delivery_attempts(id,session_id,generation,output_revision_id,target_fingerprint,claimed_at,state,reason) VALUES(?1,?2,?3,?4,?5,?6,'prepared',NULL)",params![attempt.id,attempt.session_id,i64_checked(attempt.generation)?,attempt.output_revision_id,attempt.target_fingerprint,attempt.claimed_at])?;
        tx.commit_guarded(guard.as_deref(),CommitKind::Delivery,||{})?;
        Ok(attempt)
    }
    pub fn delivery_attempt(&self,attempt_id:&str) -> StoreResult<DeliveryAttempt> {
        let row:Option<(String,String,u64,String,String,String,String,Option<String>)>=self.connection.query_row("SELECT id,session_id,generation,output_revision_id,target_fingerprint,claimed_at,state,reason FROM delivery_attempts WHERE id=?1",[attempt_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional()?;
        let (id,session_id,generation,output_revision_id,target_fingerprint,claimed_at,state,reason)=row.ok_or(StoreError::NotFound)?;
        Ok(DeliveryAttempt{id,session_id,generation,output_revision_id,target_fingerprint,claimed_at,state:decode(json_string(&state))?,reason})
    }
    pub fn transition_delivery(&mut self, attempt_id:&str, expected:DeliveryState, next:DeliveryState, reason:Option<String>) -> StoreResult<DeliveryAttempt> {
        let current=self.delivery_attempt(attempt_id)?;
        if current.state!=expected { return Err(StoreError::Conflict {current:json(&current.state)?}); }
        let allowed=matches!((expected,next),(DeliveryState::Prepared,DeliveryState::Dispatched|DeliveryState::Skipped|DeliveryState::Failed|DeliveryState::Unconfirmed)|(DeliveryState::Dispatched,DeliveryState::Confirmed|DeliveryState::Unconfirmed|DeliveryState::Failed));
        if !allowed {return Err(invalid("invalid delivery state transition"));}
        if reason.as_ref().is_some_and(|r|r.len()>256 || !r.chars().all(|c|c.is_ascii_alphanumeric()||matches!(c,'_'|'-'|'.'))) {return Err(invalid("delivery reason must be a code, not content"));}
        let mut result=current;result.state=next;result.reason=reason.clone();
        let guard=self.commit_guard.clone();let tx=CheckedTransaction::new(&mut self.connection,self.health.clone())?;
        let changed=tx.execute("UPDATE delivery_attempts SET state=?1,reason=?2 WHERE id=?3 AND state=?4",params![enum_string(next)?,reason,attempt_id,enum_string(expected)?])?;
        if changed!=1{return Err(StoreError::Conflict{current:"delivery state changed".into()});}
        // Actual native receipts/skips must remain recordable after cancellation/deletion.
        tx.commit_guarded(guard.as_deref(),CommitKind::Cleanup,||{})?;Ok(result)
    }

    pub fn recover_after_restart(&mut self) -> StoreResult<()> {
        let tx=CheckedTransaction::new(&mut self.connection,self.health.clone())?;
        tx.execute("UPDATE sessions SET status='interrupted' WHERE status='checkpointed'",[])?;
        tx.execute("UPDATE delivery_attempts SET state='unconfirmed',reason='interruptedBeforeReceipt' WHERE state='prepared'",[])?;
        tx.execute("UPDATE delivery_attempts SET state='unconfirmed',reason='interrupted' WHERE state='dispatched'",[])?;
        tx.commit()?;
        Ok(())
    }
    pub fn clear_temporary_results(&mut self) {self.memory_sessions.clear();self.latest_memory_ready=None;}
    fn validate_mode_snapshot(&self,snapshot:&ModeSnapshot) -> StoreResult<()> {
        if self.mode_snapshot(&snapshot.mode_id,Some(snapshot.revision))?!=*snapshot {return Err(invalid("Mode snapshot does not match its saved revision"));}
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn test_connection(&self) -> &Connection {&self.connection}
}

/// Always attempts and observes rollback before handing an error back to the worker.
/// rusqlite's default Transaction::Drop discards rollback errors, so it is insufficient.
struct CheckedTransaction<'a>{tx:Option<Transaction<'a>>,health:Arc<Mutex<Option<StoreError>>>}
impl<'a> CheckedTransaction<'a>{
    fn new(connection:&'a mut Connection,health:Arc<Mutex<Option<StoreError>>>)->StoreResult<Self>{Ok(Self{tx:Some(connection.transaction()?),health})}
    fn rollback_checked(&mut self)->StoreResult<()>{
        if let Some(tx)=&self.tx{if !tx.is_autocommit(){if let Err(error)=tx.execute_batch("ROLLBACK"){let error=StoreError::Unhealthy(format!("explicit rollback failed: {error}"));*self.health.lock().unwrap_or_else(|p|p.into_inner())=Some(error.clone());return Err(error);}if !tx.is_autocommit(){let error=StoreError::Unhealthy("connection still in transaction after rollback".into());*self.health.lock().unwrap_or_else(|p|p.into_inner())=Some(error.clone());return Err(error);}}}Ok(())
    }
    fn commit(self)->StoreResult<()>{self.commit_guarded(None,CommitKind::Cleanup,||{})}
    fn commit_guarded(mut self,guard:Option<&CommitGuard>,kind:CommitKind,publish:impl FnOnce())->StoreResult<()>{
        let result=if let Some(guard)=guard{guard.finalize(kind,||{self.tx.as_ref().unwrap().execute_batch("COMMIT")?;publish();Ok(())})}else{self.tx.as_ref().unwrap().execute_batch("COMMIT").map_err(StoreError::from).map(|_|publish())};
        if result.is_err(){self.rollback_checked()?;}result
    }
}
impl<'a> Deref for CheckedTransaction<'a>{type Target=Transaction<'a>;fn deref(&self)->&Self::Target{self.tx.as_ref().unwrap()}}
impl Drop for CheckedTransaction<'_>{fn drop(&mut self){let _=self.rollback_checked();}}
fn finalize_memory(guard:Option<&CommitGuard>,kind:CommitKind,publish:impl FnOnce())->StoreResult<()>{if let Some(guard)=guard{guard.finalize(kind,||{publish();Ok(())})}else{publish();Ok(())}}

fn write_dictionary_transaction(tx:&Transaction<'_>,next:&DictionarySnapshot,expected:u64,changed:bool)->StoreResult<()> {
    let updated=tx.execute("UPDATE dictionary_meta SET version=?1 WHERE singleton=1 AND version=?2",params![i64_checked(next.version)?,i64_checked(expected)?])?;
    if updated!=1 {let current:u64=tx.query_row("SELECT version FROM dictionary_meta WHERE singleton=1",[],|row|row.get(0))?;return Err(StoreError::Conflict{current:current.to_string()});}
    if changed {
        // This helper is used only after validation of the bounded snapshot.
        tx.execute("DELETE FROM dictionary_entries",[])?;
        for entry in &next.entries {tx.execute("INSERT INTO dictionary_entries(id,normalized_term,term,created_at,updated_at) VALUES(?1,?2,?3,?4,?5)",params![entry.id,folded(&entry.term),entry.term,entry.created_at,entry.updated_at])?;}
    }
    Ok(())
}

fn enum_string<T:Serialize>(v:T) -> StoreResult<String> { Ok(serde_json::from_str::<String>(&json(&v)?)?) }
fn json_string(s:&str) -> String { serde_json::to_string(s).expect("serializing a string cannot fail") }
fn builtin_modes() -> Vec<ModeRecord> {
    vec![
        ModeRecord{id:"auto".into(),name:"自动".into(),base_mode:BaseMode::Auto,rules:"".into(),revision:1,builtin:true},
        ModeRecord{id:"chat".into(),name:"聊天消息".into(),base_mode:BaseMode::Auto,rules:"".into(),revision:1,builtin:true},
        ModeRecord{id:"document".into(),name:"文档写作".into(),base_mode:BaseMode::Auto,rules:"".into(),revision:1,builtin:true},
        ModeRecord{id:"email".into(),name:"邮件沟通".into(),base_mode:BaseMode::Auto,rules:"".into(),revision:1,builtin:true},
        ModeRecord{id:"technical".into(),name:"技术沟通".into(),base_mode:BaseMode::Auto,rules:"".into(),revision:1,builtin:true},
        ModeRecord{id:"light".into(),name:"轻整理".into(),base_mode:BaseMode::Light,rules:"".into(),revision:1,builtin:true},
        ModeRecord{id:"structure".into(),name:"结构".into(),base_mode:BaseMode::Structure,rules:"".into(),revision:1,builtin:true},
    ]
}
fn validate_source(seed:&SessionSeed,source:&SourceRevision) -> StoreResult<()> {
    for value in [&seed.id,&source.id,&source.asr_attempt_id,&source.provider,&source.model,&source.adapter_version] {valid_id(value)?;}
    i64_checked(seed.generation)?;i64_checked(seed.capture_duration_ms)?;
    if source.session_id!=seed.id || source.generation!=seed.generation || !source.complete || source.items.is_empty() {return Err(invalid("checkpoint needs the complete current attempt"));}
    if seed.end_local_date.len()!=10 || seed.time_zone.is_empty() || seed.started_at.is_empty() || seed.ended_at.is_empty() {return Err(invalid("frozen end date/time metadata is required"));}
    let mut ids=HashSet::new(); let mut provider_ids=HashSet::new(); let mut previous=None;
    for item in &source.items {
        valid_id(&item.id)?; valid_id(&item.provider_item_id)?;i64_checked(item.capture_order)?;
        if !item.is_final || !ids.insert(&item.id) || !provider_ids.insert(&item.provider_item_id) || previous.is_some_and(|order|order>=item.capture_order) {return Err(invalid("source items must be final, unique and in capture order"));}
        if item.start_ms.zip(item.end_ms).is_some_and(|(start,end)|start>end) {return Err(invalid("invalid source time range"));}
        previous=Some(item.capture_order);
    }
    if source_character_count(&source.text())==0 {return Err(invalid("empty speech is not a valid checkpoint"));}
    Ok(())
}
pub fn validate_output(output:&OutputRevision,source:&SourceRevision) -> StoreResult<()> {
    valid_id(&output.id)?;
    if let Some(receipt)=&output.writing_receipt {
        if receipt.contract_version.len()>80 || receipt.contract_version.is_empty() || output.rewrite_fallback.is_some() {return Err(invalid("invalid writing receipt"));}
        if let Some(snapshot)=&receipt.personalization {snapshot.validate().map_err(invalid)?;if snapshot.scene!=receipt.scene {return Err(invalid("personalization_receipt_scene"));}}
    }
    if let Some(context)=&output.confirmed_context {
        context.reference_text(source).map_err(invalid)?;
        if output.rewrite_fallback.is_some() || matches!(output.kind,RevisionKind::Initial|RevisionKind::UseSource) {return Err(invalid("confirmed context belongs only to an explicit history revision"));}
    }
    if !source.complete || output.source_revision_id!=source.id || output.session_id!=source.session_id || output.blocks.is_empty() || output.resolved_mode==BaseMode::Auto {return Err(invalid("output must bind a complete source and resolved base Mode"));}
    if output.rewrite_fallback.is_none() && output.mode_snapshot.base_mode!=BaseMode::Auto && output.mode_snapshot.base_mode!=output.resolved_mode {return Err(invalid("resolved Mode conflicts with the selected Mode"));}
    if output.rewrite_fallback.is_some() && (output.resolved_mode!=BaseMode::Light || !matches!(output.kind,RevisionKind::Initial|RevisionKind::Undo)) {return Err(invalid("source fallback must be an initial result or an explicit undo"));}
    let final_ids:HashSet<&str>=source.items.iter().filter(|i|i.is_final).map(|i|i.id.as_str()).collect();
    let mut blocks=HashSet::new();
    for block in &output.blocks {
        valid_id(&block.id)?;
        if !blocks.insert(&block.id) || block.text.trim().is_empty() || block.text.chars().count()>1_000_000 {return Err(invalid("invalid output block"));}
        let mut refs=HashSet::new();
        for source_ref in &block.source_refs {if !final_ids.contains(source_ref.as_str()) || !refs.insert(source_ref) {return Err(invalid("source reference is not a unique Final item of this revision"));}}
        if matches!(output.kind,RevisionKind::Initial|RevisionKind::UseSource|RevisionKind::Adopt) && block.source_refs.is_empty() {return Err(invalid("generated blocks need source references"));}
    }
    if output.rewrite_fallback.is_some() {
        if output.blocks.len()!=1 || output.blocks[0].text!=source.text() || output.blocks[0].source_refs!=source.items.iter().map(|item|item.id.clone()).collect::<Vec<_>>() {return Err(invalid("rewrite fallback must preserve the complete source and its ordered references"));}
    }
    if output.kind==RevisionKind::UseSource {
        if output.blocks.len()!=source.items.len() || output.blocks.iter().zip(&source.items).any(|(block,item)|block.text!=item.text||block.source_refs!=[item.id.clone()]) {return Err(invalid("useSource must adopt unchanged items in capture order"));}
    }
    Ok(())
}
fn usage_for(seed:&SessionSeed,source:&SourceRevision) -> UsageEvent {
    UsageEvent {session_id:seed.id.clone(),source_char_count:source_character_count(&source.text()),capture_duration_ms:seed.capture_duration_ms,ended_at:seed.ended_at.clone(),end_local_date:seed.end_local_date.clone(),time_zone:seed.time_zone.clone()}
}
fn insert_output(tx:&Transaction<'_>,output:&OutputRevision) -> StoreResult<()> {
    let reference=output.writing_receipt.as_ref().and_then(|receipt|receipt.personalization.as_ref());
    if let Some(snapshot)=reference {
        snapshot.validate().map_err(invalid)?;
        if let Some(profile_id)=&snapshot.profile_id {
            let removed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM personalization_deleted_profiles WHERE profile_id=?1)",[profile_id],|row|row.get(0))?;
            if removed&&!snapshot.reference_cleared {return Err(StoreError::Cancelled);}
        }
    }
    let mut stored=output.clone();
    if let Some(snapshot)=stored.writing_receipt.as_mut().and_then(|receipt|receipt.personalization.as_mut()) {
        let was_cleared=snapshot.reference_cleared;snapshot.clear_reference();snapshot.reference_cleared=was_cleared;
    }
    tx.execute("INSERT INTO outputs(id,session_id,source_id,data) VALUES(?1,?2,?3,?4)",params![output.id,output.session_id,output.source_revision_id,json(&stored)?])?;
    if let Some(snapshot)=reference.filter(|snapshot|!snapshot.reference_cleared) {
        tx.execute("INSERT INTO personalization_references(output_id,profile_id,data) VALUES(?1,?2,?3)",params![output.id,snapshot.profile_id,json(snapshot)?])?;
    }
    Ok(())
}
fn insert_usage(tx:&Transaction<'_>,usage:&UsageEvent) -> StoreResult<()> {
    tx.execute("INSERT OR IGNORE INTO usage_events(session_id,source_char_count,capture_duration_ms,ended_at,end_local_date,time_zone) VALUES(?1,?2,?3,?4,?5,?6)",params![usage.session_id,i64_checked(usage.source_char_count)?,i64_checked(usage.capture_duration_ms)?,usage.ended_at,usage.end_local_date,usage.time_zone])?;
    Ok(())
}
fn scalar_range(text:&str,start:usize,end:usize) -> StoreResult<(usize,usize)> {
    if start>end {return Err(invalid("range start exceeds end"));}
    let mut boundaries=text.char_indices().map(|(i,_)|i).collect::<Vec<_>>();boundaries.push(text.len());
    match (boundaries.get(start),boundaries.get(end)) { (Some(start),Some(end)) => Ok((*start,*end)),_ => Err(invalid("Unicode scalar range exceeds text")) }
}
fn summary(detail:&HistoryDetail) -> HistorySummary {
    let text=detail.output.as_ref().map(OutputRevision::text).unwrap_or_else(||detail.source.text());
    let title=text.lines().find(|line|!line.trim().is_empty()).unwrap_or("").chars().take(40).collect();
    let preview=text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect();
    let mode=detail.output.as_ref().map(|o|&o.mode_snapshot).unwrap_or(&detail.session.seed.mode_snapshot);
    HistorySummary {session_id:detail.session.seed.id.clone(),title,preview,ended_at:detail.session.seed.ended_at.clone(),end_local_date:detail.session.seed.end_local_date.clone(),mode_id:mode.mode_id.clone(),mode_name:mode.name.clone(),resolved_mode:detail.output.as_ref().map(|o|o.resolved_mode),rewrite_fallback:detail.output.as_ref().and_then(|o|o.rewrite_fallback),current_revision_id:detail.session.current_output_id.clone(),source_revision_id:detail.session.current_source_id.clone(),status:detail.session.status,source_char_count:source_character_count(&detail.source.text()),capture_duration_ms:detail.session.seed.capture_duration_ms}
}
