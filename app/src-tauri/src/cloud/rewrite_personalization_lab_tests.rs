//! Lab transport/ledger checks use localhost envelopes, not model verdicts.
use super::*;
use crate::core::{AppWritingScene, DraftLanguage, writing_context::WritingContext};

fn lab_fixture(endpoint:&str)->(ProviderConfig,ModeSnapshot,SourceRevision,WritingContext){
    let (mut config,mode,source)=fixture("预算3000元，暂时不要发送。");
    config.rewrite_provider="custom".into();config.rewrite_model="lab-model".into();
    config.rewrite_endpoint=Some(endpoint.into());
    let context=WritingContext{scene:AppWritingScene::General,language:DraftLanguage::AsSpoken,input:None,personalization:None};
    (config,mode,source,context)
}

#[tokio::test]
async fn ledger_start_failure_prevents_network_dispatch(){
    let server=Server::new(vec![Reply::draft("预算3000元，暂时不要发送。")]).await;
    let(config,mode,source,context)=lab_fixture(&server.endpoint);
    let out=experiment_personalization_observed(&config,"synthetic",&mode,&source,&DictionarySnapshot::default(),&context,
        Duration::from_secs(2),|_|false).await.unwrap();
    assert_eq!(out.result.unwrap_err().code,"lab_ledger_failed");assert_eq!(server.count(),0);
    assert_eq!(out.trace.requests.len(),1);assert_eq!(out.trace.requests[0].outcome,"cancelled");
}

#[tokio::test]
async fn ledger_finish_failure_stops_before_review(){
    let server=Server::new(vec![Reply::draft("预算3000元，暂时不要发送。"),Reply::new(ACCEPT)]).await;
    let(config,mode,source,context)=lab_fixture(&server.endpoint);
    let out=experiment_personalization_observed(&config,"synthetic",&mode,&source,&DictionarySnapshot::default(),&context,
        Duration::from_secs(2),|event|event.event=="started").await.unwrap();
    assert_eq!(out.result.unwrap_err().code,"lab_ledger_failed");assert_eq!(server.count(),1);
}

#[tokio::test]
async fn observed_production_path_has_balanced_content_free_events(){
    let server=Server::new(vec![Reply::draft("预算3000元，暂时不要发送。"),Reply::new(ACCEPT)]).await;
    let(config,mode,source,context)=lab_fixture(&server.endpoint);let before=source.clone();
    let events=Arc::new(Mutex::new(Vec::new()));let observed=Arc::clone(&events);
    let out=experiment_personalization_observed(&config,"synthetic",&mode,&source,&DictionarySnapshot::default(),&context,
        Duration::from_secs(2),move|event|{observed.lock().unwrap().push(event);true}).await.unwrap();
    assert!(out.result.is_ok());assert_eq!(server.count(),2);assert_eq!(out.trace.calls,2);
    assert_eq!(out.trace.requests.len(),2);assert_eq!(source,before);
    let events=events.lock().unwrap();assert_eq!(events.len(),4);
    assert_eq!(events.iter().map(|e|(e.stage,e.event)).collect::<Vec<_>>(),vec![("generation","started"),("generation","finished"),("review","started"),("review","finished")]);
    assert!(events.iter().all(|event|!serde_json::to_string(event).unwrap().contains("3000")));
}

#[tokio::test]
async fn fatal_provider_response_is_not_retried(){
    for status in [401,403,429]{
        let mut reply=Reply::new("synthetic denial");reply.status=status;
        let server=Server::new(vec![reply,Reply::new(ACCEPT)]).await;
        let(config,mode,source,context)=lab_fixture(&server.endpoint);
        let out=experiment_personalization_observed(&config,"synthetic",&mode,&source,&DictionarySnapshot::default(),&context,
            Duration::from_secs(2),|_|true).await.unwrap();
        assert_eq!(out.result.unwrap_err().code,if status==429{"rate_limited"}else{"authentication_failed"});
        assert_eq!(server.count(),1);assert_eq!(out.trace.requests.len(),1);
    }
}
