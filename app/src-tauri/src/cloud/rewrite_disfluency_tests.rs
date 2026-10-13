use crate::{cloud::rewrite::{parse_model_output,parse_natural_output},core::{BaseMode,ModeSnapshot,SourceItem,SourceRevision}};
use serde_json::json;

fn accepts(texts:&[&str],output:&str)->bool {
    let mode=ModeSnapshot{mode_id:"auto".into(),name:"自动".into(),base_mode:BaseMode::Auto,rules:String::new(),revision:1};
    let source=SourceRevision{id:"source".into(),session_id:"session".into(),asr_attempt_id:"attempt".into(),generation:1,
        provider:"test".into(),model:"fixed".into(),adapter_version:"test".into(),complete:true,created_at:"now".into(),
        items:texts.iter().enumerate().map(|(index,text)|SourceItem{id:format!("item-{index}"),provider_item_id:index.to_string(),capture_order:index as u64,
            text:(*text).into(),is_final:true,start_ms:None,end_ms:None}).collect()};
    let original=source.clone();
    let natural=parse_natural_output(&json!({"resolvedMode":"light","text":output}).to_string(),&mode,&source);
    let baseline=parse_model_output(&json!({"resolvedMode":"light","blocks":[{"text":output,"sourceRange":[0,source.items.len()-1]}]}).to_string(),&mode,&source);
    assert_eq!(natural.is_ok(),baseline.is_ok(),"parsers must share the same meaning guard");
    assert_eq!(source,original,"guard must never edit immutable ASR source");
    if let Ok(output)=natural.as_ref() {assert_eq!(output.blocks[0].source_refs,source.items.iter().map(|item|item.id.clone()).collect::<Vec<_>>());}
    natural.is_ok()
}

#[test]
fn layout_filler_counts_and_restarted_prefixes_can_be_removed_without_rewriting_source() {
    for (source,output) in [
        (vec!["把简历的一个整个行距调大一点。"],"把简历的整个行距调大一点。"),
        (vec!["优化文档的一个整体排版。"],"优化文档的整体排版。"),
        (vec!["让这个简历的一个。","呃，简历的这样的一个呃。","就是简历的一个整个行距调大一点。"],"让简历的整个行距调大一点。"),
        (vec!["文档的一个。","呃，文档的这样的一个。","怎么说呢，就是文档的行距调大一点。"],"文档的行距调大一点。"),
        (vec!["让这个呃简历的一个。","呃，简历的这样的一个呃。","呃，怎么说呢，就是简历的一个整个行距调大一点。"],"让简历的整个行距调大一点。"),
    ] {
        assert!(accepts(&source,output),"{source:?}");
        assert!(accepts(&source,&source.join("\n")),"unchanged wording remains valid");
    }
}

#[test]
fn actual_counts_and_noun_continuations_are_not_abandoned_starts() {
    for (source,output) in [
        (vec!["把简历的一个项目删掉。"],"把简历的项目删掉。"),
        (vec!["把简历的两个项目删掉。"],"把简历的一个项目删掉。"),
        (vec!["把简历的一个。","项目删掉。"],"把简历的项目删掉。"),
        (vec!["把简历的一个。","就是把简历的一个项目删掉。"],"把简历的项目删掉。"),
        (vec!["把简历的1个整个行距调大一点。"],"把简历的整个行距调大一点。"),
        (vec!["只优化文档的一个整体排版。"],"只优化文档的整体排版。"),
        (vec!["优化文档的两个整体排版。"],"优化文档的整体排版。"),
        (vec!["文档的一个。","表格的行距调大一点。"],"表格的行距调大一点。"),
        (vec!["文档的一个。","两种字体。","文档的行距调大一点。"],"两种字体，文档的行距调大一点。"),
    ] {assert!(!accepts(&source,output),"{source:?}");}
}

