use super::*;
use crate::core::{SourceItem,DictionarySnapshot,DictionaryEntry,BaseMode,ModeSnapshot,ProviderConfig};
use super::super::RewriteResult;
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
fn app_spelling_in_application_context_passes_generation_and_admission() {
    for input in [
        "用户可以在我们的这个 A P P 里启动录音。",
        "请安装这个 A P P。",
        "Open the A P P to start recording.",
    ] {
        let expected = input.replace("A P P", "APP");
        let source = source(input);
        let mode = mode();
        let request = super::super::minimum::prepare(&ProviderConfig::default(), &mode,
            &source, &DictionarySnapshot::default(), true).unwrap();
        let content = serde_json::json!({"resolvedMode":"light","text":expected}).to_string();
        let draft = super::super::minimum::parse(&content, &mode, &source, &request).unwrap();

        assert_eq!(normalize(input, false), expected);
        assert_eq!(draft.blocks[0].text, expected);
        assert!(super::super::review_admission(&source, &draft).is_ok());
        for changed in ["AP", "APPP", "API"] {
            let changed_draft = RewriteResult { resolved_mode: BaseMode::Light,
                blocks: vec![block(&expected.replace("APP", changed))] };
            assert!(super::super::review_admission(&source, &changed_draft).is_err());
        }
    }
}

#[test]
fn app_spelling_keeps_explicit_letter_and_literal_boundaries() {
    for input in [
        "应用开发课的三个独立字母 A P P，每个字母分别读。",
        "启动时显示 A、P、P，请分别保留。",
        "请原样引用“A P P”，再启动应用。",
        "启动命令是 `A P P`。",
        "The app teaches spelling: the letters are A P P.",
        "启动 A P P。我不确定拼写。",
        "A P P",
        "启动 A P P P。",
    ] {
        assert_eq!(normalize(input, false), input);
        let draft = RewriteResult { resolved_mode: BaseMode::Light,
            blocks: vec![block(&input.replace("A P P", "APP").replace("A、P、P", "APP"))] };
        assert!(super::super::review_admission(&source(input), &draft).is_err(), "{input}");
    }
    let input = "启动 A P P，再分别写字母 A、P、P。";
    let expected = "启动 APP，再分别写字母 A、P、P。";
    assert_eq!(normalize(input, false), expected);
    assert!(validate(&source(input), &[block(expected)]).is_ok());
    assert!(validate(&source(input), &[block("启动 APP，再分别写字母 APP。")]).is_err());
}

