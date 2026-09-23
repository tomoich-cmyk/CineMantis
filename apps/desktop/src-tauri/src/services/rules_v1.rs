//! rules-1（比較基準）の凍結実装。
//!
//! PR3 以降で parser・候補生成・補完を変えても、比較基準である rules-1 の結果が
//! 動かないようにするため、rules-1 が必要とする処理をこのモジュールに**複製**して閉じ込める。
//!
//! したがって、ここから
//!   - `services::title_parser`
//!   - `services::metadata_matcher`
//! を呼んではいけない（呼ぶと共有実装の変更が比較基準に波及する）。
//! TMDB の応答も、共有 DTO をそのまま持ち回らず [`RawMovieV1`] / [`RawTvV1`] へ写してから使う。
//!
//! 凍結しているのは「判定に使うロジック」だけで、TMDB を呼ぶのは本番経路のままである。
//! rules-1 は旧経路（ファイル名・title_guess 由来）の検索結果だけを見る。
//! 埋め込みメタデータのタイトル・年・統合候補集合は一切使わない。
//!
//! アルゴリズムは PR2.5 時点の `metadata_matcher` / `title_parser` と同じ。
//! `RULES_V1_VERSION` は据え置き（物理的な凍結であって、挙動の変更ではない）。

use regex::Regex;
use serde::Serialize;
use std::sync::OnceLock;

/// 自動適用しきい値（凍結）
pub const THRESHOLD_AUTO_V1: i32 = 75;
/// 候補として返すしきい値（凍結）
pub const THRESHOLD_CANDIDATE_V1: i32 = 40;
/// merge 後に残す最大件数（凍結）
const MAX_RANKED_V1: usize = 10;

// ─── TMDB 応答の写し（共有 DTO の変更から切り離す） ──────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct RawMovieV1 {
    pub id: i64,
    pub title: String,
    pub original_title: Option<String>,
    pub release_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawTvV1 {
    pub id: i64,
    pub name: String,
    pub original_name: Option<String>,
    pub first_air_date: Option<String>,
}

impl From<&crate::models::tmdb::TmdbSearchMovie> for RawMovieV1 {
    fn from(result: &crate::models::tmdb::TmdbSearchMovie) -> Self {
        RawMovieV1 {
            id: result.id,
            title: result.title.clone(),
            original_title: result.original_title.clone(),
            release_date: result.release_date.clone(),
        }
    }
}

impl From<&crate::models::tmdb::TmdbSearchTv> for RawTvV1 {
    fn from(result: &crate::models::tmdb::TmdbSearchTv) -> Self {
        RawTvV1 {
            id: result.id,
            name: result.name.clone(),
            original_name: result.original_name.clone(),
            first_air_date: result.first_air_date.clone(),
        }
    }
}

// ─── 凍結した正規表現 ────────────────────────────────────────────────────────

fn re_episode_v1() -> &'static Regex {
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
            )",
        )
        .unwrap()
    })
}

fn re_season_only_v1() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?xi) (?:[Ss]eason|Season|シーズン)\s*(\d{1,2})").unwrap())
}

fn re_year_v1() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\b(19\d{2}|20[012]\d)\b").unwrap())
}

fn re_part_v1() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?xi)
            (?:
              [Cc][Dd]\s*(\d)
            | [Dd]isc\s*(\d)
            | [Pp]art\s*(\d)
            | (?:前編|後編|上巻|下巻|前部|後部)
            | [Pp][Tt]\s*(\d)
            )",
        )
        .unwrap()
    })
}

fn re_junk_v1() -> &'static Regex {
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
            | \[.*?\]
            ",
        )
        .unwrap()
    })
}

fn re_separator_v1() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[._\-]+").unwrap())
}

// ─── 凍結した parser ─────────────────────────────────────────────────────────

