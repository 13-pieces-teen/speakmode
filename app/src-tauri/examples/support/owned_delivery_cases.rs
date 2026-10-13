// Synthetic fixtures only; never copied from a user's input or clipboard.
pub fn payload(variant: &str) -> String {
    match variant {
        "emoji" => "SpeakMode 中文 English 👩🏽‍💻 🚀 e\u{301}，保留否定。".into(),
        "long" => (1..=40).map(|line| format!("{line:02}. SpeakMode 中文 English 👩🏽‍💻；预算 3500，不删除原始录音。\r\n"))
            .collect::<String>().trim_end_matches("\r\n").to_owned(),
        _ => "SpeakMode 合成测试。".into(),
    }
}
