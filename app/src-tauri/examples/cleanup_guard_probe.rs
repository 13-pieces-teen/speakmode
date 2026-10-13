//! Offline synthetic probe of the production rewrite parser / integrity guard.
//! No network, microphone, credentials, user profile, history or model calls.
//! The screenshot only exposes one partial text; these cases cannot establish
//! the exact source or model output from that user session.
use serde_json::json;
use speakmode_lib::{
    cloud::rewrite::parse_model_output,
    core::{BaseMode, ModeSnapshot, SourceItem, SourceRevision},
};

struct Case {
    id: &'static str,
    category: &'static str,
    source: &'static [&'static str],
    output: &'static str,
    expected_accepted: bool,
}

fn main() {
    let cases = [
        Case { id: "visible-exact", category: "screenshot-text-only", source: &["轻语的效果，然后可以让呢，目前的应用做得更平滑一些。"], output: "轻语的效果，然后可以让呢，目前的应用做得更平滑一些。", expected_accepted: true },
        Case { id: "visible-remove-filler", category: "screenshot-text-only", source: &["轻语的效果，然后可以让呢，目前的应用做得更平滑一些。"], output: "轻语的效果，可以让目前的应用做得更平滑一些。", expected_accepted: true },
        Case { id: "visible-repunctuate", category: "screenshot-text-only", source: &["轻语的效果，然后可以让呢，目前的应用做得更平滑一些。"], output: "轻语的效果：可以让目前的应用做得更平滑一些。", expected_accepted: true },
        Case { id: "currently-synonym", category: "qualifier-scope", source: &["目前应用的延迟是100毫秒。"], output: "当前应用的延迟是100毫秒。", expected_accepted: true },
        Case { id: "currently-synonym-prose", category: "qualifier-scope", source: &["目前的应用做得更平滑一些。"], output: "当前的应用做得更平滑一些。", expected_accepted: true },
        Case { id: "filler-ne", category: "filler", source: &["这个功能呢，可以让应用更平滑。"], output: "这个功能可以让应用更平滑。", expected_accepted: true },
        Case { id: "filler-ranhou", category: "filler", source: &["然后这个功能，可以让应用更平滑。"], output: "这个功能可以让应用更平滑。", expected_accepted: true },
        Case { id: "yi-xie", category: "prose-boundary", source: &["让应用更平滑一些。"], output: "让应用更平滑。", expected_accepted: true },
        Case { id: "yi-xia", category: "prose-boundary", source: &["优化一下录音体验。"], output: "优化录音体验。", expected_accepted: true },
        Case { id: "yi-dian", category: "prose-boundary", source: &["让录音体验好一点。"], output: "改善录音体验。", expected_accepted: true },
        Case { id: "yi-bian", category: "prose-boundary", source: &["我重新解释一遍，就是让用户轻声说话也能被识别。"], output: "我的意思是，让用户轻声说话也能被识别。", expected_accepted: true },
        Case { id: "first-point", category: "prose-boundary", source: &["第一点是先粘贴，第二点是清理语气词。"], output: "- 先粘贴。\n- 清理语气词。", expected_accepted: true },
        Case { id: "indefinite-count", category: "candidate-false-positive", source: &["这是一个这样的功能。"], output: "这是这样的功能。", expected_accepted: true },
        Case { id: "indefinite-filler", category: "candidate-false-positive", source: &["我们希望有一个更好的体验。"], output: "我们希望体验更好。", expected_accepted: true },
        Case { id: "count-synonym", category: "candidate-false-positive", source: &["增加一个功能。"], output: "增加一项功能。", expected_accepted: true },
        Case { id: "very-natural", category: "candidate-false-positive", source: &["这个体验十分自然。"], output: "这个体验很自然。", expected_accepted: true },
        Case { id: "very-clear", category: "candidate-false-positive", source: &["这个问题十分清楚。"], output: "这个问题很清楚。", expected_accepted: true },
        Case { id: "repeat-identical", category: "repetition", source: &["这个功能很好，这个功能很好。"], output: "这个功能很好。", expected_accepted: true },
        Case { id: "repeat-count", category: "repetition", source: &["增加一个功能。", "增加一个功能。"], output: "增加一个功能。", expected_accepted: true },
        Case { id: "filler-amount", category: "filler", source: &["嗯，那个，预算是三千五百元，就是，三千五百元。"], output: "预算是3500元。", expected_accepted: true },
        Case { id: "semantic-repair", category: "self-correction", source: &["我要的其实是提高音量，不对，我的意思是提高识别质量。"], output: "我要的是提高识别质量。", expected_accepted: true },
        Case { id: "semantic-repair-fragments", category: "self-correction", source: &["我要的其实是提高音量。", "不对，我的意思是提高识别质量。"], output: "我要的是提高识别质量。", expected_accepted: true },
        Case { id: "numeric-repair", category: "self-correction", source: &["预算是5000元，不对，是3500元。"], output: "预算是3500元。", expected_accepted: true },
        Case { id: "numeric-repair-fragments", category: "self-correction", source: &["预算是5000元。", "不对，是3500元。"], output: "预算是3500元。", expected_accepted: true },
        Case { id: "presentation-bullets", category: "layout", source: &["先粘贴结果，然后清理语气词，最后调整排版。"], output: "1. 先粘贴结果。\n2. 清理语气词。\n3. 调整排版。", expected_accepted: true },
        Case { id: "real-number-kept", category: "numeric-control", source: &["预算是三千五百元。"], output: "预算是3500元。", expected_accepted: true },
        Case { id: "real-number-changed", category: "numeric-control", source: &["预算是三千五百元。"], output: "预算是5000元。", expected_accepted: false },
        Case { id: "real-number-dropped", category: "numeric-control", source: &["预算是三千五百元。"], output: "预算已安排。", expected_accepted: false },
        Case { id: "real-count-changed", category: "numeric-control", source: &["每次发送一个请求。"], output: "每次发送两个请求。", expected_accepted: false },
        Case { id: "real-unit-changed", category: "numeric-control", source: &["延迟是100毫秒。"], output: "延迟是100秒。", expected_accepted: false },
        Case { id: "version-changed", category: "numeric-control", source: &["当前版本0.2.8。"], output: "当前版本0.2.7。", expected_accepted: false },
        Case { id: "negative-dropped", category: "semantic-control", source: &["不要发邀请。"], output: "发邀请。", expected_accepted: false },
        Case { id: "new-degree-wording", category: "prose-generalization", source: &["这个录音功能十分流畅。"], output: "这个录音功能很流畅。", expected_accepted: true },
        Case { id: "new-experience-wording", category: "prose-generalization", source: &["新界面为同学带来一个比较自然的体验。"], output: "新界面为同学带来比较自然的体验。", expected_accepted: true },
        Case { id: "new-experience-expectation", category: "prose-generalization", source: &["团队期待有一个更流畅的体验。"], output: "团队期待体验更流畅。", expected_accepted: true },
        Case { id: "experience-source-items", category: "source-boundary", source: &["我们在录音后。", "希望有一个更顺畅的体验。"], output: "我们希望录音后体验更顺畅。", expected_accepted: true },
        Case { id: "count-source-items", category: "source-boundary", source: &["我们在录音后。", "实际需要一个新功能。"], output: "我们在录音后实际需要新功能。", expected_accepted: false },
        Case { id: "ten-duration", category: "degree-control", source: &["耗时十分。"], output: "耗时五分。", expected_accepted: false },
        Case { id: "ten-minutes", category: "degree-control", source: &["耗时十分钟。"], output: "耗时五分钟。", expected_accepted: false },
        Case { id: "ten-score", category: "degree-control", source: &["评分十分。"], output: "评分九分。", expected_accepted: false },
        Case { id: "ten-fraction", category: "degree-control", source: &["十分之一会失败。"], output: "五分之一会失败。", expected_accepted: false },
        Case { id: "degree-with-real-amount", category: "degree-control", source: &["体验十分自然，预算十元。"], output: "体验很自然，预算五元。", expected_accepted: false },
        Case { id: "actual-feature-count", category: "article-control", source: &["新增一个功能。"], output: "新增功能。", expected_accepted: false },
        Case { id: "only-one", category: "article-control", source: &["仅一个这样的功能。"], output: "仅这样的功能。", expected_accepted: false },
        Case { id: "each-time-one", category: "article-control", source: &["每次提供一个更好的体验。"], output: "每次提供更好的体验。", expected_accepted: false },
        Case { id: "at-least-one", category: "article-control", source: &["至少有一个这样的功能。"], output: "有这样的功能。", expected_accepted: false },
        Case { id: "actual-one", category: "article-control", source: &["实际是一个这样的功能。"], output: "实际是这样的功能。", expected_accepted: false },
        Case { id: "total-one", category: "article-control", source: &["总数是一个这样的功能。"], output: "总数是这样的功能。", expected_accepted: false },
        Case { id: "one-sufficient", category: "article-control", source: &["这是一个这样的功能就够了。"], output: "这是这样的功能就够了。", expected_accepted: false },
        Case { id: "article-to-two", category: "article-control", source: &["这是一个这样的功能。"], output: "这是两个这样的功能。", expected_accepted: false },
        Case { id: "experience-to-two", category: "article-control", source: &["我们希望有一个更好的体验。"], output: "我们希望有两个更好的体验。", expected_accepted: false },
        Case { id: "article-negation-dropped", category: "article-control", source: &["这不是一个这样的功能。"], output: "这是这样的功能。", expected_accepted: false },
        Case { id: "feature-classifier-reverse", category: "classifier-control", source: &["增加一项功能。"], output: "增加一个功能。", expected_accepted: true },
        Case { id: "feature-classifier-three", category: "classifier-control", source: &["增加三个功能。"], output: "增加3项功能。", expected_accepted: true },
        Case { id: "feature-classifier-value-change", category: "classifier-control", source: &["增加一个功能。"], output: "增加两项功能。", expected_accepted: false },
        Case { id: "feature-classifier-drop", category: "classifier-control", source: &["增加一项功能。"], output: "增加功能。", expected_accepted: false },
        Case { id: "feature-classifier-unit-change", category: "classifier-control", source: &["增加一项功能。"], output: "增加一条功能。", expected_accepted: false },
        Case { id: "clock-unchanged-format", category: "clock-control", source: &["十点十分结束。"], output: "10:10结束。", expected_accepted: true },
        Case { id: "clock-minute-change", category: "clock-control", source: &["十点十分结束。"], output: "十点五分结束。", expected_accepted: false },
        Case { id: "clock-hour-change", category: "clock-control", source: &["十点十分结束。"], output: "九点十分结束。", expected_accepted: false },
        Case { id: "decimal-duration-kept", category: "clock-control", source: &["耗时十点五分钟。"], output: "耗时630秒。", expected_accepted: true },
        Case { id: "decimal-is-not-clock", category: "clock-control", source: &["耗时十点五分钟。"], output: "耗时10:05。", expected_accepted: false },
        Case { id: "ordinal-point-degree", category: "clock-control", source: &["第一点十分自然。"], output: "第一点很自然。", expected_accepted: true },
        Case { id: "deictic-point-degree", category: "clock-control", source: &["这一点十分清楚。"], output: "这一点很清楚。", expected_accepted: true },
    ];
    let mut rows = Vec::new();
    for case in &cases {
        let source = SourceRevision {
            id: format!("source-{}", case.id), session_id: format!("synthetic-{}", case.id),
            asr_attempt_id: format!("asr-{}", case.id), generation: 1,
            provider: "synthetic-offline".into(), model: "fixed-text".into(),
            adapter_version: "cleanup-guard-probe-v1".into(), complete: true,
            created_at: "2026-10-10T00:00:00Z".into(),
            items: case.source.iter().enumerate().map(|(i, text)| SourceItem {
                id: format!("{}-{i}", case.id), provider_item_id: i.to_string(), capture_order: i as u64,
                text: (*text).into(), is_final: true, start_ms: None, end_ms: None,
            }).collect(),
        };
        let mode = ModeSnapshot { mode_id: "light".into(), name: "轻整理".into(), base_mode: BaseMode::Light, rules: String::new(), revision: 1 };
        let wire = json!({"resolvedMode":"light","blocks":[{"text":case.output,"sourceRange":[0,source.items.len()-1]}]});
        let result = parse_model_output(&wire.to_string(), &mode, &source);
        rows.push(json!({"id":case.id,"category":case.category,"source":case.source,"output":case.output,
            "expectedAccepted":case.expected_accepted,"actualAccepted":result.is_ok(),
            "expectationMet":result.is_ok()==case.expected_accepted,
            "error":result.as_ref().err().map(|e|json!({"code":e.code,"message":e.message}))}));
    }
    let matched = rows.iter().filter(|r|r["expectationMet"]==true).count();
    println!("{}", serde_json::to_string_pretty(&json!({
        "scope":"offline-synthetic-production-parser-probe; screenshot full source/output unavailable",
        "total":rows.len(),"expectationsMet":matched,"mismatches":rows.len()-matched,"rows":rows,
    })).unwrap());
}
