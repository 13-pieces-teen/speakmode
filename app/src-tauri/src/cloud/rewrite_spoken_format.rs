//! Presentation of spoken notation, never a correction of the ASR record.
//! A finite acronym vocabulary supplies spelling, not entity facts or expansions.
use super::CloudError;
use crate::core::{SourceRevision,OutputBlock};
use serde_json::{Value,json};
use std::collections::{HashMap,HashSet};

pub(super) const VERSION:&str="spoken-format-v1";
const ACRONYMS:&[&str]=&["ADHD","LLM","RAG","API","GPU","CPU","NLP","ASR","SDK","SQL","HTTP","HTTPS","URL","JSON","HTML","CSS","CIKM","ADMA","DCG","AI","UI","UX"];
const POLICY:&str=r#"Spoken notation: use Arabic digits for unambiguous calendar years and dates. Chinese prose uses 2026年10月23日; English prose uses 2026-10-23 for a complete explicit date. Missing year/month/day stays missing. Never resolve tomorrow/weekdays from the current date, choose an interpretation of 3/4, fix an invalid recorded date, or borrow a year from another event. Literal quotes, code, addresses, identifiers and explicit verbatim wording retain their original spelling.
Spelled Latin letters can be an acronym or individual units. Join only the same letters in the same order when the local prose clearly refers to one known term, e.g. A D H D -> ADHD, L L M -> LLM, R A G -> RAG. Do not expand an acronym, infer a condition/diagnosis, guess an unfamiliar name, remove repeated letters or join across unrelated words. Explicit separate letters, spelling lessons, choices, variables and uncertain letter groups stay separate. An article a and pronoun I are not automatically AI. Chinese/English hesitation may separate an otherwise clear acronym; meaningful Chinese is never a separator to delete. Apply the same policy during generation, review and repair. Equivalent notation is not changed meaning; a changed value or merged letter list is changed meaning."#;

