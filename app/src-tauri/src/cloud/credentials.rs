use super::CloudError;
use crate::core::ProviderConfig;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const SERVICE: &str = "SpeakMode.Cloud.v1";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CredentialStage { #[default] Asr, Rewrite }

impl CredentialStage {
    pub fn provider(self, config: &ProviderConfig) -> &str {
        match self { Self::Asr => &config.provider, Self::Rewrite => &config.rewrite_provider }
    }
}

fn account(config: &ProviderConfig, stage: CredentialStage) -> Result<String, CloudError> {
    let endpoint = match stage {
        CredentialStage::Asr => super::asr_endpoint(config)?,
        CredentialStage::Rewrite => super::rewrite_endpoint(config)?,
    };
    // Preserve the exact original Aliyun account so existing installations keep
    // their Key. Shared Aliyun stages deliberately refer to this same account.
    Ok(match stage.provider(config) {
        "aliyun" => format!("aliyun:{}:{}", config.region.as_deref().unwrap_or_default(), config.workspace.as_deref().unwrap_or("default")),
        "custom" => format!("custom:{:x}", Sha256::digest(endpoint.as_bytes())),
        provider => format!("{provider}:default"),
    })
}

fn entry(config: &ProviderConfig, stage: CredentialStage) -> Result<keyring::Entry, CloudError> {
    keyring::Entry::new(SERVICE, &account(config, stage)?)
        .map_err(|_| CloudError::new("credential_unavailable", "credentials", "无法访问系统凭据存储"))
}

pub fn configured_for(config: &ProviderConfig, stage: CredentialStage) -> Result<bool, CloudError> {
    match entry(config, stage)?.get_password() {
        Ok(secret) => Ok(!secret.is_empty()),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(_) => Err(CloudError::new("credential_unavailable", "credentials", "无法读取本应用的凭据状态")),
    }
}

/// Native-only use. Never return the value through a DTO or log it.
pub fn read_for(config: &ProviderConfig, stage: CredentialStage) -> Result<String, CloudError> {
    entry(config, stage)?.get_password().map_err(|_| CloudError::new("key_required", "credentials", match stage {
        CredentialStage::Asr => "请在设置中配置语音识别供应商的 API Key",
        CredentialStage::Rewrite => "请在设置中配置文字润色供应商的 API Key",
    }))
}

pub fn set_for(config: &ProviderConfig, stage: CredentialStage, secret: &str) -> Result<(), CloudError> {
    let secret = secret.trim();
    if secret.is_empty() || secret.len() > 4096 || secret.contains(['\r', '\n']) {
        return Err(CloudError::new("invalid_key", "credentials", "API Key 不能为空或包含换行"));
    }
    entry(config, stage)?.set_password(secret).map_err(|_| CloudError::new("credential_write_failed", "credentials", "API Key 未保存，请检查本机凭据服务"))
}

pub fn delete_for(config: &ProviderConfig, stage: CredentialStage) -> Result<(), CloudError> {
    match entry(config, stage)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(CloudError::new("credential_delete_failed", "credentials", "未能删除本应用的 Key")),
    }
}

// Compatibility for existing native ASR experiments and maintenance callers.
pub fn configured(config: &ProviderConfig) -> Result<bool, CloudError> { configured_for(config, CredentialStage::Asr) }
pub fn read(config: &ProviderConfig) -> Result<String, CloudError> { read_for(config, CredentialStage::Asr) }
pub fn set(config: &ProviderConfig, secret: &str) -> Result<(), CloudError> { set_for(config, CredentialStage::Asr, secret) }
pub fn delete(config: &ProviderConfig) -> Result<(), CloudError> { delete_for(config, CredentialStage::Asr) }

/// Kept in the recording worker only, with no Debug/Serialize implementation.
pub struct SessionCredentials { pub asr: String, pub rewrite: String }
pub fn read_session(config: &ProviderConfig) -> Result<SessionCredentials, CloudError> {
    read_session_with(config, read_for)
}
fn read_session_with(config: &ProviderConfig, mut read: impl FnMut(&ProviderConfig, CredentialStage) -> Result<String, CloudError>) -> Result<SessionCredentials, CloudError> {
    let asr = read(config, CredentialStage::Asr)?;
    let rewrite = if account(config, CredentialStage::Asr)? == account(config, CredentialStage::Rewrite)? {
        asr.clone()
    } else { read(config, CredentialStage::Rewrite)? };
    Ok(SessionCredentials { asr, rewrite })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mixed() -> ProviderConfig { ProviderConfig { region: Some("beijing".into()), rewrite_provider: "openai".into(), rewrite_model: "gpt-5.4-mini".into(), ..ProviderConfig::default() } }

    #[test]
    fn aliyun_account_is_unchanged_and_shared_only_with_aliyun() {
        let mut config = ProviderConfig { region: Some("beijing".into()), ..ProviderConfig::default() };
        assert_eq!(account(&config, CredentialStage::Asr).unwrap(), "aliyun:beijing:default");
        assert_eq!(account(&config, CredentialStage::Asr).unwrap(), account(&config, CredentialStage::Rewrite).unwrap());
        config.workspace = Some("workspace-123".into());
        assert_eq!(account(&config, CredentialStage::Rewrite).unwrap(), "aliyun:beijing:workspace-123");
        let mixed = mixed();
        assert_ne!(account(&mixed, CredentialStage::Asr).unwrap(), account(&mixed, CredentialStage::Rewrite).unwrap());
    }

    #[test]
    fn stages_freeze_distinct_keys_before_settings_change() {
        let mut config = mixed();
        let mut calls = Vec::new();
        let keys = read_session_with(&config, |_, stage| { calls.push(stage); Ok(match stage { CredentialStage::Asr => "asr-only", CredentialStage::Rewrite => "rewrite-only" }.into()) }).unwrap();
        config.rewrite_provider = "deepseek".into();
        config.rewrite_model = "deepseek-chat".into();
        assert_eq!(calls, [CredentialStage::Asr, CredentialStage::Rewrite]);
        assert_eq!(keys.asr, "asr-only");
        assert_eq!(keys.rewrite, "rewrite-only");
    }

    #[test]
    fn shared_aliyun_account_is_read_once() {
        let config = ProviderConfig { region: Some("beijing".into()), ..ProviderConfig::default() };
        let mut reads = 0;
        let keys = read_session_with(&config, |_, _| { reads += 1; Ok("same-account".into()) }).unwrap();
        assert_eq!(reads, 1);
        assert_eq!(keys.asr, keys.rewrite);
    }

    #[test]
    fn custom_endpoint_changes_key_account_but_equivalent_urls_share_it() {
        let mut config=mixed();config.rewrite_provider="custom".into();config.rewrite_model="local-model".into();
        config.rewrite_endpoint=Some("https://first.example/v1".into());
        let first=account(&config,CredentialStage::Rewrite).unwrap();
        config.rewrite_endpoint=Some("https://first.example/v1/chat/completions".into());
        assert_eq!(account(&config,CredentialStage::Rewrite).unwrap(),first);
        config.rewrite_endpoint=Some("https://second.example/v1".into());
        assert_ne!(account(&config,CredentialStage::Rewrite).unwrap(),first);
        assert!(!first.contains("first.example"));
    }

    #[test]
    fn rewrite_credentials_do_not_require_an_asr_model_or_aliyun_region() {
        let mut config=mixed();config.region=None;config.asr_model="unconfigured-asr".into();
        assert_eq!(account(&config,CredentialStage::Rewrite).unwrap(),"openai:default");
        assert!(account(&config,CredentialStage::Asr).is_err());
    }
}
