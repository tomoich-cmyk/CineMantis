//! 動画ファイルのコンテナに埋め込まれたメタデータ（PR2.5）。
//!
//! ffprobe の `format.tags` と各ストリームの言語を、照合前入力として保存する。
//! タグは配信元（U-NEXT / Disney+ など）が書いたもので TMDB 由来ではないため、
//! 照合の入力に使ってもリークにならない。ただし
//! 「TMDB を見るツールが後から書いたタグ」は評価から外す必要があるので、
//! 出どころ（[`TagProvenance`]）を4段階で持つ。
//!
//! ここは純粋な変換だけを行う。DB も TMDB も触らない。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// container_tags_json の形式
pub const TAGS_SCHEMA_VERSION: u32 = 1;

/// 保存する format.tags のキー（小文字で比較する allowlist）。
/// major_brand / handler_name / vendor_id などは判断に使わないので保存しない。
const FORMAT_KEY_ALLOWLIST: [&str; 10] = [
    "title",
    "show",
    "date",
    "artist",
    "description",
    "genre",
    "episode_id",
    "season_number",
    "comment",
    "encoder",
];

// 長さの上限（文字数）。異常に大きいメタデータで DB を膨らませない
const MAX_TITLE_CHARS: usize = 300;
const MAX_DESCRIPTION_CHARS: usize = 1000;
const MAX_COMMENT_CHARS: usize = 300;
const MAX_GENERIC_CHARS: usize = 300;
const MAX_ARTIST_CHARS: usize = 1500;
/// 出演者1人分の上限
const MAX_CAST_ENTRY_CHARS: usize = 80;
/// 保存する出演者の人数
pub const MAX_CAST_ENTRIES: usize = 30;
/// container_tags_json 全体の上限（バイト）
const MAX_JSON_BYTES: usize = 8 * 1024;

/// タグの出どころ。評価指標はこの区分ごとに集計できるようにする
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagProvenance {
    /// 配信元が分かる（comment の URL など）
    ProviderKnown,
    /// 作成経路が分かる（encoder や取り込み元フォルダ）
    PipelineKnown,
    /// 手がかりなし。照合には使うが、評価では分けて数える
    Unknown,
    /// TMDB / IMDb を参照するツールの痕跡がある。正式な評価からは外す
    Suspicious,
}

impl TagProvenance {
    pub fn as_str(self) -> &'static str {
        match self {
            TagProvenance::ProviderKnown => "provider_known",
            TagProvenance::PipelineKnown => "pipeline_known",
            TagProvenance::Unknown => "unknown",
            TagProvenance::Suspicious => "suspicious",
        }
    }

    /// 精度の計算に使ってよいか（suspicious は除外する）
    pub fn is_evaluable(self) -> bool {
        self != TagProvenance::Suspicious
    }
}

/// 版の表記。配信上の違いと、作品そのものが変わりうる違いを分ける
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditionKind {
    /// 字幕版 / 吹替版。同じ作品の配信形態の違い
    Distribution,
    /// 最終章 / ディレクターズカットなど。別の版を指しうる
    Cut,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditionMarker {
    pub kind: EditionKind,
    pub label: String,
}

/// 保存するタグ一式（そのまま container_tags_json になる）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerTags {
    pub v: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub show: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub episode_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub season_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub encoder: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub audio_languages: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub subtitle_languages: Vec<String>,
    /// 長さ上限で切り詰めたキー
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub truncated: Vec<String>,
}

impl ContainerTags {
    pub fn empty() -> Self {
        ContainerTags {
            v: TAGS_SCHEMA_VERSION,
            ..Default::default()
        }
    }