#[test]
fn unrelated_sentences_do_not_share_letter_preservation_instructions() {
    for input in ["我在看 A D H D 的研究。四个选项分别是 A、B、C、D，请分别保留。",
        "四个选项分别是 A、B、C、D，请分别保留。我在看 A D H D 的研究。"] {
        let expected=input.replace("A D H D","ADHD");
        assert_eq!(normalize(input,false),expected);
        assert!(validate(&source(input),&[block(&expected)]).is_ok());
        assert!(validate(&source(input),&[block(&expected.replace("A、B、C、D","ABCD"))]).is_err());
    }
    for input in ["研究 A D H D。不要合并这些字母。","We study A D H D. Keep these letters separate.",
        "讨论 A D H D。我不确定拼写。"] {
        assert_eq!(normalize(input,false),input);
        assert!(validate(&source(input),&[block(&input.replace("A D H D","ADHD"))]).is_err());
    }
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
fn ideographic_enumerations_override_term_context_at_each_occurrence() {
    for (input,bad) in [
        ("接口评审表的表头是 A、P、I，这里不是接口缩写。","接口评审表的表头是 API，这里不是接口缩写。"),
        ("页面标题写 SDK，表中的标记依次是 S、D、K。","页面标题写 SDK，表中的标记依次是 SDK。"),
        ("我们研究 API，试验位置写 A、P I。","我们研究 API，试验位置写 API。"),
        ("系统评审中的记录是 A、A、I、I，重复位置也要记录。","系统评审中的记录是 A、I，重复位置也要记录。"),
        ("资料介绍 A、I、U、I 的排列。","资料介绍 AI、UI 的排列。"),
    ] {
        let source=source(input);let before=source.clone();
        assert_eq!(normalize(input,false),input,"{input}");
        assert!(validate(&source,&[block(input)]).is_ok(),"{input}");
        assert!(validate(&source,&[block(bad)]).is_err(),"{bad}");assert_eq!(source,before);
    }
}
#[test]
fn another_spaced_term_cannot_mask_a_merged_list_with_the_same_letters() {
    let input="我们研究 A P I，表中记作 A、P、I。";
    let source=source(input);
    assert_eq!(normalize(input,false),"我们研究 API，表中记作 A、P、I。");
    assert!(validate(&source,&[block("我们研究 API，表中记作 A P I。")]).is_ok());
    assert!(validate(&source,&[block("我们研究 A P I，表中记作 API。")]).is_err());
    assert!(validate(&source,&[block("我们研究 API，表中记作 API。")]).is_err());
}
#[test]
fn punctuation_between_known_terms_and_hesitation_inside_terms_still_normalize() {
    for (input,expected) in [
        ("团队讨论 L L M、R A G 的技术资料。","团队讨论 LLM、RAG 的技术资料。"),
        ("服务使用 A，呃，P I，并讨论 R A G。","服务使用 API，并讨论 RAG。"),
        ("We are using A, um, P I and discussing R A G.","We are using API and discussing RAG."),
    ] {
        assert_eq!(normalize(input,false),expected,"{input}");
        assert!(validate(&source(input),&[block(expected)]).is_ok(),"{input}");
    }
}
#[test]
fn english_hesitation_is_only_consumed_inside_an_allowed_spelled_term() {
    for (input,expected) in [
        ("UM, we use A, uh, P I, um, after the review.","UM, we use API, um, after the review."),
        ("We use A, UM, P I.","We use API."),
        ("We use A, umlaut, P I.","We use A, umlaut, P I."),
        ("We use A, uh huh, P I.","We use A, uh huh, P I."),
        ("The letters are A, uh, P I.","The letters are A, uh, P I."),
        ("We discuss A、um、P、I.","We discuss A、um、P、I."),
    ] {
        assert_eq!(normalize(input,false),expected,"{input}");
        assert!(validate(&source(input),&[block(expected)]).is_ok(),"{input}");
    }
}
#[test]
fn dictionary_matching_cannot_override_a_containing_enumeration() {
    let dictionary=DictionarySnapshot{version:1,entries:vec![DictionaryEntry{id:"api".into(),term:"API".into(),created_at:"now".into(),updated_at:"now".into()}]};
    for input in ["资料讨论 A、P、I 的排列。","资料讨论 A P I、D 的排列。",
        "资料讨论 A、P I 的排列。","资料讨论 A P I、D D D 的排列。"] {
        assert_eq!(super::super::terms::canonicalize(input,&dictionary),input,"{input}");
        assert!(super::super::terms::matching_terms(&source(input),&dictionary).is_empty(),"{input}");
        assert_eq!(normalize(input,false),input,"{input}");
    }
    assert_eq!(super::super::terms::canonicalize("资料讨论 A P I 的接口。",&dictionary),"资料讨论 API 的接口。");
}
#[test]
fn long_enumerations_keep_the_separator_protection_across_the_old_scan_limit() {
    let input="资料讨论缩写与表格，序列是 A P I A P I A P I A P I、D。";
    let dictionary=DictionarySnapshot{version:1,entries:vec![DictionaryEntry{id:"api".into(),term:"API".into(),created_at:"now".into(),updated_at:"now".into()}]};
    assert_eq!(normalize(input,false),input);
    assert_eq!(super::super::terms::canonicalize(input,&dictionary),input);
    assert!(validate(&source(input),&[block("资料讨论缩写与表格，序列是 APIAPIAPIAPI、D。")]).is_err());
}
fn mode()->ModeSnapshot {ModeSnapshot{mode_id:"light".into(),name:"轻整理".into(),base_mode:BaseMode::Light,rules:String::new(),revision:1}}
#[test]
fn an_all_pass_semantic_verdict_cannot_waive_an_occurrence_local_enumeration() {
    let accept=r#"{"decision":"accept","checks":{"coverage":"pass","facts":"pass","scope":"pass","intent":"pass","cleanup":"pass"},"issues":[]}"#;
    for (input,bad) in [
        ("页面标题写 SDK，表中记作 S、D、K。","页面标题写 SDK，表中记作 SDK。"),
        ("我们研究 A P I，表中记作 A、P、I。","我们研究 A P I，表中记作 API。"),
    ] {
        let source=source(input);let mode=mode();let config=ProviderConfig::default();
        let draft=RewriteResult{resolved_mode:BaseMode::Light,blocks:vec![block(bad)]};
        let request=super::super::semantic_review::prepare_minimum_with_format(&config,&mode,&source,&draft,
            super::super::semantic_review::MinimumReviewFormat::Named).unwrap();
        let error=super::super::semantic_review::parse_minimum(accept,&request,&mode,&source,&draft).err().expect("five passes cannot waive native admission");
        assert_eq!(error.code,"rewrite_literal_changed");
    }
}
#[test]
fn generation_review_and_repair_share_source_bound_separate_letter_hints() {
    let lead="嗯，呃呃，就是就是，我我觉得方案可以讨论。";
    let source=source(&format!("{lead}页面写 SDK，表头为 S、D、K，接口资料用 A P I。"));let before=source.clone();
    let config=ProviderConfig::default();let mode=mode();
    let draft=RewriteResult{resolved_mode:BaseMode::Light,blocks:vec![block(&source.text())]};
    let generation=super::super::minimum::prepare(&config,&mode,&source,&DictionarySnapshot::default(),false).unwrap();
    let request=super::super::semantic_review::prepare_minimum_with_format(&config,&mode,&source,&draft,
        super::super::semantic_review::MinimumReviewFormat::Named).unwrap();
    let rejection=json!({"decision":"reject","checks":{"coverage":"pass","facts":"pass","scope":"pass","intent":"pass","cleanup":"fail"},
        "issues":[{"dimension":"cleanup","sourceQuote":lead,"draftQuote":lead,"reason":"清理本句卡顿"}]}).to_string();
    let review=super::super::semantic_review::parse_minimum(&rejection,&request,&mode,&source,&draft).unwrap();
    let repair=super::super::local_repair::prepare(&config,&mode,&source,&draft,&review).unwrap().unwrap();
    for body in [&generation.body,&request.body,&repair.body] {
        let system=body["messages"][0]["content"].as_str().unwrap();
        assert_eq!(system.matches(VERSION).count(),1);assert!(system.contains("An ideographic comma"));
        assert!(system.contains("Generic degrees or 度 retain an unspecified scale"));
        let data:Value=serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(data["notationHints"]["separateLetterGroups"],json!([{"spoken":"S、D、K","letters":["S","D","K"]}]));
        assert_eq!(data["notationHints"]["acronyms"],json!([{"spoken":"A P I","written":"API"}]));
    }
    assert_eq!(source,before);
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
