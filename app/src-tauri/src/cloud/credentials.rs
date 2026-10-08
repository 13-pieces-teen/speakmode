use super::{CloudError, endpoints};
use crate::core::ProviderConfig;

const SERVICE: &str = "SpeakMode.Cloud.v1";

fn entry(config: &ProviderConfig) -> Result<keyring::Entry, CloudError> {
    endpoints(config)?;
    let account = format!("aliyun:{}:{}", config.region.as_deref().unwrap_or_default(), config.workspace.as_deref().unwrap_or("default"));
    keyring::Entry::new(SERVICE, &account)
        .map_err(|_| CloudError::new("credential_unavailable", "credentials", "无法访问 Windows 凭据管理器"))
}

pub fn configured(config: &ProviderConfig) -> Result<bool, CloudError> {
    match entry(config)?.get_password() {
        Ok(secret) => Ok(!secret.is_empty()),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(_) => Err(CloudError::new("credential_unavailable", "credentials", "无法读取本应用的凭据状态")),
    }
}

/// Native-only use. Never return the value through a DTO or log it.
pub fn read(config: &ProviderConfig) -> Result<String, CloudError> {
    entry(config)?.get_password().map_err(|_| CloudError::new("key_required", "credentials", "请在设置中配置该地域的 API Key"))
}

pub fn set(config: &ProviderConfig, secret: &str) -> Result<(), CloudError> {
    let secret = secret.trim();
    if secret.is_empty() || secret.len() > 4096 || secret.contains(['\r', '\n']) {
        return Err(CloudError::new("invalid_key", "credentials", "API Key 不能为空或包含换行"));
    }
    entry(config)?.set_password(secret).map_err(|_| CloudError::new("credential_write_failed", "credentials", "API Key 未保存，请检查本机凭据服务"))
}

pub fn delete(config: &ProviderConfig) -> Result<(), CloudError> {
    match entry(config)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(CloudError::new("credential_delete_failed", "credentials", "未能删除本应用的 Key")),
    }
}
