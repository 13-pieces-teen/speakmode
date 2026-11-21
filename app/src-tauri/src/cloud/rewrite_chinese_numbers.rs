//! Arabic notation for clear Chinese quantities and spoken digit strings.
use super::super::{dates, integrity, literals};

const DIGITS: &str = "零一二三四五六七八九";

fn digit(c: char) -> Option<u8> {
    if c == '〇' {
        return Some(0);
    }
    if c == '幺' {
        return Some(1);
    }
    DIGITS.chars().position(|d| d == c).map(|d| d as u8)
}

fn digits(raw: &str) -> Option<String> {
    raw.chars().map(|c| digit(c).map(|d| char::from(b'0' + d))).collect()
}

// The existing fact parser accepts some abbreviated spoken amounts. Require
// conventional place-value spelling before committing one value to the draft:
// 一百零二 is 102, whereas 一百二 can also mean 120.
fn explicit_cardinal(raw: &str) -> Option<String> {
    let value = integrity::chinese_number(raw)?;
    let mut written = String::new();
    let mut gap = false;
    for (divisor, unit) in [(1_000_000_000_000, "万亿"), (100_000_000, "亿"), (10_000, "万"), (1, "")] {
        let group = value / divisor % 10_000;
        if group == 0 {
            gap |= !written.is_empty();
            continue;
        }
        if !written.is_empty() && (gap || group < 1_000) {
            written.push('零');
        }
        gap = false;
        let mut section = false;
        for (place, suffix) in [(1_000, "千"), (100, "百"), (10, "十"), (1, "")] {
            let d = group / place % 10;
            if d == 0 {
                gap |= section;
                continue;
            }
            if gap {
                written.push('零');
                gap = false;
            }
            written.push(DIGITS.chars().nth(d as usize)?);
            written.push_str(suffix);
            section = true;
        }
        written.push_str(unit);
        gap = false;
    }
    let raw = raw.replace('〇', "零").replace('两', "二");
    (raw == written || written.strip_prefix('一').is_some_and(|tail| tail.starts_with('十') && tail == raw))
        .then(|| value.to_string())
}

fn has_unit(tail: &str) -> bool {
    // These units can accompany a single digit. Generic classifiers such as
    // 个/种/段 can be determiners and are left to the surrounding prose.
    if ["元化", "元论", "元里", "元一次", "元二次", "秒杀", "秒懂", "米线", "米粒", "米粉", "米饭", "米粥"]
        .iter().any(|word| tail.starts_with(word))
    {
        return false;
    }
    ["毫秒", "秒", "分钟", "小时", "元", "美元", "块钱", "公斤", "千克", "米", "摄氏度", "华氏度", "开尔文", "%", "％"]
        .iter().any(|unit| tail.starts_with(unit))
}

fn has_quantity_suffix(tail: &str) -> bool {
    has_unit(tail) || ["人", "次", "条", "项", "个", "倍", "天", "个月", "年", "辆", "台", "套", "张", "件", "页", "本", "岁", "百分点"]
        .iter().any(|unit| tail.starts_with(unit))
}

fn has_numeric_label(label: &str) -> bool {
    ["金额", "预算", "费用", "价格", "得分", "分数", "数值", "读数", "结果", "等于", "百分之", "温度", "长度", "距离", "重量", "时长", "浓度", "比例"]
        .iter().any(|cue| label.ends_with(cue))
}

fn ends_phrase(tail: &str) -> bool {
    tail.starts_with('\n')
        || tail.trim_start().chars().next().is_none_or(|c| "，,。.!！?？；;、)）".contains(c))
}

fn has_numeric_followup(tail: &str) -> bool {
    ["呢", "应该", "等于", "加", "减", "乘以", "除以", "分"]
        .iter().any(|cue| tail.starts_with(cue))
        || ["和", "或", "到", "至"].iter().any(|link| tail.strip_prefix(link)
            .and_then(|rest| rest.trim_start().chars().next())
            .is_some_and(|c| digit(c).is_some() || c.is_ascii_digit() || "两十百千万亿".contains(c)))
}

