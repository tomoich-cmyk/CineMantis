use crate::models::match_status::MatchStatus;
use crate::models::tmdb::*;
use crate::services::title_parser::ParsedTitle;
use regex::Regex;
use serde::Serialize;
use std::sync::OnceLock;

/// 自動適用しきい値
pub const THRESHOLD_AUTO: i32 = 75;
/// 候補提示しきい値（これ以上なら candidate として返す）
pub const THRESHOLD_CANDIDATE: i32 = 40;

// ─── スコアリング ─────────────────────────────────────────────────────────────

/// 映画候補にスコアを付ける
pub fn score_movie(result: &TmdbSearchMovie, parsed: &ParsedTitle) -> TmdbCandidate {
    let mut score = 0i32;
    let mut reasons = Vec::<String>::new();

    let query_title = parsed.normalized_title.to_lowercase();
    let result_title = result.title.to_lowercase();
    let result_orig = result
        .original_title
        .as_deref()
        .unwrap_or("")
        .to_lowercase();

    // タイトル一致（language=ja-JP では title が日本語になるため、
    // title と original_title の両方を同等の重みで照合する）
    let title_score = title_match_score(&result_title, &query_title);
    let orig_score  = if result_orig.is_empty() { 0 } else {
        title_match_score(&result_orig, &query_title)
    };
    let best_title_score = title_score.max(orig_score);
    if best_title_score > 0 {
        score += best_title_score;
        if title_score >= orig_score {
            reasons.push(format!("title match({})", best_title_score));
        } else {
            reasons.push(format!("orig_title match({})", best_title_score));
        }
    }
    // クロス言語ボーナス: title と original_title の両方がクエリにマッチする場合
    // 例: "unknown アンノウン" に対して title="アンノウン" と orig="Unknown" が両方マッチ → +10
    if best_title_score > 0 && !result_orig.is_empty() && title_score >= 25 && orig_score >= 25 {
        score += 10;
        reasons.push("cross-lang both matched(+10)".to_string());
    }

    // 年一致
    let result_year = parse_year_from_date(result.release_date.as_deref());
    if let (Some(ry), Some(py)) = (result_year, parsed.year_hint) {
        if ry == py {
            score += 20;
            reasons.push("year exact match".to_string());
        } else if (ry - py).abs() == 1 {
            score += 10;
            reasons.push("year ±1 match".to_string());
        }
    }

    // media_kind 一致（movie → +15）
    if parsed.media_kind == "movie" || parsed.media_kind == "unknown" {
        score += 15;
        reasons.push("media_kind movie".to_string());
    }
    // TV パターンがあるのに movie 候補は減点
    if parsed.media_kind == "tv" {
        score -= 30;
        reasons.push("tv pattern conflicts with movie".to_string());
    }

    let year = result_year.map(|y| y as i32);

    TmdbCandidate {
        tmdb_id: result.id,
        media_type: "movie".to_string(),
        title: result.title.clone(),
        original_title: result.original_title.clone(),
        year,
        poster_path: result.poster_path.clone(),
        overview: result.overview.clone(),
        original_language: result.original_language.clone(),
        confidence: score.clamp(0, 100),
        reasons,
    }
}

/// TV候補にスコアを付ける
pub fn score_tv(result: &TmdbSearchTv, parsed: &ParsedTitle) -> TmdbCandidate {
    let mut score = 0i32;
    let mut reasons = Vec::<String>::new();

    let query_title = parsed.normalized_title.to_lowercase();
    let result_title = result.name.to_lowercase();
    let result_orig = result
        .original_name
        .as_deref()
        .unwrap_or("")
        .to_lowercase();

    // title / original_name 両方を同等の重みで照合
    let title_score = title_match_score(&result_title, &query_title);
    let orig_score  = if result_orig.is_empty() { 0 } else {
        title_match_score(&result_orig, &query_title)
    };
    let best_title_score = title_score.max(orig_score);
    if best_title_score > 0 {
        score += best_title_score;
        if title_score >= orig_score {
            reasons.push(format!("title match({})", best_title_score));
        } else {
            reasons.push(format!("orig_title match({})", best_title_score));
        }
    }
    // クロス言語ボーナス
    if best_title_score > 0 && !result_orig.is_empty() && title_score >= 25 && orig_score >= 25 {
        score += 10;
        reasons.push("cross-lang both matched(+10)".to_string());
    }

    let result_year = parse_year_from_date(result.first_air_date.as_deref());
    if let (Some(ry), Some(py)) = (result_year, parsed.year_hint) {
        if ry == py {
            score += 20;
            reasons.push("year exact match".to_string());
        } else if (ry - py).abs() == 1 {
            score += 10;
            reasons.push("year ±1 match".to_string());
        }
    }

    // TV パターン確認
    if parsed.media_kind == "tv" {
        score += 15;
        reasons.push("tv episode pattern detected".to_string());
    }
    // movie判定なのに tv 候補は少し減点
    if parsed.media_kind == "movie" {
        score -= 20;
        reasons.push("movie pattern conflicts with tv".to_string());
    }

    let year = result_year.map(|y| y as i32);

    TmdbCandidate {
        tmdb_id: result.id,
        media_type: "tv".to_string(),
        title: result.name.clone(),
        original_title: result.original_name.clone(),
        year,
        poster_path: result.poster_path.clone(),
        overview: result.overview.clone(),
        original_language: result.original_language.clone(),
        confidence: score.clamp(0, 100),
        reasons,
    }
}

