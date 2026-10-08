pub mod asr;
mod asr_task;
pub mod credentials;
pub mod models;
pub mod rewrite;

use crate::core::ProviderConfig;
use serde::Serialize;

#[derive(Debug, Clone, thiserror::Error, Serialize)]
#[error("{message}")]
#[serde(rename_all = "camelCase")]
pub struct CloudError {
    pub code: String,
    pub stage: String,
    pub message: String,
}
impl CloudError {
    pub fn new(code: &str, stage: &str, message: &str) -> Self {
        Self { code: code.into(), stage: stage.into(), message: message.into() }
    }
}

pub struct Endpoints { pub asr: String, pub rewrite: String }

pub fn endpoints(config: &ProviderConfig) -> Result<Endpoints, CloudError> {
    if config.provider != "aliyun" {
        return Err(CloudError::new("unsupported_provider", "configuration", "当前版本只支持阿里云百炼"));
    }
    for model in [&config.asr_model, &config.rewrite_model] {
        if model.is_empty() || model.len() > 100 || !model.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)) {
            return Err(CloudError::new("invalid_model", "configuration", "模型名称无效"));
        }
    }
    let asr_model = models::validate_asr_adapter(config)?;
    models::validate_rewrite_model(config)?;
    let (legacy, region) = match config.region.as_deref() {
        Some("beijing") => ("dashscope.aliyuncs.com", "cn-beijing"),
        Some("singapore") => ("dashscope-intl.aliyuncs.com", "ap-southeast-1"),
        _ => return Err(CloudError::new("region_required", "configuration", "请先选择 Key 所属的服务地域")),
    };
    let host = match config.workspace.as_deref().filter(|s| !s.is_empty()) {
        Some(workspace) => {
            if workspace.len() > 80 || !workspace.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
                return Err(CloudError::new("invalid_workspace", "configuration", "业务空间 ID 无效"));
            }
            format!("{workspace}.{region}.maas.aliyuncs.com")
        }
        None => legacy.into(),
    };
    Ok(Endpoints {
        asr: match asr_model.protocol {
            models::AsrProtocol::TaskStreaming => format!("wss://{host}/api-ws/v1/inference"),
            _ => format!("wss://{host}/api-ws/v1/realtime?model={}", config.asr_model),
        },
        rewrite: format!("https://{host}/compatible-mode/v1/chat/completions"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audio31_streaming_uses_task_endpoint_and_own_adapter() {
        let mut config = ProviderConfig::default();
        config.region = Some("beijing".into());
        config.asr_model = "qwen-audio-3.1-asr-flash-streaming".into();
        config.adapter_version = "qwen-task-streaming-v1".into();
        let endpoint = endpoints(&config).expect("implemented Audio 3.1 must have its own task transport");
        assert_eq!(endpoint.asr, "wss://dashscope.aliyuncs.com/api-ws/v1/inference");
        assert_eq!(models::validate_recognition(&config, "zh").unwrap().protocol, models::AsrProtocol::TaskStreaming);
    }
    #[test]
    fn unsupported_streaming_model_is_rejected_before_constructing_a_legacy_endpoint() {
        let mut config = ProviderConfig::default();
        config.region = Some("beijing".into());
        config.asr_model = "qwen-audio-3.0-asr-flash-streaming".into();
        let error = endpoints(&config).err().expect("a run-task model must not use session.update");
        assert_eq!(error.code, "asr_adapter_not_implemented");
    }
    #[test]
    fn arbitrary_hosts_and_model_query_injection_are_not_allowed() {
        let mut c = ProviderConfig::default();
        assert!(endpoints(&c).is_err());
        c.region = Some("beijing".into());
        assert!(endpoints(&c).unwrap().asr.starts_with("wss://dashscope.aliyuncs.com/"));
        c.workspace = Some("evil.example/path".into());
        assert!(endpoints(&c).is_err());
        c.workspace = Some("workspace-123".into());
        assert!(endpoints(&c).unwrap().rewrite.starts_with("https://workspace-123.cn-beijing.maas.aliyuncs.com/"));
        c.asr_model = "model&key=other".into();
        assert!(endpoints(&c).is_err());
    }
}
