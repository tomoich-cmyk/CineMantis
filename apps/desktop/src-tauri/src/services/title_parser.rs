use regex::Regex;
use serde::Serialize;
use std::sync::OnceLock;

// ─── 正規表現キャッシュ ────────────────────────────────────────────────────────

fn re_episode() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?xi)
            (?:
              [Ss](\d{1,3})[Ee](\d{1,3})   # S01E03
            | [Ss]eason\s*(\d{1,3})\s*[Ee]p(?:isode)?\s*(\d{1,3})  # Season 1 Ep 3
            | \b(\d{1,2})x(\d{2,3})\b       # 1x03
            | (?:第|ep\.?\s*)(\d{1,3})(?:話|回|章|ep)?  # 第3話
            | \bEP\.?\s*(\d{1,3})\b         # EP03
            )"
        )
        .unwrap()
    })
}

fn re_season_only() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?xi) (?:[Ss]eason|Season|シーズン)\s*(\d{1,2})").unwrap()
    })
}

fn re_year() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"\b(19\d{2}|20[012]\d)\b").unwrap()
    })
}

fn re_part() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?xi)
            (?:
              [Cc][Dd]\s*(\d)
            | [Dd]isc\s*(\d)
            | [Pp]art\s*(\d)
            | (?:前編|後編|上巻|下巻|前部|後部)
            | [Pp][Tt]\s*(\d)
            )"
        )
        .unwrap()
    })
}

fn re_junk() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?xi)
            \b(?:
              2160p|1080[pi]|720p|480p|576p|4k|uhd|hd
            | bluray|blu-ray|bdrip|brrip
            | webrip|web-dl|webdl|web\s*rip
            | hdrip|dvdrip|dvd|dvdscr|ts|cam|hdtv
            | x264|x265|h264|h265|hevc|avc|xvid|divx
            | aac|ac3|dts|truehd|flac|mp3|dd5\.?1|atmos
            | remux|proper|repack|extended|directors\.?cut
            | theatrical|unrated|retail
            | 10bit|8bit|hdr|hdr10|dv|dolby
            | yts|yify|rarbg|ettv|fgt|eztv|publichd
            )\b
            | \[.*?\]        # [タグ]
            | \((?!\d{4}\)) .*? \)   # (タグ) ただし年号(4桁)は除外
            "
        )
        .unwrap()
    })
}

fn re_separator() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[._\-]+").unwrap())
}

// ─── 公開型 ───────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone)]
pub struct ParsedTitle {
    pub raw: String,
    pub normalized_title: String,
    pub media_kind: String,       // "movie" | "tv" | "unknown"
    pub season_no: Option<i32>,
    pub episode_no: Option<i32>,
    pub year_hint: Option<i32>,
    pub part_hint: Option<String>,
    pub search_tokens: Vec<String>,
}

// ─── 公開関数 ─────────────────────────────────────────────────────────────────

/// ファイル名から ParsedTitle を生成する
pub fn parse_title(file_name: &str) -> ParsedTitle {
    let without_ext = remove_extension(file_name);

    let year_hint = extract_year(&without_ext);
    let (season_no, episode_no) = extract_episode(&without_ext);
    let part_hint = extract_part(&without_ext);

    let media_kind = if season_no.is_some() || episode_no.is_some() {
        "tv"
    } else {
        "unknown"  // movie か tv かは TMDb 照合後に確定
    };

    let normalized_title = normalize_title(&without_ext);
    let search_tokens = build_search_tokens(&normalized_title, year_hint);

    ParsedTitle {
        raw: file_name.to_string(),
        normalized_title,
        media_kind: media_kind.to_string(),
        season_no,
        episode_no,
        year_hint,
        part_hint,
        search_tokens,
    }
}

/// DB の title（scan 時点での推定文字列）をさらに正規化して検索用文字列を返す
pub fn normalize_for_search(raw_title: &str) -> String {
    let cleaned = re_junk().replace_all(raw_title, " ");
    let cleaned = re_separator().replace_all(&cleaned, " ");
    let collapsed: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.trim().to_string()
}