/// 映画 + TV 候補を混ぜて confidence 降順にソートし、
/// THRESHOLD_CANDIDATE 以上だけを返す
pub fn merge_and_rank(mut candidates: Vec<TmdbCandidate>) -> Vec<TmdbCandidate> {
    candidates.sort_by(|a, b| b.confidence.cmp(&a.confidence));
    candidates
        .into_iter()
        .filter(|c| c.confidence >= THRESHOLD_CANDIDATE)
        .take(10)
        .collect()
}

/// 最良候補を返す（rules-1 の自動適用判定）。
/// 本番の自動照合は rules_safe_best_candidate に切り替えたが、比較基準として挙動は変えない。
#[cfg_attr(not(test), allow(dead_code))]
pub fn best_candidate(candidates: &[TmdbCandidate]) -> Option<&TmdbCandidate> {
    candidates
        .iter()
        .max_by_key(|c| c.confidence)
        .filter(|c| c.confidence >= THRESHOLD_AUTO)
}

// ─── rules-safe ──────────────────────────────────────────────────────────────
//
// score_movie / score_tv / best_candidate は比較基準（rules-1）として挙動を変えない。
// 本番の自動照合は rules_safe_best_candidate を使い、次をすべて満たすときだけ AUTO にする。
//   1. THRESHOLD_AUTO 以上の候補がちょうど1件（同点・複数は不可）
//   2. 照合前入力（title_guess / ファイル名）由来の年ヒントがある
//   3. 候補の年が年ヒントから YEAR_TOLERANCE 以内（score_* の「year ±1」加点と同じ範囲）
//   4. 明示的な矛盾（episode 表記 vs 映画、Part 番号違い）がない
// AUTO にできないが候補がある場合は REVIEW、候補がなければ UNRESOLVED。

/// 年ヒントとの許容差（score_movie / score_tv の「year ±1 match」加点範囲と同じ）
pub const YEAR_TOLERANCE: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SafeDecision {
    Auto,
    Review,
    Unresolved,
}

impl SafeDecision {
    /// works.match_status への対応（既存の CHECK 制約の4値に収める）
    pub fn match_status(self) -> MatchStatus {
        match self {
            SafeDecision::Auto => MatchStatus::Matched,
            SafeDecision::Review => MatchStatus::Pending,
            SafeDecision::Unresolved => MatchStatus::Unmatched,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SafeDecision::Auto => "AUTO",
            SafeDecision::Review => "REVIEW",
            SafeDecision::Unresolved => "UNRESOLVED",
        }
    }
}

/// AUTO を止めた理由コード（UI とログで使う）
pub mod reason {
    pub const NO_CANDIDATES: &str = "NO_CANDIDATES";
    pub const BELOW_THRESHOLD: &str = "BELOW_THRESHOLD";
    pub const MULTIPLE_ABOVE_THRESHOLD: &str = "MULTIPLE_ABOVE_THRESHOLD";
    pub const TIED_TOP: &str = "TIED_TOP";
    pub const NO_YEAR_HINT: &str = "NO_YEAR_HINT";
    pub const CANDIDATE_YEAR_UNKNOWN: &str = "CANDIDATE_YEAR_UNKNOWN";
    pub const YEAR_OUT_OF_RANGE: &str = "YEAR_OUT_OF_RANGE";
    pub const EPISODE_MARKER_VS_MOVIE: &str = "EPISODE_MARKER_VS_MOVIE";
    pub const PART_MISMATCH: &str = "PART_MISMATCH";

