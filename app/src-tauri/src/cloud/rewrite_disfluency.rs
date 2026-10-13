//! Narrow permission to omit an unasserted Chinese 一个 in spoken constructions.
//! This only supplies receipts for the forward count check. It never changes
//! ASR source, output text, source references, or the check for added quantities.
use super::{facts,Span};
use crate::core::SourceRevision;
use std::collections::HashSet;

#[path="rewrite_evaluation_disfluency.rs"]
mod evaluation;

fn trim_restart(mut text:&str)->&str {
    loop {
        text=text.trim();
        let next=["怎么说呢","就是说","就是","然后","这个","那个","嗯","呃","啊","让","把"]
            .iter().find_map(|prefix|text.strip_prefix(prefix));
        match next {Some(rest)=>text=rest,None=>return text}
    }
}
fn pause_only(text:&str)->bool {
    let mut text=text.trim();
    loop {
        match ["嗯","呃","啊"].iter().find_map(|pause|text.strip_prefix(pause)) {
            Some(rest)=>text=rest.trim(),None=>return text.is_empty(),
        }
    }
}
fn layout_property(text:&str)->bool {
    let text=text.strip_prefix("一个").unwrap_or(text);
    let text=text.strip_prefix("整个").or_else(||text.strip_prefix("整体")).unwrap_or(text);
    ["行距","页边距","排版"].iter().any(|property|text.strip_prefix(property).is_some_and(|tail|
        tail.is_empty()||["调","改","恢复","增","减","放","缩","设","优化","更"].iter().any(|verb|tail.starts_with(verb))))
}
fn asserted_count(before:&str)->bool {
    ["仅","只","每","至少","至多","最多","最少","不超过","不多于","不少于","超过","少于","多于",
     "总共","一共","共有","总数","数量","数目","合计","实际","恰好","正好","刚好","唯一"]
        .iter().any(|word|before.contains(word))
}
fn preceding_context(source:&SourceRevision,span:&Span<'_>)->String {
    let Some(origin)=span.origins.first() else{return String::new();};
    let Some(index)=source.items.iter().position(|item|item.id==origin.source_id) else{return String::new();};
    let mut reversed=Vec::new();
    // A provider item boundary is not a sentence boundary. Read a bounded
    // original-text prefix across items; a full sentence boundary ends it.
    // Commas and ASR fragment breaks cannot detach 只/每次 from the quantity.
    'prefix: for text in std::iter::once(&source.items[index].text[..origin.start])
        .chain(source.items[..index].iter().rev().map(|item|item.text.as_str())) {
        for c in text.chars().rev() {
            if reversed.len()==128||matches!(c,'。'|'！'|'？'|'；'|'.'|'!'|'?'|';') {break 'prefix;}
            reversed.push(c);
        }
    }
    reversed.into_iter().rev().collect()
}
fn introductory_continuation(spans:&[Span<'_>],index:usize)->bool {
    for span in spans.iter().skip(index+1).take(8) {
        let mut text=span.text.trim();
        loop {
            let next=["怎么说呢","就是说","就是","然后","嗯","呃","啊"]
                .iter().find_map(|prefix|text.strip_prefix(prefix));
            match next {Some(rest)=>text=rest.trim(),None=>break}
        }
        if text.is_empty() {continue;}
        // Only the first substantive continuation can support an introduction.
        // Counted comparisons (including another speaker's quantity) are not
        // explanations authorizing deletion of the introduction's own count.
        if asserted_count(text)||facts::extract(text).iter().any(|fact|fact.key.starts_with("count:")||fact.key.starts_with("item:"))
            ||["另一个","另一","第二","其他"].iter().any(|word|text.contains(word)) {return false;}
        return ["我","这","希望","想","让","把","可以","比如","例如"].iter().any(|prefix|text.starts_with(prefix));
    }
    false
}
fn repeated_subject<'a>(text:&'a str,fact:&facts::Fact)->Option<&'a str> {
    if fact.key!="count:1" || &text[fact.start..fact.end]!="一个" {return None;}
    let before=&text[..fact.start];
    if asserted_count(before) {return None;}
    let before=before.strip_suffix("的这样的").or_else(||before.strip_suffix("这样的"))
        .or_else(||before.strip_suffix('的'))?;
    let subject=trim_restart(before);
    (!subject.is_empty()&&subject.chars().count()<=16).then_some(subject)
}

