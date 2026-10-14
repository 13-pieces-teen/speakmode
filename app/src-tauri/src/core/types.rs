use serde::{Deserialize, Serialize};

pub const DICTIONARY_MAX_ENTRIES: usize = 200;
pub const DICTIONARY_MAX_TERM_CHARS: usize = 80;
pub const DICTIONARY_MAX_CONTEXT_BYTES: usize = 4000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DictionaryEntry {
    pub id: String,
    pub term: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A personal recognition vocabulary, never a transcript replacement table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DictionarySnapshot {
    pub version: u64,
    pub entries: Vec<DictionaryEntry>,
}
impl Default for DictionarySnapshot {
    fn default() -> Self { Self { version: 1, entries: Vec::new() } }
}
impl DictionarySnapshot {
    /// Stable plain context for the ASR adapter. Empty dictionaries omit corpus entirely.
    pub fn asr_context(&self) -> Option<String> {
        if self.entries.is_empty() { return None; }
        let terms = self.entries.iter().map(|entry| entry.term.as_str()).collect::<Vec<_>>();
        Some(format!("常用词汇：{}", serde_json::to_string(&terms).expect("string lists serialize")))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DictionaryDraft { pub id: Option<String>, pub term: String }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BaseMode { Auto, Light, Structure }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliveryMode { Direct, Preview }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    pub version: u64,
    pub shortcut: String,
    pub microphone_id: Option<String>,
    pub language: String,
    pub default_mode_id: String,
    pub delivery_mode: DeliveryMode,
    /// Global capture preference, frozen when a recording begins. Old profiles stay bypassed.
    #[serde(default)]
    pub soft_voice_enabled: bool,
    #[serde(default)]
    pub app_writing: super::app_writing::AppWritingPreferences,
    pub keep_history: bool,
    pub launch_at_login: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self { version: 1, shortcut: "alt_right".into(), microphone_id: None,
            language: "auto".into(), default_mode_id: "auto".into(),
            delivery_mode: DeliveryMode::Direct, soft_voice_enabled: false, app_writing: Default::default(), keep_history: true, launch_at_login: false }
    }
}

/// No URL or secret fields: endpoints are constructed by the provider adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderConfig {
    pub version: u64,
    pub provider: String,
    pub region: Option<String>,
    pub workspace: Option<String>,
    pub asr_model: String,
    pub rewrite_model: String,
    pub adapter_version: String,
}
impl Default for ProviderConfig {
    fn default() -> Self {
        Self { version: 1, provider: "aliyun".into(), region: None, workspace: None,
            asr_model: "qwen3-asr-flash-realtime".into(), rewrite_model: "qwen3.8-flash".into(),
            adapter_version: "qwen-realtime-v1".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModeRecord {
    pub id: String,
    pub name: String,
    pub base_mode: BaseMode,
    pub rules: String,
    pub revision: u64,
    pub builtin: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModeSnapshot {
    pub mode_id: String,
    pub name: String,
    pub base_mode: BaseMode,
    pub rules: String,
    pub revision: u64,
}
impl From<ModeRecord> for ModeSnapshot {
    fn from(mode: ModeRecord) -> Self {
        Self { mode_id: mode.id, name: mode.name, base_mode: mode.base_mode,
            rules: mode.rules, revision: mode.revision }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModeDraft {
    pub id: Option<String>,
    pub name: String,
    pub base_mode: BaseMode,
    pub rules: String,
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StoragePolicy { History, MemoryOnly }

/// Already frozen by the session coordinator. Wall-clock strings are supplied there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSeed {
    pub id: String,
    pub generation: u64,
    pub started_at: String,
    pub ended_at: String,
    pub capture_duration_ms: u64,
    pub end_local_date: String,
    pub time_zone: String,
    pub storage_policy: StoragePolicy,
    pub mode_snapshot: ModeSnapshot,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StoredSessionStatus { Checkpointed, Ready, Interrupted }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    #[serde(flatten)]
    pub seed: SessionSeed,
    pub status: StoredSessionStatus,
    pub current_source_id: String,
    pub current_output_id: Option<String>,
    pub undo_target_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceItem {
    /// Stable application ID; provider identity is additionally scoped by this source attempt.
    pub id: String,
    pub provider_item_id: String,
    pub capture_order: u64,
    pub text: String,
    #[serde(rename = "final")]
    pub is_final: bool,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRevision {
    pub id: String,
    pub session_id: String,
    pub asr_attempt_id: String,
    pub generation: u64,
    pub provider: String,
    pub model: String,
    pub adapter_version: String,
    pub complete: bool,
    pub items: Vec<SourceItem>,
    pub created_at: String,
}
impl SourceRevision {
    pub fn text(&self) -> String { self.items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>().join("\n") }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputBlock {
    pub id: String,
    pub text: String,
    pub source_refs: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RevisionKind { Initial, UseSource, Adopt, Edit, Restore, Undo }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CreationReason { NormalInitial, Retry, UseSource, History, Preview }
/// A complete ASR source was delivered when the optional cloud rewrite was unavailable.
/// This is not a successful Mode rewrite or an external application's paste receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RewriteFallbackReason { Timeout, ServiceUnavailable, InvalidOutput, MeaningRisk }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputRevision {
    /// Explicit source corrections used for this candidate; retained for review/undo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_context: Option<super::ConfirmedContext>,
    pub id: String,
    pub session_id: String,
    pub parent_revision_id: Option<String>,
    pub base_revision_id: Option<String>,
    pub source_revision_id: String,
    pub mode_snapshot: ModeSnapshot,
    pub resolved_mode: BaseMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewrite_fallback: Option<RewriteFallbackReason>,
    pub blocks: Vec<OutputBlock>,
    pub kind: RevisionKind,
    pub creation_reason: CreationReason,
    pub created_at: String,
}
impl OutputRevision {
    pub fn text(&self) -> String { self.blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n\n") }
    /// A deliberate edit is no longer the unchanged source fallback. Retain its selected Mode.
    pub fn clear_rewrite_fallback(&mut self) {
        if self.rewrite_fallback.take().is_some() && self.mode_snapshot.base_mode != BaseMode::Auto {
            self.resolved_mode = self.mode_snapshot.base_mode;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEvent {
    pub session_id: String,
    pub source_char_count: u64,
    pub capture_duration_ms: u64,
    pub ended_at: String,
    pub end_local_date: String,
    pub time_zone: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsage {
    pub date: String,
    pub count: u64,
    pub source_char_count: u64,
    pub capture_duration_ms: u64,
}

/// Lifetime counts of completed dictations, independent of retained history bodies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub count: u64,
    pub source_char_count: u64,
    pub capture_duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySummary {
    pub session_id: String,
    pub title: String,
    pub preview: String,
    pub ended_at: String,
    pub end_local_date: String,
    pub mode_id: String,
    pub mode_name: String,
    pub resolved_mode: Option<BaseMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewrite_fallback: Option<RewriteFallbackReason>,
    pub current_revision_id: Option<String>,
    pub source_revision_id: String,
    pub status: StoredSessionStatus,
    pub source_char_count: u64,
    pub capture_duration_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage { pub items: Vec<HistorySummary>, pub total: u64, pub next_cursor: Option<u64> }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryDetail {
    pub session: SessionRecord,
    pub source: SourceRevision,
    pub output: Option<OutputRevision>,
    pub can_undo: bool,
}

/// Explicit user-selected Unicode scalar ranges; never inferred from block array position.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSelection { pub item_id: String, pub start: usize, pub end: usize }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestorePatch {
    pub block_id: String,
    pub start: usize,
    pub end: usize,
    pub source_selections: Vec<SourceSelection>,
}

/// A bounded, explicit occurrence in one displayed output revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorrectionPatch {
    pub block_id: String,
    pub start: usize,
    pub end: usize,
    pub expected_text: String,
    pub replacement: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CorrectionDictionaryStatus { Added, Existing, NotRequested }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionReceipt {
    pub detail: HistoryDetail,
    pub dictionary: Option<DictionarySnapshot>,
    pub dictionary_status: CorrectionDictionaryStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliveryState { Prepared, Dispatched, Confirmed, Unconfirmed, Skipped, Failed }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryAttempt {
    pub id: String,
    pub session_id: String,
    pub generation: u64,
    pub output_revision_id: String,
    /// Only opaque HWND/PID/control hashes; do not place title or text here.
    pub target_fingerprint: String,
    pub claimed_at: String,
    pub state: DeliveryState,
    pub reason: Option<String>,
}
