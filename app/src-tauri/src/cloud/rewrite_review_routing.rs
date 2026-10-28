//! Content-free routing evidence. Shadow-only: no delivery authority.
use super::RewriteResult;
use crate::core::{AppWritingScene,BaseMode,DraftLanguage,ModeSnapshot,SourceRevision,writing_context::WritingContext};
use serde::Serialize;

#[derive(Debug,Clone,Serialize)]
#[serde(rename_all="camelCase")]
pub struct ReviewRoute {pub version:&'static str,pub would_skip_review:bool,pub reasons:Vec<&'static str>}

pub fn assess(mode:&ModeSnapshot,source:&SourceRevision,draft:&RewriteResult,context:Option<&WritingContext>)->ReviewRoute {
    let original=source.text();let output=draft.blocks.iter().map(|b|b.text.as_str()).collect::<Vec<_>>().join("\n\n");
    let mut reasons=Vec::new();
    if original.trim()!=output.trim(){reasons.push("textChanged");}
    if !source.complete||source.items.iter().any(|item|!item.is_final){reasons.push("incompleteSource");}
    if draft.resolved_mode==BaseMode::Structure||mode.base_mode==BaseMode::Structure{reasons.push("structuredLayout");}
    if !mode.rules.trim().is_empty()||context.is_some_and(|c|c.language!=DraftLanguage::AsSpoken||c.personalization.is_some()
        ||c.input.is_some()||!matches!(c.scene,AppWritingScene::General|AppWritingScene::Chat)){reasons.push("explicitWritingPolicy");}
    if super::local_repair::residue_hint(source,draft).is_some(){reasons.push("spokenResidue");}
    // Do not infer low risk from short length or model confidence. Even an
    // unchanged short instruction can contain ambiguity requiring inspection.
    let lower=original.to_lowercase();
    if original.chars().any(|c|c.is_numeric())||["不","没","别","如果","除非","可能","改成","不对","not ","don't","unless ","if ","maybe "].iter().any(|word|lower.contains(word)) {
        reasons.push("factsConditionsOrCorrection");
    }
    ReviewRoute{version:"review-routing-shadow-v1",would_skip_review:reasons.is_empty(),reasons}
}

#[cfg(test)] mod tests {
    use super::*;
    use crate::core::{SourceItem,OutputBlock};
    fn route(source_text:&str,draft_text:&str)->ReviewRoute {
        let source=SourceRevision{id:"source".into(),session_id:"s".into(),asr_attempt_id:"a".into(),generation:1,provider:"fixture".into(),model:"fixture".into(),adapter_version:"fixture".into(),complete:true,created_at:"2026-10-28T00:00:00Z".into(),items:vec![SourceItem{id:"i".into(),provider_item_id:"0".into(),capture_order:0,text:source_text.into(),is_final:true,start_ms:None,end_ms:None}]};
        let mode=ModeSnapshot{mode_id:"auto".into(),name:"auto".into(),base_mode:BaseMode::Auto,rules:String::new(),revision:1};
        let draft=RewriteResult{resolved_mode:BaseMode::Light,blocks:vec![OutputBlock{id:"b".into(),text:draft_text.into(),source_refs:vec!["i".into()]}]};
        assess(&mode,&source,&draft,None)
    }
    #[test] fn short_negation_numbers_and_actual_rewrites_require_review(){
        for (source,draft) in [("明天不要付款。","明天不要付款。"),("预算 3000。","预算 3000。"),("如果通过再上线。","如果通过再上线。"),("会议在周五。","会议在周四。"),("甲给乙发邮件。","乙给甲发邮件。")]{assert!(!route(source,draft).would_skip_review);}
        assert!(route("早上好，今天见。","早上好，今天见。").would_skip_review);
    }
}
