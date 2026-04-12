use crate::models::tmdb::*;
use crate::services::title_parser::ParsedTitle;

/// 自動適用しきい値
pub const THRESHOLD_AUTO: i32 = 90;
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

    // タイトル一致
    if result_title == query_title {
        score += 50;
        reasons.push("title exact match".to_string());
    } else if normalize_str(&result_title) == normalize_str(&query_title) {
        score += 35;
        reasons.push("normalized title match".to_string());
    } else if result_title.contains(&query_title) || query_title.contains(&result_title) {
        score += 20;
        reasons.push("title partial match".to_string());
    }

    // 原題一致
    if !result_orig.is_empty() {
        if normalize_str(&result_orig) == normalize_str(&query_title) {
            score += 10;
            reasons.push("original title match".to_string());
        }
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

    if result_title == query_title {
        score += 50;
        reasons.push("title exact match".to_string());
    } else if normalize_str(&result_title) == normalize_str(&query_title) {
        score += 35;
        reasons.push("normalized title match".to_string());
    } else if result_title.contains(&query_title) || query_title.contains(&result_title) {
        score += 20;
        reasons.push("title partial match".to_string());
    }

    if !result_orig.is_empty() && normalize_str(&result_orig) == normalize_str(&query_title) {
        score += 10;
        reasons.push("original title match".to_string());
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

/// 最良候補を返す（自動適用用）
pub fn best_candidate(candidates: &[TmdbCandidate]) -> Option<&TmdbCandidate> {
    candidates
        .iter()
        .max_by_key(|c| c.confidence)
        .filter(|c| c.confidence >= THRESHOLD_AUTO)
}

// ─── ユーティリティ ───────────────────────────────────────────────────────────

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