// ─── 内部ヘルパー ─────────────────────────────────────────────────────────────

fn remove_extension(file_name: &str) -> String {
    match file_name.rfind('.') {
        Some(pos) if pos > 0 => file_name[..pos].to_string(),
        _ => file_name.to_string(),
    }
}

fn extract_year(s: &str) -> Option<i32> {
    re_year()
        .find(s)
        .and_then(|m| m.as_str().parse::<i32>().ok())
        .filter(|&y| (1888..=2099).contains(&y))
}

fn extract_episode(s: &str) -> (Option<i32>, Option<i32>) {
    let Some(caps) = re_episode().captures(s) else {
        return (None, None);
    };

    // S01E03 形式
    if let (Some(s), Some(e)) = (caps.get(1), caps.get(2)) {
        let season = s.as_str().parse().ok();
        let episode = e.as_str().parse().ok();
        return (season, episode);
    }
    // Season N Episode M 形式
    if let (Some(s), Some(e)) = (caps.get(3), caps.get(4)) {
        let season = s.as_str().parse().ok();
        let episode = e.as_str().parse().ok();
        return (season, episode);
    }
    // NxMM 形式
    if let (Some(s), Some(e)) = (caps.get(5), caps.get(6)) {
        let season = s.as_str().parse().ok();
        let episode = e.as_str().parse().ok();
        return (season, episode);
    }
    // 第N話 / EPN 形式 — season 不明
    let ep_group = caps.get(7).or(caps.get(8));
    if let Some(e) = ep_group {
        return (Some(1), e.as_str().parse().ok());
    }

    (None, None)
}

fn extract_part(s: &str) -> Option<String> {
    re_part()
        .find(s)
        .map(|m| m.as_str().to_lowercase())
}

fn normalize_title(s: &str) -> String {
    // 1. エピソード情報を除去
    let no_ep = re_episode().replace_all(s, " ");
    // 2. 年号を除去
    let no_year = re_year().replace_all(&no_ep, " ");
    // 3. ジャンクタグを除去
    let no_junk = re_junk().replace_all(&no_year, " ");
    // 4. シーズン表記を除去
    let no_season = re_season_only().replace_all(&no_junk, " ");
    // 5. パート表記を除去
    let no_part = re_part().replace_all(&no_season, " ");
    // 6. セパレーターをスペースに
    let spaced = re_separator().replace_all(&no_part, " ");
    // 7. 折りたたみ
    let collapsed: String = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.trim().to_string()
}

fn build_search_tokens(title: &str, year: Option<i32>) -> Vec<String> {
    let mut tokens = vec![title.to_string()];
    if let Some(y) = year {
        tokens.push(format!("{title} {y}"));
    }
    // 長いタイトルなら先頭3語も候補に
    let words: Vec<&str> = title.split_whitespace().collect();
    if words.len() > 3 {
        tokens.push(words[..3].join(" "));
    }
    tokens
}

// ─── テスト ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_episode_s01e03() {
        let p = parse_title("Breaking.Bad.S01E03.1080p.BluRay.mkv");
        assert_eq!(p.season_no, Some(1));
        assert_eq!(p.episode_no, Some(3));
        assert_eq!(p.media_kind, "tv");
    }

    #[test]
    fn test_movie_year() {
        let p = parse_title("Blade.Runner.2049.2017.1080p.BluRay.x264.mkv");
        assert_eq!(p.year_hint, Some(2017));
        assert!(p.season_no.is_none());
        assert!(p.normalized_title.contains("Blade Runner"));
    }

    #[test]
    fn test_japanese_episode() {
        let p = parse_title("孤独のグルメ_第3話.mp4");
        assert_eq!(p.episode_no, Some(3));
        assert_eq!(p.media_kind, "tv");
    }

    #[test]
    fn test_normalize_search() {
        let s = normalize_for_search("Drive.My.Car.2021.1080p.BluRay");
        assert_eq!(s, "Drive My Car 2021");
    }
}