/// rules-1 が使う解析結果（`ParsedTitle` とは別の型にして共有実装から切り離す）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParsedTitleV1 {
    pub raw: String,
    pub normalized_title: String,
    /// "movie" | "tv" | "unknown"
    pub media_kind: String,
    pub season_no: Option<i32>,
    pub episode_no: Option<i32>,
    pub year_hint: Option<i32>,
    pub part_hint: Option<String>,
}

/// rules-1 の title parsing（凍結）
pub fn parse_title_v1(file_name: &str) -> ParsedTitleV1 {
    let without_ext = remove_extension_v1(file_name);

    let year_hint = extract_year_v1(&without_ext);
    let (season_no, episode_no) = extract_episode_v1(&without_ext);
    let part_hint = extract_part_v1(&without_ext);

    let media_kind = if season_no.is_some() || episode_no.is_some() {
        "tv"
    } else {
        "unknown"
    };

    ParsedTitleV1 {
        raw: file_name.to_string(),
        normalized_title: normalize_title_v1(&without_ext),
        media_kind: media_kind.to_string(),
        season_no,
        episode_no,
        year_hint,
        part_hint,
    }
}

fn remove_extension_v1(file_name: &str) -> String {
    match file_name.rfind('.') {
        Some(pos) if pos > 0 => file_name[..pos].to_string(),
        _ => file_name.to_string(),
    }
}

/// rules-1 の year 解析（凍結）
pub fn extract_year_v1(value: &str) -> Option<i32> {
    re_year_v1()
        .find(value)
        .and_then(|m| m.as_str().parse::<i32>().ok())
        .filter(|&y| (1888..=2099).contains(&y))
}

fn extract_episode_v1(value: &str) -> (Option<i32>, Option<i32>) {
    let Some(caps) = re_episode_v1().captures(value) else {
        return (None, None);
    };
    if let (Some(s), Some(e)) = (caps.get(1), caps.get(2)) {
        return (s.as_str().parse().ok(), e.as_str().parse().ok());
    }
    if let (Some(s), Some(e)) = (caps.get(3), caps.get(4)) {
        return (s.as_str().parse().ok(), e.as_str().parse().ok());
    }
    if let (Some(s), Some(e)) = (caps.get(5), caps.get(6)) {
        return (s.as_str().parse().ok(), e.as_str().parse().ok());
    }
    if let Some(e) = caps.get(7).or(caps.get(8)) {
        return (Some(1), e.as_str().parse().ok());
    }
    (None, None)
}

fn extract_part_v1(value: &str) -> Option<String> {
    re_part_v1().find(value).map(|m| m.as_str().to_lowercase())
}