    /// ffprobe の出力から作る。
    /// `format_tags` はキーの大文字小文字を問わない。
    /// `streams` は (codec_type, language) の並び。
    pub fn from_ffprobe(
        format_tags: &BTreeMap<String, String>,
        streams: &[(String, Option<String>)],
    ) -> Self {
        let mut lower: BTreeMap<String, String> = BTreeMap::new();
        for (key, value) in format_tags {
            let key = key.trim().to_lowercase();
            if !FORMAT_KEY_ALLOWLIST.contains(&key.as_str()) {
                continue;
            }
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            // 同じキーが大文字小文字違いで来たら最初の値を使う
            lower.entry(key).or_insert_with(|| value.to_string());
        }

        let mut truncated = Vec::new();
        let mut take = |key: &str, limit: usize| -> Option<String> {
            let value = lower.get(key)?.clone();
            Some(cut(&value, limit, key, &mut truncated))
        };

        let title = take("title", MAX_TITLE_CHARS);
        let show = take("show", MAX_TITLE_CHARS);
        let date = take("date", MAX_GENERIC_CHARS);
        let artist = take("artist", MAX_ARTIST_CHARS);
        let description = take("description", MAX_DESCRIPTION_CHARS);
        let genre = take("genre", MAX_GENERIC_CHARS);
        let episode_id = take("episode_id", MAX_GENERIC_CHARS);
        let season_number = take("season_number", MAX_GENERIC_CHARS);
        let comment = take("comment", MAX_COMMENT_CHARS);
        let encoder = take("encoder", MAX_GENERIC_CHARS);

        let collect_languages = |kind: &str| -> Vec<String> {
            let mut out: Vec<String> = Vec::new();
            for (codec_type, language) in streams {
                if codec_type != kind {
                    continue;
                }
                let Some(code) = language.as_deref().and_then(normalize_language) else {
                    continue;
                };
                if !out.iter().any(|c| c == code) {
                    out.push(code.to_string());
                }
            }
            out
        };

        let mut tags = ContainerTags {
            v: TAGS_SCHEMA_VERSION,
            title,
            show,
            date,
            artist,
            description,
            genre,
            episode_id,
            season_number,
            comment,
            encoder,
            audio_languages: collect_languages("audio"),
            subtitle_languages: collect_languages("subtitle"),
            truncated,
        };
        tags.enforce_size_limit();
        tags
    }

    /// JSON 全体が大きすぎる場合、あらすじ → 出演者の順に落とす
    fn enforce_size_limit(&mut self) {
        for _ in 0..2 {
            let size = serde_json::to_string(self).map(|s| s.len()).unwrap_or(0);
            if size <= MAX_JSON_BYTES {
                return;
            }
            if self.description.is_some() {
                self.description = None;
                mark_truncated(&mut self.truncated, "description");
                continue;
            }
            if self.artist.is_some() {
                self.artist = None;
                mark_truncated(&mut self.truncated, "artist");
            }
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| r#"{"v":1}"#.to_string())
    }

    pub fn from_json(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }

    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.show.is_none()
            && self.date.is_none()
            && self.artist.is_none()
            && self.description.is_none()
            && self.audio_languages.is_empty()
            && self.subtitle_languages.is_empty()
    }

    /// 検索に使えるタイトル（版の表記を落とし、全角英数をそろえたもの）
    pub fn cleaned_title(&self) -> Option<String> {
        self.title.as_deref().and_then(clean_title)
    }

    pub fn cleaned_show(&self) -> Option<String> {
        self.show.as_deref().and_then(clean_title)
    }

    /// タグの年（配信年のことがあるので、これ単独では AUTO の根拠にしない）
    pub fn year(&self) -> Option<i32> {
        self.date.as_deref().and_then(extract_year)
    }

    /// 出演者。trim・重複排除・人数と長さの制限をかけたもの
    pub fn cast(&self) -> Vec<String> {
        let Some(raw) = self.artist.as_deref() else {
            return Vec::new();
        };
        let mut out: Vec<String> = Vec::new();
        for part in raw.split([',', '，', '、', ';', '；']) {
            let name = part.trim();
            if name.is_empty() {
                continue;
            }
            let name: String = name.chars().take(MAX_CAST_ENTRY_CHARS).collect();
            if !out.iter().any(|n| *n == name) {
                out.push(name);
            }
            if out.len() >= MAX_CAST_ENTRIES {
                break;
            }
        }
        out
    }

    /// タイトルに含まれる版の表記
    pub fn editions(&self) -> Vec<EditionMarker> {
        let mut found = Vec::new();
        for text in [self.title.as_deref(), self.show.as_deref()].into_iter().flatten() {
            for marker in edition_markers(text) {
                if !found.contains(&marker) {
                    found.push(marker);
                }
            }
        }
        found
    }

    /// 作品そのものが変わりうる版の表記だけ（字幕版・吹替版は含めない）
    pub fn cut_editions(&self) -> Vec<String> {
        self.editions()
            .into_iter()
            .filter(|m| m.kind == EditionKind::Cut)
            .map(|m| m.label)
            .collect()
    }

