//! Local quantity-role receipts over immutable source spans.
//!
//! This is not a noun parser. It recognizes an adjacent determiner restart and
//! an explicit copular classification, compares their complete dynamic tails,
//! and leaves every other quantity to the existing fact guard. No text is
//! rewritten. In particular, 个/款 are never globally equivalent units.
use super::{facts, scoped_candidates, CloudError, Span};
use crate::core::{OutputBlock, SourceRevision};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct Receipts {
    pub(super) source: HashSet<(usize, usize)>,
    pub(super) output: HashSet<(usize, usize, usize)>,
}

const RESTART: &str = "一个这样的一个";
fn compact(text: &str) -> String { text.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect() }
fn quoted(text: &str) -> bool { text.chars().any(|c| matches!(c,'“'|'”'|'‘'|'’'|'「'|'」'|'『'|'』'|'"'|'\''|'`')) }

// Only leading discourse markers, never arbitrary substring replacement. Keep
// quantities, conditions, negation and actual owners in the rest of the frame.
fn lead(mut text: &str) -> &str {
    loop {
        text = text.trim();
        let next = ["首先", "其次", "然后", "就是说", "就是在", "就是", "现在", "嗯", "呃", "啊"]
            .iter().find_map(|prefix| text.strip_prefix(prefix));
        match next { Some(rest) => text = rest, None => return text }
    }
}
fn owner(text: &str) -> String {
    let text = lead(text);
    let text = text.strip_prefix("我们这个").map(|tail| format!("这个{tail}")).unwrap_or_else(||text.to_owned());
    compact(&text)
}
fn plain_head(text: &str) -> Option<String> {
    let text = text.trim().strip_prefix("这样的").unwrap_or(text.trim());
    if text.is_empty() || text.chars().count() > 64 || quoted(text)
        || !text.chars().all(|c| c.is_alphanumeric() || c.is_whitespace() || c=='-')
        || ["以及","还有","另一个","另外","然后","但是","并且","也就是","是一个","是一款"]
            .iter().any(|word|text.contains(word))
        || !facts::extract(text).is_empty() {return None;}
    Some(compact(text))
}

// A title can precede an unchanged dynamic object head only when an immediately
// following source naming clause supplies that exact title. Book-title marks
// are presentation here; arbitrary quoted instructions remain ineligible.
fn named_head(text: &str) -> Option<(String, String)> {
    let tail = text.trim().strip_prefix("名为《")?;
    let (title, head) = tail.split_once("》的")?;
    if title.is_empty() || quoted(title) { return None; }
    Some((plain_head(head)?, compact(title)))
}
fn naming(text: &str) -> Option<String> {
    let text = lead(text);
    let rest = ["名字是叫这个", "名字叫这个", "名字是叫", "名字叫", "名叫"]
        .iter().find_map(|prefix|text.strip_prefix(prefix))?;
    let rest = rest.trim().trim_matches(['《','》']);
    (!rest.is_empty() && !quoted(rest)).then(||compact(rest))
}
fn discourse(text: &str) -> bool {
    let text = text.strip_prefix("那首先").map(lead).unwrap_or_else(||lead(text)).trim().trim_start_matches(['•','-','*']);
    text.is_empty() || text=="我觉得" || text.bytes().all(|c|c.is_ascii_digit())
        || ["第一点","第二点","第三点","第一","第二","第三"]
            .iter().any(|marker|text==*marker || text==format!("{marker}的话"))
}

#[derive(Debug)]
struct Restart {
    prefix: String,
    head: String,
    first: usize,
    alias: usize,
}
fn restart(text: &str) -> Option<Restart> {
    let start = text.find(RESTART)?;
    if text[start+RESTART.len()..].contains(RESTART) || quoted(text) {return None;}
    let numeric = facts::extract(text);
    let alias = start + "一个这样的".len();
    // No second object or unrelated measurement can be absorbed into a receipt.
    if numeric.len()!=2 || numeric[0].start!=start || numeric[1].start!=alias
        || numeric.iter().any(|fact|fact.key!="count:1") {return None;}
    Some(Restart {prefix:compact(lead(&text[..start])),head:plain_head(&text[start+RESTART.len()..])?,first:start,alias})
}

