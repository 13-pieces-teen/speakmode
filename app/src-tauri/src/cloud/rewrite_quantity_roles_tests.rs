use crate::{cloud::rewrite::{parse_model_output,parse_natural_output},core::{BaseMode,ModeSnapshot,SourceItem,SourceRevision}};
use serde_json::json;

fn accepts(texts:&[&str],output:&str)->bool {
    let mode=ModeSnapshot{mode_id:"auto".into(),name:"自动".into(),base_mode:BaseMode::Auto,rules:String::new(),revision:1};
    let source=SourceRevision{id:"quantity-source".into(),session_id:"session".into(),asr_attempt_id:"attempt".into(),generation:1,
        provider:"synthetic".into(),model:"fixed".into(),adapter_version:"test".into(),complete:true,created_at:"now".into(),
        items:texts.iter().enumerate().map(|(index,text)|SourceItem{id:format!("item-{index}"),provider_item_id:index.to_string(),capture_order:index as u64,
            text:(*text).into(),is_final:true,start_ms:None,end_ms:None}).collect()};
    let original=source.clone();
    let natural=parse_natural_output(&json!({"resolvedMode":"light","text":output}).to_string(),&mode,&source);
    let baseline=parse_model_output(&json!({"resolvedMode":"light","blocks":[{"text":output,"sourceRange":[0,source.items.len()-1]}]}).to_string(),&mode,&source);
    assert_eq!(natural.is_ok(),baseline.is_ok(),"both parsers must use the same quantity roles");
    assert_eq!(source,original,"role views cannot modify ASR");
    if let Ok(result)=natural.as_ref(){assert_eq!(result.blocks[0].source_refs,source.items.iter().map(|item|item.id.clone()).collect::<Vec<_>>());}
    natural.is_ok()
}

#[test]
fn quantity_roles_repeated_determiner_keeps_one_count_without_noun_allowlist() {
    for noun in ["游戏","工具","笔记","索引器"] {
        assert!(accepts(&[&format!("我想做一个这样的一个{noun}。")],&format!("我想做一个{noun}。")),"{noun}");
        assert!(accepts(&[&format!("我想做一个这样的一个{noun}。")],&format!("我想做一个这样的{noun}。")),"{noun}");
    }
}

#[test]
fn quantity_roles_restart_does_not_erase_counts_or_merge_objects() {
    for (source,output) in [
        ("我想做一个这样的一个游戏。","我想做游戏。"),
        ("我想做一个这样的一个游戏。","我想做两个游戏。"),
        ("我想做一个这样的一个游戏。","我想做一个工具。"),
        ("我想做一个这样的两个游戏。","我想做一个游戏。"),
        ("我想做一个这样的一个游戏，另一个游戏先保留。","我想做一个游戏。"),
        ("我想做一个这样的一个游戏和一个工具。","我想做一个游戏。"),
        ("我想做一个，测试这样的一个游戏。","我想做一个游戏。"),
        ("只做一个这样的一个游戏。","做一个游戏。"),
        ("每次做一个这样的一个游戏。","每次做两个游戏。"),
        ("甲想做一个这样的一个游戏。","乙想做一个游戏。"),
    ] {assert!(!accepts(&[source],output),"{source} -> {output}");}
}

