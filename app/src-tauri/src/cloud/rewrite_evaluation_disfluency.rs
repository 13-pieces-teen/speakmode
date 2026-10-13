//! Finite evaluation-property construction, not a general indefinite-article rule.
//! A receipt needs an audible pause, a genitive owner, a complete property head,
//! an observation context, and candidate evidence retaining that owner/property.
//! Neither source text nor candidate text is rewritten here.
use super::{asserted_count,facts,pause_only,preceding_context,Span};
use crate::core::{OutputBlock,SourceRevision};
use std::collections::HashSet;

const HEADS:[&str;4]=["效果","情况","状态","表现"];
const OBSERVE:[&str;9]=["测试","观察","对比","比较","看看","看一下","查看","了解","评估"];
const COUNTED_ACTIONS:[&str;16]=["创建","新增","添加","挑选","选择","删除","移除","切换","枚举","其中","任意","某个","某一个","任选","选取","从"];

fn normalized(text:&str)->String {
    text.chars().filter(|c|!c.is_whitespace()).flat_map(char::to_lowercase).collect()
}
fn pause_prefix(text:&str)->(&str,bool) {
    let mut rest=text.trim();let mut paused=false;
    loop {
        match ["嗯","呃","啊"].iter().find_map(|pause|rest.strip_prefix(pause)) {
            Some(tail)=>{rest=tail.trim_start();paused=true;},None=>return(rest,paused),
        }
    }
}
#[derive(Clone,Copy)]
struct Property {head:&'static str,actual:bool}
fn property(text:&str)->Option<Property> {
    let mut rest=text.trim();let mut actual=false;let mut modified=false;
    for modifier in ["实际的","实际","整体的","整体","最终的","最终"] {
        if let Some(tail)=rest.strip_prefix(modifier) {rest=tail;actual=modifier.starts_with("实际");modified=true;break;}
    }
    let rest=rest.strip_suffix('吧').or_else(||rest.strip_suffix('呢')).unwrap_or(rest).trim();
    HEADS.into_iter().find(|head|rest==*head&&(modified||*head=="效果")).map(|head|Property{head,actual})
}
fn observation_prefix(text:&str)->Option<(&'static str,&str)> {
    let text=strip_speech_prefix(text);
    OBSERVE.into_iter().filter_map(|action|text.strip_prefix(action).map(|rest|(action,rest)))
        .max_by_key(|(action,_)|action.len())
}
fn same_observation(actual:&str,expected:&str)->bool {
    actual==expected
        ||(["对比","比较"].contains(&actual)&&["对比","比较"].contains(&expected))
        ||(["看看","看一下","查看"].contains(&actual)&&["看看","看一下","查看"].contains(&expected))
}
fn source_action(prefix:&str,context:&str,owners:&[String])->Option<&'static str> {
    let prefix=normalized(prefix);
    if let Some((action,_))=observation_prefix(&prefix) {return Some(action);}
    // A fragmented genitive can inherit the nearest preceding action clause,
    // but an owner label such as 测试模块 is never reinterpreted as that action.
    context.split(['，',',','\n','：',':']).rev().find_map(|clause| {
        let clause=normalized(clause);
        if owners.contains(&clause) {return None;}
        observation_prefix(&clause).map(|(action,_)|action)
    })
}
fn strip_speech_prefix(mut text:&str)->&str {
    loop {
        let next=["现在的话","就是说","就是","然后","现在","同步的","同步","我们","这个","那个","嗯","呃","啊","再","去","我"]
            .iter().find_map(|prefix|text.strip_prefix(prefix));
        match next {Some(rest)=>text=rest,None=>return text}
    }
}
fn bare_owner(text:&str)->Option<String> {
    let normalized=normalized(text);let mut owner=strip_speech_prefix(&normalized);
    owner=owner.strip_suffix("对应").unwrap_or(owner);
    owner=owner.strip_suffix("这个").or_else(||owner.strip_suffix("那个")).unwrap_or(owner);
    if owner.is_empty()||owner.chars().count()>24||owner.contains(['的','和','与','及','或'])
        ||!owner.chars().all(|c|c.is_alphanumeric())||!facts::extract(owner).is_empty()
        ||COUNTED_ACTIONS.iter().any(|word|owner.contains(word)) {return None;}
    Some(owner.into())
}
fn owners(spans:&[Span<'_>],index:usize,prefix:&str)->Option<(Vec<String>,Option<usize>)> {
    let prefix=normalized(prefix);
    // Consume at most one leading action; words inside the remaining owner
    // (查看 + 测试模块) stay part of the owner and must survive in the candidate.
    let owner_text=observation_prefix(&prefix).map(|(_,rest)|rest).unwrap_or(&prefix);
    let owner=bare_owner(owner_text)?;let mut owners=vec![owner.clone()];let mut antecedent=None;
    if matches!(owner.as_str(),"它"|"其") {
        for previous in (0..index).rev().take(6) {
            let text=spans[previous].text.trim();
            if pause_only(text) {continue;}
            // Resolve only the immediately preceding standalone label. Do not
            // strip an observation/action clause into a guessed antecedent.
            if !OBSERVE.iter().any(|word|text.contains(word))&&!asserted_count(text) {
                if let Some(label)=bare_owner(text).filter(|label|!matches!(label.as_str(),"它"|"其"|"他"|"她")) {
                    owners.push(label);antecedent=Some(previous);
                }
            }
            break;
        }
    }
    Some((owners,antecedent))
}
fn other_counted_property(spans:&[Span<'_>],first:usize,last:usize,property:Property)->bool {
    spans.iter().enumerate().any(|(index,span)| {
        if (first..=last).contains(&index) {return false;}
        let related=span.text.contains(property.head)
            ||(property.head=="情况"&&span.text.contains("表现"))
            ||(property.head=="表现"&&span.text.contains("情况"));
        related&&(!facts::extract(&span.text).is_empty()
            ||["另一个","另一","第二","第三","其他","其中","任意","全部","所有","部分"].iter().any(|cue|span.text.contains(cue)))
    })
}
fn detached_limiter(spans:&[Span<'_>],first:usize)->bool {
    // A short standalone limiter followed only by pauses is not an independent
    // sentence, even if ASR inserted a full stop. This only restricts the new
    // evaluation rule; existing omission rules keep their original scope.
    for previous in (0..first).rev().take(6) {
        let text=spans[previous].text.trim();
        if pause_only(text) {continue;}
        return text.chars().count()<=4&&asserted_count(text);
    }
    false
}
fn mentioned_label(context:&str,label:&str)->bool {
    context.match_indices(label).any(|(offset,_)| {
        // Product labels are case/space normalized, not substring hints.
        let left=context[..offset].chars().next_back();
        let right=context[offset+label.len()..].chars().next();
        let ascii_left=label.chars().next().is_some_and(|c|c.is_ascii_alphanumeric());
        let ascii_right=label.chars().next_back().is_some_and(|c|c.is_ascii_alphanumeric());
        (!ascii_left||left.is_none_or(|c|!c.is_ascii_alphanumeric()))
            &&(!ascii_right||right.is_none_or(|c|!c.is_ascii_alphanumeric()))
    })
}
fn positive_arguments(arguments:&str,owner:&str,context:&str)->bool {
    if bare_owner(arguments).as_deref()==Some(owner) {return true;}
    let labels=arguments.split(['和','与','及']).collect::<Vec<_>>();
    (2..=4).contains(&labels.len())&&labels.last()==Some(&owner)&&labels.iter().all(|label| {
        !label.starts_with(['不','未','没','别','勿'])
            &&bare_owner(label).as_deref()==Some(*label)&&mentioned_label(context,label)
    })
}
fn candidate_supports(block:&OutputBlock,owners:&[String],property:Property,action:&str,context:&str)->bool {
    if block.text.contains(['“','”','‘','’','「','」','『','』','"','\'','`']) {return false;}
    for clause in block.text.split(['，','。','！','？','；',',','.','!','?',';','\n']) {
        let clause=normalized(clause);
        for head in HEADS {
            let same=head==property.head;
            // The only cross-head evidence is actual situation -> actual
            // performance in this observation frame, not generic synonymy.
            if !same&&!(property.head=="情况"&&property.actual&&head=="表现") {continue;}
            for (offset,_) in clause.match_indices(head) {
                let tail=&clause[offset+head.len()..];
                if !matches!(tail,""|"吧"|"呢") {continue;}
                let mut prefix=&clause[..offset];
                let candidate_actual=prefix.ends_with("实际");
                if !same&&!candidate_actual {continue;}
                for modifier in ["实际","整体","最终"] {
                    if let Some(rest)=prefix.strip_suffix(modifier) {prefix=rest;break;}
                }
                prefix=prefix.strip_suffix("对应的").or_else(||prefix.strip_suffix('的'))
                    .or_else(||prefix.strip_suffix("对应")).unwrap_or(prefix);
                let Some((candidate_action,arguments))=observation_prefix(prefix) else {continue;};
                if same_observation(candidate_action,action)
                    &&owners.iter().any(|owner|positive_arguments(arguments,owner,context)) {return true;}
            }
        }
    }
    false
}

pub(super) fn omittable_counts(source:&SourceRevision,spans:&[Span<'_>],blocks:&[OutputBlock])->HashSet<(usize,usize)> {
    let mut receipts=HashSet::new();
    for (first,span) in spans.iter().enumerate() {
        if detached_limiter(spans,first) {continue;}
        for fact in facts::extract(&span.text) {
            if fact.key!="count:1"||&span.text[fact.start..fact.end]!="一个" {continue;}
            let Some(prefix)=span.text[..fact.start].trim_end().strip_suffix('的') else {continue;};
            let preceding=preceding_context(source,span);
            let context=format!("{}{}",preceding,&span.text[..fact.start]);
            if asserted_count(&context)||COUNTED_ACTIONS.iter().any(|word|context.contains(word)) {continue;}
            let Some((owners,antecedent))=owners(spans,first,prefix) else {continue;};
            let Some(action)=source_action(prefix,&preceding,&owners) else {continue;};
            let normalized_context=normalized(&context);
            let (rest,mut paused)=pause_prefix(&span.text[fact.end..]);
            let mut last=first;let mut positions=vec![(first,fact.start)];
            let mut head=paused.then(||property(rest)).flatten();
            if rest.is_empty() {
                // A restart may cross pause-only clauses, never a different
                // subject, a noun change, or another substantive statement.
                for next in (first+1..spans.len()).take(6) {
                    let text=spans[next].text.trim();
                    if pause_only(text) {paused|=!text.is_empty();continue;}
                    let Some(after)=text.strip_prefix("一个") else {break;};
                    let (after,another_pause)=pause_prefix(after);paused|=another_pause;
                    head=paused.then(||property(after)).flatten();
                    if head.is_some() {last=next;positions.push((next,0));}
                    break;
                }
            }
            let Some(head)=head else {continue;};
            if other_counted_property(spans,first,last,head) {continue;}
            let supported=blocks.iter().any(|block|spans[first..=last].iter().all(|span|span.is_cited_by(block))
                &&antecedent.is_none_or(|index|spans[index].is_cited_by(block))&&candidate_supports(block,&owners,head,action,&normalized_context));
            if supported {receipts.extend(positions);}
        }
    }
    receipts
}