#[derive(Debug)]
struct Classification {
    owner: String,
    head: String,
    start: usize,
    unit: char,
}
fn classification(text: &str) -> Option<Classification> {
    if quoted(text) {return None;}
    let (copula, marker, unit) = [("是一个",'个'),("是一款",'款')].iter()
        .filter_map(|(marker,unit)|text.find(marker).map(|index|(index,*marker,*unit))).min_by_key(|(index,_,_)|*index)?;
    let subject = owner(&text[..copula]);
    if subject.is_empty() {return None;}
    // Complete tail matching is intentionally strict: no noun vocabulary and
    // no prefix-only type match (索引工具 and 索引工具包 are different types).
    Some(Classification {owner:subject,head:plain_head(&text[copula+marker.len()..])?,start:copula+'是'.len_utf8(),unit})
}

struct Introduction { prefix: String, head: String, title: Option<String> }
fn introduction(text: &str) -> Option<Introduction> {
    let normalized = text.replace(RESTART,"一个");
    let text = normalized.as_str();
    if quoted(text) {return None;}
    let numeric = facts::extract(text);
    if numeric.len()!=1 || numeric[0].key!="count:1" || &text[numeric[0].start..numeric[0].end]!="一个" {return None;}
    let prefix=compact(lead(&text[..numeric[0].start]));
    if !["做","开发","设计","制作","建立","创建"].iter().any(|verb|prefix.ends_with(verb)) {return None;}
    let tail=&text[numeric[0].end..];
    let (head,title)=if let Some((head,title))=named_head(tail) {(head,Some(title))} else {(plain_head(tail)?,None)};
    // Conjoined objects are not a unique antecedent, even when only the first
    // object carries a classifier.
    if ['和','与','及'].iter().any(|join|head.contains(*join)) {return None;}
    Some(Introduction{prefix,head,title})
}

