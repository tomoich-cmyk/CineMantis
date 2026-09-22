//! よみ（ひらがな）から NAS の振り分け先フォルダを決める。
//!
//! 実際の NAS 構成:
//!   \\TYNAS\home\Movie\【01】洋画\【あ-】\【あ】\<作品>.mp4
//!   \\TYNAS\home\Movie\【02】邦画\【わ-】\【わ】\<作品>.mp4
//!
//! 注意: わ行のフォルダ名だけ洋画と邦画で表記が揺れている（洋画=【わ】/ 邦画=【わ-】）。
//! 既存フォルダに合わせるため、行フォルダ名は country_type で出し分ける。

/// 振り分け先（NAS ルートからの相対 2 階層）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KanaBucket {
    /// 行フォルダ。例: "【あ-】"
    pub group_dir: String,
    /// 五十音フォルダ。例: "【あ】"
    pub leaf_dir: String,
}

/// 清音ひらがな一文字に正規化する。
///
/// - カタカナ → ひらがな
/// - 濁音・半濁音 → 清音（が→か、ぱ→は）
/// - 小書き → 大書き（ゃ→や、っ→つ、ぁ→あ）
/// - ヴ → う
///
/// 判定に使えない文字（漢字・英数・記号・長音符）は None。
fn normalize_kana(ch: char) -> Option<char> {
    // カタカナ → ひらがな（ヴ ゛゜ を除く基本範囲）
    let ch = if ('\u{30A1}'..='\u{30F6}').contains(&ch) {
        char::from_u32(ch as u32 - 0x60)?
    } else {
        ch
    };

    // ヴ(\u{30F4}) は上の変換で ゔ(\u{3094}) になる
    let normalized = match ch {
        // あ行
        'ぁ' | 'あ' => 'あ',
        'ぃ' | 'い' => 'い',
        'ぅ' | 'う' | 'ゔ' => 'う',
        'ぇ' | 'え' => 'え',
        'ぉ' | 'お' => 'お',
        // か行
        'か' | 'が' | 'ゕ' => 'か',
        'き' | 'ぎ' => 'き',
        'く' | 'ぐ' => 'く',
        'け' | 'げ' | 'ゖ' => 'け',
        'こ' | 'ご' => 'こ',
        // さ行
        'さ' | 'ざ' => 'さ',
        'し' | 'じ' => 'し',
        'す' | 'ず' => 'す',
        'せ' | 'ぜ' => 'せ',
        'そ' | 'ぞ' => 'そ',
        // た行
        'た' | 'だ' => 'た',
        'ち' | 'ぢ' => 'ち',
        'っ' | 'つ' | 'づ' => 'つ',
        'て' | 'で' => 'て',
        'と' | 'ど' => 'と',
        // な行
        'な' => 'な',
        'に' => 'に',
        'ぬ' => 'ぬ',
        'ね' => 'ね',
        'の' => 'の',
        // は行
        'は' | 'ば' | 'ぱ' => 'は',
        'ひ' | 'び' | 'ぴ' => 'ひ',
        'ふ' | 'ぶ' | 'ぷ' => 'ふ',
        'へ' | 'べ' | 'ぺ' => 'へ',
        'ほ' | 'ぼ' | 'ぽ' => 'ほ',
        // ま行
        'ま' => 'ま',
        'み' => 'み',
        'む' => 'む',
        'め' => 'め',
        'も' => 'も',
        // や行
        'ゃ' | 'や' => 'や',
        'ゅ' | 'ゆ' => 'ゆ',
        'ょ' | 'よ' => 'よ',
        // ら行
        'ら' => 'ら',
        'り' => 'り',
        'る' => 'る',
        'れ' => 'れ',
        'ろ' => 'ろ',
        // わ行（を・ん もここに吸収する）
        'ゎ' | 'わ' | 'を' | 'ん' => 'わ',
        _ => return None,
    };
    Some(normalized)
}