    // ─── PR2.5: 埋め込みメタデータ由来（AUTO を止める方向にだけ使う） ──────────
    /// タグの年と候補の年が2年以上違う
    pub const EMBEDDED_YEAR_CONFLICT: &str = "EMBEDDED_YEAR_CONFLICT";
    /// タグの年とファイル名の年が2年以上違う（どちらが正しいか分からない）
    pub const YEAR_SOURCES_DISAGREE: &str = "YEAR_SOURCES_DISAGREE";
    /// 音声の原語と候補の原語が違う（und / jpn のときは判定しない）
    pub const ORIGINAL_LANGUAGE_MISMATCH: &str = "ORIGINAL_LANGUAGE_MISMATCH";
    /// 版の表記（最終章・ディレクターズカットなど）が候補側に無い
    pub const EDITION_MARKER: &str = "EDITION_MARKER";
    /// 配信元の慣習で映画と分かるのに TV 候補
    pub const EPISODE_ID_VS_TV: &str = "EPISODE_ID_VS_TV";
}

/// 埋め込みメタデータ由来の材料。rules-safe では AUTO を止めるためだけに使い、
/// rules-tags-shadow では年ヒントとしても使う。
#[derive(Debug, Clone, Default)]
pub struct EmbeddedEvidence {
    /// タグの年（配信年のことがあるので単独では信用しない）
    pub embedded_year: Option<i32>,
    /// ファイル名・title_guess 由来の年
    pub filename_year: Option<i32>,
    /// 音声ストリームの言語（3文字）
    pub audio_languages: Vec<String>,
    /// 作品そのものが変わりうる版の表記
    pub cut_editions: Vec<String>,
    /// 配信元の慣習で「映画」と分かるか
    pub episode_id_marks_movie: bool,
}

impl EmbeddedEvidence {
    pub fn is_empty(&self) -> bool {
        self.embedded_year.is_none()
            && self.audio_languages.is_empty()
            && self.cut_editions.is_empty()
            && !self.episode_id_marks_movie
    }
}

/// 埋め込みメタデータと候補の矛盾。候補は除外せず、AUTO だけを止める。
pub fn embedded_conflicts(
    candidate: &TmdbCandidate,
    evidence: &EmbeddedEvidence,
) -> Vec<&'static str> {
    let mut conflicts = Vec::new();

    if let (Some(embedded), Some(candidate_year)) = (evidence.embedded_year, candidate.year) {
        if (embedded - candidate_year).abs() > EMBEDDED_YEAR_TOLERANCE {
            conflicts.push(reason::EMBEDDED_YEAR_CONFLICT);
        }
    }
    if let (Some(embedded), Some(filename)) = (evidence.embedded_year, evidence.filename_year) {
        if (embedded - filename).abs() > EMBEDDED_YEAR_TOLERANCE {
            conflicts.push(reason::YEAR_SOURCES_DISAGREE);
        }
    }

    // 音声が日本語・不明のときは吹替の可能性があるので何も判定しない
    let informative: Vec<&str> = evidence
        .audio_languages
        .iter()
        .map(String::as_str)
        .filter(|code| crate::services::container_tags::is_informative_audio_language(code))
        .collect();
    if !informative.is_empty() {
        if let Some(candidate_language) = candidate.original_language.as_deref() {
            let matches = informative.iter().any(|code| {
                crate::services::container_tags::to_iso639_1(code)
                    .map(|iso1| iso1.eq_ignore_ascii_case(candidate_language))
                    .unwrap_or(false)
            });
            if !matches {
                conflicts.push(reason::ORIGINAL_LANGUAGE_MISMATCH);
            }
        }
    }

    // 版の表記が候補のタイトルに見当たらない
    if !evidence.cut_editions.is_empty() {
        let haystack = format!(
            "{} {}",
            candidate.title.to_lowercase(),
            candidate
                .original_title
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
        );
        if !evidence
            .cut_editions
            .iter()
            .any(|label| haystack.contains(&label.to_lowercase()))
        {
            conflicts.push(reason::EDITION_MARKER);
        }
    }

    if evidence.episode_id_marks_movie && candidate.media_type == "tv" {
        conflicts.push(reason::EPISODE_ID_VS_TV);
    }

    conflicts
}

/// タグの年の許容差。±1 は配信年と公開年のずれとして起こりうる
pub const EMBEDDED_YEAR_TOLERANCE: i32 = 1;

#[derive(Debug, Clone)]
pub struct SafeMatchOutcome<'a> {
    pub decision: SafeDecision,
    /// 最上位の候補（同点時は並び順で先の候補）。AUTO のときはこれを適用する
    pub top: Option<&'a TmdbCandidate>,
    pub reasons: Vec<&'static str>,
}

/// rules-safe の判定。`parsed` は照合前入力（title_guess、なければ title）を parse したもの。
/// works.year は TMDB 適用で書き換わるため年ヒントには使わない。
pub fn rules_safe_best_candidate<'a>(
    candidates: &'a [TmdbCandidate],
    parsed: &ParsedTitle,
) -> SafeMatchOutcome<'a> {
    rules_core(candidates, parsed, parsed.year_hint, None)
}

