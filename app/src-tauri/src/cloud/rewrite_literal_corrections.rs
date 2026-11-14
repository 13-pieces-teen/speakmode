//! Occurrence-specific receipts for an adjacent, explicit correction of a name.
//! The immutable source is never edited. A receipt only retires preservation of
//! the old value; the complete source still goes to generation and full review.
use std::ops::Range;

struct Clause<'a> { text: &'a str, start: usize }
struct Name<'a> { prefix: &'a str, verb: &'a str, range: Range<usize> }

fn clauses(text: &str) -> Vec<Clause<'_>> {
    let literals=super::literals::spans(text);
    let mut result=Vec::new();let mut start=0;
    for (at,c) in text.char_indices().chain(std::iter::once((text.len(),'。'))) {
        if !matches!(c,','|'，'|'。'|';'|'；'|'\n'|'!'|'！'|'?'|'？')
            || literals.iter().any(|span|at>=span.start&&at<span.end) {continue;}
        let raw=&text[start..at];let trimmed=raw.trim();
        if !trimmed.is_empty(){result.push(Clause{text:trimmed,start:start+raw.len()-raw.trim_start().len()});}
        start=at+c.len_utf8();
    }
    result
}

fn name<'a>(clause: &Clause<'a>) -> Option<Name<'a>> {
    let text=clause.text;
    let literals=super::literals::spans(text);
    let value_start=if let Some(span)=literals.last() {
        // Code and an unterminated quote are never retired. Nested/extra
        // literals in a label are not a unique naming frame either.
        let first=text[span.start..].chars().next()?;
        let end=match first {'《'=>'》','“'=>'”','‘'=>'’','「'=>'」','『'=>'』','\''=>'\'','"'=>'"',_=>return None};
        if literals.len()!=1 || span.end!=text.len() || !text.ends_with(end) || span.end-span.start<=first.len_utf8()+end.len_utf8(){return None;}
        span.start
    }else{
        let start=text.char_indices().rev().take_while(|(_,c)|c.is_ascii_alphanumeric()||matches!(c,'.'|'_'|'-')).last()?.0;
        let value=&text[start..];
        if !value.chars().next()?.is_ascii_alphabetic() || value.len()>80{return None;}
        start
    };
    let prefix=text[..value_start].trim_end();
    if prefix.chars().count()>32 || ["原来","原先","之前","以前","过去","后来","曾经","原名","旧称","更名","改为","历史","如果","可能","不是","不要","不能","并非","未","没有","和","与","、"]
        .iter().any(|cue|prefix.contains(cue)){return None;}
    let verb=["叫做","名为","叫","是","用"].into_iter().find(|verb|prefix.ends_with(verb))?;
    Some(Name{prefix,verb,range:clause.start+value_start..clause.start+text.len()})
}

fn marker(text:&str)->bool {
    let rest=text.trim_start_matches(['嗯','呃','啊']).trim();
    ["不对","说错了","我说错了","更正一下","准确地说"].contains(&rest)
}

pub(super) fn retired_ranges(text:&str)->Vec<Range<usize>> {
    let clauses=clauses(text);let mut retired=Vec::new();
    for group in clauses.windows(3) {
        if !marker(group[1].text){continue;}
        let (Some(old),Some(new))=(name(&group[0]),name(&group[2]))else{continue;};
        let owner=old.prefix.strip_suffix(old.verb).unwrap_or("");
        // A complete repeated label, or a bare continuation of the same naming
        // verb, identifies exactly the adjacent object. Generic 是/用 requires
        // an explicit name/title label; arbitrary assertions are not inferred.
        if owner.is_empty() || (matches!(old.verb,"是"|"用") && !["标题","名字","名称","书名","文件名","变量名","函数名","接口名","用户名","项目名"]
            .iter().any(|label|owner.ends_with(label))){continue;}
        if new.prefix==old.prefix || new.prefix==old.verb || (old.verb!="用" && new.prefix=="是") {
            retired.push(old.range);
        }
    }
    retired
}

pub(super) fn effective_text(text:&str)->String {
    let mut result=String::new();let mut at=0;
    for range in retired_ranges(text){result.push_str(&text[at..range.start]);at=range.end;}
    result.push_str(&text[at..]);result
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn only_explicit_adjacent_name_values_receive_receipts(){
        for text in ["标题叫《旧名》，不对，叫《新名》。","项目叫 OpenCloud，不对，叫 OpenViking。","文件名是‘old’，我说错了，文件名是‘new’。"] {
            assert_eq!(retired_ranges(text).len(),1,"{text}");
        }
        for text in ["原来标题叫《旧名》，不对，叫《新名》。","标题叫《旧名》，可能不对，叫《新名》。","甲项目叫 A，乙项目叫 B，不对，甲项目叫 C。","他说‘标题叫《旧名》，不对，叫《新名》’。","代码是 `old`，不对，是 `new`。","标题叫《旧名》，另外保留原文，不对，叫《新名》。"] {
            assert!(retired_ranges(text).is_empty(),"{text}");
        }
    }
}
