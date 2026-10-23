//! Published raw-prompt experiments keep their original Aliyun protocol scope.
//! A saved multi-provider setting must not send an ASR credential or an old
//! vendor-specific request to an unrelated text service.
use speakmode_lib::{cloud::{self, CloudError}, core::ProviderConfig};

#[allow(dead_code)]
pub fn rewrite_endpoint(config: &ProviderConfig) -> Result<String, CloudError> {
    if config.rewrite_provider != "aliyun" {
        return Err(CloudError::new("lab_provider_unsupported", "configuration",
            "此历史实验仅支持阿里云文字整理；请使用多供应商协议测试或选择阿里云后再运行"));
    }
    cloud::rewrite_endpoint(config)
}

#[allow(dead_code)]
pub fn paired_voice(config: &ProviderConfig) -> Result<(), CloudError> {
    rewrite_endpoint(config)?;
    if config.provider != "aliyun" {
        return Err(CloudError::new("lab_provider_unsupported", "configuration", "此固定音频对比实验仅支持阿里云语音识别"));
    }
    cloud::asr_endpoint(config)?;
    Ok(())
}

#[allow(dead_code)]
pub fn file_asr_endpoint(config: &ProviderConfig) -> Result<String, CloudError> {
    if config.provider != "aliyun" {
        return Err(CloudError::new("lab_provider_unsupported", "configuration", "此文件恢复实验仅支持阿里云语音识别"));
    }
    // This old Qwen file-ASR experiment uses its fixed audio model through
    // DashScope Chat Completions, not the streaming ASR WebSocket. Only the
    // ASR account's Aliyun region/workspace may determine that origin.
    let endpoint_config = ProviderConfig { rewrite_provider: "aliyun".into(),
        rewrite_model: "qwen3.8-flash".into(), rewrite_endpoint: None, ..config.clone() };
    cloud::rewrite_endpoint(&endpoint_config)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_rewrite_experiments_reject_other_providers_before_credentials() {
        let mut config = ProviderConfig { region: Some("beijing".into()), ..ProviderConfig::default() };
        for provider in ["openai", "deepseek", "kimi", "zhipu", "custom"] {
            config.rewrite_provider = provider.into();
            assert_eq!(rewrite_endpoint(&config).unwrap_err().code, "lab_provider_unsupported");
        }
    }
    #[test]
    fn file_asr_origin_uses_asr_account_without_a_rewrite_dependency() {
        let mut config = ProviderConfig { region: Some("beijing".into()), rewrite_provider: "custom".into(),
            rewrite_model: "unrelated".into(), rewrite_endpoint: Some("https://unrelated.invalid/v1".into()), ..ProviderConfig::default() };
        assert_eq!(file_asr_endpoint(&config).unwrap(), "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions");
        assert_eq!(config.rewrite_provider, "custom");
        config.provider = "google".into();
        assert_eq!(file_asr_endpoint(&config).unwrap_err().code, "lab_provider_unsupported");
        assert_eq!(paired_voice(&config).unwrap_err().code, "lab_provider_unsupported");
    }
}