/// 本番の rules-safe に、埋め込みメタデータの矛盾を足したもの。
/// 年ヒントは従来どおりファイル名側だけを使うので、**AUTO が増えることはない**。
/// タグが矛盾を示したときに AUTO を止める（安全側）ためだけに使う。
pub fn rules_safe_with_embedded<'a>(
    candidates: &'a [TmdbCandidate],
    parsed: &ParsedTitle,
    evidence: &EmbeddedEvidence,
) -> SafeMatchOutcome<'a> {
    rules_core(candidates, parsed, parsed.year_hint, Some(evidence))
}

/// rules-tags-shadow（PR2.5 では記録・比較専用）。
/// タグの年を年ヒントとして認めるので、rules-safe より AUTO が増えうる。
/// 本番には適用しない。既存ラベルと監査で精度を確かめてから昇格する。
pub fn rules_tags_shadow_best_candidate<'a>(
    candidates: &'a [TmdbCandidate],
    parsed: &ParsedTitle,
    evidence: &EmbeddedEvidence,
) -> SafeMatchOutcome<'a> {
    let year_hint = parsed.year_hint.or(evidence.embedded_year);
    rules_core(candidates, parsed, year_hint, Some(evidence))
}

fn rules_core<'a>(
    candidates: &'a [TmdbCandidate],
    parsed: &ParsedTitle,
    year_hint: Option<i32>,
    evidence: Option<&EmbeddedEvidence>,
) -> SafeMatchOutcome<'a> {
    let mut top: Option<&TmdbCandidate> = None;
    for candidate in candidates {
        if top.map_or(true, |t| candidate.confidence > t.confidence) {
            top = Some(candidate);
        }
    }
    let Some(top) = top else {
        return SafeMatchOutcome {
            decision: SafeDecision::Unresolved,
            top: None,
            reasons: vec![reason::NO_CANDIDATES],
        };
    };

    let mut reasons = Vec::new();

    match candidates
        .iter()
        .filter(|c| c.confidence >= THRESHOLD_AUTO)
        .count()
    {
        0 => reasons.push(reason::BELOW_THRESHOLD),
        1 => {}
        _ => reasons.push(reason::MULTIPLE_ABOVE_THRESHOLD),
    }
    if candidates
        .iter()
        .filter(|c| c.confidence == top.confidence)
        .count()
        > 1
    {
        reasons.push(reason::TIED_TOP);
    }

    match (year_hint, top.year) {
        (None, _) => reasons.push(reason::NO_YEAR_HINT),
        (Some(_), None) => reasons.push(reason::CANDIDATE_YEAR_UNKNOWN),
        (Some(hint), Some(year)) if (year - hint).abs() > YEAR_TOLERANCE => {
            reasons.push(reason::YEAR_OUT_OF_RANGE)
        }
        _ => {}
    }

    reasons.extend(explicit_conflicts(top, parsed));
    if let Some(evidence) = evidence {
        for conflict in embedded_conflicts(top, evidence) {
            if !reasons.contains(&conflict) {
                reasons.push(conflict);
            }
        }
    }

    SafeMatchOutcome {
        decision: if reasons.is_empty() {
            SafeDecision::Auto
        } else {
            SafeDecision::Review
        },
        top: Some(top),
        reasons,
    }
}

/// 明示的な矛盾。候補は除外せず、AUTO だけを止める。
pub fn explicit_conflicts(candidate: &TmdbCandidate, parsed: &ParsedTitle) -> Vec<&'static str> {
    let mut conflicts = Vec::new();

    // episode 表記（S01E03 / 1x03 / EP03 / 第N話）があるのに映画候補
    // 「第9地区」のような誤検出も含むが、AUTO を止める方向にしか働かない
    if candidate.media_type == "movie" && (parsed.season_no.is_some() || parsed.episode_no.is_some())
    {
        conflicts.push(reason::EPISODE_MARKER_VS_MOVIE);
    }

    // 両側に Part 番号が明示されていて異なる（CD1 / Disc1 はファイル分割なので対象外）
    let local_part = parsed.part_hint.as_deref().and_then(part_number);
    let candidate_part = part_number(&candidate.title).or_else(|| {
        candidate
            .original_title
            .as_deref()
            .and_then(part_number)
    });
    if let (Some(local), Some(remote)) = (local_part, candidate_part) {
        if local != remote {
            conflicts.push(reason::PART_MISMATCH);
        }
    }

    conflicts
}

fn re_part_number() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)(?:part|pt|パート)\.?\s*(\d{1,2}|x|ix|viii|vii|vi|iv|v|iii|ii|i)(?:[^a-z0-9]|$)",
        )
        .unwrap()
    })
}

