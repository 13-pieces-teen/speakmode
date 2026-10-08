//! Local protocol compatibility, not proof of account access or model quality.
//! New ASR families must supply a tested wire adapter before becoming selectable.
use super::CloudError;
use crate::core::ProviderConfig;
use serde::Serialize;

pub const QWEN_REALTIME_ADAPTER: &str = "qwen-realtime-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AsrProtocol { QwenRealtime, TaskStreaming, FileTranscription }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DictionarySupport {
    /// Kept for compatibility with the existing small fixed-audio experiment;
    /// current public documentation does not establish general support.
    LegacyCorpusUnverified,
    VocabularyAndContext,
    Vocabulary,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrModel {
    pub id: &'static str,
    pub label: &'static str,
    pub protocol: AsrProtocol,
    pub implemented: bool,
    pub adapter_version: Option<&'static str>,
    pub dictionary_support: DictionarySupport,
    pub timestamps: bool,
    pub recognition_languages: &'static [&'static str],
    /// Product-side recording limit. Never presented as provider's limit.
    pub product_audio_limit_seconds: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RewriteModel {
    pub id: &'static str,
    pub label: &'static str,
    pub implemented: bool,
    pub json_schema: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalog { pub asr: Vec<AsrModel>, pub rewrite: Vec<RewriteModel> }

const LANGUAGES: &[&str] = &["auto", "zh", "en"];

pub fn catalog() -> ModelCatalog {
    let asr = [
        ("qwen3-asr-flash-realtime", "Qwen3 实时（当前默认）", AsrProtocol::QwenRealtime, DictionarySupport::LegacyCorpusUnverified, false),
        ("qwen3-asr-flash-realtime-2025-10-27", "Qwen3 实时 · 2025-10-27", AsrProtocol::QwenRealtime, DictionarySupport::LegacyCorpusUnverified, false),
        ("qwen3-asr-flash-realtime-2026-02-10", "Qwen3 实时 · 2026-02-10（待评测）", AsrProtocol::QwenRealtime, DictionarySupport::LegacyCorpusUnverified, false),
        ("qwen-audio-3.1-asr-flash-streaming", "Qwen Audio 3.1（尚未接入）", AsrProtocol::TaskStreaming, DictionarySupport::VocabularyAndContext, true),
        ("qwen-audio-3.0-asr-flash-streaming", "Qwen Audio 3.0（尚未接入）", AsrProtocol::TaskStreaming, DictionarySupport::VocabularyAndContext, true),
        ("fun-asr-realtime", "Fun-ASR 实时（尚未接入）", AsrProtocol::TaskStreaming, DictionarySupport::Vocabulary, true),
        ("qwen-audio-3.1-asr-flash", "Qwen Audio 3.1 文件识别（尚未接入）", AsrProtocol::FileTranscription, DictionarySupport::VocabularyAndContext, true),
    ].into_iter().map(|(id,label,protocol,dictionary_support,timestamps)| {
        let implemented = protocol == AsrProtocol::QwenRealtime;
        AsrModel { id, label, protocol, implemented,
            adapter_version: implemented.then_some(QWEN_REALTIME_ADAPTER), dictionary_support,
            timestamps, recognition_languages: LANGUAGES, product_audio_limit_seconds: 600 }
    }).collect();
    let rewrite = [
        ("qwen-flash", "Qwen Flash（当前默认）", false),
        ("qwen-plus", "Qwen Plus（待评测）", false),
        ("qwen3.8-flash", "Qwen 3.8 Flash（待评测）", true),
        ("qwen3.7-plus", "Qwen 3.7 Plus（待评测）", true),
        ("qwen3.7-plus-2026-05-26", "Qwen 3.7 Plus · 2026-05-26（待评测）", true),
    ].into_iter().map(|(id,label,json_schema)|RewriteModel { id,label,implemented:true,json_schema }).collect();
    ModelCatalog { asr, rewrite }
}

pub fn validate_asr_adapter(config: &ProviderConfig) -> Result<AsrModel, CloudError> {
    let model = catalog().asr.into_iter().find(|m|m.id == config.asr_model)
        .ok_or_else(||CloudError::new("unsupported_asr_model", "configuration", "此语音模型尚未列入兼容清单，请选择已接入的实时识别模型"))?;
    if !model.implemented {
        return Err(CloudError::new("asr_adapter_not_implemented", "configuration", "此模型需要另一套识别接口，当前版本尚未接入；请选择 Qwen3 实时识别模型"));
    }
    if model.adapter_version != Some(config.adapter_version.as_str()) {
        return Err(CloudError::new("asr_adapter_mismatch", "configuration", "语音模型与接口版本不匹配，请重新选择已接入的实时识别模型"));
    }
    Ok(model)
}

pub fn validate_recognition(config:&ProviderConfig,language:&str)->Result<AsrModel,CloudError> {
    let model=validate_asr_adapter(config)?;
    if !model.recognition_languages.contains(&language) {
        return Err(CloudError::new("unsupported_asr_language", "configuration", "当前识别接口仅接入自动、中、英文，请更改识别语言"));
    }
    Ok(model)
}

pub fn validate_rewrite_model(config:&ProviderConfig)->Result<RewriteModel,CloudError> {
    catalog().rewrite.into_iter().find(|m|m.id == config.rewrite_model && m.implemented)
        .ok_or_else(||CloudError::new("unsupported_rewrite_model", "configuration", "此整理模型尚未验证接口兼容性，请选择兼容清单中的文本模型"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_and_known_realtime_snapshots_keep_the_existing_adapter() {
        for id in ["qwen3-asr-flash-realtime","qwen3-asr-flash-realtime-2025-10-27","qwen3-asr-flash-realtime-2026-02-10"] {
            let mut config=ProviderConfig::default();config.asr_model=id.into();
            let model=validate_recognition(&config,"auto").unwrap();
            assert_eq!(model.adapter_version,Some(QWEN_REALTIME_ADAPTER));
            assert_eq!(model.dictionary_support,DictionarySupport::LegacyCorpusUnverified);
        }
    }
    #[test]
    fn unknown_models_wrong_adapters_and_unsupported_languages_fail_without_fallback() {
        let mut config=ProviderConfig::default();config.asr_model="qwen3-asr-flash-realtime-2099-01-01".into();
        assert_eq!(validate_asr_adapter(&config).unwrap_err().code,"unsupported_asr_model");
        config=ProviderConfig::default();config.adapter_version="task-streaming".into();
        assert_eq!(validate_asr_adapter(&config).unwrap_err().code,"asr_adapter_mismatch");
        config=ProviderConfig::default();assert_eq!(validate_recognition(&config,"yue").unwrap_err().code,"unsupported_asr_language");
        config.rewrite_model="unknown-text-model".into();
        assert_eq!(validate_rewrite_model(&config).unwrap_err().code,"unsupported_rewrite_model");
    }
    #[test]
    fn listed_future_protocols_are_visible_but_not_selectable() {
        assert!(catalog().asr.iter().find(|m|m.id == "qwen-audio-3.1-asr-flash").unwrap().timestamps);
        for model in catalog().asr.into_iter().filter(|m|!m.implemented) {
            let mut config=ProviderConfig::default();config.asr_model=model.id.into();
            assert_eq!(validate_asr_adapter(&config).unwrap_err().code,"asr_adapter_not_implemented");
        }
    }
}
