use super::*;
fn request(format:MinimumReviewFormat)->(ModeSnapshot,SourceRevision,RewriteResult,MinimumReviewRequest){
 let (mode,source,draft)=super::tests::fixture();
 let r=prepare_minimum_with_format(&ProviderConfig::default(),&mode,&source,&draft,format).unwrap();(mode,source,draft,r)
}
fn verdict()->Value{json!({"decision":"accept","checks":["pass","pass","pass","pass","pass"],"issues":[]})}
#[test]
fn ordered_review_keeps_five_independent_checks_and_original_bindings(){
 let (m,s,d,r)=request(MinimumReviewFormat::Ordered);
 let original=s.clone();let review=parse_minimum(&verdict().to_string(),&r,&m,&s,&d).unwrap();
 assert!(review.accepted());assert_eq!(review.protocol_version,ORDERED_REVIEW_VERSION);assert_eq!(s,original);
 for i in 0..5 {for status in ["fail","uncertain"] {
  let mut v=verdict();v["decision"]=json!("reject");v["checks"][i]=json!(status);
  let review=parse_minimum(&v.to_string(),&r,&m,&s,&d).unwrap();assert!(!review.accepted());
  let checks=[review.checks.coverage,review.checks.facts,review.checks.scope,review.checks.intent,review.checks.cleanup];
  assert_eq!(checks[i],if status=="fail"{Status::Fail}else{Status::Uncertain});
  v["decision"]=json!("accept");assert!(parse_minimum(&v.to_string(),&r,&m,&s,&d).is_err());
 }}
 let mut changed=s.clone();changed.generation+=1;assert!(parse_minimum(&verdict().to_string(),&r,&m,&changed,&d).is_err());
 let mut changed=d.clone();changed.blocks[0].text.push('。');assert!(parse_minimum(&verdict().to_string(),&r,&m,&s,&changed).is_err());
 let mut changed=m.clone();changed.rules.push_str("新规则");assert!(parse_minimum(&verdict().to_string(),&r,&changed,&s,&d).is_err());
}
#[test]
fn ordered_review_rejects_missing_extra_invalid_duplicate_or_unrequested_fields(){
 let (m,s,d,r)=request(MinimumReviewFormat::Ordered);
 for checks in [json!([]),json!(vec!["pass";4]),json!(vec!["pass";6]),json!(["pass","pass",true,"pass","pass"]),json!(["pass","pass","NA","pass","pass"])]{
  let mut v=verdict();v["checks"]=checks;assert!(parse_minimum(&v.to_string(),&r,&m,&s,&d).is_err());
 }
 let mut v=verdict();v["text"]=json!("替换正文");assert!(parse_minimum(&v.to_string(),&r,&m,&s,&d).is_err());
 let mut v=verdict();v.as_object_mut().unwrap().remove("issues");assert!(parse_minimum(&v.to_string(),&r,&m,&s,&d).is_err());
 let raw=r#"{"decision":"accept","checks":["pass","pass","pass","pass","pass"],"checks":["pass","pass","pass","fail","pass"],"issues":[]}"#;
 assert!(parse_minimum(raw,&r,&m,&s,&d).is_err());
 let (_,_,_,named)=request(MinimumReviewFormat::Named);assert!(parse_minimum(&verdict().to_string(),&named,&m,&s,&d).is_err());
}
#[test]
fn ordered_review_preserves_fidelity_instructions_and_grounded_issues(){
 let (m,s,d,r)=request(MinimumReviewFormat::Ordered);let (_,_,_,named)=request(MinimumReviewFormat::Named);
 let content=|r:&MinimumReviewRequest|r.body["messages"][0]["content"].as_str().unwrap().to_owned();
 let a=content(&r);let b=content(&named);
 let rules=|s:String|s.split_once("你是听写成稿复核员").unwrap().1.split_once("只返回严格 JSON").unwrap().0.to_owned();
 assert_eq!(rules(a),rules(b));assert_eq!(r.body["messages"][1],named.body["messages"][1]);
 for key in ["enable_thinking","temperature","max_tokens","stream"]{assert_eq!(r.body[key],named.body[key]);}
 let mut v=verdict();v["decision"]=json!("reject");v["checks"][4]=json!("fail");
 v["issues"]=json!([{"dimension":"cleanup","sourceQuote":s.items[0].text,"draftQuote":d.blocks[0].text,"reason":"仍有卡顿"}]);
 assert!(!parse_minimum(&v.to_string(),&r,&m,&s,&d).unwrap().accepted());
 v["issues"][0]["sourceQuote"]=json!("伪造的来源");assert!(parse_minimum(&v.to_string(),&r,&m,&s,&d).is_err());
}
#[test]
fn explicit_english_context_retains_its_existing_review_schema(){
 let (mut m,s,d,_)=request(MinimumReviewFormat::Ordered);m.mode_id="chat".into();let mut context=super::super::writing_context::for_mode(&m).unwrap();
 context.language=crate::core::DraftLanguage::English;
 let r=prepare_minimum_with_writing_context_and_format(&ProviderConfig::default(),&m,&s,&d,Some(&context),MinimumReviewFormat::Ordered).unwrap();
 assert!(!r.ordered);assert!(parse_minimum(&verdict().to_string(),&r,&m,&s,&d).is_err());
}
#[test]
fn ordered_default_is_limited_to_the_evaluated_model_region_and_feature(){
 let (_,s,_,_)=request(MinimumReviewFormat::Named);let mut c=ProviderConfig::default();c.region=Some("beijing".into());
 assert!(matches!(minimum_format(&c,&s),MinimumReviewFormat::Ordered)==!cfg!(feature="experimental-english-dictation"));
 c.region=Some("singapore".into());assert!(matches!(minimum_format(&c,&s),MinimumReviewFormat::Named));
 c.region=Some("beijing".into());c.rewrite_model="qwen-plus".into();assert!(matches!(minimum_format(&c,&s),MinimumReviewFormat::Named));
}
