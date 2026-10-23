//! Finite calendar notation. Missing fields, relative dates and numeric slash
//! dates are never completed. Coordinates always refer to the supplied text.
use super::english_years;
#[derive(Clone,Debug)]
pub(super) struct Mention {pub start:usize,pub end:usize,pub year:Option<u16>,pub month:Option<u8>,pub day:Option<u8>}
impl Mention {
    pub(super) fn keys(&self)->Vec<String> {let mut keys=Vec::new();
        if let Some(v)=self.year {keys.push(format!("year:{v}"));}if let Some(v)=self.month {keys.push(format!("month:{v}"));}
        if let Some(v)=self.day {keys.push(format!("date-day:{v}"));}keys}
    fn render(&self,english:bool)->String {match (self.year,self.month,self.day,english) {
        (Some(y),Some(m),Some(d),true)=>format!("{y:04}-{m:02}-{d:02}"),
        (y,m,d,false)=>format!("{}{}{}",y.map(|v|format!("{v}年")).unwrap_or_default(),m.map(|v|format!("{v}月")).unwrap_or_default(),d.map(|v|format!("{v}日")).unwrap_or_default()),
        (None,Some(m),Some(d),true)=>format!("{} {d}",MONTHS[(m-1) as usize]),
        (Some(y),None,None,true)=>y.to_string(),
        (None,Some(m),None,true)=>MONTHS[(m-1) as usize].into(),
        _=>String::new(),
    }}
}
const MONTHS:[&str;12]=["January","February","March","April","May","June","July","August","September","October","November","December"];
fn digit(c:char)->Option<u16>{match c {'零'|'〇'=>Some(0),'一'=>Some(1),'二'|'两'=>Some(2),'三'=>Some(3),'四'=>Some(4),'五'=>Some(5),'六'=>Some(6),'七'=>Some(7),'八'=>Some(8),'九'=>Some(9),_=>c.to_digit(10).map(|v|v as u16)}}
fn integer(raw:&str)->Option<u16>{
    if raw.chars().all(|c|digit(c).is_some()) {return raw.chars().try_fold(0u16,|n,c|n.checked_mul(10)?.checked_add(digit(c)?));}
    let (mut total,mut number)=(0u16,0u16);
    for c in raw.chars(){if let Some(v)=digit(c){number=number.checked_mul(10)?.checked_add(v)?;continue;}
        let unit=match c {'十'=>10,'百'=>100,'千'=>1000,_=>return None};total=total.checked_add(number.max(1).checked_mul(unit)?)?;number=0;
    }total.checked_add(number)
}
fn number(text:&str,start:usize)->Option<(u16,usize)> {
    let len=text[start..].chars().take(37).take_while(|c|digit(*c).is_some()||matches!(c,'十'|'百'|'千')).map(char::len_utf8).sum::<usize>();
    (len>0&&len<=36).then_some(())?;Some((integer(&text[start..start+len])?,start+len))
}
fn chinese(text:&str,start:usize)->Option<Mention>{
    let (first,mut end)=number(text,start)?;let mut result=Mention{start,end,year:None,month:None,day:None};
    if text[end..].starts_with('年')&&first>=1000 {result.year=Some(first);end+='年'.len_utf8();result.end=end;
        let next=text[end..].trim_start();let at=text.len()-next.len();
        if let Some((month,next))=number(text,at).filter(|(m,e)|(1..=12).contains(m)&&text[*e..].starts_with('月')) {result.month=Some(month as u8);end=next+'月'.len_utf8();result.end=end;}else{return Some(result);}
    }else if text[end..].starts_with('月')&&(1..=12).contains(&first) {result.month=Some(first as u8);end+='月'.len_utf8();result.end=end;}
    else{return None;}
    let next=text[end..].trim_start();let at=text.len()-next.len();
    if let Some((day,last))=number(text,at).filter(|(d,e)|(1..=31).contains(d)&&text[*e..].starts_with(['日','号'])) {
        result.day=Some(day as u8);result.end=last+text[last..].chars().next()?.len_utf8();
    }
    Some(result)
}
#[derive(Clone)]struct Word<'a>{text:&'a str,start:usize,end:usize}
fn words(text:&str,start:usize)->Vec<Word<'_>>{
    let mut out=Vec::new();let mut at=start;
    while at<text.len()&&out.len()<10 {let gap=text[at..].chars().take_while(|c|c.is_whitespace()||matches!(c,','|'-')).map(char::len_utf8).sum::<usize>();at+=gap;
        let len=text[at..].bytes().take_while(u8::is_ascii_alphanumeric).count();if len==0{break;}
        out.push(Word{text:&text[at..at+len],start:at,end:at+len});at+=len;
    }out
}
fn small(word:&str)->Option<u16>{
    let value=word.to_ascii_lowercase();
    let digits=value.bytes().take_while(u8::is_ascii_digit).count();
    if digits>0 {
        let n=value[..digits].parse::<u16>().ok()?;let suffix=&value[digits..];
        let ordinal=match n%100 {11..=13=>"th",_=>match n%10 {1=>"st",2=>"nd",3=>"rd",_=>"th"}};
        if n<=99&&(suffix.is_empty()||suffix==ordinal){return Some(n);}return None;
    }
    ["zero","one","two","three","four","five","six","seven","eight","nine","ten","eleven","twelve","thirteen","fourteen","fifteen","sixteen","seventeen","eighteen","nineteen"].iter().position(|s|*s==value).map(|n|n as u16)
        .or_else(||["zeroth","first","second","third","fourth","fifth","sixth","seventh","eighth","ninth","tenth","eleventh","twelfth","thirteenth","fourteenth","fifteenth","sixteenth","seventeenth","eighteenth","nineteenth"].iter().position(|s|*s==value).map(|n|n as u16))
        .or_else(||match value.as_str(){"twenty"|"twentieth"=>Some(20),"thirty"|"thirtieth"=>Some(30),"forty"=>Some(40),"fifty"=>Some(50),"sixty"=>Some(60),"seventy"=>Some(70),"eighty"=>Some(80),"ninety"=>Some(90),_=>None})
}
fn two(tokens:&[Word<'_>])->Option<(u16,usize)> {
    let first=small(tokens.first()?.text)?;
    if first>=20&&first%10==0 {if let Some(next)=tokens.get(1).and_then(|t|small(t.text)).filter(|v|(1..=9).contains(v)) {return Some((first+next,2));}}
    Some((first,1))
}
fn year(tokens:&[Word<'_>])->Option<(u16,usize)> {
    let first=tokens.first()?.text.to_ascii_lowercase();
    if first.len()==4 {if let Ok(y @ 1000..=9999)=first.parse::<u16>() {return Some((y,1));}}
    if first=="two"&&tokens.get(1)?.text.eq_ignore_ascii_case("thousand") {
        let begin=if tokens.get(2).is_some_and(|w|w.text.eq_ignore_ascii_case("and")){3}else{2};
        return if let Some((tail,len))=two(&tokens[begin..]) {Some((2000+tail,begin+len))}else{Some((2000,2))};
    }
    let base=match first.as_str(){"nineteen"=>1900,"twenty"=>2000,_=>return None};let (tail,len)=two(&tokens[1..])?;
    (tail>=10).then_some((base+tail,1+len))
}
// A supported prefix of a larger spoken number is not a complete year. Keep
// the whole unsupported phrase unchanged rather than converting e.g. the
// first three words of "two thousand five hundred" into 2005.
fn following_word(text:&str,end:usize)->String {
    let rest=text[end..].trim_start_matches(|c:char|c.is_whitespace()||c=='-');
    rest.chars().take_while(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase()
}
fn incomplete_year(text:&str,end:usize)->bool {
    let word=following_word(text,end);
    !word.is_empty()&&(small(&word).is_some()||["hundred","thousand","million","billion","trillion","and","point"].contains(&word.as_str()))
}
fn quantity_after_year(text:&str,end:usize)->bool {
    let word=following_word(text,end);
    ["people","person","persons","participant","participants","attendee","attendees","guest","guests","student","students",
        "employee","employees","project","projects","team","teams","item","items","case","cases","dollar","dollars","euro","euros","yuan",
        "second","seconds","minute","minutes","hour","hours","day","days","month","months","year","years","percent","percentage","times"]
        .contains(&word.as_str())
}
fn english(text:&str,start:usize,calendar_years:&[(usize,usize,u16)])->Option<Mention>{
    if !text.as_bytes().get(start)?.is_ascii_alphabetic(){return None;}
    if text[..start].chars().next_back().is_some_and(|c|c.is_ascii_alphanumeric()||"_/@\\:.-".contains(c)){return None;}
    let tokens=words(text,start);let first=tokens.first()?;if first.start!=start{return None;}
    if let Some(month)=MONTHS.iter().position(|m|first.text.eq_ignore_ascii_case(m)) {
        let (day,used)=two(&tokens[1..])?;if !(1..=31).contains(&day){return None;}
        if tokens.get(used+1).is_some_and(|w|["party","class","hand","person","floor","option","place"].iter().any(|v|w.text.eq_ignore_ascii_case(v))){return None;}
        let mut result=Mention{start,end:tokens[used].end,year:None,month:Some((month+1) as u8),day:Some(day as u8)};
        if let Some((y,len))=year(&tokens[used+1..]) {
            let end=tokens[used+len].end;
            if incomplete_year(text,end){return None;}
            // A comma may separate the date from a new quantity clause. In
            // that case only month/day are established; never borrow its value.
            if !quantity_after_year(text,end){result.year=Some(y);result.end=end;}
        }
        else if tokens.get(used+1).is_some_and(|w|small(w.text).is_some()||w.text.eq_ignore_ascii_case("thousand")) {return None;}
        return Some(result);
    }
    if first.text.eq_ignore_ascii_case("in")||first.text.eq_ignore_ascii_case("year") {
        let index=calendar_years.binary_search_by_key(&start,|(at,_,_)|*at).ok()?;
        let (_,end,y)=calendar_years[index];
        // A number followed by a measured quantity is not a calendar year.
        if incomplete_year(text,end)||quantity_after_year(text,end){return None;}
        return Some(Mention{start:first.end,end,year:Some(y),month:None,day:None});
    }
    None
}
fn protected_notation(text:&str,start:usize,end:usize)->bool {
    // Year-only English mentions include the gap after "in"/"year" so that
    // rendering retains its space. Their token boundary starts after that gap.
    let start=start+text[start..end].len()-text[start..end].trim_start().len();
    let identifier=|c:char|c.is_ascii_alphanumeric()||"_/@\\:-".contains(c);
    let before_chars=text[..start].chars().rev().take(2).collect::<Vec<_>>();
    if text[..start].chars().next_back().is_some_and(identifier)
        ||(before_chars.first()==Some(&'.')&&before_chars.get(1).is_some_and(|c|c.is_ascii_alphanumeric()))
        ||text[end..].chars().next().is_some_and(identifier)
        ||(text[end..].starts_with('.')&&text[end+1..].chars().next().is_some_and(|c|c.is_ascii_alphanumeric())) {return true;}
    // Keep protection local: a filename discussed in another clause does not
    // turn the rest of a long utterance into literal text. A trailing explicit
    // verbatim request may follow a comma after the date itself.
    let boundary=|c:char|matches!(c,'。'|'！'|'？'|'!'|'?'|';'|'；'|'\n'|'.');
    let window=text[..start].chars().rev().take(96).collect::<String>().chars().rev().collect::<String>().to_ascii_lowercase();
    let before=window.rsplit(|c:char|boundary(c)||matches!(c,','|'，')).next().unwrap_or("");
    let window=text[end..].chars().take(96).collect::<String>().to_ascii_lowercase();
    let after=window.split(boundary).next().unwrap_or("");
    let local=format!("{before} {after}");
    ["逐字","原样","字面","verbatim","exact spelling","exactly as spoken"].iter().any(|cue|local.contains(cue))
        ||["文件名","文件夹名","标识符","变量","路径","网址","邮箱","filename","identifier","variable name","file path","filepath","path:","path ","url","email address"]
            .iter().any(|cue|before.contains(cue))
}
fn iso(text:&str,start:usize)->Option<Mention>{
    let bytes=text[start..].as_bytes();if bytes.len()<10||bytes[4]!=b'-'||bytes[7]!=b'-'{return None;}
    if !bytes[..4].iter().chain(bytes[5..7].iter()).chain(bytes[8..10].iter()).all(u8::is_ascii_digit){return None;}
    let tail=&text[start+10..];
    if text[..start].chars().next_back().is_some_and(|c|c.is_ascii_alphanumeric()||"_/@\\:.-".contains(c))
        ||tail.chars().next().is_some_and(|c|c.is_ascii_alphanumeric()||"_/@\\:-".contains(c))
        ||(tail.starts_with('.')&&tail[1..].chars().next().is_some_and(|c|c.is_ascii_alphanumeric())) {return None;}
    let y=text[start..start+4].parse().ok()?;let m=text[start+5..start+7].parse().ok()?;let d=text[start+8..start+10].parse().ok()?;
    ((1000..=9999).contains(&y)&&(1..=12).contains(&m)&&(1..=31).contains(&d)).then_some(Mention{start,end:start+10,year:Some(y),month:Some(m),day:Some(d)})
}
pub(super) fn mentions(text:&str)->Vec<Mention>{
    let mut result=Vec::new();let mut at=0;let mut literals=super::literals::spans(text).into_iter().peekable();
    let calendar_years=english_years::ranges(text);
    while at<text.len(){while literals.peek().is_some_and(|s|s.end<=at){literals.next();}if let Some(span)=literals.peek().filter(|s|s.start<=at&&at<s.end){at=span.end;continue;}
        if let Some(value)=iso(text,at).or_else(||chinese(text,at)).or_else(||english(text,at,&calendar_years)) {
            if !literals.peek().is_some_and(|s|s.start<value.end){
                at=value.end;
                if !protected_notation(text,value.start,value.end){result.push(value);}
                continue;
            }
        }at+=text[at..].chars().next().unwrap().len_utf8();
    }result
}
pub(super) fn fact_keys(text:&str)->Vec<String>{mentions(text).into_iter().filter(|m|m.month.is_some()).flat_map(|m|m.keys()).collect()}
pub(super) fn components(text:&str,mention:&Mention)->Vec<(usize,usize,String)> {
    if mention.month.is_none(){return Vec::new();}
    let mut result=Vec::new();let start=mention.start;let raw=&text[start..mention.end];
    if iso(text,start).is_some_and(|date|date.end==mention.end) {
        result.push((start,start+4,format!("year:{}",mention.year.unwrap())));
        result.push((start+5,start+7,format!("month:{}",mention.month.unwrap())));
        result.push((start+8,start+10,format!("date-day:{}",mention.day.unwrap())));return result;
    }
    if raw.chars().next().is_some_and(|c|c.is_ascii_alphabetic()) {
        let tokens=words(text,start);let (_,used)=two(&tokens[1..]).unwrap();
        result.push((tokens[0].start,tokens[0].end,format!("month:{}",mention.month.unwrap())));
        result.push((tokens[1].start,tokens[used].end,format!("date-day:{}",mention.day.unwrap())));
        if let Some(y)=mention.year {result.push((tokens[used+1].start,mention.end,format!("year:{y}")));}return result;
    }
    let mut at=start;
    for (value,label) in [(mention.year.map(u32::from),"year"),(mention.month.map(u32::from),"month"),(mention.day.map(u32::from),"date-day")] {
        if let Some(value)=value {at=text.len()-text[at..].trim_start().len();if let Some((_,end))=number(text,at) {let last=end+text[end..].chars().next().map(char::len_utf8).unwrap_or(0);result.push((at,last,format!("{label}:{value}")));at=last;}}
    }result
}
pub(super) fn normalize(text:&str,english:bool)->String {
    let mut result=String::new();let mut offset=0;
    for mention in mentions(text){let written=mention.render(english);if written.is_empty(){continue;}
        result.push_str(&text[offset..mention.start]);
        // Year-only English mentions start at the preceding preposition's end.
        if text[mention.start..mention.end].starts_with(char::is_whitespace){result.push(' ');}
        result.push_str(&written);offset=mention.end;
    }result.push_str(&text[offset..]);result
}

#[cfg(test)]mod tests {
    use super::*;
    #[test]fn hyphenated_month_names_are_not_iso_component_offsets(){
        for (text,keys) in [("We meet on June-third.",vec!["month:6","date-day:3"]),
            ("We meet on July-third,2027.",vec!["month:7","date-day:3","year:2027"])] {
            let dates=mentions(text);assert_eq!(dates.len(),1);
            let parts=components(text,&dates[0]);
            assert_eq!(parts.iter().map(|(_,_,key)|key.as_str()).collect::<Vec<_>>(),keys);
            assert!(parts.iter().all(|(start,end,_)|text.is_char_boundary(*start)&&text.is_char_boundary(*end)&&start<end));
        }
    }
    #[test]fn long_whitespace_is_not_reparsed_as_repeated_english_date_starts(){
        let padding=" ".repeat(60_000);let input=format!("{padding}June third.");
        assert_eq!(normalize(&input,true),format!("{padding}June 3."));
    }
    #[test]fn complete_partial_and_spoken_calendar_notation(){for(input,en,output)in[
        ("日期二零二六年十月二十三日。",false,"日期2026年10月23日。"),
        ("The meeting is on June third, two thousand and twenty seven.",true,"The meeting is on 2027-06-03."),
        ("I will graduate in twenty twenty seven.",true,"I will graduate in 2027."),
        ("六月三日再确认，年份不知道。",false,"6月3日再确认，年份不知道。"),
        ("The backup is June fourth.",true,"The backup is June 4."),
        ("记录二零二七年二月三十日，待核对。",false,"记录2027年2月30日，待核对。"),
    ]{assert_eq!(normalize(input,en),output,"{input}");}}
    #[test]fn literal_relative_ambiguous_and_other_numbers_are_not_completed(){for text in[
        "原话‘二零二七年六月三日’，代码`2027-06-03`。","下周五不是五号，明天再定。","3/4尚不确定。",
        "The budget is twenty seven dollars.","in twenty twenty seven dollars","file2027-06-03.txt","June third party candidate","June 23bananas","June twenty twenty seven",
    ]{assert_eq!(normalize(text,true),text,"{text}");}}
    #[test]fn fragmented_year_uses_the_same_frozen_source_interpretation_as_admission(){
        assert_eq!(normalize("I graduate in twenty thirty.\nTwo.",true),"I graduate in 2032.");
        assert_eq!(normalize("In twenty twenty.\nTwo teams joined.",true),"In 2020.\nTwo teams joined.");
        assert_eq!(normalize("The meeting is on 2027-06-03.",false),"The meeting is on 2027年6月3日.");
    }
    #[test]fn a_following_quantity_cannot_supply_a_calendar_year(){
        for (input,expected) in [
            ("The meeting was June third, two thousand people attended.","The meeting was June 3, two thousand people attended."),
            ("The meeting was June third, 2027 participants attended.","The meeting was June 3, 2027 participants attended."),
            ("The meeting was June third, two thousand and seven guests attended.","The meeting was June 3, two thousand and seven guests attended."),
        ] {assert_eq!(normalize(input,true),expected,"{input}");assert!(!fact_keys(input).iter().any(|key|key.starts_with("year:")));}
        for input in ["June third, two thousand five hundred.","June third, two thousand five hundred people attended.",
            "June third, two thousand and twenty seven hundred.","I arrived in two thousand five hundred.","I arrived in two thousand participants."] {
            assert_eq!(normalize(input,true),input,"{input}");
        }
        assert_eq!(normalize("The meeting is June third, two thousand.",true),"The meeting is 2000-06-03.");
        assert_eq!(normalize("The meeting is June third, two thousand and twenty seven.",true),"The meeting is 2027-06-03.");
    }
    #[test]fn unquoted_identifiers_paths_and_verbatim_dates_keep_exact_spelling(){
        for input in [r"路径 C:\二零二六年十月二十三日\report.txt。",r"保存到 ./二零二六年十月二十三日/report.txt。",
            "文件名叫二零二六年十月二十三日。","标识符 date_二零二六年十月二十三日_value。","二零二六年十月二十三日.md",
            "请逐字保留二零二六年十月二十三日。","二零二六年十月二十三日，请保持原样。",
            "Keep June third, two thousand and twenty seven verbatim."] {
            for english in [false,true]{assert_eq!(normalize(input,english),input,"{input}");}
        }
        assert_eq!(normalize("检查文件路径，会议日期是二零二六年十月二十三日。",false),"检查文件路径，会议日期是2026年10月23日。");
        assert_eq!(normalize("六月三日再确认，年份不知道。",false),"6月3日再确认，年份不知道。");
    }
}
