pub mod asr;
mod asr_task;
mod asr_google;
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
    Ok(Endpoints{asr:asr_endpoint(config)?,rewrite:rewrite_endpoint(config)?})
}

fn aliyun_host(config:&ProviderConfig)->Result<String,CloudError> {
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
    Ok(host)
}

pub fn asr_endpoint(config:&ProviderConfig)->Result<String,CloudError> {
    let model=models::validate_asr_adapter(config)?;
    match config.provider.as_str() {
        "aliyun"=>{let host=aliyun_host(config)?;Ok(match model.protocol {
            models::AsrProtocol::TaskStreaming=>format!("wss://{host}/api-ws/v1/inference"),
            _=>format!("wss://{host}/api-ws/v1/realtime?model={}",config.asr_model),
        })},
        "google"=>Ok("https://generativelanguage.googleapis.com/v1beta/interactions".into()),
        _=>Err(CloudError::new("unsupported_asr_provider","configuration","此供应商尚未接入语音识别协议")),
    }
}

pub fn rewrite_endpoint(config:&ProviderConfig)->Result<String,CloudError> {
    models::validate_rewrite_model(config)?;
    if config.rewrite_provider!="custom" && config.rewrite_endpoint.is_some() {
        return Err(CloudError::new("invalid_endpoint","configuration","预设供应商不能覆盖服务地址"));
    }
    Ok(match config.rewrite_provider.as_str() {
        "aliyun"=>format!("https://{}/compatible-mode/v1/chat/completions",aliyun_host(config)?),
        "openai"=>"https://api.openai.com/v1/chat/completions".into(),
        "deepseek"=>"https://api.deepseek.com/chat/completions".into(),
        "kimi"=>"https://api.moonshot.cn/v1/chat/completions".into(),
        "zhipu"=>"https://open.bigmodel.cn/api/paas/v4/chat/completions".into(),
        "custom"=>custom_rewrite_endpoint(config.rewrite_endpoint.as_deref())?,
        _=>return Err(CloudError::new("unsupported_rewrite_provider","configuration","此供应商尚未接入文本整理协议")),
    })
}

/// Only an explicit compatible-service configuration can choose an origin.
/// Reject embedded secrets, redirects and malformed paths before touching keys.
pub fn custom_rewrite_endpoint(value:Option<&str>)->Result<String,CloudError> {
    let invalid=||CloudError::new("invalid_endpoint","configuration","请输入 HTTPS Base URL；本机服务可使用 HTTP，地址不能包含账号、查询参数或片段");
    let value=value.filter(|s|!s.is_empty()&&s.len()<=512&&!s.chars().any(|c|c.is_control()||c.is_whitespace())).ok_or_else(invalid)?;
    let mut url=reqwest::Url::parse(value).map_err(|_|invalid())?;
    let loopback=url.host_str().is_some_and(|host|host=="localhost" || host.trim_matches(['[',']']).parse::<std::net::IpAddr>().is_ok_and(|ip|ip.is_loopback()));
    if url.host_str().is_none()||!url.username().is_empty()||url.password().is_some()||url.query().is_some()||url.fragment().is_some()
        || !(url.scheme()=="https" || (url.scheme()=="http"&&loopback)) {return Err(invalid());}
    let path=url.path().trim_end_matches('/');
    if path.contains('%')||path.contains('\\') {return Err(invalid());}
    let path=if path.ends_with("/chat/completions") {path.to_owned()} else {format!("{path}/chat/completions")};
    url.set_path(&path);
    Ok(url.into())
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
    #[test]
    fn stages_route_independently_and_presets_cannot_override_origins() {
        let mut c=ProviderConfig {region:Some("beijing".into()),rewrite_provider:"deepseek".into(),rewrite_model:"deepseek-flash".into(),..ProviderConfig::default()};
        assert!(asr_endpoint(&c).unwrap().contains("dashscope.aliyuncs.com"));
        assert_eq!(rewrite_endpoint(&c).unwrap(),"https://api.deepseek.com/chat/completions");
        c.region=None;
        assert!(asr_endpoint(&c).is_err());assert!(rewrite_endpoint(&c).is_ok());
        c.provider="google".into();c.asr_model="gemini-3.5-transcribe".into();c.adapter_version=models::GOOGLE_TRANSCRIBE_ADAPTER.into();
        assert!(endpoints(&c).is_ok());
        c.rewrite_endpoint=Some("https://example.test/v1".into());assert!(rewrite_endpoint(&c).is_err());
    }
    #[test]
    fn custom_endpoints_have_explicit_validated_origins() {
        for invalid in ["http://cloud.example/v1","https://user:pass@cloud.example/v1","https://cloud.example/v1?api_key=secret","https://cloud.example/v1#key","file:///tmp/service"," https://cloud.example/v1","https://cloud.example/%2Fother","https://cloud.example/v1\tmore"] {
            assert!(custom_rewrite_endpoint(Some(invalid)).is_err(),"{invalid}");
        }
        assert_eq!(custom_rewrite_endpoint(Some("https://cloud.example/v1/")).unwrap(),"https://cloud.example/v1/chat/completions");
        assert_eq!(custom_rewrite_endpoint(Some("http://127.0.0.1:8080/v1")).unwrap(),"http://127.0.0.1:8080/v1/chat/completions");
        assert_eq!(custom_rewrite_endpoint(Some("http://[::1]:8080/v1")).unwrap(),"http://[::1]:8080/v1/chat/completions");
        assert_eq!(custom_rewrite_endpoint(Some("https://cloud.example/v1/chat/completions/")).unwrap(),"https://cloud.example/v1/chat/completions");
    }
}
