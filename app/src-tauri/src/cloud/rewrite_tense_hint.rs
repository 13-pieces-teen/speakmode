//! Finite warning for a present/past switch in the same English verb frame.
//! This is not an English parser or proof of temporal equivalence. No text edits.
use std::collections::HashMap;

fn frames(text:&str)->HashMap<(String,usize,String),[usize;2]> {
    let mut prose=String::new();let mut offset=0;
    for span in super::literals::spans(text){prose.push_str(&text[offset..span.start]);prose.push_str(" . ");offset=span.end;}
    prose.push_str(&text[offset..]);
    let pairs=[("work","worked"),("focus","focused"),("develop","developed"),("research","researched"),
        ("maintain","maintained"),("manage","managed"),("lead","led"),("use","used"),("study","studied"),
        ("build","built"),("have","had"),("am","was"),("own","owned"),("support","supported"),
        ("test","tested"),("design","designed")];
    let mut found=HashMap::new();
    for clause in prose.split(['.','!','?',';',':','\n']) {
        let words=clause.split(|c:char|!c.is_ascii_alphabetic()).filter(|w|!w.is_empty()).map(str::to_ascii_lowercase).collect::<Vec<_>>();
        for i in 0..words.len() {
            if !["i","we","you","they"].contains(&words[i].as_str()){continue;}
            let mut at=i+1;
            while at<words.len() && ["currently","still","mainly","also"].contains(&words[at].as_str()){at+=1;}
            if at+1>=words.len(){continue;}
            for (verb,(present,past)) in pairs.iter().enumerate() {
                let tense=if words[at]==*present{0}else if words[at]==*past{1}else{continue;};
                let counts=found.entry((words[i].clone(),verb,words[at+1].clone())).or_insert([0,0]);counts[tense]+=1;
            }
        }
    }
    found
}

pub(super) fn changed(reference:&str,draft:&str)->bool {
    let before=frames(reference);let after=frames(draft);
    before.iter().any(|(frame,old)|after.get(frame).is_some_and(|new|
        (old[0]>new[0] && old[1]<new[1]) || (old[1]>new[1] && old[0]<new[0])))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn present_past_switch_warns_but_cleanup_and_existing_mixed_tense_do_not() {
        for (before,after) in [
            ("My previous job was in evaluation, where I focus on research.","My previous job was in evaluation, where I focused on research."),
            ("I currently build tools.","I currently built tools."),
            ("We worked on infrastructure.","We work on infrastructure."),
            ("I had two reports.","I have two reports."),
        ] {assert!(changed(before,after),"{before}");}
        for (before,after) in [
            ("I I focus on research，呃，and A I agents.","I focus on research and AI agents."),
            ("I worked on evaluation. I work on research.","I worked on evaluation. I work on research."),
            ("I work on design. I work on design.","I work on design."),
            ("Keep `I work on design` exactly.","Keep `I worked on design` exactly."),
            ("I work on design.","I collaborate on design."),
        ] {assert!(!changed(before,after),"{before}");}
    }
}
