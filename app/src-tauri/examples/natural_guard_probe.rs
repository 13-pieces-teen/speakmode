//! Offline fixed-text acceptance probes for natural cleanup. No provider,
//! credentials, microphone, app profile or history access. These deliberately
//! small cases diagnose a guard boundary, not ASR or overall rewrite quality.
use serde_json::json;
use speakmode_lib::{cloud::rewrite::{parse_model_output,parse_natural_output},
    core::{BaseMode,ModeSnapshot,SourceItem,SourceRevision}};

struct Case { id:&'static str, source:&'static [&'static str], output:&'static str, accepted:bool }

fn main() {
    let cases = [
        Case{id:"abandoned-resume-prefix",source:&["让这个简历的一个。","呃，简历的这样的一个呃。","就是简历的一个整个行距调大一点。"],output:"让简历的整个行距调大一点。",accepted:true},
        Case{id:"possessive-filler-whole",source:&["把简历的一个整个行距调大一点。"],output:"把简历的整个行距调大一点。",accepted:true},
        Case{id:"possessive-filler-layout",source:&["优化文档的一个整体排版。"],output:"优化文档的整体排版。",accepted:true},
        Case{id:"abandoned-prefix-before-new-verb",source:&["文档的一个。","呃，文档的这样的一个。","怎么说呢，就是文档的行距调大一点。"],output:"文档的行距调大一点。",accepted:true},
        Case{id:"real-one-project-cannot-disappear",source:&["把简历的一个项目删掉。"],output:"把简历的项目删掉。",accepted:false},
        Case{id:"real-two-projects-cannot-change",source:&["把简历的两个项目删掉。"],output:"把简历的一个项目删掉。",accepted:false},
        Case{id:"fragment-completes-counted-noun",source:&["把简历的一个。","项目删掉。"],output:"把简历的项目删掉。",accepted:false},
        Case{id:"abandoned-prefix-with-real-count",source:&["把简历的一个。","就是把简历的一个项目删掉。"],output:"把简历的项目删掉。",accepted:false},
        Case{id:"quoted-fragment-count-preserved",source:&["请保留原文“简历的一个”。"],output:"请保留原文“简历”。",accepted:false},
        Case{id:"quoted-whole-filler-preserved",source:&["请保留原文“文档的一个整体排版”。"],output:"请保留原文“文档的整体排版”。",accepted:false},
        Case{id:"point-synonym",source:&["可以减少一个点。"],output:"可以删减一个点。",accepted:true},
        Case{id:"point-source-words",source:&["可以减少一个点。"],output:"可以减少一个点。",accepted:true},
        Case{id:"point-count-change",source:&["可以减少一个点。"],output:"可以删减两个点。",accepted:false},
        Case{id:"point-count-drop",source:&["可以减少一个点。"],output:"可以删减内容。",accepted:false},
        Case{id:"question-is-can-to-whether-can",source:&["我在思考是不是能够减少一个点？"],output:"我在思考是否能够减少一个点？",accepted:true},
        Case{id:"question-is-can-to-can",source:&["是不是能打开文件？"],output:"能否打开文件？",accepted:true},
        Case{id:"question-stays-question",source:&["是不是能打开文件？"],output:"是不是能打开文件？",accepted:true},
        Case{id:"question-cannot-be-assertion",source:&["是不是能打开文件？"],output:"能打开文件。",accepted:false},
        Case{id:"question-cannot-change-action",source:&["是不是能打开文件？"],output:"能否删除文件？",accepted:false},
        Case{id:"negative-cannot-be-question",source:&["不能打开文件。"],output:"能否打开文件？",accepted:false},
        Case{id:"real-negative-fact-kept",source:&["不是要删除文件。"],output:"要删除文件。",accepted:false},
        Case{id:"prohibition-stays",source:&["别删除这个文件。"],output:"删除这个文件。",accepted:false},
        Case{id:"uncertainty-stays",source:&["之前可能调得有点宽了。"],output:"之前调得有点宽了。",accepted:false},
        Case{id:"numeric-format-allowed",source:&["会议安排在二零二七年十月。"],output:"会议安排在2027年10月。",accepted:true},
        Case{id:"numeric-value-protected",source:&["会议安排在二零二七年十月。"],output:"会议安排在2028年10月。",accepted:false},
        Case{id:"count-owner-protected",source:&["甲减少一个点，乙减少两个点。"],output:"甲减少两个点，乙减少一个点。",accepted:false},
    ];
    let mode=ModeSnapshot{mode_id:"auto".into(),name:"自动".into(),base_mode:BaseMode::Auto,rules:String::new(),revision:1};
    let mut rows=Vec::new();
    for case in &cases {
        let source=SourceRevision{id:format!("source-{}",case.id),session_id:format!("session-{}",case.id),asr_attempt_id:format!("attempt-{}",case.id),
            generation:1,provider:"offline".into(),model:"fixed-text".into(),adapter_version:"natural-guard-probe-v1".into(),complete:true,
            created_at:"2026-10-11T00:00:00Z".into(),items:case.source.iter().enumerate().map(|(index,text)|SourceItem{
                id:format!("item-{index}"),provider_item_id:index.to_string(),capture_order:index as u64,text:(*text).into(),is_final:true,start_ms:None,end_ms:None}).collect()};
        let natural=json!({"resolvedMode":"light","text":case.output});
        let baseline=json!({"resolvedMode":"light","blocks":[{"text":case.output,"sourceRange":[0,source.items.len()-1]}]});
        for (parser,result) in [("natural",parse_natural_output(&natural.to_string(),&mode,&source)),("baseline",parse_model_output(&baseline.to_string(),&mode,&source))] {
            rows.push(json!({"id":case.id,"parser":parser,"expectedAccepted":case.accepted,"actualAccepted":result.is_ok(),"expectationMet":result.is_ok()==case.accepted,
                "errorCode":result.as_ref().err().map(|error|error.code.as_str()),"errorMessage":result.as_ref().err().map(|error|error.message.as_str())}));
        }
    }
    let matched=rows.iter().filter(|row|row["expectationMet"]==true).count();
    println!("{}",serde_json::to_string_pretty(&json!({"scope":"offline fixed synthetic text only; no cloud or user profile", "cases":cases.len(),"rows":rows,
        "total":cases.len()*2,"expectationsMet":matched,"mismatches":cases.len()*2-matched})).unwrap());
}
