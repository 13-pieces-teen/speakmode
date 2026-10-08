// Test-only parser copied verbatim from the SHA256-verified 0.1.1 source.
// Baseline file SHA256: 2a6050915d7b41157ce9e2b08267ca393edd58e07119607174315760fe972f5f
// Surrounding types and validate_source were unchanged by the Q1 increment.
use super::*;

pub fn parse_model_output(content: &str, mode: &ModeSnapshot, source: &SourceRevision) -> Result<RewriteResult, CloudError> {
    validate_source(source)?;
    if content.trim().is_empty(){return Err(CloudError::new("empty_output","rewrite","整理响应为空"));}
    if content.len()>1024*1024 {return Err(CloudError::new("response_too_large","rewrite","整理响应超过容量限制"));}
    let decoded: ModelOutput = serde_json::from_str(content).map_err(|_| CloudError::new("invalid_output", "rewrite", "整理结果格式无效，可重试或使用识别稿"))?;
    if decoded.resolved_mode == BaseMode::Auto || (mode.base_mode != BaseMode::Auto && decoded.resolved_mode != mode.base_mode) || decoded.blocks.is_empty() || decoded.blocks.len() > 300 {
        return Err(CloudError::new("invalid_output", "rewrite", "整理方式或结果结构无效"));
    }
    let allowed: HashSet<&str> = source.items.iter().filter(|i| i.is_final).map(|i| i.id.as_str()).collect();
    let mut total = 0;
    let mut blocks = Vec::new();
    for b in decoded.blocks {
        total += b.text.chars().count();
        let unique: HashSet<_> = b.source_refs.iter().collect();
        if b.text.trim().is_empty() || total > 80_000 || b.source_refs.is_empty() || unique.len() != b.source_refs.len() || b.source_refs.iter().any(|s| !allowed.contains(s.as_str())) {
            return Err(CloudError::new("invalid_source_mapping", "rewrite", "结果缺少有效来源，请核对或重试"));
        }
        blocks.push(OutputBlock { id: Uuid::new_v4().to_string(), text:b.text, source_refs:b.source_refs });
    }
    Ok(RewriteResult {resolved_mode:decoded.resolved_mode, blocks})
}

