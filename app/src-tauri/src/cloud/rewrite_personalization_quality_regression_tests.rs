//! Synthetic development regressions, never reruns of the exposed holdout.
use super::*;
use crate::core::{AppWritingScene,DraftLanguage,PersonalizationConfig,PersonalProfile,writing_context::WritingContext};

fn context()->WritingContext {
    let config=PersonalizationConfig{enabled:true,active_profile_id:Some("local".into()),profiles:vec![PersonalProfile{
        id:"local".into(),name:"local only".into(),profession:"软件工程".into(),domain:"系统研发".into(),
        description:String::new(),tasks:vec!["汇报进展".into()],audience:"同事".into(),
        scenes:vec![AppWritingScene::Technical],instructions:String::new(),}],..Default::default()};
    WritingContext{scene:AppWritingScene::Technical,language:DraftLanguage::AsSpoken,input:None,
        personalization:config.freeze(AppWritingScene::Technical)}
}
async fn run(server:&Server,input:&str)->Outcome {
    let(config,mode,source)=fixture(input);
    execute_until_with_writing_context(&client(),&config,"synthetic",&mode,&source,&DictionarySnapshot::default(),
        CancellationToken::new(),&server.endpoint,RewriteExecution::for_duration(Duration::from_secs(3)),None,Some(&context())).await
}

#[tokio::test]
async fn personalized_letter_lists_are_rejected_even_with_an_all_pass_reviewer(){
    for (input,bad) in [
        ("网格列名依次为 S、D、K，每列一个字母。旁边说明讨论 SDK 这个缩写。","网格列名依次为 SDK，每列一个字母。旁边说明讨论 SDK 这个缩写。"),
        ("标题为 API；分组标签依次是 A、P、I，要逐项展示。","标题为 API；分组标签依次是 API，要逐项展示。"),
    ] {
        let server=Server::new(vec![Reply::draft(bad),Reply::new(ACCEPT)]).await;
        let out=run(&server,input).await;
        assert!(out.result.is_err(),"merged list must never reach delivery");
        assert_eq!(server.count(),1,"native list protection must precede semantic acceptance");
    }
}

#[tokio::test]
async fn personalized_generic_degrees_cannot_gain_a_temperature_scale(){
    let input="样件外壳最高四十二度，内部没有测量。";
    for bad in ["样件外壳最高42℃，内部没有测量。","样件外壳最高42℉，内部没有测量。"] {
        let server=Server::new(vec![Reply::draft(bad),Reply::new(ACCEPT)]).await;
        let out=run(&server,input).await;
        assert!(out.result.is_err());assert_eq!(server.count(),1);
    }
}

#[tokio::test]
async fn explicit_scale_and_person_classifier_equivalence_still_reach_review(){
    for (input,draft) in [
        ("读数三十二摄氏度，暂时不要关闭。","读数32℃，暂时不要关闭。"),
        ("现场六个人，至少留三个人值守。","现场六人，至少留三人值守。"),
    ] {
        let server=Server::new(vec![Reply::draft(draft),Reply::new(ACCEPT)]).await;
        let out=run(&server,input).await;
        assert!(out.result.is_ok(),"explicit equivalent notation should not be a new fact: {:?}",out.result.err());
        assert_eq!(server.count(),2);assert!(out.trace.review.is_some());
    }
}