    pub fn episode_id_number(&self) -> Option<i64> {
        self.episode_id.as_deref()?.trim().parse::<i64>().ok()
    }

    /// 配信元の手がかり（comment の URL から）
    pub fn provider_hint(&self) -> Option<&'static str> {
        provider_from_url(self.comment.as_deref()?)
    }

    /// タグの出どころ。`path_hint` には取り込み元のパスを渡してよい（経路の手がかり）
    pub fn provenance(&self, path_hint: Option<&str>) -> TagProvenance {
        let haystack = format!(
            "{} {}",
            self.comment.as_deref().unwrap_or(""),
            self.encoder.as_deref().unwrap_or("")
        )
        .to_lowercase();
        const SUSPICIOUS: [&str; 9] = [
            "themoviedb",
            "tmdb",
            "imdb.com",
            "tinymediamanager",
            "mediaelch",
            "filebot",
            "ember",
            "plex",
            "jellyfin",
        ];
        if SUSPICIOUS.iter().any(|marker| haystack.contains(marker)) {
            return TagProvenance::Suspicious;
        }
        if self.provider_hint().is_some() {
            return TagProvenance::ProviderKnown;
        }
        if let Some(path) = path_hint {
            if pipeline_from_path(path).is_some() {
                return TagProvenance::PipelineKnown;
            }
        }
        if self
            .encoder
            .as_deref()
            .map(|e| known_pipeline_encoder(e))
            .unwrap_or(false)
        {
            return TagProvenance::PipelineKnown;
        }
        TagProvenance::Unknown
    }

    /// episode_id = -1 を「映画」と読んでよい配信元か。
    /// U-NEXT で確認した慣習なので、出どころが分からないときは判断材料にしない。
    pub fn episode_id_marks_movie(&self, path_hint: Option<&str>) -> bool {
        let provider = self
            .provider_hint()
            .or_else(|| path_hint.and_then(provider_from_path));
        matches!(provider, Some("u-next")) && self.episode_id_number() == Some(-1)
    }
}

fn mark_truncated(truncated: &mut Vec<String>, key: &str) {
    if !truncated.iter().any(|k| k == key) {
        truncated.push(key.to_string());
    }
}

fn cut(value: &str, limit: usize, key: &str, truncated: &mut Vec<String>) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    mark_truncated(truncated, key);
    value.chars().take(limit).collect()
}

/// 既知の配信元 URL
fn provider_from_url(comment: &str) -> Option<&'static str> {
    let lower = comment.to_lowercase();
    const PROVIDERS: [(&str, &str); 8] = [
        ("unext.jp", "u-next"),
        ("video.unext", "u-next"),
        ("disneyplus.com", "disney-plus"),
        ("netflix.com", "netflix"),
        ("primevideo.com", "prime-video"),
        ("hulu.jp", "hulu"),
        ("abema.tv", "abema"),
        ("wowow.co.jp", "wowow"),
    ];
    PROVIDERS
        .iter()
        .find(|(needle, _)| lower.contains(needle))
        .map(|(_, name)| *name)
}

/// 取り込み元フォルダからの配信元の手がかり（StreamFab の出力は配信元ごとに分かれている）
fn provider_from_path(path: &str) -> Option<&'static str> {
    let lower = path.to_lowercase();
    const PROVIDERS: [(&str, &str); 5] = [
        ("u-next", "u-next"),
        ("unext", "u-next"),
        ("disney", "disney-plus"),
        ("netflix", "netflix"),
        ("prime", "prime-video"),
    ];
    PROVIDERS
        .iter()
        .find(|(needle, _)| lower.contains(needle))
        .map(|(_, name)| *name)
}

fn pipeline_from_path(path: &str) -> Option<&'static str> {
    let lower = path.to_lowercase();
    if lower.contains("streamfab") {
        Some("streamfab")
    } else {
        None
    }
}

fn known_pipeline_encoder(encoder: &str) -> bool {
    let lower = encoder.to_lowercase();
    ["lavf", "handbrake", "streamfab", "ffmpeg"]
        .iter()
        .any(|name| lower.contains(name))
}

