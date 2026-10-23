use super::*;
use crate::core::{SourceItem,DictionarySnapshot,DictionaryEntry};
fn source(text:&str)->SourceRevision {SourceRevision{id:"source".into(),session_id:"s".into(),asr_attempt_id:"a".into(),generation:1,
    provider:"test".into(),model:"test".into(),adapter_version:"test".into(),complete:true,created_at:"test".into(),
    items:vec![SourceItem{id:"item".into(),provider_item_id:"1".into(),capture_order:0,text:text.into(),is_final:true,start_ms:None,end_ms:None}]}}
fn block(text:&str)->OutputBlock {OutputBlock{id:"b".into(),text:text.into(),source_refs:vec!["item".into()]}}
#[test]
fn context_recovers_known_terms_without_changing_letters_or_acronym_meaning() {
    for (input,expected) in [
        ("我最近在了解 A D H D 的相关资料，不是在说我已经确诊。","我最近在了解 ADHD 的相关资料，不是在说我已经确诊。"),
        ("We are comparing A D\nH D screening notes, but no conclusion has been reached.","We are comparing ADHD screening notes, but no conclusion has been reached."),
        ("We discussed A I agents and L L M evaluation.","We discussed AI agents and LLM evaluation."),
        ("I am using A I to prepare a presentation, and I still want a human review.","I am using AI to prepare a presentation, and I still want a human review."),
        ("团队评估 L L M，R A G，A I agents，三个方向还没上线。","团队评估 LLM，RAG，AI agents，三个方向还没上线。"),
        ("这个缩写是 X Q Z，我们还没有对外使用。","这个缩写是 XQZ，我们还没有对外使用。"),
        ("说明文字讨论 A，呃，D H D，不要补充诊断。","说明文字讨论 ADHD，不要补充诊断。"),
    ] {assert_eq!(normalize(input,false),expected,"{input}");}
}
#[test]
fn explicit_letters_choices_literals_identifiers_and_uncertain_groups_stay_separate() {
    for input in ["四个独立字母 A B C D，每个写三遍，不要当成一个词。",
        "请讨论四个字母 A D H D，每个字母分别读。","The letters are A and I.",
        "His name is Jack. Let me spell it: J A C K, four separate letters.",
        "The label is X Q Z. I do not know whether it is one abbreviation or three separate letters.",
        "请原样引用“a d h d”，讨论 `A D H D.txt` 与 `a_d_h_d`。",
        "Batch IV is different from batch I V. The labels are case sensitive.",
        "I, a, and you are different words.","a I","A D H H D", "A D H D D", "Learn about a.d.h.d.example.com.",
        "同时评估四个模型 A、D、H、D 的效果。","Compare four models A, D, H and D.","使用三个变量 A P I 来计算。"] {
        assert_eq!(normalize(input,false),input,"{input}");
    }
}
#[test]
fn literal_scope_does_not_disable_prose_normalization_elsewhere() {
    let input="代码保留 `let A = \"D H D\";` 和字段 `a_d_h_d`，说明文字讨论 A D H D，但不要改代码。";
    assert_eq!(normalize(input,false),"代码保留 `let A = \"D H D\";` 和字段 `a_d_h_d`，说明文字讨论 ADHD，但不要改代码。");
}
#[test]
fn independent_letter_lists_cannot_be_approved_as_one_acronym() {
    for (input,bad) in [("请讨论四个字母 A D H D，每个字母分别读。","请讨论 ADHD，每个字母分别读。"),
        ("这里是 A B C D。","这里是 ABCD。"),
        ("选项 A B C D，暂时选 B。","选项 ABCD，暂时选 B。"),
        ("同时评估四个模型 A、D、H、D 的效果。","同时评估四个模型 ADHD 的效果。"),
        ("The label is X Q Z. I do not know whether it is one abbreviation or three separate letters.","The label is XQZ. I do not know whether it is one abbreviation or three separate letters.")] {
        let source=source(input);assert!(validate(&source,&[block(input)]).is_ok());assert!(validate(&source,&[block(bad)]).is_err());
    }
}
#[test]
fn dictionary_hint_cannot_join_deliberate_spelling_units() {
    let dictionary=DictionarySnapshot{version:1,entries:vec![DictionaryEntry{id:"d".into(),term:"ADHD".into(),created_at:"now".into(),updated_at:"now".into()}]};
    let text="我在练习四个独立字母 A D H D，每个字母分开说。";
    assert_eq!(super::super::terms::canonicalize(text,&dictionary),text);
    let prose="我在研究 A D H D 的资料。";assert_eq!(super::super::terms::canonicalize(prose,&dictionary),"我在研究 ADHD 的资料。");
}
#[test]
fn long_unpunctuated_text_preserves_units_and_repeated_date_values() {
    let digits=format!("{}年", "1".repeat(10_000));
    assert_eq!(normalize(&digits,false),digits);
    let text="同时评估四个模型 A、D、H、D 的效果 ".repeat(200);
    assert_eq!(normalize(&text,false),text);
    let text="We graduate in twenty twenty seven ".repeat(200);
    assert_eq!(normalize(&text,true),"We graduate in 2027 ".repeat(200));
}
#[test]
fn frozen_synthetic_notation_corpus_meets_positive_and_preservation_requirements() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../docs/experiments/fixtures/spoken-format-2026-10-23.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input=case["fragments"].as_array().unwrap().iter().map(|s|s.as_str().unwrap()).collect::<Vec<_>>().join("\n");
        let before=source(&input);let result=normalize(&input,case["english"].as_bool().unwrap());
        for needle in case["mustContain"].as_array().unwrap(){assert!(result.contains(needle.as_str().unwrap()),"{} missing {} in {}",case["id"],needle,result);}
        for needle in case["mustNotContain"].as_array().unwrap(){assert!(!result.contains(needle.as_str().unwrap()),"{} invented {} in {}",case["id"],needle,result);}
        assert!(validate(&before,&[block(&result)]).is_ok(),"{}",case["id"]);assert_eq!(before.text(),input);
    }
}