fn local_source_intro(spans: &[Span<'_>], index:usize, block:&OutputBlock)->Option<(Introduction,Option<String>)> {
    let mut title=None;
    let mut cursor=index;
    while cursor>0 && index-cursor<8 {
        cursor-=1;
        let span=&spans[cursor];
        if !span.is_cited_by(block) {return None;}
        if discourse(&span.text) {continue;}
        if let Some(name)=naming(&span.text) {if title.is_some(){return None;} title=Some(name);continue;}
        if cursor>0 && naming_label(&spans[cursor-1].text) && spans[cursor-1].is_cited_by(block)
            && plain_head(&span.text).is_some() {
            if title.is_some(){return None;}
            title=Some(compact(&span.text));cursor-=1;continue;
        }
        return introduction(&span.text).map(|intro|(intro,title));
    }
    None
}
fn naming_label(text:&str)->bool {
    ["名字是叫这个","名字叫这个","名字是叫","名字叫","名叫"].contains(&lead(text))
}
fn title_after(spans:&[Span<'_>],index:usize,block:&OutputBlock)->Option<String> {
    for (position,span) in spans.iter().enumerate().skip(index+1).take(4) {
        if !span.is_cited_by(block) {return None;}
        if discourse(&span.text) {continue;}
        if let Some(name)=naming(&span.text) {return Some(name);}
        if naming_label(&span.text) {
            let next=spans.get(position+1)?;
            if next.is_cited_by(block)&&!quoted(&next.text)&&plain_head(&next.text).is_some() {return Some(compact(&next.text));}
        }
        return None;
    }
    None
}
fn candidate_head_matches(text:&str,head:&str,title:Option<&str>)->bool {
    plain_head(text).is_some_and(|candidate|candidate==head)
        || named_head(text).is_some_and(|(candidate,name)|candidate==head && title==Some(name.as_str()))
}
fn same_class_owner(expected:&Classification,candidate:&Classification,spans:&[Span<'_>],source_index:usize,
    clauses:&[String],clause_index:usize,block:&OutputBlock)->bool {
    if expected.owner==candidate.owner {return true;}
    if candidate.owner!="这" {return false;}
    let Some(head)=expected.owner.strip_prefix("这个") else{return false;};
    let Some((source_intro,source_title))=local_source_intro(spans,source_index,block) else{return false;};
    if source_intro.head!=head {return false;}
    let Some(previous)=clauses[..clause_index].iter().rev().find(|text|!discourse(text)) else{return false;};
    let Some(output_intro)=introduction(previous) else{return false;};
    source_intro.head==output_intro.head && source_intro.prefix==output_intro.prefix
        && output_intro.title.as_ref().is_none_or(|title|source_intro.title.as_ref().or(source_title.as_ref())==Some(title))
}

pub(super) fn validate(source:&SourceRevision,spans:&[Span<'_>],blocks:&[OutputBlock],omittable:&HashSet<(usize,usize)>)->Result<Receipts,CloudError> {
    let mut receipts=Receipts::default();
    // Source quotation may straddle ASR items. Conservatively decline all new
    // permissions for a quoted/code-bearing source, preserving old guard checks.
    if source.items.iter().any(|item|quoted(&item.text)) {return Ok(receipts);}
    let candidates=blocks.iter().map(|block|scoped_candidates(&block.text)).collect::<Vec<_>>();
    let mut grants=blocks.iter().map(|_|Receipts::default()).collect::<Vec<_>>();
    let mut aliases=blocks.iter().map(|_|HashSet::new()).collect::<Vec<_>>();
    let mut classified_outputs=blocks.iter().map(|_|HashSet::new()).collect::<Vec<_>>();
    for (span_index,span) in spans.iter().enumerate() {
        if let Some(restart)=restart(&span.text) {
            for (block_index,block) in blocks.iter().enumerate().filter(|(_,block)|span.is_cited_by(block)) {
                let title=title_after(spans,span_index,block);
                for clause in &candidates[block_index] {
                    if quoted(clause) {continue;}
                    let numbers=facts::extract(clause);
                    // The remaining asserted count must be present, same value,
                    // full object head, owner and action. Never waive it too.
                    if numbers.len()!=1 || numbers[0].key!="count:1" {continue;}
                    let number=&numbers[0];
                    if compact(lead(&clause[..number.start]))==restart.prefix
                        && candidate_head_matches(&clause[number.end..],&restart.head,title.as_deref()) {
                        grants[block_index].source.insert((span_index,restart.alias));
                        aliases[block_index].insert((span_index,restart.alias));
                        debug_assert!(restart.first<restart.alias);
                    }
                }
            }
        }
        if let Some(expected)=classification(&span.text) {
            let mut matched=false;
            for (block_index,block) in blocks.iter().enumerate().filter(|(_,block)|span.is_cited_by(block)) {
                let clauses=&candidates[block_index];
                for (clause_index,clause) in clauses.iter().enumerate() {
                    let Some(candidate)=classification(clause) else{continue;};
                    if expected.head==candidate.head && same_class_owner(&expected,&candidate,spans,span_index,clauses,clause_index,block) {
                        matched=true;
                        classified_outputs[block_index].insert(clause_index);
                        // The exact source/count pair is discharged by this
                        // relation, not by an unrelated one elsewhere in text.
                        if expected.unit=='个' {grants[block_index].source.insert((span_index,expected.start));}
                        if candidate.unit=='个' {grants[block_index].output.insert((block_index,clause_index,candidate.start));}
                    }
                }
            }
            // 款 is intentionally absent from the old generic unit vocabulary.
            // Once this explicit classification is recognized, its count still
            // cannot disappear/change just because no generic fact was emitted.
            if !matched && expected.unit=='款' {return Err(CloudError::new("rewrite_meaning_risk","rewrite","整理结果可能改变分类的数量或对象，请核对识别稿或重新整理"));}
        }
    }
    for (block_index,grant) in grants.into_iter().enumerate() {
        // The old scanner does not emit 款 facts. An ungrounded additional
        // classification cannot borrow this new permission or inflate the
        // available-count tally below.
        if candidates[block_index].iter().enumerate().any(|(index,clause)|
            classification(clause).is_some_and(|frame|frame.unit=='款') && !classified_outputs[block_index].contains(&index)) {continue;}
        // A new restart/classification permission must not enable the old
        // prefix-only matcher to reuse one candidate count for two objects.
        // Require at least one occurrence per remaining source count/value in
        // this cited block. This is only a lower bound, not a semantic proof.
        let mut required=HashMap::<String,usize>::new();
        for (span_index,span) in spans.iter().enumerate().filter(|(_,span)|span.is_cited_by(&blocks[block_index])) {
            for fact in facts::extract(&span.text).into_iter().filter(|fact|fact.key.starts_with("count:")) {
                if !omittable.contains(&(span_index,fact.start)) && !aliases[block_index].contains(&(span_index,fact.start)) {
                    *required.entry(fact.key).or_default()+=1;
                }
            }
            if classification(&span.text).is_some_and(|frame|frame.unit=='款') {*required.entry("count:1".into()).or_default()+=1;}
        }
        let mut available=HashMap::<String,usize>::new();
        for clause in &candidates[block_index] {
            for fact in facts::extract(clause).into_iter().filter(|fact|fact.key.starts_with("count:")) {*available.entry(fact.key).or_default()+=1;}
            if classification(clause).is_some_and(|frame|frame.unit=='款') {*available.entry("count:1".into()).or_default()+=1;}
        }
        if required.iter().all(|(key,count)|available.get(key).copied().unwrap_or(0)>=*count) {
            receipts.source.extend(grant.source);receipts.output.extend(grant.output);
        }
    }
    Ok(receipts)
}