/// 版の表記を取り出す
fn edition_markers(text: &str) -> Vec<EditionMarker> {
    const DISTRIBUTION: [&str; 6] = ["字幕版", "吹替版", "日本語吹替版", "吹き替え版", "字幕", "吹替"];
    // 作品そのものが変わりうるもの（「劇場版」は題名の一部であることが多いので入れない）
    const CUT: [&str; 11] = [
        "最終章",
        "ディレクターズカット",
        "ディレクターズ・カット",
        "director's cut",
        "final cut",
        "ファイナルカット",
        "完全版",
        "無修正版",
        "エクステンデッド",
        "extended",
        "リマスター",
    ];
    let lower = text.to_lowercase();
    let mut found = Vec::new();
    for label in CUT {
        if lower.contains(label) {
            found.push(EditionMarker {
                kind: EditionKind::Cut,
                label: label.to_string(),
            });
        }
    }
    for label in DISTRIBUTION {
        // 「字幕」は「字幕版」に含まれるので、既に拾っていれば足さない
        if lower.contains(label)
            && !found
                .iter()
                .any(|m: &EditionMarker| m.label.contains(label))
        {
            found.push(EditionMarker {
                kind: EditionKind::Distribution,
                label: label.to_string(),
            });
        }
    }
    found
}

/// 検索に使えるようタイトルを整える。
/// - 配信形態の表記（字幕版 / 吹替版）を落とす
/// - 全角英数・記号を半角にそろえる
/// - 空白を畳む
/// 整形の結果が空になったら None（呼び出し側は元の値を使う）
pub fn clean_title(raw: &str) -> Option<String> {
    use regex::Regex;
    use std::sync::OnceLock;
    static EDITION: OnceLock<Regex> = OnceLock::new();
    let edition = EDITION.get_or_init(|| {
        Regex::new(r"(?i)[（(\[【]?\s*(日本語吹替版|吹き替え版|字幕版|吹替版|字幕|吹替)\s*[)）\]】]?")
            .unwrap()
    });

    let without_edition = edition.replace_all(raw, " ");
    let normalized = normalize_widths(&without_edition);
    let collapsed = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(|c: char| c == '-' || c == '_' || c.is_whitespace());
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 全角英数と一部の記号を半角にそろえる
pub fn normalize_widths(raw: &str) -> String {
    raw.chars()
        .map(|c| match c {
            '\u{3000}' => ' ',
            '／' => '/',
            '：' => ':',
            '！' => '!',
            '？' => '?',
            '＆' => '&',
            '＋' => '+',
            '，' => ',',
            'Ａ'..='Ｚ' | 'ａ'..='ｚ' | '０'..='９' => {
                char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)
            }
            other => other,
        })
        .collect()
}

/// 文字列から4桁の年を抜く
pub fn extract_year(value: &str) -> Option<i32> {
    use regex::Regex;
    use std::sync::OnceLock;
    static YEAR: OnceLock<Regex> = OnceLock::new();
    let year = YEAR.get_or_init(|| Regex::new(r"(19|20)\d{2}").unwrap());
    year.find(value)?.as_str().parse::<i32>().ok()
}

/// 言語コードを3文字（ISO 639-2/B 寄り）にそろえる。
/// "ja" / "ja-JP" / "jpn" はすべて "jpn" になる。判別できない値は None。
pub fn normalize_language(raw: &str) -> Option<&'static str> {
    let code = raw.trim().to_lowercase();
    let code = code.split(['-', '_']).next()?.to_string();
    if code.is_empty() {
        return None;
    }
    for (iso1, iso3) in LANGUAGE_TABLE {
        if code == *iso1 || code == *iso3 {
            return Some(iso3);
        }
    }
    match code.as_str() {
        "und" | "unknown" => Some("und"),
        "mul" => Some("mul"),
        _ => None,
    }
}

/// TMDB の original_language（2文字）と比べるために2文字へ戻す
pub fn to_iso639_1(code: &str) -> Option<&'static str> {
    let code = code.trim().to_lowercase();
    LANGUAGE_TABLE
        .iter()
        .find(|(iso1, iso3)| code == *iso1 || code == *iso3)
        .map(|(iso1, _)| *iso1)
}

/// 判断に使える言語か（und は情報なし、jpn は吹替の可能性があるので使わない）
pub fn is_informative_audio_language(code: &str) -> bool {
    !matches!(code, "und" | "jpn" | "mul" | "")
}