fn has_decimal_context(before: &str, label: &str, tail: &str) -> bool {
    // A clock cue takes precedence. The clock is preserved for semantic review.
    if ["上午", "下午", "凌晨", "中午", "晚上", "早上", "早晨", "傍晚"]
        .iter().any(|cue| before.ends_with(cue))
        || tail.starts_with('刻')
        || tail.starts_with('分') && !tail.starts_with("分钟")
    {
        return false;
    }
    // Require a measurement or direct numeric label before turning 点 into a
    // decimal point. An unpunctuated list ("三点一是…") has neither.
    let measured = |suffix: &str| {
        has_unit(suffix) || ["度", "倍", "个百分点", "天", "个月", "年"]
            .iter().any(|unit| suffix.starts_with(unit))
    };
    measured(tail) || has_numeric_label(label)
        || ["万亿", "亿", "万", "千"].iter()
            .any(|scale| tail.strip_prefix(scale).is_some_and(|rest| measured(rest) || ends_phrase(rest)))
}

fn replacement(text: &str, start: usize, end: usize) -> Option<String> {
    let before = text[..start].trim_end();
    let raw = &text[start..end];
    let tail = text[end..].trim_start();
    let label = before.trim_end_matches(['是', '为', '叫', ':', '：']).trim_end();
    let version_label = label.get(label.len().saturating_sub("version".len())..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case("version"));
    if dates::protected_notation(text, start, end)
        || version_label
        || ["版本", "版本号", "型号", "标题", "命名", "片名", "书名", "名字"].iter().any(|cue| label.ends_with(cue))
        || before.ends_with("名叫") || before.ends_with("名为")
        || before.ends_with(['第', '几', '数'])
        || raw == "十" && before.ends_with('好')
        || ["周", "星期", "礼拜"].iter().any(|word| before.ends_with(word))
        || tail.starts_with(['余', '多', '来']) && !tail.starts_with("来自")
        || ["十分", "十足", "万一", "千万", "一点一滴", "三点一线", "三十而立", "三十六计", "十二生肖", "一千零一夜", "七十二变"]
            .iter().any(|word| text[start..].starts_with(word) && !((raw == "十" || raw == "千万") && has_unit(tail)))
    {
        return None;
    }
    if let Some((integer, fraction)) = raw.split_once('点') {
        if fraction.is_empty() || raw.contains('幺') || !has_decimal_context(before, label, tail) {
            return None;
        }
        let integer = digits(integer).or_else(|| explicit_cardinal(integer))?;
        return Some(format!("{integer}.{}", digits(fraction)?));
    }
    let digit_count = raw.chars().count();
    if let Some(written) = digits(raw) {
        // Short runs can be ranges, and a run glued to a word can be part of
        // that word or a title. Join clear standalone or quantified runs.
        return (digit_count >= 3 && (ends_phrase(&text[end..]) || has_numeric_followup(tail) || has_unit(tail))
            || digit_count == 1 && raw != "幺" && has_unit(tail)).then_some(written);
    }
    if raw == "两" {
        return has_unit(tail).then(|| "2".to_owned());
    }
    if raw == "十" && !has_unit(tail) || raw.contains("两十") {
        return None;
    }
    let has_number_before_de = tail.starts_with('的')
        && ["有", "是", "为", "扣了", "加了"].iter().any(|cue| before.ends_with(cue));
    if ends_phrase(&text[end..]) || has_quantity_suffix(tail) || has_numeric_followup(tail)
        || has_number_before_de || has_numeric_label(label)
    {
        return explicit_cardinal(raw);
    }
    None
}

fn starts_thousands_group(text: &str) -> bool {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    digits == 3 && text[digits..].chars().next().is_none_or(|c| !c.is_ascii_alphanumeric())
}