fn discourse_indefinite(spans:&[Span<'_>],index:usize,fact:&facts::Fact)->bool {
    let text=&spans[index].text;
    if fact.key!="count:1"||&text[fact.start..fact.end]!="一个" {return false;}
    let before=&text[..fact.start];let after=text[fact.end..].trim();
    if asserted_count(before) {return false;}
    // A speaker introducing the explanation that follows: 我现在有一个想法，…
    // This is not permission to drop counted objects, somebody else's quantity,
    // or a standalone statement of how many ideas the speaker has.
    let mut opening=before.to_owned();
    for filler in ["就是说","就是","然后","现在","的话","我们","我","嗯","呃","啊"] {opening=opening.replace(filler,"");}
    let noun=after.strip_prefix("这样的").unwrap_or(after);
    if matches!(opening.trim(),"有"|"还有")&&matches!(noun,"想法"|"体验"|"感受")
        &&introductory_continuation(spans,index) {return true;}
    // An open request for improvement points, not an instruction to make one
    // change. An explicit contrast/count after 点 is deliberately excluded.
    if before.contains("有什么")&&["优化的","改进的","调整的"].iter().any(|suffix|before.ends_with(suffix))&&after=="点" {return true;}
    // Predicate adjective + size is a target attribute, not one of two sizes.
    // Requiring the complete noun phrase excludes size是小号，另一个是大号.
    ["正常的","合适的"].iter().any(|suffix|before.ends_with(suffix))&&matches!(after,"size"|"尺寸"|"大小")
}

pub(super) fn omittable_counts(source:&SourceRevision,spans:&[Span<'_>],blocks:&[crate::core::OutputBlock])->HashSet<(usize,usize)> {
    let mut result=HashSet::new();
    // Conservative whole-document exclusion handles quotes spanning ASR items,
    // punctuation and fragments without guessing whether a quote has closed.
    if source.items.iter().any(|item|item.text.contains(['“','”','‘','’','「','」','『','』','"','\'','`'])) {return result;}
    for (span_index,span) in spans.iter().enumerate() {
        if asserted_count(&preceding_context(source,span)) {continue;}
        for fact in facts::extract(&span.text) {
            if discourse_indefinite(spans,span_index,&fact) {result.insert((span_index,fact.start));continue;}
            let Some(subject)=repeated_subject(&span.text,&fact) else {continue;};
            let after=span.text[fact.end..].trim_start();
            // A count before a counted noun (one project, one paragraph, ...)
            // is never exempt. These are only abstract layout properties.
            if layout_property(after) {
                result.insert((span_index,fact.start));continue;
            }
            if !pause_only(after) {continue;}
            // A dangling 一个 is only abandoned when an immediately following
            // bounded chain clearly restarts the same subject and a layout
            // property. A noun continuation such as 项目 ends this search.
            for next in spans.iter().skip(span_index+1).take(8) {
                let text=trim_restart(&next.text);
                if text.is_empty() {continue;}
                let Some(rest)=text.strip_prefix(subject).and_then(|text|text.strip_prefix("这样的").or_else(||text.strip_prefix('的'))) else {break;};
                if layout_property(rest) {
                    result.insert((span_index,fact.start));break;
                }
                let pending=rest.strip_prefix("这样的").unwrap_or(rest);
                if pending.strip_prefix("一个").is_some_and(pause_only) {continue;}
                break;
            }
        }
    }
    result.extend(evaluation::omittable_counts(source,spans,blocks));
    result
}

#[cfg(test)]
#[path="rewrite_disfluency_tests.rs"]
mod tests;
