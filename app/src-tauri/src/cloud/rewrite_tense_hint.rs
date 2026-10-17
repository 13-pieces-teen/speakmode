//! Finite warning for a present/past switch in the same English verb frame.
//! This is not an English parser or proof of temporal equivalence. No text edits.
use std::collections::HashMap;

const SUBJECTS: [&str;4]=["i","we","you","they"];
const ADVERBS: [&str;4]=["currently","still","mainly","also"];
const VERBS: [(&str,&str);16]=[("work","worked"),("focus","focused"),("develop","developed"),("research","researched"),
    ("maintain","maintained"),("manage","managed"),("lead","led"),("use","used"),("study","studied"),
    ("build","built"),("have","had"),("am","was"),("own","owned"),("support","supported"),
    ("test","tested"),("design","designed")];
const MAX_COMPLEMENT_WORDS: usize=12;

fn words(clause:&str)->Vec<String> {
    let mut result=Vec::new();
    for part in clause.split([',','，']) {
        // Only isolated, explicitly marked hesitation is removable. An acronym
        // such as UM or an embedded Chinese word is not classified as filler.
        if ["uh","Uh","um","Um","er","Er","erm","Erm","呃","嗯"].contains(&part.trim()){continue;}
        let raw=part.split(|c:char|!c.is_alphanumeric()).filter(|w|!w.is_empty()).collect::<Vec<_>>();
        let letter=|word:&str|word.len()==1&&word.as_bytes()[0].is_ascii_uppercase();
        let mut at=0;
        while at<raw.len() {
            let mut end=at;
            while end<raw.len()&&letter(raw[end]){end+=1;}
            // Same-letter acronym cleanup must not hide a concurrent tense
            // change. Keep repeated "I I" as pronoun hesitation, not "II".
            if (2..=8).contains(&(end-at))&&!raw[at..end].iter().all(|word|*word=="I") {
                result.push(raw[at..end].concat().to_ascii_lowercase());at=end;
            } else {result.push(raw[at].to_lowercase());at+=1;}
        }
    }
    result
}

fn verb_at(words:&[String],subject:usize)->Option<(usize,usize,usize)> {
    if !SUBJECTS.contains(&words[subject].as_str()){return None;}
    let mut at=subject+1;
    while at<words.len()&&ADVERBS.contains(&words[at].as_str()){at+=1;}
    let word=words.get(at)?;
    VERBS.iter().enumerate().find_map(|(verb,(present,past))| {
        if word==present {Some((at,verb,0))} else if word==past {Some((at,verb,1))} else {None}
    })
}

fn frames(text:&str)->HashMap<(String,usize,Vec<String>),[usize;2]> {
    let mut prose=String::new();let mut offset=0;
    for span in super::literals::spans(text){prose.push_str(&text[offset..span.start]);prose.push_str(" . ");offset=span.end;}
    prose.push_str(&text[offset..]);
    let mut found=HashMap::new();
    // An ASR item can end after "I" or in a complement. A bare newline is
    // whitespace, not evidence of a new grammatical sentence.
    for clause in prose.split(['.','!','?',';',':','。','！','？','；','：']) {
        let words=words(clause);
        for i in 0..words.len() {
            let Some((at,verb,tense))=verb_at(&words,i) else {continue;};
            // Bind the finite verb to its own complement, rather than just
            // "on"/"the". Otherwise swapping tense between two objects keeps
            // the same global counts and hides both changes. Retain names and
            // digits; do not collapse different objects to a shared prefix.
            let mut end=(at+1..words.len()).find(|&next|verb_at(&words,next).is_some()).unwrap_or(words.len());
            while end>at+1&&["and","but","or","where","while","when","if","because","although"].contains(&words[end-1].as_str()){end-=1;}
            let complement=&words[at+1..end];
            // A long or unrecognized frame remains unknown to this narrow
            // warning. Truncating it could conflate unrelated objects.
            if complement.is_empty()||complement.len()>MAX_COMPLEMENT_WORDS {continue;}
            let counts=found.entry((words[i].clone(),verb,complement.to_vec())).or_insert([0,0]);counts[tense]+=1;
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

    #[test]
    fn local_frames_keep_fragment_cleanup_objects_and_literals_separate() {
        for (before,after) in [
            ("I\nfocus on research.","I focus on research."),
            ("I I\nfocus on research，呃，and AI agents.","I focus on research and AI agents."),
            ("I focus on research. I focused on testing.","I focused on testing. I focus on research."),
            ("I focus on project 12. I focused on project 13.","I focused on project 13. I focus on project 12."),
            ("I focus on research and I worked on testing.","I focus on research. I worked on testing."),
            ("I focus on A I agent research and development.","I focus on AI agent research and development."),
            ("I I\nfocus on A I agent research，呃，and development.","I focus on AI agent research and development."),
            ("I focus on A I research, um, and R A G tools.","I focus on AI research and RAG tools."),
            ("Keep `I\nfocus on research` exact.","Keep `I focused on research` exact."),
        ] {assert!(!changed(before,after),"{before}");}
        for (before,after) in [
            ("I\nfocus on research.","I focused on research."),
            ("I focus on research. I focused on testing.","I focused on research. I focus on testing."),
            ("I focus on project 12. I focused on project 13.","I focused on project 12. I focus on project 13."),
            ("I focus on research and I worked on testing.","I focused on research. I worked on testing."),
            ("I focus on A I agent research and development.","I focused on AI agent research and development."),
            ("I I\nfocus on A I agent research，呃，and development.","I focused on AI agent research and development."),
            ("I focus on A I research, um, and R A G tools.","I focused on AI research and RAG tools."),
        ] {assert!(changed(before,after),"{before}");}
        assert!(!changed("I focus on UM, research.","I focused on research."));
        // The finite guard does not pretend a shared prefix proves ownership.
        let long="a b c d e f g h i j k l m";
        assert!(!changed(&format!("I focus on {long}."),&format!("I focused on {long}.")));
    }
}