pub(super) fn normalize(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut offset = 0;
    let mut chars = text.char_indices().peekable();
    let mut literals = literals::spans(text).into_iter().peekable();
    let is_number = |c| digit(c).is_some() || "两十百千万亿".contains(c);
    while let Some((start, c)) = chars.next() {
        if !is_number(c) {
            continue;
        }
        while literals.peek().is_some_and(|span| span.end <= start) {
            literals.next();
        }
        let mut end = start + c.len_utf8();
        let mut decimal = false;
        while let Some(&(at, next)) = chars.peek() {
            if next == '千' && text[at..].starts_with("千克")
                || !is_number(next) && next != '点'
                || decimal && matches!(next, '千' | '万' | '亿')
            {
                break;
            }
            decimal |= next == '点';
            end = at + next.len_utf8();
            chars.next();
        }
        if literals.peek().is_some_and(|span| span.start < end)
            || c == '千' && text[start..].starts_with("千克")
        {
            continue;
        }
        if let Some(written) = replacement(text, start, end) {
            output.push_str(&text[offset..start]);
            // A comma adjacent to a newly Arabic three-digit group could
            // turn two values into a single thousands-grouped fact.
            let ends_in_digit = |s: &str| s.as_bytes().last().is_some_and(u8::is_ascii_digit);
            if starts_thousands_group(&written) && output.strip_suffix(',').is_some_and(ends_in_digit) {
                output.push(' ');
            }
            output.push_str(&written);
            offset = end;
            if ends_in_digit(&written) && text[end..].strip_prefix(',').is_some_and(starts_thousands_group) {
                output.push_str(", ");
                offset += 1;
            }
        }
    }
    output.push_str(&text[offset..]);
    output
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn formats_clear_amounts_measurements_and_digit_strings() {
        for (input, expected) in [
            ("我有一个疑问：之前有三百的法石，那去除这个罚时，岂不是之前的四七一应该是一七一呢？这是我的一个问题。",
             "我有一个疑问：之前有300的法石，那去除这个罚时，岂不是之前的471应该是171呢？这是我的一个问题。"),
            ("得分零零七，预算三百零二元。", "得分007，预算302元。"),
            ("报价两万零三百元，等三分钟。", "报价20300元，等3分钟。"),
            ("等待十分钟，重量是三千克。", "等待10分钟，重量是3千克。"),
            ("耗时三点五分钟，预算三点五万元。", "耗时3.5分钟，预算3.5万元。"),
            ("数值为三点一四，排队三百人。", "数值为3.14，排队300人。"),
            ("编号是幺二三四。", "编号是1234。"),
            ("他说「三百元」，报价却是三百零五元。", "他说「三百元」，报价却是305元。"),
        ] {
            assert_eq!(normalize(input), expected, "{input}");
            assert_eq!(normalize(expected), expected, "{expected}");
        }
    }

    #[test]
    fn preserves_uncertain_values_literals_and_non_numeric_words() {
        for input in [
            "三四天之后，可能有三四百人，预算一百二左右。",
            "我有一个问题。这件事十分清楚，万一出错，千万不要改。这都是一点一滴积累的。",
            "几十分钟后有三百多人到场，演出叫《三十六计》。",
            "周三三点半见，八点三十分再确认。",
            "我们下午三点零五见，八点一到就出发。",
            "还有三点一是成本，二是时间，三是质量。",
            "请记三点一人一票，二次抽检，三方共同确认。",
            "下午三点一刻见。请解释一元二次方程。",
            "版本四七一，版本号四七一，片名叫一二三。我们去玩一二三木头人。",
            "先读四百击这本书。五十步笑百步。",
            "请原样保留三百。文件名叫四七一。路径 report_三百.txt。",
            "代码 `四七一` 和字段 A三百。",
            "请先评估四个模型、三个变量和两种提示方案。",
        ] {
            assert_eq!(normalize(input), input, "{input}");
        }
    }

    #[test]
    fn preserves_each_value_at_ascii_commas() {
        for (input, expected) in [
            ("编号一二三,四五六。", "编号123, 456。"),
            ("编号一二三,456。", "编号123, 456。"),
            ("编号123,四五六。", "编号123, 456。"),
            ("金额分别为三百,四百和五百。", "金额分别为300, 400和500。"),
            ("报价3,500元，三百元已到账。", "报价3,500元，300元已到账。"),
        ] {
            assert_eq!(normalize(input), expected, "{input}");
            assert_eq!(normalize(expected), expected, "{expected}");
        }
    }
}