/// タイトル中の明示的な Part 番号（Part 2 / PART II / 前編 / 後編）
fn part_number(text: &str) -> Option<u32> {
    if text.contains("前編") || text.contains("前篇") || text.contains("上巻") {
        return Some(1);
    }
    if text.contains("後編") || text.contains("後篇") || text.contains("下巻") {
        return Some(2);
    }
    let caps = re_part_number().captures(text)?;
    let token = caps.get(1)?.as_str().to_ascii_lowercase();
    token.parse::<u32>().ok().or_else(|| roman_to_u32(&token))
}

fn roman_to_u32(token: &str) -> Option<u32> {
    Some(match token {
        "i" => 1,
        "ii" => 2,
        "iii" => 3,
        "iv" => 4,
        "v" => 5,
        "vi" => 6,
        "vii" => 7,
        "viii" => 8,
        "ix" => 9,
        "x" => 10,
        _ => return None,
    })
}

// ─── ユーティリティ ───────────────────────────────────────────────────────────

/// タイトル一致スコアを返す
/// 完全一致65 / 正規化一致55 / トークン一致50 / 部分一致25 / 不一致0
/// result_title と query_title はどちらも既に to_lowercase() 済みであること。
///
/// 「トークン一致」: クエリを空白で分割した各トークンが結果タイトルと完全一致する場合。
/// 例: query="unknown アンノウン", result="アンノウン" → "アンノウン" はクエリの1トークン → 50点
/// これにより「英語タイトル + カタカナ表記」形式のフォルダ名でも正しく照合できる。
fn title_match_score(result_title: &str, query_title: &str) -> i32 {
    if result_title == query_title {
        return 65;
    }
    let r_norm = normalize_str(result_title);
    let q_norm = normalize_str(query_title);
    if r_norm == q_norm {
        return 55;
    }
    // トークン一致: クエリの単語として結果タイトルが完全一致（2文字以上）
    if r_norm.chars().count() >= 2 && q_norm.split_whitespace().any(|t| t == r_norm) {
        return 50;
    }
    // 部分一致（一方が他方を含む）
    if r_norm.contains(q_norm.as_str()) || q_norm.contains(r_norm.as_str()) {
        25
    } else {
        0
    }
}