#[derive(Clone)]
struct Token {start:usize,end:usize}
#[derive(Clone)]
struct Run {start:usize,end:usize,letters:String,allowed:bool}
fn tokens(text:&str)->Vec<Token> {
    let mut literals=super::literals::spans(text).into_iter().peekable();let mut result=Vec::new();let mut start=None;
    for (at,c) in text.char_indices().chain(std::iter::once((text.len(),' '))) {
        if c.is_ascii_alphabetic() {start.get_or_insert(at);} else if let Some(begin)=start.take() {
            let identifier=|c:char|c.is_ascii_digit()||"_/@\\:-".contains(c);
            let dotted_left=text[..begin].chars().rev().take(2).collect::<Vec<_>>();
            let dotted_right=text[at..].chars().take(2).collect::<Vec<_>>();
            let dotted=(dotted_left.first()==Some(&'.')&&dotted_left.get(1).is_some_and(|c|c.is_ascii_alphanumeric()))
                ||(dotted_right.first()==Some(&'.')&&dotted_right.get(1).is_some_and(|c|c.is_ascii_alphanumeric()));
            while literals.peek().is_some_and(|s|s.end<=begin){literals.next();}
            if !dotted && !literals.peek().is_some_and(|s|begin<s.end&&at>s.start)
                && !text[..begin].chars().next_back().is_some_and(identifier)
                && !text[at..].chars().next().is_some_and(identifier) {
                result.push(Token{start:begin,end:at});
            }
        }
    }result
}
fn gap(text:&str)->bool {
    if text.is_empty()||text.len()>48{return false;}
    let stripped=text.replace("呃","").replace("嗯","").replace("额","");
    let stripped=stripped.split_whitespace().filter(|s|!matches!(s.trim_matches([',','，','.']),"uh"|"um"|"Uh"|"Um")).collect::<Vec<_>>().join(" ");
    stripped.chars().all(|c|c.is_whitespace()||matches!(c,','|'，'|'.'|'。'|'、'))
}
fn listed_gap(text:&str)->bool {matches!(text.trim().to_ascii_lowercase().as_str(),"and"|"or"|"和"|"与"|"以及")}
fn context(text:&str,start:usize,end:usize)->(String,String) {
    let before=text[..start].chars().rev().take(90).collect::<String>().chars().rev().collect::<String>().to_ascii_lowercase();
    let after=text[end..].chars().take(90).collect::<String>().to_ascii_lowercase();(before,after)
}
fn labelled_group(text:&str,start:usize)->bool {
    let window=text[..start].chars().rev().take(48).collect::<String>().chars().rev().collect::<String>().to_ascii_lowercase();
    let before=window.rsplit(['。','.','!','！','?','？',';','；','\n']).next().unwrap_or("");
    let chinese=["两个","三个","四个","五个","六个","2个","3个","4个","5个","6个","两种","三种","四种","五种","六种"];
    let nouns=["模型","标签","方案","选项","变量","组别","答案","等级","字段"];
    chinese.iter().any(|count|nouns.iter().any(|noun|before.contains(&format!("{count}{noun}"))))
        || ["two","three","four","five","six","2","3","4","5","6"].iter().any(|count|
            ["models","labels","options","variables","groups","answers","columns"].iter().any(|noun|before.contains(&format!("{count} {noun}"))))
}
fn separate(text:&str,start:usize,end:usize)->bool {
    let (before,after)=context(text,start,end);let local=format!("{before} {after}");
    labelled_group(text,start)||["独立字母","单个字母","每个字母","四个字母","三个字母","两个字母","分别读","逐字","不要合并","别合并","四个选项","选项","separate letter","individual letter","each letter","letters are","letters were","spell it","spelling","options","case sensitive","case-sensitive"]
        .iter().any(|cue|local.contains(cue))
        || ["字段","变量","标识符","文件名","原样","字面","variable","identifier","filename","verbatim"].iter()
            .any(|cue|before.trim_end_matches(|c:char|c.is_whitespace()||matches!(c,':'|'：'|'是')).ends_with(cue))
}
fn term_context(text:&str,start:usize,end:usize,letters:&str)->bool {
    if preserve_letters(text,start,end){return false;}
    let (before,after)=context(text,start,end);let prose=format!("{before} {after}");
    let entity=["了解","讨论","研究","评估","资料","相关","使用","模型","系统","技术","关于","学习","感兴趣","筛查","访谈","报告","提到","介绍","开发","来自","称为","叫做",
        "缩写","简称","about","discuss","using","use ","with ","worked","research","study","studies","learning","comparing","evaluation","screening","agents","agent","model","system","topic","acronym","abbreviation"]
        .iter().any(|cue|prose.contains(cue));
    if !entity {return false;}
    // Two-letter words are much more easily ordinary articles/pronouns. Keep
    // lowercase a/i unless followed by an explicit technical noun.
    if letters=="AI" && text[start..end].chars().any(|c|c=='a'||c=='i') {
        return ["agent","model","system","技术","模型","系统"].iter().any(|cue|after.trim_start().starts_with(cue));
    }
    true
}
pub(super) fn preserve_letters(text:&str,start:usize,end:usize)->bool {
    let (before,after)=context(text,start,end);let local=format!("{before} {after}");
    separate(text,start,end)||["不知道怎么拼","不确定拼写","未知拼写","not know whether","uncertain spelling","unknown spelling"].iter().any(|cue|local.contains(cue))
}
fn segments(letters:&str,words:&[Token],text:&str,begin:usize,end:usize)->Option<Vec<(usize,usize)>> {
    fn solve(at:usize,end:usize,begin:usize,letters:&str,words:&[Token],text:&str)->Option<Vec<(usize,usize)>> {
        if at>end {return Some(Vec::new());}
        for last in (at+1..=end).rev() {
            if !ACRONYMS.contains(&&letters[at-begin..=last-begin]) {continue;}
            if last<end && !text[words[last].end..words[last+1].start].contains([',','，','、']) {continue;}
            if let Some(mut tail)=solve(last+1,end,begin,letters,words,text) {tail.insert(0,(at,last));return Some(tail);}
        }None
    }
    solve(begin,end,begin,letters,words,text)
}
fn runs(text:&str)->Vec<Run> {
    let words=tokens(text);let mut result=Vec::new();let mut i=0;
    while i<words.len() {
        if words[i].end-words[i].start!=1 {i+=1;continue;}
        let mut last=i;let mut letters=text[words[i].start..words[i].end].to_ascii_uppercase();
        while last+1<words.len()&&words[last+1].end-words[last+1].start==1&&letters.len()<12
            &&(gap(&text[words[last].end..words[last+1].start])
                ||(listed_gap(&text[words[last].end..words[last+1].start])&&separate(text,words[i].start,words[last+1].end))) {
            last+=1;letters.push_str(&text[words[last].start..words[last].end].to_ascii_uppercase());
        }
        if letters.len()>=2 {
            // A comma can separate two different spelled terms. Partition only
            // at punctuation and only when both sides are known acronyms.
            if let Some(parts)=segments(&letters,&words,text,i,last) {
                for (a,b) in parts {let letters=words[a..=b].iter().map(|w|&text[w.start..w.end]).collect::<String>().to_ascii_uppercase();
                    result.push(Run{start:words[a].start,end:words[b].end,allowed:term_context(text,words[a].start,words[b].end,&letters),letters});}
            } else {
                let (before,after)=context(text,words[i].start,words[last].end);let local=format!("{before} {after}");
                let explicit=letters.len()>=3&&["缩写","英文简称","acronym","abbreviation"].iter().any(|cue|local.contains(cue));
                result.push(Run{start:words[i].start,end:words[last].end,allowed:(ACRONYMS.contains(&letters.as_str())||explicit)&&term_context(text,words[i].start,words[last].end,&letters),letters});
            }
        }
        i=last+1;
    }result
}
pub(super) fn normalize(text:&str,english:bool)->String {
    let runs=runs(text);let mut output=String::new();let mut offset=0;
    for run in runs.into_iter().filter(|r|r.allowed) {
        output.push_str(&text[offset..run.start]);output.push_str(&run.letters);offset=run.end;
    }
    output.push_str(&text[offset..]);super::dates::normalize(&output,english)
}
pub(super) fn validate(source:&SourceRevision,blocks:&[OutputBlock])->Result<(),CloudError> {
    let source_text=source.text();let mut expected=HashMap::<String,usize>::new();
    let source_runs=runs(&source_text);
    for run in source_runs.iter().filter(|r|!r.allowed) {
        let distinct=run.letters.bytes().collect::<std::collections::HashSet<_>>().len();
        let ambiguous=run.letters.len()>=3&&distinct>=2&&!term_context(&source_text,run.start,run.end,&run.letters);
        if preserve_letters(&source_text,run.start,run.end)||ambiguous {*expected.entry(run.letters.clone()).or_default()+=1;}
    }
    let mut actual=HashMap::<String,usize>::new();
    for block in blocks {for run in runs(&block.text){*actual.entry(run.letters).or_default()+=1;}}
    if expected.iter().any(|(letters,n)|actual.get(letters).copied().unwrap_or(0)<*n) {
        return Err(CloudError::new("rewrite_literal_changed","rewrite","整理稿可能合并了独立字母或不明确的拼写，识别原文仍保留"));
    }
    let output=blocks.iter().map(|b|normalize(&b.text,false)).collect::<Vec<_>>().join("\n");
    let output_words=tokens(&output).into_iter().map(|w|output[w.start..w.end].to_ascii_uppercase()).collect::<HashSet<_>>();
    if source_runs.iter().filter(|r|r.allowed).any(|run|!output_words.contains(&run.letters)) {
        return Err(CloudError::new("rewrite_literal_changed","rewrite","整理稿可能改变了口述缩写的字母，识别原文仍保留"));
    }
    Ok(())
}
pub(super) fn attach(body:&mut Value,source:&SourceRevision,english:bool)->Result<(),CloudError> {
    let invalid=||CloudError::new("invalid_request","rewrite","口述格式请求无效");
    let system=body["messages"][0]["content"].as_str().ok_or_else(invalid)?;
    if !system.contains(VERSION) {body["messages"][0]["content"]=json!(format!("{system}\n{VERSION}\n{POLICY}"));}
    let mut data:Value=serde_json::from_str(body["messages"][1]["content"].as_str().ok_or_else(invalid)?).map_err(|_|invalid())?;
    let text=source.text();
    data["notationHints"]=json!({"dateStyle":if english{"ISO numeric; missing components stay missing"}else{"Chinese numeric year/month/day"},
        "acronyms":runs(&text).into_iter().filter(|r|r.allowed).take(24).map(|r|json!({"spoken":&text[r.start..r.end],"written":r.letters})).collect::<Vec<_>>()});
    body["messages"][1]["content"]=json!(data.to_string());Ok(())
}

#[cfg(test)]
#[path="rewrite_spoken_format_tests.rs"]
mod tests;