/// (タグ) を除去するが (1999) のような4桁年号は残す（凍結）
fn remove_paren_tags_v1(value: &str) -> String {
    let mut result = String::new();
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '(' {
            let mut inner = String::new();
            let mut closed = false;
            for d in chars.by_ref() {
                if d == ')' {
                    closed = true;
                    break;
                }
                inner.push(d);
            }
            if closed {
                let trimmed = inner.trim();
                if trimmed.len() == 4 && trimmed.chars().all(|x| x.is_ascii_digit()) {
                    result.push('(');
                    result.push_str(trimmed);
                    result.push(')');
                } else {
                    result.push(' ');
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// rules-1 の検索語生成（凍結）。検索に投げるのはこの文字列
pub fn normalize_title_v1(value: &str) -> String {
    let no_ep = re_episode_v1().replace_all(value, " ");
    let no_year = re_year_v1().replace_all(&no_ep, " ");
    let no_junk = re_junk_v1().replace_all(&no_year, " ");
    let no_paren = remove_paren_tags_v1(&no_junk);
    let no_season = re_season_only_v1().replace_all(&no_paren, " ");
    let no_part = re_part_v1().replace_all(&no_season, " ");
    let spaced = re_separator_v1().replace_all(&no_part, " ");
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ─── 凍結した scorer ─────────────────────────────────────────────────────────

/// rules-1 が出した候補
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CandidateV1 {
    pub tmdb_id: i64,
    /// "movie" | "tv"
    pub media_type: String,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub confidence: i32,
    pub reasons: Vec<String>,
}

/// rules-1 の文字列正規化（凍結）
pub fn normalize_str_v1(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn title_match_score_v1(result_title: &str, query_title: &str) -> i32 {
    if result_title == query_title {
        return 65;
    }
    let r_norm = normalize_str_v1(result_title);
    let q_norm = normalize_str_v1(query_title);
    if r_norm == q_norm {
        return 55;
    }
    if r_norm.chars().count() >= 2 && q_norm.split_whitespace().any(|t| t == r_norm) {
        return 50;
    }
    if r_norm.contains(q_norm.as_str()) || q_norm.contains(r_norm.as_str()) {
        25
    } else {
        0
    }
}

fn parse_year_from_date_v1(date: Option<&str>) -> Option<i32> {
    date.and_then(|d| d.split('-').next())
        .and_then(|y| y.parse::<i32>().ok())
        .filter(|&y| (1888..=2099).contains(&y))
}

/// 映画候補の採点（凍結）
pub fn score_movie_v1(result: &RawMovieV1, parsed: &ParsedTitleV1) -> CandidateV1 {
    let mut score = 0i32;
    let mut reasons = Vec::<String>::new();

    let query_title = parsed.normalized_title.to_lowercase();
    let result_title = result.title.to_lowercase();
    let result_orig = result
        .original_title
        .as_deref()
        .unwrap_or("")
        .to_lowercase();

    let title_score = title_match_score_v1(&result_title, &query_title);
    let orig_score = if result_orig.is_empty() {
        0
    } else {
        title_match_score_v1(&result_orig, &query_title)
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
    if best_title_score > 0 && !result_orig.is_empty() && title_score >= 25 && orig_score >= 25 {
        score += 10;
        reasons.push("cross-lang both matched(+10)".to_string());
    }

    let result_year = parse_year_from_date_v1(result.release_date.as_deref());
    if let (Some(ry), Some(py)) = (result_year, parsed.year_hint) {
        if ry == py {
            score += 20;
            reasons.push("year exact match".to_string());
        } else if (ry - py).abs() == 1 {
            score += 10;
            reasons.push("year ±1 match".to_string());
        }
    }

    if parsed.media_kind == "movie" || parsed.media_kind == "unknown" {
        score += 15;
        reasons.push("media_kind movie".to_string());
    }
    if parsed.media_kind == "tv" {
        score -= 30;
        reasons.push("tv pattern conflicts with movie".to_string());
    }

    CandidateV1 {
        tmdb_id: result.id,
        media_type: "movie".to_string(),
        title: result.title.clone(),
        original_title: result.original_title.clone(),
        year: result_year,
        confidence: score.clamp(0, 100),
        reasons,
    }
}

/// TV 候補の採点（凍結）
pub fn score_tv_v1(result: &RawTvV1, parsed: &ParsedTitleV1) -> CandidateV1 {
    let mut score = 0i32;
    let mut reasons = Vec::<String>::new();

    let query_title = parsed.normalized_title.to_lowercase();
    let result_title = result.name.to_lowercase();
    let result_orig = result.original_name.as_deref().unwrap_or("").to_lowercase();

    let title_score = title_match_score_v1(&result_title, &query_title);
    let orig_score = if result_orig.is_empty() {
        0
    } else {
        title_match_score_v1(&result_orig, &query_title)
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
    if best_title_score > 0 && !result_orig.is_empty() && title_score >= 25 && orig_score >= 25 {
        score += 10;
        reasons.push("cross-lang both matched(+10)".to_string());
    }

    let result_year = parse_year_from_date_v1(result.first_air_date.as_deref());
    if let (Some(ry), Some(py)) = (result_year, parsed.year_hint) {
        if ry == py {
            score += 20;
            reasons.push("year exact match".to_string());
        } else if (ry - py).abs() == 1 {
            score += 10;
            reasons.push("year ±1 match".to_string());
        }
    }

    if parsed.media_kind == "tv" {
        score += 15;
        reasons.push("tv episode pattern detected".to_string());
    }
    if parsed.media_kind == "movie" {
        score -= 20;
        reasons.push("movie pattern conflicts with tv".to_string());
    }

    CandidateV1 {
        tmdb_id: result.id,
        media_type: "tv".to_string(),
        title: result.name.clone(),
        original_title: result.original_name.clone(),
        year: result_year,
        confidence: score.clamp(0, 100),
        reasons,
    }
}

/// 候補の統合と順位付け（凍結）
pub fn merge_and_rank_v1(mut candidates: Vec<CandidateV1>) -> Vec<CandidateV1> {
    candidates.sort_by(|a, b| b.confidence.cmp(&a.confidence));
    candidates
        .into_iter()
        .filter(|c| c.confidence >= THRESHOLD_CANDIDATE_V1)
        .take(MAX_RANKED_V1)
        .collect()
}

/// 最良候補（凍結）。同点のときは並び順で後ろの候補を返す（`max_by_key` の挙動）
pub fn best_candidate_v1(candidates: &[CandidateV1]) -> Option<&CandidateV1> {
    candidates
        .iter()
        .max_by_key(|c| c.confidence)
        .filter(|c| c.confidence >= THRESHOLD_AUTO_V1)
}

// ─── 候補生成（旧経路のみ） ──────────────────────────────────────────────────

/// rules-1 が投げるはずの検索（凍結）。本番の検索計画と一致するかの照合に使う
#[derive(Debug, Clone, PartialEq)]
pub struct SearchPlanV1 {
    pub query: String,
    pub year: Option<i32>,
    pub search_movie: bool,
    pub search_tv: bool,
    /// 年ヒントがあるときは年なし検索も追加する（旧経路の挙動）
    pub movie_retry_without_year: bool,
}

/// rules-1 の検索計画。`media_kind` はソースの設定（movie / tv / unknown）
pub fn search_plan_v1(raw_title: &str, media_kind: &str) -> SearchPlanV1 {
    let parsed = parse_title_v1(raw_title);
    SearchPlanV1 {
        query: parsed.normalized_title,
        year: parsed.year_hint,
        search_movie: media_kind != "tv",
        search_tv: media_kind != "movie",
        movie_retry_without_year: parsed.year_hint.is_some(),
    }
}

/// rules-1 に渡す入力（旧経路の検索結果だけ）
pub struct RulesV1Input<'a> {
    /// 旧経路の検索語のもとになった文字列（title_guess、なければ元ファイル名由来）
    pub raw_title: &'a str,
    /// ソースの media_kind（movie / tv / unknown）
    pub media_kind: &'a str,
    /// 旧経路の映画検索の生結果
    pub movies: &'a [RawMovieV1],
    /// 旧経路の TV 検索の生結果
    pub tv: &'a [RawTvV1],
    /// 本番が実際に投げた検索語（凍結側とずれていないかの確認用）
    pub production_query: Option<&'a str>,
}

/// 本番が投げた検索語が凍結した検索語と違っていた。
///
/// PR3 以降、旧経路の検索は `search_plan_v1` が支配するので通常は立たない。
/// 呼び出し側が別の検索語を使うように変わった場合の診断用として残す。
pub const NOTE_QUERY_DIVERGED: &str = "QUERY_DIVERGED_FROM_FROZEN";

#[derive(Debug, Clone, PartialEq)]
pub struct RulesV1Outcome {
    /// "AUTO" | "UNRESOLVED"（rules-1 に REVIEW は無い）
    pub decision: &'static str,
    pub top: Option<CandidateV1>,
    pub ranked: Vec<CandidateV1>,
    pub parsed: ParsedTitleV1,
    pub plan: SearchPlanV1,
    pub notes: Vec<&'static str>,
}

/// rules-1 の判定。旧経路の検索結果だけを、凍結した parser / scorer で評価する。
/// TMDB を呼ばない（本番が取得済みの応答を受け取る）。
pub fn evaluate(input: &RulesV1Input) -> RulesV1Outcome {
    let parsed = parse_title_v1(input.raw_title);
    let plan = search_plan_v1(input.raw_title, input.media_kind);

    let mut all: Vec<CandidateV1> = input
        .movies
        .iter()
        .map(|movie| score_movie_v1(movie, &parsed))
        .collect();
    all.extend(input.tv.iter().map(|tv| score_tv_v1(tv, &parsed)));
    let ranked = merge_and_rank_v1(all);

    let top = best_candidate_v1(&ranked).cloned();
    let decision = if top.is_some() { "AUTO" } else { "UNRESOLVED" };

    let mut notes = Vec::new();
    if let Some(production_query) = input.production_query {
        if production_query != plan.query {
            // 凍結側と本番側の検索語がずれた。rules-1 は本番が取った候補しか見られないので、
            // この run の rules-1 verdict は厳密な比較基準ではないことを記録する
            notes.push(NOTE_QUERY_DIVERGED);
        }
    }

    RulesV1Outcome {
        decision,
        top,
        ranked,
        parsed,
        plan,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::tmdb::{TmdbSearchMovie, TmdbSearchTv};

    // ─── golden fixture（PR1 から変更しない） ───────────────────────────────

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
        #[allow(dead_code)]
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

    fn evaluate_case(case: &Rules1Case) -> RulesV1Outcome {
        let movies: Vec<RawMovieV1> = case.movie_results.iter().map(RawMovieV1::from).collect();
        let tv: Vec<RawTvV1> = case.tv_results.iter().map(RawTvV1::from).collect();
        evaluate(&RulesV1Input {
            raw_title: &case.raw,
            media_kind: "unknown",
            movies: &movies,
            tv: &tv,
            production_query: None,
        })
    }

    /// 凍結実装でも golden fixture の点数・順位・best が一致すること
    #[test]
    fn frozen_rules_v1_matches_the_golden_fixture() {
        for case in fixture_cases() {
            let outcome = evaluate_case(&case);
            let actual: Vec<(i64, &str, i32)> = outcome
                .ranked
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
                outcome.top.as_ref().map(|c| c.tmdb_id),
                case.expected_best,
                "best_candidate: {}",
                case.name
            );
            assert_eq!(
                outcome.decision,
                if case.expected_best.is_some() { "AUTO" } else { "UNRESOLVED" },
                "decision: {}",
                case.name
            );
        }
    }

    /// 凍結した時点（PR3 冒頭）で、共有実装と差が無かったことの証拠。
    ///
    /// 共有の parser / scorer を意図的に改善すると、**正しい変更でも**このテストは落ちる。
    /// そのため通常のテスト実行からは外してある（`cargo test -- --ignored` で実行できる）。
    /// rules-1 の継続的な回帰保証は次の3つを正とする。
    ///   - golden fixture `rules_v1_baseline.json`
    ///   - `representative_cases_are_pinned`
    ///   - `search_plan_follows_the_legacy_path`（凍結した検索計画）
    #[test]
    #[ignore]
    fn frozen_implementation_matches_the_shared_one_at_freeze_time() {
        use crate::services::metadata_matcher::{best_candidate, merge_and_rank, score_movie, score_tv};
        use crate::services::title_parser::parse_title;

        for case in fixture_cases() {
            let shared_parsed = parse_title(&case.raw);
            let mut shared: Vec<crate::models::tmdb::TmdbCandidate> = case
                .movie_results
                .iter()
                .map(|r| score_movie(r, &shared_parsed))
                .collect();
            shared.extend(case.tv_results.iter().map(|r| score_tv(r, &shared_parsed)));
            let shared_ranked = merge_and_rank(shared);

            let frozen = evaluate_case(&case);
            let frozen_parsed = &frozen.parsed;

            assert_eq!(frozen_parsed.normalized_title, shared_parsed.normalized_title, "{}", case.name);
            assert_eq!(frozen_parsed.year_hint, shared_parsed.year_hint, "{}", case.name);
            assert_eq!(frozen_parsed.media_kind, shared_parsed.media_kind, "{}", case.name);
            assert_eq!(frozen_parsed.season_no, shared_parsed.season_no, "{}", case.name);
            assert_eq!(frozen_parsed.episode_no, shared_parsed.episode_no, "{}", case.name);
            assert_eq!(frozen_parsed.part_hint, shared_parsed.part_hint, "{}", case.name);

            let frozen_scores: Vec<(i64, String, i32, Vec<String>)> = frozen
                .ranked
                .iter()
                .map(|c| (c.tmdb_id, c.media_type.clone(), c.confidence, c.reasons.clone()))
                .collect();
            let shared_scores: Vec<(i64, String, i32, Vec<String>)> = shared_ranked
                .iter()
                .map(|c| (c.tmdb_id, c.media_type.clone(), c.confidence, c.reasons.clone()))
                .collect();
            assert_eq!(frozen_scores, shared_scores, "scores/rank: {}", case.name);
            assert_eq!(
                frozen.top.as_ref().map(|c| c.tmdb_id),
                best_candidate(&shared_ranked).map(|c| c.tmdb_id),
                "best: {}",
                case.name
            );
        }
    }

    // ─── 代表ケースの固定（PR2.5 時点の値） ──────────────────────────────────

    /// parsed title / year hint / media kind / score / rank / best / AUTO を固定する
    #[test]
    fn representative_cases_are_pinned() {
        // 年ヒント無し・邦題一致 → rules-1 は AUTO
        let outcome = evaluate(&RulesV1Input {
            raw_title: "アルマゲドン.mp4",
            media_kind: "unknown",
            movies: &[
                RawMovieV1 {
                    id: 95,
                    title: "アルマゲドン".into(),
                    original_title: Some("Armageddon".into()),
                    release_date: Some("1998-07-01".into()),
                },
                RawMovieV1 {
                    id: 1001,
                    title: "アルマゲドン2".into(),
                    original_title: Some("Armageddon 2".into()),
                    release_date: Some("2010-01-01".into()),
                },
            ],
            tv: &[],
            production_query: None,
        });
        assert_eq!(outcome.parsed.normalized_title, "アルマゲドン");
        assert_eq!(outcome.parsed.year_hint, None);
        assert_eq!(outcome.parsed.media_kind, "unknown");
        assert_eq!(
            outcome.ranked.iter().map(|c| (c.tmdb_id, c.confidence)).collect::<Vec<_>>(),
            vec![(95, 80), (1001, 40)]
        );
        assert_eq!(outcome.top.as_ref().map(|c| c.tmdb_id), Some(95));
        assert_eq!(outcome.decision, "AUTO");

        // 年つきリリース名 → 年ヒントが効いて 100 点
        let outcome = evaluate(&RulesV1Input {
            raw_title: "Blade.Runner.2049.2017.1080p.BluRay.x264.mkv",
            media_kind: "unknown",
            movies: &[RawMovieV1 {
                id: 335984,
                title: "ブレードランナー 2049".into(),
                original_title: Some("Blade Runner 2049".into()),
                release_date: Some("2017-10-04".into()),
            }],
            tv: &[],
            production_query: None,
        });
        // 2049 は年として扱わない（(19\d{2}|20[012]\d) に一致しない）
        assert_eq!(outcome.parsed.year_hint, Some(2017));
        assert!(outcome.parsed.normalized_title.contains("Blade Runner"));
        assert_eq!(outcome.ranked[0].confidence, 100);
        assert_eq!(outcome.decision, "AUTO");

        // エピソード表記 → media_kind = tv、映画候補は減点で閾値未満
        let outcome = evaluate(&RulesV1Input {
            raw_title: "Breaking.Bad.S01E03.1080p.BluRay.mkv",
            media_kind: "unknown",
            movies: &[RawMovieV1 {
                id: 9999,
                title: "Breaking Bad".into(),
                original_title: None,
                release_date: Some("2008-01-20".into()),
            }],
            tv: &[RawTvV1 {
                id: 1396,
                name: "ブレイキング・バッド".into(),
                original_name: Some("Breaking Bad".into()),
                first_air_date: Some("2008-01-20".into()),
            }],
            production_query: None,
        });
        assert_eq!(outcome.parsed.media_kind, "tv");
        assert_eq!(outcome.parsed.season_no, Some(1));
        assert_eq!(outcome.parsed.episode_no, Some(3));
        assert_eq!(
            outcome.ranked.iter().map(|c| (c.tmdb_id, c.confidence)).collect::<Vec<_>>(),
            vec![(1396, 80)]
        );

        // 候補なし → UNRESOLVED
        let empty = evaluate(&RulesV1Input {
            raw_title: "unknown_file_xyz.mkv",
            media_kind: "unknown",
            movies: &[],
            tv: &[],
            production_query: None,
        });
        assert_eq!(empty.decision, "UNRESOLVED");
        assert_eq!(empty.top, None);
    }

    #[test]
    fn search_plan_follows_the_legacy_path() {
        let plan = search_plan_v1("Blade.Runner.2049.2017.1080p.mkv", "unknown");
        // 2049 は年として扱わないので検索語に残る。2017 だけが年ヒント
        assert_eq!(plan.query, "Blade Runner 2049");
        assert_eq!(plan.year, Some(2017));
        assert!(plan.search_movie && plan.search_tv);
        assert!(plan.movie_retry_without_year);

        // ソースが映画専用なら TV 検索はしない（旧経路と同じ）
        let movie_only = search_plan_v1("Alien.mkv", "movie");
        assert!(movie_only.search_movie && !movie_only.search_tv);
        assert!(!movie_only.movie_retry_without_year);

        let tv_only = search_plan_v1("Alien.mkv", "tv");
        assert!(!tv_only.search_movie && tv_only.search_tv);
    }

    /// 本番の検索語がずれたら記録する（PR3 で parser を変えたときの検知）
    #[test]
    fn divergent_production_query_is_recorded() {
        let same = evaluate(&RulesV1Input {
            raw_title: "Alien.1979.mkv",
            media_kind: "unknown",
            movies: &[],
            tv: &[],
            production_query: Some("Alien"),
        });
        assert!(same.notes.is_empty());

        let diverged = evaluate(&RulesV1Input {
            raw_title: "Alien.1979.mkv",
            media_kind: "unknown",
            movies: &[],
            tv: &[],
            production_query: Some("Alien 1979 Director's Cut"),
        });
        assert_eq!(diverged.notes, vec![NOTE_QUERY_DIVERGED]);
    }

    /// 埋め込みメタデータは rules-1 の入力に入らない（型として受け取れない）
    #[test]
    fn rules_v1_only_sees_the_legacy_path() {
        // 入力は旧経路の生結果だけ。埋め込みタイトル・年・統合候補集合を渡す口が無い
        let outcome = evaluate(&RulesV1Input {
            raw_title: "THE GUILTY.mkv",
            media_kind: "unknown",
            movies: &[RawMovieV1 {
                id: 1,
                title: "THE GUILTY/ギルティ".into(),
                original_title: Some("Den skyldige".into()),
                release_date: Some("2018-06-29".into()),
            }],
            tv: &[],
            production_query: None,
        });
        // タグの年（2019）は一切効かないので、年の加点は無い
        assert_eq!(outcome.parsed.year_hint, None);
        assert!(!outcome.ranked[0].reasons.iter().any(|r| r.contains("year")));
    }
}
