//! Provider wire adaptation happens immediately before every HTTP request.
//! Generation, review and repair keep the same local response contracts; native
//! JSON mode is only a formatting aid, never a fidelity or delivery verdict.
use super::CloudError;
use crate::core::ProviderConfig;
use serde_json::{json, Value};

pub(super) fn for_provider(config: &ProviderConfig, body: &Value) -> Result<Value, CloudError> {
    let mut result = body.clone();
    let fields = result.as_object_mut()
        .ok_or_else(|| CloudError::new("invalid_request", "rewrite", "整理请求格式无效"))?;
    match config.rewrite_provider.as_str() {
        // Preserve the existing DashScope request exactly, including each
        // stage's bounded output budget and temperature.
        "aliyun" => {},
        "openai" => {
            fields.remove("enable_thinking");
            fields.remove("thinking");
            fields.remove("temperature");
            fields.insert("reasoning_effort".into(), json!("none"));
            if let Some(limit) = fields.remove("max_tokens") {
                fields.insert("max_completion_tokens".into(), limit);
            }
        },
        "deepseek" | "kimi" | "zhipu" => {
            fields.remove("enable_thinking");
            fields.remove("reasoning_effort");
            fields.remove("max_completion_tokens");
            fields.insert("thinking".into(), json!({"type":"disabled"}));
            // Kimi K2.6 fixes non-thinking temperature to 0.6. Sending the
            // Aliyun 0/0.2 temperatures is an API error, so use its default.
            if config.rewrite_provider == "kimi" { fields.remove("temperature"); }
        },
        "custom" => {
            // An OpenAI-compatible URL does not identify a model family.
            // Never leak a vendor-specific reasoning switch to that service.
            fields.remove("enable_thinking");
            fields.remove("thinking");
            fields.remove("reasoning_effort");
            fields.remove("max_completion_tokens");
        },
        _ => return Err(CloudError::new("unsupported_rewrite_provider", "configuration", "此整理供应商尚未接入")),
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptation_preserves_contract_payload_and_stage_budgets() {
        let body = json!({"model":"test", "messages":[{"role":"system","content":"Return JSON"}],
            "response_format":{"type":"json_object"},"enable_thinking":false,
            "temperature":0,"max_tokens":512,"stream":false});
        for provider in ["aliyun", "openai", "deepseek", "kimi", "zhipu", "custom"] {
            let mut config = ProviderConfig::default(); config.rewrite_provider = provider.into();
            let wire = for_provider(&config, &body).unwrap();
            assert_eq!(wire["messages"], body["messages"]);
            assert_eq!(wire["response_format"], body["response_format"]);
            assert_eq!(wire["stream"], false);
            if provider == "aliyun" { assert_eq!(wire, body); }
            else { assert!(wire.get("enable_thinking").is_none()); }
            if provider == "openai" {
                assert_eq!(wire["max_completion_tokens"], 512);
                assert!(wire.get("max_tokens").is_none());
                assert_eq!(wire["reasoning_effort"], "none");
            } else { assert_eq!(wire["max_tokens"], 512); }
            if matches!(provider, "deepseek" | "kimi" | "zhipu") {
                assert_eq!(wire["thinking"]["type"], "disabled");
            } else { assert!(wire.get("thinking").is_none()); }
            if matches!(provider, "openai" | "kimi") { assert!(wire.get("temperature").is_none()); }
        }
        let mut config = ProviderConfig::default(); config.rewrite_provider = "unknown".into();
        assert_eq!(for_provider(&config, &body).unwrap_err().code, "unsupported_rewrite_provider");
        assert_eq!(body["enable_thinking"], false, "adapting never mutates the frozen request");
    }
}