#[test]
fn quotes_and_added_counts_stay_protected_during_layout_cleanup() {
    for (source,output) in [
        (vec!["请保留原文“简历的一个”。"],"请保留原文“简历”。"),
        (vec!["请保留原文“文档的一个整体排版”。"],"请保留原文“文档的整体排版”。"),
        (vec!["请保留原文“简历的一个。","就是简历的行距”。"],"请保留原文“简历的行距”。"),
        (vec!["把简历的一个整个行距调大一点。"],"把简历的两个整个行距调大一点。"),
        (vec!["把简历的一个整个行距调大一点，预算十元。"],"把简历的整个行距调大一点，预算五元。"),
    ] {assert!(!accepts(&source,output),"{source:?}");}
}

#[test]
fn affirmative_negative_ability_question_is_not_a_negative_fact() {
    for (source,output) in [
        ("我在思考是不是能够减少一个点？","我在思考是否能够减少一个点？"),
        ("是不是能打开文件？","能否打开文件？"),
        ("是不是可以保存文件？","可不可以保存文件？"),
        ("是不是能打开文件？","是不是能打开文件？"),
    ] {assert!(accepts(&[source],output),"{source}");}
    for (source,output) in [
        ("是不是能打开文件？","能打开文件。"),
        ("是不是能打开文件？","能否删除文件？"),
        ("不能打开文件。","能否打开文件？"),
        ("不是要删除文件。","要删除文件。"),
        ("别删除这个文件。","删除这个文件。"),
        ("是不是不能打开文件？","能否打开文件？"),
    ] {assert!(!accepts(&[source],output),"{source}");}
}

#[test]
fn reduction_synonym_retains_count_and_single_letter_party_ownership() {
    assert!(accepts(&["可以减少一个点。"],"可以删减一个点。"));
    assert!(accepts(&["可以删减一个点。"],"可以减少一个点。"));
    assert!(accepts(&["甲减少一个点，乙减少两个点。"],"甲删减一个点，乙删减两个点。"));
    for (source,output) in [
        ("可以减少一个点。","可以删减两个点。"),
        ("可以减少一个点。","可以删减内容。"),
        ("甲减少一个点，乙减少两个点。","甲减少两个点，乙减少一个点。"),
        ("甲减少一个点。","乙删减一个点。"),
        ("甲减少一个点。","减少一个点。"),
    ] {assert!(!accepts(&[source],output),"{source}");}
}

#[test]
fn discourse_introductions_and_attribute_phrases_are_not_exact_quantities() {
    for (source,output) in [
        (vec!["现在的话我有一个这样的想法。","我想让页面更紧凑。"],"我的想法是让页面更紧凑。"),
        (vec!["嗯，我现在有一个想法。","这个页面可以再紧凑一点。"],"这个页面可以再紧凑一点。"),
        (vec!["然后还有一个体验。","我停顿时最好继续录音。"],"我停顿时最好继续录音。"),
        (vec!["可以帮我想想这份材料还有什么可以优化的一个点？"],"可以帮我想想这份材料还有什么可以优化的点？"),
        (vec!["把页边距调回正常的一个 size。"],"把页边距调回正常的 size。"),
        (vec!["把材料的一个。","嗯，材料这样的一个行距调大一点。"],"把材料的行距调大一点。"),
    ] {assert!(accepts(&source,output),"{source:?}");}
}

#[test]
fn discourse_rules_do_not_remove_asserted_counts_choices_or_unrelated_subjects() {
    for (source,output) in [
        (vec!["我只有一个想法。","把页面缩小。"],"我的想法是把页面缩小。"),
        (vec!["我有一个想法。"],"我有想法。"),
        (vec!["提出两个想法。","把页面缩小并换个字体。"],"提出想法，把页面缩小并换个字体。"),
        (vec!["优化一个点而不是两个。"],"优化点。"),
        (vec!["一个size是小号，另一个是大号。"],"size是小号和大号。"),
        (vec!["正常的一个size是小号，另一个是大号。"],"正常的size是小号，另一个是大号。"),
        (vec!["他有一个想法。","我希望把页面缩小。"],"他有想法，我希望把页面缩小。"),
        (vec!["我有一个方案。","把页面缩小。"],"我的方案是把页面缩小。"),
        (vec!["请保留原话“我有一个想法”。","然后把页面缩小。"],"请保留原话“我有想法”，然后把页面缩小。"),
        (vec!["材料这样的一个行距方案被采纳。"],"材料这样的行距方案被采纳。"),
    ] {assert!(!accepts(&source,output),"{source:?}");}
}