const LANGUAGE_TABLE: &[(&str, &str)] = &[
    ("ja", "jpn"),
    ("en", "eng"),
    ("es", "spa"),
    ("fr", "fra"),
    ("de", "deu"),
    ("it", "ita"),
    ("ko", "kor"),
    ("zh", "zho"),
    ("ru", "rus"),
    ("pt", "por"),
    ("da", "dan"),
    ("sv", "swe"),
    ("no", "nor"),
    ("nl", "nld"),
    ("fi", "fin"),
    ("tr", "tur"),
    ("ar", "ara"),
    ("hi", "hin"),
    ("th", "tha"),
    ("pl", "pol"),
    ("cs", "ces"),
    ("hu", "hun"),
    ("el", "ell"),
    ("he", "heb"),
    ("id", "ind"),
    ("vi", "vie"),
    ("uk", "ukr"),
    ("ro", "ron"),
    ("sr", "srp"),
    ("hr", "hrv"),
    ("fa", "fas"),
    ("is", "isl"),
    ("ca", "cat"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)], streams: &[(&str, &str)]) -> ContainerTags {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let streams: Vec<(String, Option<String>)> = streams
            .iter()
            .map(|(kind, lang)| (kind.to_string(), Some(lang.to_string())))
            .collect();
        ContainerTags::from_ffprobe(&map, &streams)
    }

    #[test]
    fn only_allowlisted_keys_are_kept() {
        let parsed = tags(
            &[
                ("TITLE", "THE GUILTY／ギルティ(字幕版)"),
                ("major_brand", "isom"),
                ("handler_name", "VideoHandler"),
                ("Date", "2019"),
                ("vendor_id", "[0][0][0][0]"),
            ],
            &[("audio", "dan")],
        );
        let json = parsed.to_json();
        assert!(!json.contains("major_brand"));
        assert!(!json.contains("handler_name"));
        assert!(!json.contains("vendor_id"));
        // 大文字小文字は吸収する
        assert_eq!(parsed.title.as_deref(), Some("THE GUILTY／ギルティ(字幕版)"));
        assert_eq!(parsed.year(), Some(2019));
    }

    #[test]
    fn title_is_cleaned_for_search_but_the_raw_value_is_kept() {
        let parsed = tags(&[("title", "THE GUILTY／ギルティ(字幕版)")], &[]);
        assert_eq!(parsed.cleaned_title().as_deref(), Some("THE GUILTY/ギルティ"));
        assert_eq!(parsed.title.as_deref(), Some("THE GUILTY／ギルティ(字幕版)"));

        assert_eq!(clean_title("ＲＥＣ／レック２").as_deref(), Some("REC/レック2"));
        assert_eq!(clean_title("マルティナは海 (字幕版)").as_deref(), Some("マルティナは海"));
        assert_eq!(clean_title("In The Valley Of Elah (字幕版)").as_deref(), Some("In The Valley Of Elah"));
        // 整形して空になるなら None
        assert_eq!(clean_title("（字幕版）"), None);
        assert_eq!(clean_title("   "), None);
    }

    #[test]
    fn distribution_and_cut_editions_are_separated() {
        let sub = tags(&[("title", "レディ・バード (字幕版)")], &[]);
        assert!(sub.cut_editions().is_empty(), "字幕版は作品の違いではない");
        assert_eq!(sub.editions().len(), 1);
        assert_eq!(sub.editions()[0].kind, EditionKind::Distribution);

        let cut = tags(&[("title", "ゴッドファーザー<最終章>: マイケル・コルレオーネの最期")], &[]);
        assert_eq!(cut.cut_editions(), vec!["最終章".to_string()]);

        // 「劇場版」は題名の一部なので版として扱わない
        let theatrical = tags(&[("title", "劇場版 コナン")], &[]);
        assert!(theatrical.editions().is_empty());
    }

    #[test]
    fn cast_is_trimmed_deduplicated_and_capped() {
        let many: Vec<String> = (0..50).map(|i| format!("俳優{i}")).collect();
        let parsed = tags(&[("artist", &format!("{}, 俳優0", many.join(", ")))], &[]);
        let cast = parsed.cast();
        assert_eq!(cast.len(), MAX_CAST_ENTRIES);
        assert_eq!(cast[0], "俳優0");
        assert_eq!(cast.iter().filter(|n| *n == "俳優0").count(), 1);

        // 区切りは , / 、 / ， が混ざる
        let mixed = tags(&[("artist", "ロビン・ウィリアムズ,サリー・フィールド、ピアース・ブロスナン")], &[]);
        assert_eq!(mixed.cast().len(), 3);
    }

    #[test]
    fn languages_are_normalized_and_deduplicated() {
        let parsed = tags(
            &[("title", "x")],
            &[("audio", "ja"), ("audio", "jpn"), ("subtitle", "en"), ("video", "und")],
        );
        assert_eq!(parsed.audio_languages, vec!["jpn".to_string()]);
        assert_eq!(parsed.subtitle_languages, vec!["eng".to_string()]);

        assert_eq!(normalize_language("ja-JP"), Some("jpn"));
        assert_eq!(normalize_language("SPA"), Some("spa"));
        assert_eq!(normalize_language("zxx"), None);
        assert_eq!(to_iso639_1("dan"), Some("da"));
        assert!(is_informative_audio_language("spa"));
        // 吹替で jpn になるので、日本語音声は判断材料にしない
        assert!(!is_informative_audio_language("jpn"));
        assert!(!is_informative_audio_language("und"));
    }

    #[test]
    fn long_values_are_truncated_and_recorded() {
        let long = "あ".repeat(5000);
        let parsed = tags(&[("title", "x"), ("description", &long)], &[]);
        assert!(parsed.truncated.contains(&"description".to_string()));
        assert!(parsed.to_json().len() <= MAX_JSON_BYTES);

        // 上限を超えるタグばかりでも JSON は上限内に収める
        let huge = tags(
            &[
                ("title", &"た".repeat(1000)),
                ("description", &long),
                ("artist", &"俳優、".repeat(500)),
            ],
            &[],
        );
        assert!(huge.to_json().len() <= MAX_JSON_BYTES, "{}", huge.to_json().len());
        assert!(huge.title.is_some(), "タイトルは残す");
    }

    #[test]
    fn provenance_has_four_levels() {
        let provider = tags(
            &[("title", "x"), ("comment", "https://www.disneyplus.com/ja-jp/movies/mrs-doubtfire/Rp")],
            &[],
        );
        assert_eq!(provider.provenance(None), TagProvenance::ProviderKnown);
        assert_eq!(provider.provider_hint(), Some("disney-plus"));

        let pipeline = tags(&[("title", "x"), ("encoder", "Lavf58.76.100")], &[]);
        assert_eq!(pipeline.provenance(None), TagProvenance::PipelineKnown);

        let by_path = tags(&[("title", "x")], &[]);
        assert_eq!(by_path.provenance(None), TagProvenance::Unknown);
        assert_eq!(
            by_path.provenance(Some(r"C:\Users\x\Documents\StreamFab\StreamFab\Output\U-NEXT")),
            TagProvenance::PipelineKnown
        );

        let suspicious = tags(
            &[("title", "x"), ("comment", "https://www.themoviedb.org/movie/348"), ("encoder", "Lavf58")],
            &[],
        );
        assert_eq!(suspicious.provenance(None), TagProvenance::Suspicious);
        assert!(!TagProvenance::Suspicious.is_evaluable());
        assert!(TagProvenance::Unknown.is_evaluable());
    }

    #[test]
    fn episode_id_is_a_provider_specific_rule() {
        let unext = tags(&[("title", "x"), ("episode_id", "-1")], &[]);
        // 出どころが分からなければ判断材料にしない
        assert!(!unext.episode_id_marks_movie(None));
        assert!(unext.episode_id_marks_movie(Some(r"D:\StreamFab\Output\U-NEXT\x.mp4")));

        let other = tags(
            &[("title", "x"), ("episode_id", "-1"), ("comment", "https://www.netflix.com/title/1")],
            &[],
        );
        assert!(!other.episode_id_marks_movie(None), "慣習が確認できた配信元だけに限る");
    }

    #[test]
    fn empty_tags_round_trip_through_json() {
        let empty = ContainerTags::empty();
        assert!(empty.is_empty());
        assert_eq!(empty.to_json(), r#"{"v":1}"#);
        assert_eq!(ContainerTags::from_json(&empty.to_json()), Some(empty));

        let parsed = tags(&[("title", "x"), ("date", "2019-05-25")], &[("audio", "eng")]);
        assert_eq!(ContainerTags::from_json(&parsed.to_json()), Some(parsed.clone()));
        assert_eq!(parsed.year(), Some(2019));
        assert!(!parsed.is_empty());
    }
}