/// 比較用正規化（小文字・記号除去・スペース折りたたみ）
pub fn normalize_str(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn parse_year_from_date(date: Option<&str>) -> Option<i32> {
    date.and_then(|d| d.split('-').next())
        .and_then(|y| y.parse::<i32>().ok())
        .filter(|&y| (1888..=2099).contains(&y))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::services::title_parser::parse_title;

    #[test]
    fn exact_japanese_movie_title_reaches_auto_threshold() {
        let score = title_match_score("アルマゲドン", "アルマゲドン") + 15;
        assert!(score >= THRESHOLD_AUTO);
    }

    // ─── rules-1 回帰（fixture）──────────────────────────────────────────────

    #[derive(serde::Deserialize)]
    struct Rules1Case {
        name: String,
        raw: String,
        #[serde(default)]
        movie_results: Vec<TmdbSearchMovie>,
        #[serde(default)]
        tv_results: Vec<TmdbSearchTv>,
        expected: Vec<ExpectedCandidate>,
        expected_best: Option<i64>,
        expected_safe: String,
    }

    #[derive(serde::Deserialize)]
    struct ExpectedCandidate {
        tmdb_id: i64,
        media_type: String,
        confidence: i32,
    }

    fn fixture_cases() -> Vec<Rules1Case> {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/matcher/rules_v1_baseline.json"
        ))
        .expect("rules_v1_baseline.json が読めること")
    }

    /// fetch_candidates と同じ順序（movie → tv）でスコアを付けて merge_and_rank する
    fn rank_case(case: &Rules1Case) -> (ParsedTitle, Vec<TmdbCandidate>) {
        let parsed = parse_title(&case.raw);
        let mut all: Vec<TmdbCandidate> = case
            .movie_results
            .iter()
            .map(|r| score_movie(r, &parsed))
            .collect();
        all.extend(case.tv_results.iter().map(|r| score_tv(r, &parsed)));
        (parsed, merge_and_rank(all))
    }

    #[test]
    fn rules1_scores_and_best_candidate_match_fixture() {
        for case in fixture_cases() {
            let (_, ranked) = rank_case(&case);
            let actual: Vec<(i64, &str, i32)> = ranked
                .iter()
                .map(|c| (c.tmdb_id, c.media_type.as_str(), c.confidence))
                .collect();
            let expected: Vec<(i64, &str, i32)> = case
                .expected
                .iter()
                .map(|c| (c.tmdb_id, c.media_type.as_str(), c.confidence))
                .collect();
            assert_eq!(actual, expected, "ranked candidates: {}", case.name);
            assert_eq!(
                best_candidate(&ranked).map(|c| c.tmdb_id),
                case.expected_best,
                "best_candidate: {}",
                case.name
            );
        }
    }

    #[test]
    fn rules_safe_decisions_match_fixture() {
        for case in fixture_cases() {
            let (parsed, ranked) = rank_case(&case);
            let outcome = rules_safe_best_candidate(&ranked, &parsed);
            assert_eq!(
                outcome.decision.as_str(),
                case.expected_safe,
                "rules-safe: {} reasons={:?}",
                case.name,
                outcome.reasons
            );
        }
    }

    // ─── rules-safe ─────────────────────────────────────────────────────────

    fn cand(tmdb_id: i64, confidence: i32, year: Option<i32>) -> TmdbCandidate {
        TmdbCandidate {
            tmdb_id,
            media_type: "movie".to_string(),
            title: format!("Movie {tmdb_id}"),
            original_title: None,
            year,
            poster_path: None,
            overview: None,
            original_language: None,
            confidence,
            reasons: Vec::new(),
        }
    }

    fn with_year() -> ParsedTitle {
        parse_title("Some.Movie.2010.1080p.mkv")
    }

    #[test]
    fn exact_title_without_year_is_not_auto() {
        let parsed = parse_title("アルマゲドン.mp4");
        let movie = TmdbSearchMovie {
            id: 95,
            title: "アルマゲドン".to_string(),
            original_title: Some("Armageddon".to_string()),
            overview: None,
            release_date: Some("1998-07-01".to_string()),
            poster_path: None,
            original_language: None,
            vote_average: None,
            genre_ids: None,
        };
        let ranked = merge_and_rank(vec![score_movie(&movie, &parsed)]);
        assert!(best_candidate(&ranked).is_some(), "rules-1 は AUTO する");
        let outcome = rules_safe_best_candidate(&ranked, &parsed);
        assert_eq!(outcome.decision, SafeDecision::Review);
        assert!(outcome.reasons.contains(&reason::NO_YEAR_HINT));
    }

    #[test]
    fn tied_top_candidates_are_not_auto() {
        let candidates = vec![cand(1, 80, Some(2010)), cand(2, 80, Some(2010))];
        let outcome = rules_safe_best_candidate(&candidates, &with_year());
        assert_eq!(outcome.decision, SafeDecision::Review);
        assert!(outcome.reasons.contains(&reason::MULTIPLE_ABOVE_THRESHOLD));
        assert!(outcome.reasons.contains(&reason::TIED_TOP));
        assert_eq!(outcome.top.map(|c| c.tmdb_id), Some(1));
    }

    #[test]
    fn two_candidates_above_threshold_are_not_auto() {
        let candidates = vec![cand(1, 90, Some(2010)), cand(2, 80, Some(2010))];
        let outcome = rules_safe_best_candidate(&candidates, &with_year());
        assert_eq!(outcome.decision, SafeDecision::Review);
        assert_eq!(outcome.reasons, vec![reason::MULTIPLE_ABOVE_THRESHOLD]);
    }

    #[test]
    fn single_candidate_above_threshold_with_matching_year_is_auto() {
        let candidates = vec![cand(1, 90, Some(2010)), cand(2, 70, Some(2010))];
        let outcome = rules_safe_best_candidate(&candidates, &with_year());
        assert_eq!(outcome.decision, SafeDecision::Auto);
        assert!(outcome.reasons.is_empty());
        assert_eq!(outcome.top.map(|c| c.tmdb_id), Some(1));
    }

    #[test]
    fn year_within_tolerance_is_auto_and_beyond_is_not() {
        let within = vec![cand(1, 90, Some(2011))];
        assert_eq!(
            rules_safe_best_candidate(&within, &with_year()).decision,
            SafeDecision::Auto
        );

        let beyond = vec![cand(1, 90, Some(2012))];
        let outcome = rules_safe_best_candidate(&beyond, &with_year());
        assert_eq!(outcome.decision, SafeDecision::Review);
        assert_eq!(outcome.reasons, vec![reason::YEAR_OUT_OF_RANGE]);

        let unknown = vec![cand(1, 90, None)];
        let outcome = rules_safe_best_candidate(&unknown, &with_year());
        assert_eq!(outcome.decision, SafeDecision::Review);
        assert_eq!(outcome.reasons, vec![reason::CANDIDATE_YEAR_UNKNOWN]);
    }

    #[test]
    fn candidates_below_threshold_are_review() {
        let candidates = vec![cand(1, 60, Some(2010))];
        let outcome = rules_safe_best_candidate(&candidates, &with_year());
        assert_eq!(outcome.decision, SafeDecision::Review);
        assert_eq!(outcome.reasons, vec![reason::BELOW_THRESHOLD]);
    }

    #[test]
    fn no_candidates_is_unresolved() {
        let outcome = rules_safe_best_candidate(&[], &with_year());
        assert_eq!(outcome.decision, SafeDecision::Unresolved);
        assert!(outcome.top.is_none());
        assert_eq!(outcome.reasons, vec![reason::NO_CANDIDATES]);
    }

    #[test]
    fn episode_marker_blocks_auto_for_movie_candidate() {
        let parsed = parse_title("Some.Show.2010.S01E03.mkv");
        let candidates = vec![cand(1, 90, Some(2010))];
        let outcome = rules_safe_best_candidate(&candidates, &parsed);
        assert_eq!(outcome.decision, SafeDecision::Review);
        assert!(outcome.reasons.contains(&reason::EPISODE_MARKER_VS_MOVIE));
    }

    #[test]
    fn explicit_part_mismatch_blocks_auto() {
        let parsed = parse_title("Harry Potter Deathly Hallows Part 1 (2010).mkv");
        assert_eq!(parsed.part_hint.as_deref(), Some("part 1"));
        let mut part2 = cand(12445, 90, Some(2011));
        part2.title = "ハリー・ポッターと死の秘宝 PART2".to_string();
        let outcome = rules_safe_best_candidate(std::slice::from_ref(&part2), &parsed);
        assert!(outcome.reasons.contains(&reason::PART_MISMATCH));

        let mut part1 = cand(12444, 90, Some(2010));
        part1.original_title = Some("Harry Potter and the Deathly Hallows: Part 1".to_string());
        let outcome = rules_safe_best_candidate(std::slice::from_ref(&part1), &parsed);
        assert!(!outcome.reasons.contains(&reason::PART_MISMATCH));
    }

    #[test]
    fn part_numbers_are_read_from_titles() {
        assert_eq!(part_number("Kill Bill: Vol. 1"), None);
        assert_eq!(part_number("Dune: Part Two"), None);
        assert_eq!(part_number("Part II"), Some(2));
        assert_eq!(part_number("死の秘宝 PART1"), Some(1));
        assert_eq!(part_number("るろうに剣心 京都大火編 前編"), Some(1));
        assert_eq!(part_number("cd1"), None);
    }

    // ─── PR2.5: 埋め込みメタデータ ───────────────────────────────────────────

    fn evidence_with_year(embedded: Option<i32>, filename: Option<i32>) -> EmbeddedEvidence {
        EmbeddedEvidence {
            embedded_year: embedded,
            filename_year: filename,
            ..Default::default()
        }
    }

    /// PR2.5 の最重要事項: タグがあっても本番の AUTO 範囲は広がらない
    #[test]
    fn embedded_year_never_widens_production_auto() {
        // 年ヒントがファイル名に無い作品（今までは REVIEW）
        let parsed = parse_title("Alien.mkv");
        let candidates = vec![cand(348, 95, Some(1979))];
        let evidence = evidence_with_year(Some(1979), None);

        let production = rules_safe_with_embedded(&candidates, &parsed, &evidence);
        assert_eq!(production.decision, SafeDecision::Review);
        assert!(production.reasons.contains(&reason::NO_YEAR_HINT));

        // 影判定はタグの年を年ヒントとして認めるので AUTO になる（記録だけ）
        let shadow = rules_tags_shadow_best_candidate(&candidates, &parsed, &evidence);
        assert_eq!(shadow.decision, SafeDecision::Auto);
        assert!(shadow.reasons.is_empty());
    }

    /// 安全側（AUTO を止める）には使ってよい
    #[test]
    fn embedded_year_conflict_stops_auto() {
        let parsed = parse_title("Niagara.2013.mkv");
        let candidates = vec![cand(1, 95, Some(2013))];
        let evidence = evidence_with_year(Some(1953), Some(2013));

        // タグが無ければ AUTO
        assert_eq!(
            rules_safe_best_candidate(&candidates, &parsed).decision,
            SafeDecision::Auto
        );
        // タグの年が候補ともファイル名とも食い違う → REVIEW
        let outcome = rules_safe_with_embedded(&candidates, &parsed, &evidence);
        assert_eq!(outcome.decision, SafeDecision::Review);
        assert!(outcome.reasons.contains(&reason::EMBEDDED_YEAR_CONFLICT));
        assert!(outcome.reasons.contains(&reason::YEAR_SOURCES_DISAGREE));

        // ±1 は配信年のずれとして許す
        let close = evidence_with_year(Some(2012), Some(2013));
        assert_eq!(
            rules_safe_with_embedded(&candidates, &parsed, &close).decision,
            SafeDecision::Auto
        );
    }

    #[test]
    fn original_language_mismatch_only_counts_for_informative_audio() {
        let parsed = parse_title("The.Guilty.2019.mkv");
        let mut candidate = cand(1, 95, Some(2019));
        candidate.original_language = Some("en".to_string());

        let danish = EmbeddedEvidence {
            audio_languages: vec!["dan".to_string()],
            ..Default::default()
        };
        let outcome = rules_safe_with_embedded(std::slice::from_ref(&candidate), &parsed, &danish);
        assert!(outcome.reasons.contains(&reason::ORIGINAL_LANGUAGE_MISMATCH));

        // 吹替で jpn になるので、日本語音声は判断材料にしない
        let dubbed = EmbeddedEvidence {
            audio_languages: vec!["jpn".to_string(), "und".to_string()],
            ..Default::default()
        };
        let outcome = rules_safe_with_embedded(std::slice::from_ref(&candidate), &parsed, &dubbed);
        assert!(!outcome.reasons.contains(&reason::ORIGINAL_LANGUAGE_MISMATCH));
        assert_eq!(outcome.decision, SafeDecision::Auto);

        // 一致していれば止めない
        let english = EmbeddedEvidence {
            audio_languages: vec!["eng".to_string()],
            ..Default::default()
        };
        let outcome = rules_safe_with_embedded(std::slice::from_ref(&candidate), &parsed, &english);
        assert!(!outcome.reasons.contains(&reason::ORIGINAL_LANGUAGE_MISMATCH));
    }

    #[test]
    fn cut_edition_without_a_matching_candidate_stops_auto() {
        let parsed = parse_title("Godfather.1990.mkv");
        let mut candidate = cand(1, 95, Some(1990));
        candidate.title = "ゴッドファーザー PART III".to_string();
        let evidence = EmbeddedEvidence {
            cut_editions: vec!["最終章".to_string()],
            ..Default::default()
        };
        let outcome = rules_safe_with_embedded(std::slice::from_ref(&candidate), &parsed, &evidence);
        assert!(outcome.reasons.contains(&reason::EDITION_MARKER));

        // 候補側にも同じ表記があれば止めない
        candidate.title = "ゴッドファーザー 最終章".to_string();
        let outcome = rules_safe_with_embedded(std::slice::from_ref(&candidate), &parsed, &evidence);
        assert!(!outcome.reasons.contains(&reason::EDITION_MARKER));
    }

    #[test]
    fn provider_specific_episode_id_conflicts_with_tv_candidates() {
        let parsed = parse_title("Something.2019.mkv");
        let mut tv = cand(1, 95, Some(2019));
        tv.media_type = "tv".to_string();
        let movie = cand(2, 95, Some(2019));
        let evidence = EmbeddedEvidence {
            episode_id_marks_movie: true,
            ..Default::default()
        };

        let outcome = rules_safe_with_embedded(std::slice::from_ref(&tv), &parsed, &evidence);
        assert!(outcome.reasons.contains(&reason::EPISODE_ID_VS_TV));
        let outcome = rules_safe_with_embedded(std::slice::from_ref(&movie), &parsed, &evidence);
        assert!(!outcome.reasons.contains(&reason::EPISODE_ID_VS_TV));

        // 出どころが分からない（フラグが立たない）なら何もしない
        let unknown = EmbeddedEvidence::default();
        let outcome = rules_safe_with_embedded(std::slice::from_ref(&tv), &parsed, &unknown);
        assert!(!outcome.reasons.contains(&reason::EPISODE_ID_VS_TV));
    }

    /// タグが無い作品では PR2 と同じ判定になる
    #[test]
    fn works_without_tags_decide_exactly_as_before() {
        let parsed = parse_title("Blade.Runner.2049.2017.mkv");
        for candidates in [vec![], vec![cand(1, 95, Some(2017))], vec![cand(1, 80, Some(2017)), cand(2, 80, Some(2017))]] {
            let before = rules_safe_best_candidate(&candidates, &parsed);
            let after = rules_safe_with_embedded(&candidates, &parsed, &EmbeddedEvidence::default());
            assert_eq!(before.decision, after.decision);
            assert_eq!(before.reasons, after.reasons);
            assert_eq!(before.top.map(|c| c.tmdb_id), after.top.map(|c| c.tmdb_id));
        }
    }

    #[test]
    fn decisions_map_to_existing_match_status_values() {
        assert_eq!(SafeDecision::Auto.match_status(), MatchStatus::Matched);
        assert_eq!(SafeDecision::Review.match_status(), MatchStatus::Pending);
        assert_eq!(SafeDecision::Unresolved.match_status(), MatchStatus::Unmatched);
    }
}
