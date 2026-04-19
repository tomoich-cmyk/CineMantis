use crate::models::tmdb::*;
use crate::services::title_parser::ParsedTitle;

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

/// タイトル一致スコアを返す（完全一致65 / 正規化一致55 / 部分一致25 / 不一致0）
/// result_title と query_title はどちらも既に to_lowercase() 済みであること。
fn title_match_score(result_title: &str, query_title: &str) -> i32 {
    if result_title == query_title {
        65
    } else if normalize_str(result_title) == normalize_str(query_title) {
        55
    } else if result_title.contains(query_title) || query_title.contains(result_title) {
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
