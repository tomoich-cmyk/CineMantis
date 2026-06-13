pub fn infer_reading(title: &str) -> Option<String> {
    let mut output = String::new();
    let mut has_kana = false;

    for ch in title.chars() {
        if ('\u{3041}'..='\u{3096}').contains(&ch) {
            output.push(ch);
            has_kana = true;
        } else if ('\u{30A1}'..='\u{30F6}').contains(&ch) {
            output.push(char::from_u32(ch as u32 - 0x60).unwrap_or(ch));
            has_kana = true;
        } else if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
        } else if ch.is_whitespace()
            || matches!(ch, '・' | '･' | '-' | '—' | '〜' | '～' | '/' | '／')
        {
            output.push(' ');
        } else if ch.is_alphabetic() {
            return None;
        }
    }

    let reading = output.split_whitespace().collect::<Vec<_>>().join(" ");
    (has_kana && !reading.is_empty()).then_some(reading)
}

#[cfg(test)]
mod tests {
    use super::infer_reading;

    #[test]
    fn converts_katakana_to_hiragana() {
        assert_eq!(
            infer_reading("アイ・アム・レジェンド"),
            Some("あい あむ れじぇんど".to_string())
        );
    }

    #[test]
    fn avoids_guessing_kanji() {
        assert_eq!(infer_reading("アナと雪の女王"), None);
    }
}