#[test]
fn weekday_repair_recognizes_liang_hour_without_erasing_ambiguity_or_clock_facts() {
    assert!(accepts(&["会议定在周四下午两点，不对，是周五下午两点，地点还是八零二会议室。"],
        "会议定在周五下午两点，地点还是八零二会议室。"));
    assert!(accepts(&["会议定在周四下午两点。","不对，是周五下午两点。"],"会议定在周五下午两点。"));
    for (source,output) in [
        ("会议定在周四下午两点，不对，是周五下午两点。","会议定在周五下午三点。"),
        ("甲周四下午两点开会，乙也周四下午两点开会，不对，是周五下午两点。","甲周五下午两点开会，乙也周五下午两点开会。"),
        ("会议原来定在周四下午两点，后来改到周五下午两点。","会议定在周五下午两点。"),
    ] {assert!(!accepts(&[source],output),"{source}");}
}

#[test]
fn counted_comparison_is_not_an_explanation_and_split_limiters_are_retained() {
    for (source,output) in [
        (vec!["我有一个想法。","你有两个想法。"],"我有想法。你有两个想法。"),
        (vec!["我有一个想法。","嗯。","你有两个想法。"],"我有想法。你有两个想法。"),
        (vec!["只提供","合适的一个尺寸。"],"只提供合适的尺寸。"),
        (vec!["只","提供","合适的一个尺寸。"],"只提供合适的尺寸。"),
        (vec!["每次提供","正常的一个 size。"],"每次提供正常的 size。"),
    ] {assert!(!accepts(&source,output),"{source:?}");}
}

#[test]
fn evaluation_property_restart_counts_are_spoken_scaffolding() {
    for (source,output) in [
        (vec!["测试这个编辑器的一个，嗯。","一个效果吧。"],"测试这个编辑器的效果吧。"),
        (vec!["测试这个编辑器的一个。","嗯。","一个效果吧。"],"测试这个编辑器的效果吧。"),
        (vec!["看看这个模块的一个呃实际的效果。"],"看看这个模块的实际效果。"),
        (vec!["看看这个模块对应的一个呃实际的情况。"],"看看这个模块对应的实际情况。"),
        (vec!["观察模块的一个呃实际的状态。"],"观察模块的实际状态。"),
        (vec!["比较编辑器的一个呃整体的表现。"],"比较编辑器的整体表现。"),
        (vec!["对比这个工具，Tool A，","它对应的一个呃实际的情况。"],"对比 Tool A 的实际表现。"),
        (vec!["测试这个编辑器的一个，嗯。","一个效果吧，再看看它对应的一个呃实际的情况。"],
            "测试这个编辑器的效果吧，再看看它对应的实际情况。"),
    ] {
        assert!(accepts(&source,output),"{source:?}");
        assert!(accepts(&source,&source.join("\n")),"unchanged source remains valid");
    }
}