/// 清音ひらがなから行の代表文字を返す。例: 'き' → 'か'
fn row_head(kana: char) -> char {
    match kana {
        'あ'..='お' => 'あ',
        'か'..='こ' => 'か',
        'さ'..='そ' => 'さ',
        'た'..='と' => 'た',
        'な'..='の' => 'な',
        'は'..='ほ' => 'は',
        'ま'..='も' => 'ま',
        'や' | 'ゆ' | 'よ' => 'や',
        'ら'..='ろ' => 'ら',
        _ => 'わ',
    }
}

/// よみから振り分け先を決める。
///
/// `country_type` は "domestic"（邦画）か、それ以外（洋画扱い）。
/// わ行のフォルダ名の揺れを吸収するために必要。
///
/// よみに使える仮名が一文字も無ければ None（＝振り分け不可、一覧に残す）。
pub fn bucket_for(reading: &str, country_type: &str) -> Option<KanaBucket> {
    let kana = reading.chars().find_map(normalize_kana)?;
    let head = row_head(kana);

    // わ行だけ洋画と邦画で既存フォルダ名が違う
    let group_dir = if head == 'わ' && country_type != "domestic" {
        "【わ】".to_string()
    } else {
        format!("【{head}-】")
    };

    Some(KanaBucket {
        group_dir,
        leaf_dir: format!("【{kana}】"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(reading: &str, ct: &str) -> (String, String) {
        let x = bucket_for(reading, ct).expect("bucket");
        (x.group_dir, x.leaf_dir)
    }

    #[test]
    fn basic_rows() {
        assert_eq!(b("あい", "foreign"), ("【あ-】".into(), "【あ】".into()));
        assert_eq!(b("きみ", "foreign"), ("【か-】".into(), "【き】".into()));
        assert_eq!(b("ふね", "domestic"), ("【は-】".into(), "【ふ】".into()));
    }

    #[test]
    fn dakuten_folds_to_seion() {
        // が→か、ぱ→は、ゔ→う
        assert_eq!(b("がっこう", "foreign"), ("【か-】".into(), "【か】".into()));
        assert_eq!(b("ぱんだ", "foreign"), ("【は-】".into(), "【は】".into()));
        assert_eq!(b("ゔぁんぱいあ", "foreign"), ("【あ-】".into(), "【う】".into()));
    }

    #[test]
    fn small_kana_folds_to_large() {
        assert_eq!(b("ゃくざ", "foreign"), ("【や-】".into(), "【や】".into()));
        assert_eq!(b("っと", "foreign"), ("【た-】".into(), "【つ】".into()));
    }

    #[test]
    fn katakana_is_accepted() {
        assert_eq!(b("アリ", "foreign"), ("【あ-】".into(), "【あ】".into()));
        assert_eq!(b("ヴァンパイア", "foreign"), ("【あ-】".into(), "【う】".into()));
    }

    #[test]
    fn wo_and_n_go_to_wa() {
        assert_eq!(b("をとこ", "domestic"), ("【わ-】".into(), "【わ】".into()));
        assert_eq!(b("んーぱ", "domestic"), ("【わ-】".into(), "【わ】".into()));
    }

    /// 洋画と邦画で わ行 のフォルダ名が違う（既存 NAS に合わせる）
    #[test]
    fn wa_row_naming_differs_by_country() {
        assert_eq!(b("わたし", "foreign").0, "【わ】");
        assert_eq!(b("わたし", "domestic").0, "【わ-】");
        // わ行以外は両方とも同じ表記
        assert_eq!(b("あい", "foreign").0, "【あ-】");
        assert_eq!(b("あい", "domestic").0, "【あ-】");
    }

    /// 先頭が記号や英数でも、最初に見つかった仮名で判定する
    #[test]
    fn skips_leading_non_kana() {
        assert_eq!(b("  ！あい", "foreign"), ("【あ-】".into(), "【あ】".into()));
        assert_eq!(b("2001ねん", "foreign"), ("【な-】".into(), "【ね】".into()));
    }

    /// 長音符だけ・英数だけ・漢字だけは判定不能
    #[test]
    fn returns_none_when_no_kana() {
        assert_eq!(bucket_for("", "foreign"), None);
        assert_eq!(bucket_for("ーー", "foreign"), None);
        assert_eq!(bucket_for("matrix", "foreign"), None);
        assert_eq!(bucket_for("戦争と平和", "foreign"), None);
    }
}