#[test]
fn quantity_roles_local_classification_allows_ge_kuan_but_retains_type_and_owner() {
    for (source,output) in [
        ("这个作品是一个 roguelike 游戏。","这个作品是一款 roguelike 游戏。"),
        ("这个作品是一款 roguelike 游戏。","这个作品是一个 roguelike 游戏。"),
        ("这个组件是一个索引工具。","这个组件是一款索引工具。"),
        ("我们这个游戏是一个 rogue-like 游戏。","这个游戏是一款 rogue-like 游戏。"),
    ] {assert!(accepts(&[source],output),"{source} -> {output}");}
    for (source,output) in [
        ("这个作品是一个 roguelike 游戏。","这个作品是两款 roguelike 游戏。"),
        ("这个作品是一款 roguelike 游戏。","这个作品是两款 roguelike 游戏。"),
        ("这个作品是一款 roguelike 游戏。","这个作品是 roguelike 游戏。"),
        ("甲作品是一个 roguelike 游戏。","乙作品是一款 roguelike 游戏。"),
        ("这个作品是一个 roguelike 游戏。","这个作品是一款消除游戏。"),
        ("这个作品是一个 roguelike 游戏。","这个作品需要一款 roguelike 游戏。"),
        ("做一个索引工具。","做一款索引工具。"),
        ("这个作品不是一个 roguelike 游戏。","这个作品是一款 roguelike 游戏。"),
        ("这个作品可能是一个 roguelike 游戏。","这个作品是一款 roguelike 游戏。"),
        ("我觉得这个工具是一个危险工具。","这个工具是一款危险工具。"),
        ("我想做一个游戏，这个游戏是一个 roguelike 游戏。","这个游戏是一款 roguelike 游戏。"),
        ("甲作品是一个索引工具。","甲作品是一款索引工具。乙作品是一款编辑器。"),
    ] {assert!(!accepts(&[source],output),"{source} -> {output}");}
}

#[test]
fn quantity_roles_single_local_introduction_can_support_demonstrative_classification() {
    let source=["我想做一个这样的一个工具。","就是在我们这个工具是一个索引工具。"];
    assert!(accepts(&source,"我想做一个工具。这是一款索引工具。"));
    assert!(accepts(&["我想做一个这样的一个工具，名字叫星图。","就是在我们这个工具是一个索引工具。"],"我想做一个名为《星图》的工具。这是一款索引工具。"));
    assert!(accepts(&["我想做一个这样的一个工具，嗯，名字叫，星图。那首先第一点，我觉得，就是在我们这个工具是一个索引工具。"],"我想做一个名为《星图》的工具。首先，这是一款索引工具。"));
    assert!(!accepts(&["我想做一个这样的一个工具，嗯，名字叫，星图。那首先第一点，就是在我们这个工具是一个索引工具。"],"我想做一个名为《名字叫》的工具。这是一款索引工具。"));
    for (source,output) in [
        (vec!["我想做一个工具和一个程序。","这个工具是一个索引工具。"],"我想做一个工具和一个程序。这是一款索引工具。"),
        (vec!["我想做一个工具。","我想做一个程序。","这个工具是一个索引工具。"],"我想做一个工具。我想做一个程序。这是一款索引工具。"),
        (vec!["我想做一个工具。","先测试性能。","这个工具是一个索引工具。"],"我想做一个工具。先测试性能。这是一款索引工具。"),
    ] {assert!(!accepts(&source,output),"{source:?}");}
}

#[test]
fn quantity_roles_outline_numbers_are_not_counts_but_item_content_still_is() {
    assert!(accepts(&["第一点，这个工具是一个索引工具。第二点，面向的是小学生，每次使用三分钟。"],
        "1. 这个工具是一款索引工具。\n2. 面向小学生，每次使用3分钟。"));
    assert!(!accepts(&["第一点，这个工具是一个索引工具。第二点，面向的是小学生，每次使用三分钟。"],
        "1. 这个工具是一款索引工具。\n2. 面向小学生，每次使用5分钟。"));
    assert!(!accepts(&["如果学校同意，才面向小学生。第一点，这个工具是一个索引工具。"],
        "面向小学生。1. 这个工具是一款索引工具。"));
}

#[test]
fn quantity_roles_quotes_and_source_references_never_receive_alias_permission() {
    for (source,output) in [
        ("请保留原文“做一个这样的一个游戏”。","请保留原文“做一个游戏”。"),
        ("请保留代码 `它是一个索引工具`。","请保留代码 `它是一款索引工具`。"),
        ("我想做一个这样的一个游戏，预算三百元。","我想做一个游戏，预算五百元。"),
        ("先不要做一个这样的一个游戏。","先做一个游戏。"),
    ] {assert!(!accepts(&[source],output),"{source}");}
    for source in ["我想做一个这样的一个游戏。","这个工具是一款索引工具。","请保留原文“做一个这样的一个游戏”。"] {
        assert!(accepts(&[source],source),"identity follows the original guard: {source}");
    }
}