#[test]
fn evaluation_property_cleanup_keeps_counts_subjects_and_later_contrasts() {
    for (source,output) in [
        (vec!["测试模块的一个效果。"],"测试模块的效果。"),
        (vec!["测试模块的两个呃效果。"],"测试模块的效果。"),
        (vec!["测试模块的1个呃效果。"],"测试模块的效果。"),
        (vec!["只测试模块的一个，嗯。","一个效果吧。"],"只测试模块的效果吧。"),
        (vec!["仅","测试模块的一个，嗯。","一个效果吧。"],"仅测试模块的效果吧。"),
        (vec!["最多","看看模块对应的一个呃实际的情况。"],"最多看看模块对应的实际情况。"),
        (vec!["每次","看看模块对应的一个呃实际的情况。"],"每次看看模块对应的实际情况。"),
        (vec!["测试甲模块的一个，嗯。","乙模块的一个效果吧。"],"测试乙模块的效果吧。"),
        (vec!["测试甲模块的一个，嗯。","乙模块的效果吧。"],"测试乙模块的效果吧。"),
        (vec!["测试模块的一个，嗯。","一个选项吧。"],"测试模块的选项吧。"),
        (vec!["测试模块的一个呃实际的效果方案。"],"测试模块的实际效果方案。"),
        (vec!["测试模块的一个，嗯。","一个效果吧。","第二个效果再单独测试。"],
            "测试模块的效果吧，第二个效果再单独测试。"),
        (vec!["看看模块对应的一个呃实际的情况。","另外两个情况暂不处理。"],
            "看看模块对应的实际情况，另外两个情况暂不处理。"),
        (vec!["测试模块的一个，嗯。","一个效果吧。","不是两个效果。"],
            "测试模块的效果吧，不是两个效果。"),
        (vec!["保留原文“测试模块的一个，嗯。","一个效果吧”。"],"保留原文“测试模块的效果吧”。"),
        (vec!["保留代码 `模块对应的一个呃实际的情况`。"],"保留代码 `模块对应的实际情况`。"),
        (vec!["看看模块对应的一个呃实际的情况，预算三百元。"],"看看模块对应的实际情况，预算五百元。"),
        (vec!["看看模块对应的一个呃实际的情况。"],"看看模块对应的两个实际情况。"),
        (vec!["测试甲模块的一个，嗯。","一个效果吧。"],"测试乙模块的效果吧。"),
        (vec!["测试甲模块的一个，嗯。","一个效果吧。"],"先看看甲模块，再测试乙模块的效果吧。"),
        (vec!["看看甲模块对应的一个呃实际的情况。"],"看看甲模块，再看看乙模块的实际表现。"),
        (vec!["看看模块对应的一个呃实际的情况。"],"看看模块对应的实际成本。"),
        (vec!["测试模块的一个呃效果图。"],"测试模块的效果图。"),
        (vec!["检查模块的一个呃状态机。"],"检查模块的状态机。"),
        (vec!["看看模块的一个呃情况说明。"],"看看模块的情况说明。"),
        (vec!["创建模块的一个呃实际的状态。"],"创建模块的实际状态。"),
        (vec!["挑选模块的一个呃实际的效果。"],"挑选模块的实际效果。"),
        (vec!["删除模块的一个呃实际的状态。"],"删除模块的实际状态。"),
        (vec!["切换到模块的一个呃实际的状态。"],"切换到模块的实际状态。"),
        (vec!["观察机器的一个嗯状态。"],"观察机器的状态。"),
        (vec!["从多种运行状态中，观察机器的一个嗯实际的状态。"],"从多种运行状态中，观察机器的实际状态。"),
        (vec!["观察机器的一个嗯实际的状态，不是全部状态。"],"观察机器的实际状态，不是全部状态。"),
        (vec!["观察 Tool A 的一个嗯实际的情况，评价 Tool A 的实际表现。"],"评价 Tool A 的实际表现。"),
        (vec!["测试 Tool A 的一个嗯效果，再记录 Tool A 的效果。"],"再记录 Tool A 的效果。"),
        (vec!["只测试它的一个，","嗯。","一个效果吧。"],"只测试它的效果吧。"),
    ] {assert!(!accepts(&source,output),"{source:?}");}
}

#[test]
fn evaluation_property_positive_conjunction_cannot_hide_negative_action() {
    assert!(!accepts(&["测试 A 的效果。","测试模块的一个嗯效果。"],"不测试 A 和模块的效果。"));
}

#[test]
fn evaluation_property_action_words_inside_owner_are_not_actions() {
    assert!(!accepts(&["查看测试模块的一个嗯效果。"],"测试模块的效果。"));
    assert!(accepts(&["查看测试模块的一个嗯效果。"],"查看测试模块的效果。"));
}

#[test]
fn evaluation_property_pause_sentences_cannot_detach_quantity_limiters() {
    for (source,output) in [
        (vec!["只，嗯。","测试模块的一个呃效果。"],"只测试模块的效果。"),
        (vec!["只，嗯。","测试模块的一个呃效果。"],"测试模块的效果。"),
        (vec!["每次，嗯。","测试模块的一个呃效果。"],"每次测试模块的效果。"),
        (vec!["每次，嗯。","测试模块的一个呃效果。"],"测试模块的效果。"),
    ] {assert!(!accepts(&source,output),"{source:?}");}
}
