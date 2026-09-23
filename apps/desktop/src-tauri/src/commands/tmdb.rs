use crate::commands::settings::get_api_key_internal;
use crate::db::DbState;
use crate::models::match_source::MatchSource;
use crate::models::match_status::MatchStatus;
use crate::models::tmdb::*;
use crate::services::{
    match_history::{
        self, CandidateSource, LabelMethod, LabelWrite, RejectionSource, RunInput, SearchQuery,
        TriggerKind,
    },
    metadata_matcher::{
        best_candidate, merge_and_rank, rules_safe_best_candidate, rules_safe_with_embedded,
        rules_tags_shadow_best_candidate, score_movie, score_tv, SafeDecision,
    },
    poster_store::store_poster_for_work,
    prematch_snapshot::{LocalEvidence, PreMatchSnapshot, QuerySource},
    title_parser::{parse_title, ParsedTitle},
    tmdb_client::TmdbClient,
};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

// ─── 照合状態の共通ルール ─────────────────────────────────────────────────────

/// locked の作品は明示的な unlock なしに変更しない
pub(crate) const LOCKED_ERROR: &str =
    "固定中の作品は変更できません。先に「固定を解除」してください。";

/// 固定解除後の状態（tmdb_id の有無に合わせ、CHECK 制約の有効値に戻す）
pub(crate) const UNLOCKED_STATUS_SQL: &str =
    "CASE WHEN tmdb_id IS NULL THEN 'unmatched' ELSE 'matched' END";

/// 照合解除で書き戻す列
pub(crate) const CLEAR_MATCH_ASSIGNMENTS: &str = "tmdb_id = NULL, tmdb_media_type = NULL,
    poster_path = NULL, match_status = 'unmatched', match_confidence = NULL,
    metadata_updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')";

/// 照合前の状態（locked 判定・ポスター整理・付け替えの記録に使う）
struct CurrentMatch {
    status: String,
    tmdb_id: Option<i64>,
    media_type: Option<String>,
    /// 今ある照合の出どころ（rules_safe_auto / manual / legacy）
    match_source: Option<String>,
}

fn load_current_match(conn: &Connection, work_id: i64) -> Result<CurrentMatch, String> {
    conn.query_row(
        "SELECT match_status, tmdb_id, tmdb_media_type, match_source FROM works WHERE id = ?1",
        rusqlite::params![work_id],
        |row| {
            Ok(CurrentMatch {
                status: row.get(0)?,
                tmdb_id: row.get(1)?,
                media_type: row.get(2)?,
                match_source: row.get(3)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("work {work_id} not found"))
}

/// match_confidence の書き方
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfidenceWrite {
    /// 自動照合のスコアを記録する
    Score(i32),
    /// 人が選んだ照合。スコアではないので NULL にする
    /// TODO(PR2): 手動確定という操作由来は match_source / match history で管理する
    Clear,
    /// メタデータ再取得など、照合そのものは変わらない
    Keep,
}

/// works に TMDB 照合結果を書き込む内容
pub(crate) struct TmdbMatchWrite<'a> {
    pub work_id: i64,
    pub tmdb_id: i64,
    pub media_type: &'a str,
    pub match_status: MatchStatus,
    pub confidence: ConfidenceWrite,
    pub title: &'a str,
    pub original_title: Option<&'a str>,
    pub overview: Option<&'a str>,
    pub year: Option<i32>,
    pub release_date: Option<&'a str>,
    pub genres_json: Option<&'a str>,
    pub country_json: Option<&'a str>,
    pub runtime_sec: Option<f64>,
    pub imdb_id: Option<&'a str>,
    pub poster_local_path: Option<&'a str>,
    pub external_rating: Option<f64>,
    pub reading: Option<&'a str>,
    pub country_type: Option<&'a str>,
    pub media_category: &'a str,
}

/// TMDB 照合結果を works に書き込む。locked の作品は変更せず false を返す。
/// - title_guess（照合前入力）は変更しない
/// - ポスター取得に失敗した場合、同じ TMDB 作品なら既存の poster_path を残し、
///   別の作品に変わる場合は古い作品のポスターを残さないよう NULL にする
fn write_tmdb_match(conn: &Connection, w: &TmdbMatchWrite) -> Result<bool, String> {
    let (confidence, keep_confidence) = match w.confidence {
        ConfidenceWrite::Score(score) => (Some(score), 0),
        ConfidenceWrite::Clear => (None, 0),
        ConfidenceWrite::Keep => (None, 1),
    };
    let updated = conn
        .execute(
            "UPDATE works SET
               title            = ?1,
               original_title   = COALESCE(?19, original_title),
               synopsis         = COALESCE(?2, synopsis),
               year             = COALESCE(?3, year),
               release_date     = COALESCE(?4, release_date),
               genres_json      = COALESCE(?5, genres_json),
               country_json     = COALESCE(?6, country_json),
               runtime_sec      = COALESCE(?7, runtime_sec),
               tmdb_id          = ?8,
               tmdb_media_type  = ?9,
               imdb_id          = COALESCE(?10, imdb_id),
               poster_path      = CASE
                                    WHEN ?11 IS NOT NULL THEN ?11
                                    WHEN tmdb_id IS ?8 AND tmdb_media_type IS ?9 THEN poster_path
                                    ELSE NULL
                                  END,
               media_kind       = ?9,
               external_rating  = ?12,
               external_rating_source = 'tmdb',
               reading          = COALESCE(NULLIF(TRIM(COALESCE(reading, '')), ''), ?13),
               match_status     = ?14,
               match_confidence = CASE WHEN ?20 = 1 THEN match_confidence ELSE ?15 END,
               country_type     = COALESCE(NULLIF(country_type, 'unknown'), ?17, country_type),
               media_category   = COALESCE(NULLIF(media_category, 'other'), ?18),
               metadata_updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?16 AND match_status <> 'locked'",
            rusqlite::params![
                w.title,
                w.overview,
                w.year,
                w.release_date,
                w.genres_json,
                w.country_json,
                w.runtime_sec,
                w.tmdb_id,
                w.media_type,
                w.imdb_id,
                w.poster_local_path,
                w.external_rating,
                w.reading,
                w.match_status.as_str(),
                confidence,
                w.work_id,
                w.country_type,
                w.media_category,
                w.original_title,
                keep_confidence,
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(updated > 0)
}

/// 自動照合で AUTO にならなかった結果（REVIEW / UNRESOLVED）を記録する。
/// 照合済み（tmdb_id あり）や locked の作品は変更しない。変更後の状態を返す。
fn record_unapplied_decision(
    conn: &Connection,
    work_id: i64,
    status: MatchStatus,
) -> Result<MatchStatus, String> {
    conn.execute(
        "UPDATE works SET match_status = ?2,
                metadata_updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1 AND tmdb_id IS NULL AND match_status <> 'locked'",
        rusqlite::params![work_id, status.as_str()],
    )
    .map_err(|e| e.to_string())?;
    let current = load_current_match(conn, work_id)?;
    MatchStatus::parse(&current.status)
}

/// locked → 有効な状態へ戻す。変更した件数を返す
fn unlock_work(conn: &Connection, work_id: i64) -> Result<usize, String> {
    conn.execute(
        &format!(
            "UPDATE works SET match_status = {UNLOCKED_STATUS_SQL},
                    metadata_updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1 AND match_status = 'locked'"
        ),
        rusqlite::params![work_id],
    )
    .map_err(|e| e.to_string())
}

/// 照合を解除する。locked の作品は拒否する
fn clear_match(conn: &Connection, work_id: i64) -> Result<(), String> {
    let current = load_current_match(conn, work_id)?;
    if current.status == MatchStatus::Locked.as_str() {
        return Err(LOCKED_ERROR.to_string());
    }
    conn.execute(
        &format!("UPDATE works SET {CLEAR_MATCH_ASSIGNMENTS} WHERE id = ?1 AND match_status <> 'locked'"),
        rusqlite::params![work_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn unapplied_result(
    work_id: i64,
    status: MatchStatus,
    decision: Option<SafeDecision>,
    confidence: i32,
    reasons: Vec<String>,
) -> AutoMatchResult {
    AutoMatchResult {
        work_id,
        matched: false,
        status: status.as_str().to_string(),
        confidence,
        tmdb_id: None,
        title: None,
        poster_local_path: None,
        decision: decision.map(|d| d.as_str().to_string()),
        reasons,
    }
}

// ─── 内部ヘルパー ─────────────────────────────────────────────────────────────

/// 照合対象の状態。照合の入力そのものは [`PreMatchSnapshot`] からしか作らない
struct MatchTarget {
    snapshot: PreMatchSnapshot,
    match_status: String,
}

fn load_match_target(conn: &Connection, work_id: i64) -> Result<MatchTarget, String> {
    let match_status: String = conn
        .query_row(
            "SELECT match_status FROM works WHERE id = ?1",
            rusqlite::params![work_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("work {work_id} not found"))?;
    Ok(MatchTarget {
        snapshot: PreMatchSnapshot::capture(conn, work_id)?,
        match_status,
    })
}

fn load_match_target_locked(db: &DbState, work_id: i64) -> Result<MatchTarget, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    load_match_target(&conn, work_id)
}

/// 1回の検索で分かったこと（履歴にそのまま残す）
struct CandidateSearch {
    /// 旧経路（ファイル名・title_guess 由来の検索語）だけの候補。
    /// rules-1 と本番の rules-safe はこちらだけを見る（PR2.5 で経路を変えない）
    legacy_ranked: Vec<TmdbCandidate>,
    legacy_all: Vec<TmdbCandidate>,
    /// 埋め込みメタデータ由来の検索も混ぜた候補（rules-tags-shadow と候補ダイアログ用）
    combined_ranked: Vec<TmdbCandidate>,
    combined_all: Vec<TmdbCandidate>,
    /// 候補がどの検索語から出てきたか
    sources: Vec<CandidateSource>,
    /// 旧経路の検索語を解析したもの（rules-1 / rules-safe の入力）
    parsed: ParsedTitle,
    queries: Vec<SearchQuery>,
    tmdb_calls: usize,
}

/// 同じ作品の候補を1つにまとめる（スコアは高い方を採る）。
/// これは旧経路とは別の matcher（rules-tags-shadow）の入力になる。
fn merge_candidate_sets(
    legacy: &[TmdbCandidate],
    embedded: &[TmdbCandidate],
) -> (Vec<TmdbCandidate>, Vec<CandidateSource>) {
    let mut merged: Vec<TmdbCandidate> = legacy.to_vec();
    let mut sources: Vec<CandidateSource> = legacy
        .iter()
        .map(|c| CandidateSource {
            tmdb_id: c.tmdb_id,
            media_type: c.media_type.clone(),
            query_source: "legacy".to_string(),
        })
        .collect();

    for candidate in embedded {
        match merged
            .iter_mut()
            .find(|c| c.tmdb_id == candidate.tmdb_id && c.media_type == candidate.media_type)
        {
            Some(existing) => {
                if candidate.confidence > existing.confidence {
                    *existing = candidate.clone();
                }
                if let Some(source) = sources.iter_mut().find(|s| {
                    s.tmdb_id == candidate.tmdb_id && s.media_type == candidate.media_type
                }) {
                    source.query_source = "both".to_string();
                }
            }
            None => {
                merged.push(candidate.clone());
                sources.push(CandidateSource {
                    tmdb_id: candidate.tmdb_id,
                    media_type: candidate.media_type.clone(),
                    query_source: "embedded".to_string(),
                });
            }
        }
    }
    (merged, sources)
}

/// 候補を検索してスコアを付ける。入力は照合前スナップショット由来の [`LocalEvidence`] だけ。
/// 年ヒントもファイル名側の値を使う（works.year は TMDB 適用で書き換わるため）。
async fn fetch_candidates(
    client: &TmdbClient,
    evidence: &LocalEvidence,
) -> Result<CandidateSearch, String> {
    let inputs = evidence.search_inputs();
    let mut legacy_all: Vec<TmdbCandidate> = Vec::new();
    let mut embedded_all: Vec<TmdbCandidate> = Vec::new();
    let mut legacy_parsed: Option<ParsedTitle> = None;
    let mut queries: Vec<SearchQuery> = Vec::new();
    let mut tmdb_calls = 0usize;
    let mut attempted = 0usize;
    let mut succeeded = 0usize;
    let mut errors = Vec::new();

    // ソースの設定（movie / tv / unknown）に応じて検索種別を決める
    let search_movie = evidence.media_kind() != "tv";
    let search_tv = evidence.media_kind() != "movie";

    for input in &inputs {
        let parsed = parse_title(&input.title);
        // 旧経路は「検索語から解析した年」だけを使う（PR2.5 で経路を変えない）
        let year = input.year.or(parsed.year_hint);
        let source = input.source.as_str();
        let bucket: &mut Vec<TmdbCandidate> = match input.source {
            QuerySource::Legacy => &mut legacy_all,
            QuerySource::Embedded => &mut embedded_all,
        };

        // 映画検索
        if search_movie {
            attempted += 1;
            tmdb_calls += 1;
            queries.push(SearchQuery {
                kind: "movie",
                query: parsed.normalized_title.clone(),
                year,
                source,
            });
            eprintln!(
                "[TMDb] search_movie({source}): '{}' year={:?}",
                &parsed.normalized_title, year
            );
            match client.search_movie(&parsed.normalized_title, year).await {
                Ok(results) => {
                    succeeded += 1;
                    eprintln!("[TMDb] got {} movie results", results.len());
                    for r in &results {
                        let c = score_movie(r, &parsed);
                        bucket.push(c);
                    }
                    // 年ヒントがある場合は年なし検索も追加
                    if year.is_some() {
                        tmdb_calls += 1;
                        queries.push(SearchQuery {
                            kind: "movie",
                            query: parsed.normalized_title.clone(),
                            year: None,
                            source,
                        });
                        if let Ok(results2) =
                            client.search_movie(&parsed.normalized_title, None).await
                        {
                            for r in &results2 {
                                let scored = score_movie(r, &parsed);
                                if !bucket.iter().any(|c| c.tmdb_id == scored.tmdb_id) {
                                    bucket.push(scored);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[TMDb] movie search error: {}", e);
                    errors.push(format!("movie search: {e}"));
                }
            }
        }

        // TV 検索
        if search_tv {
            attempted += 1;
            tmdb_calls += 1;
            queries.push(SearchQuery {
                kind: "tv",
                query: parsed.normalized_title.clone(),
                year,
                source,
            });
            eprintln!(
                "[TMDb] search_tv({source}): '{}' year={:?}",
                &parsed.normalized_title, year
            );
            match client.search_tv(&parsed.normalized_title, year).await {
                Ok(results) => {
                    succeeded += 1;
                    eprintln!("[TMDb] got {} tv results", results.len());
                    for r in &results {
                        let c = score_tv(r, &parsed);
                        bucket.push(c);
                    }
                }
                Err(e) => {
                    eprintln!("[TMDb] tv search error: {}", e);
                    errors.push(format!("tv search: {e}"));
                }
            }
        }

        if input.source == QuerySource::Legacy {
            legacy_parsed = Some(parsed);
        }
    }

    if attempted > 0 && succeeded == 0 {
        return Err(format!("TMDb search failed: {}", errors.join(" / ")));
    }

    let parsed = legacy_parsed.unwrap_or_else(|| parse_title(evidence.title()));
    let (combined_all, sources) = merge_candidate_sets(&legacy_all, &embedded_all);
    Ok(CandidateSearch {
        legacy_ranked: merge_and_rank(legacy_all.clone()),
        legacy_all,
        combined_ranked: merge_and_rank(combined_all.clone()),
        combined_all,
        sources,
        parsed,
        queries,
        tmdb_calls,
    })
}

/// 検索 → 判定 → 記録 → （AUTO なら）適用。単体照合と一括照合で共通。
/// TMDB 検索が失敗した場合も ERROR の run として残してから Err を返す。
async fn match_once(
    app: &AppHandle,
    db: &DbState,
    client: &TmdbClient,
    snapshot: &PreMatchSnapshot,
    trigger_kind: TriggerKind,
    batch_id: Option<&str>,
) -> Result<AutoMatchResult, String> {
    let work_id = snapshot.work_id;
    let evidence = snapshot.local_evidence();
    let started = std::time::Instant::now();

    // 照合に使えるタイトルが無い作品は TMDB を呼ばずに UNRESOLVED
    let search = if evidence.title().trim().is_empty() && evidence.embedded_title().is_none() {
        Ok(CandidateSearch {
            legacy_ranked: Vec::new(),
            legacy_all: Vec::new(),
            combined_ranked: Vec::new(),
            combined_all: Vec::new(),
            sources: Vec::new(),
            parsed: parse_title(""),
            queries: Vec::new(),
            tmdb_calls: 0,
        })
    } else {
        fetch_candidates(client, &evidence).await
    };

    let search = match search {
        Ok(search) => search,
        Err(error) => {
            // 失敗も記録して、失敗率を後から数えられるようにする
            let parsed = parse_title(evidence.title());
            let outcome = rules_safe_best_candidate(&[], &parsed);
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let _ = match_history::record_run(
                &conn,
                &RunInput {
                    work_id,
                    batch_id,
                    trigger_kind,
                    snapshot,
                    parsed: &parsed,
                    queries: &[],
                    ranked: &[],
                    all: &[],
                    rules_safe: &outcome,
                    rules_one: None,
                    shadow: None,
                    candidate_sources: &[],
                    tmdb_calls: 0,
                    latency_ms: started.elapsed().as_millis() as u64,
                    error_text: Some(error.clone()),
                },
            );
            return Err(error);
        }
    };

    // 本番の判定は旧経路の候補だけを見る。タグは矛盾を示したときに AUTO を止めるだけで、
    // タグのおかげで AUTO が増えることはない（rules-tags-shadow で比較してから昇格する）
    let embedded = snapshot.embedded_evidence();
    let outcome =
        rules_safe_with_embedded(&search.legacy_ranked, &search.parsed, &embedded);
    let shadow = rules_tags_shadow_best_candidate(&search.combined_ranked, &search.parsed, &embedded);
    let reasons: Vec<String> = outcome.reasons.iter().map(|r| r.to_string()).collect();
    let top_confidence = outcome.top.map(|c| c.confidence).unwrap_or(0);
    let applied_candidate = outcome
        .top
        .map(|c| (c.tmdb_id, c.media_type.clone(), c.confidence));

    let run_id = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        match_history::record_run(
            &conn,
            &RunInput {
                work_id,
                batch_id,
                trigger_kind,
                snapshot,
                parsed: &search.parsed,
                queries: &search.queries,
                // 候補は影判定が見たものも含めて残す
                ranked: &search.combined_ranked,
                all: &search.combined_all,
                rules_safe: &outcome,
                // rules-1 は旧経路の候補だけから選ぶ（凍結）
                rules_one: best_candidate(&search.legacy_ranked),
                shadow: Some(&shadow),
                candidate_sources: &search.sources,
                tmdb_calls: search.tmdb_calls,
                latency_ms: started.elapsed().as_millis() as u64,
                error_text: None,
            },
        )?
    };

    match (outcome.decision, applied_candidate) {
        (SafeDecision::Auto, Some((tmdb_id, media_type, confidence))) => {
            let result = apply_match_internal(
                app,
                db,
                client,
                work_id,
                tmdb_id,
                &media_type,
                MatchStatus::Matched,
                ConfidenceWrite::Score(confidence),
            )
            .await?;
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            match_history::mark_run_applied(
                &conn,
                run_id,
                work_id,
                tmdb_id,
                &media_type,
                MatchStatus::Matched,
                MatchSource::RulesSafeAuto,
            )?;
            Ok(AutoMatchResult {
                decision: Some(SafeDecision::Auto.as_str().to_string()),
                ..result
            })
        }
        (decision, _) => {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let status = record_unapplied_decision(&conn, work_id, decision.match_status())?;
            match_history::finish_run(&conn, run_id, status)?;
            Ok(unapplied_result(
                work_id,
                status,
                Some(decision),
                top_confidence,
                reasons,
            ))
        }
    }
}

/// 詳細取得 + DB 更新 + ポスター保存（movie / tv 共通）
async fn apply_match_internal(
    app: &AppHandle,
    db: &DbState,
    client: &TmdbClient,
    work_id: i64,
    tmdb_id: i64,
    media_type: &str,
    new_match_status: MatchStatus,
    confidence: ConfidenceWrite,
) -> Result<AutoMatchResult, String> {
    // locked は明示的な unlock なしに変更しない。シリーズ解析用に照合前のタイトルも先取りする
    let (previous, orig_work_title) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let previous = load_current_match(&conn, work_id)?;
        if previous.status == MatchStatus::Locked.as_str() {
            return Err(LOCKED_ERROR.to_string());
        }
        let orig_work_title: String = conn
            .query_row(
                "SELECT COALESCE(title_guess, title) FROM works WHERE id = ?1",
                rusqlite::params![work_id],
                |row| row.get(0),
            )
            .unwrap_or_default();
        (previous, orig_work_title)
    };

    // 詳細取得
    let mut movie_collection: Option<crate::models::tmdb::TmdbCollection> = None;

    let (
        title,
        original_title,
        overview,
        release_date,
        genres_json,
        country_json,
        poster_remote,
        runtime_sec,
        imdb_id,
        external_rating,
    ) = if media_type == "movie" {
        let d = client.get_movie_detail(tmdb_id).await?;
        movie_collection = d.belongs_to_collection.clone();
        let genres = genres_to_json(d.genres.as_deref());
        let countries = countries_to_json(d.production_countries.as_deref());
        (
            d.title,
            d.original_title,
            d.overview,
            d.release_date,
            genres,
            countries,
            d.poster_path,
            d.runtime.map(|r| r as f64 * 60.0),
            d.imdb_id,
            d.vote_average,
        )
    } else {
        let d = client.get_tv_detail(tmdb_id).await?;
        let genres = genres_to_json(d.genres.as_deref());
        let countries =
            Some(serde_json::to_string(&d.origin_country.unwrap_or_default()).unwrap_or_default());
        let ep_runtime = d
            .episode_run_time
            .as_deref()
            .and_then(|r| r.first())
            .map(|&s| s as f64 * 60.0);
        (
            d.name,
            d.original_name,
            d.overview,
            d.first_air_date,
            genres,
            countries,
            d.poster_path,
            ep_runtime,
            None,
            d.vote_average,
        )
    };
    let original_title = original_title.filter(|t| !t.trim().is_empty());

    let year = release_date
        .as_deref()
        .and_then(|d| d.split('-').next())
        .and_then(|y| y.parse::<i32>().ok());
    let inferred_reading = crate::services::reading::infer_reading(&title);

    // country_type を国情報から推定（JP含む → domestic, それ以外 → foreign）
    // 国情報が空配列のときは判断材料がないので NULL のまま据え置く
    let inferred_country_type: Option<&str> = country_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok())
        .filter(|codes| !codes.is_empty())
        .map(|codes| {
            if codes.iter().any(|c| c.eq_ignore_ascii_case("JP")) {
                "domestic"
            } else {
                "foreign"
            }
        });

    // media_category を media_type から推定（手動設定済みなら SQL 側で保持）
    let inferred_media_category = if media_type == "movie" { "movie" } else { "drama" };

    // ポスター保存
    let poster_local_path = if let Some(ref rp) = poster_remote {
        store_poster_for_work(app, client, work_id, rp).await
    } else {
        None
    };
    let same_tmdb_work =
        previous.tmdb_id == Some(tmdb_id) && previous.media_type.as_deref() == Some(media_type);

    // DB 更新
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let updated = write_tmdb_match(
            &conn,
            &TmdbMatchWrite {
                work_id,
                tmdb_id,
                media_type,
                match_status: new_match_status,
                confidence,
                title: &title,
                original_title: original_title.as_deref(),
                overview: overview.as_deref(),
                year,
                release_date: release_date.as_deref(),
                genres_json: genres_json.as_deref(),
                country_json: country_json.as_deref(),
                runtime_sec,
                imdb_id: imdb_id.as_deref(),
                poster_local_path: poster_local_path.as_deref(),
                external_rating,
                reading: inferred_reading.as_deref(),
                country_type: inferred_country_type,
                media_category: inferred_media_category,
            },
        )?;
        if !updated {
            // 詳細取得中に固定された
            return Err(LOCKED_ERROR.to_string());
        }
    }
    // 別の作品に付け替えてポスターが取れなかった場合、古い作品のポスターファイルを残さない
    if poster_local_path.is_none() && !same_tmdb_work {
        crate::services::poster_store::delete_poster_for_work(app, work_id);
    }

    // ── 人物情報（credits）取得・保存 ─────────────────────────────────────────
    let credits = if media_type == "movie" {
        client.get_movie_credits(tmdb_id).await.ok()
    } else {
        client.get_tv_credits(tmdb_id).await.ok()
    };
    if let Some(ref c) = credits {
        let _ = crate::commands::persons::store_credits(db, work_id, c);
    }

    // ── シリーズ自動リンク ────────────────────────────────────────────────────
    if let Some(ref coll) = movie_collection {
        // 映画コレクション（例: MCU、007 など）
        let sort_order = year.unwrap_or(0);
        let _ = crate::commands::series::ensure_movie_collection(
            db, coll.id, &coll.name, work_id, sort_order,
        );
    } else if media_type == "tv" {
        // TVシリーズ: オリジナルタイトルから season/episode を解析
        let parsed = parse_title(&orig_work_title);
        let _ = crate::commands::series::ensure_tv_series(
            db,
            tmdb_id,
            &title,
            overview.as_deref(),
            work_id,
            parsed.season_no,
            parsed.episode_no,
        );
    }

    // イベント emit
    let _ = app.emit(
        "metadata:updated",
        MetadataUpdatedEvent {
            work_id,
            status: new_match_status.as_str().to_string(),
            poster_local_path: poster_local_path.clone(),
            title: title.clone(),
            year,
        },
    );

    Ok(AutoMatchResult {
        work_id,
        matched: true,
        status: new_match_status.as_str().to_string(),
        confidence: match confidence {
            ConfidenceWrite::Score(score) => score,
            ConfidenceWrite::Clear | ConfidenceWrite::Keep => 0,
        },
        tmdb_id: Some(tmdb_id),
        title: Some(title),
        poster_local_path,
        decision: None,
        reasons: Vec::new(),
    })
}

// ─── Tauri コマンド ──────────────────────────────────────────────────────────

/// 1作品の TMDb 候補一覧を返す
/// `query_override`: 検索語を明示指定（省略時はタイトルから推定）
/// `media_type_hint`: "movie" / "tv" / null（省略時は media_kind から推定）
#[tauri::command]
pub async fn search_tmdb_candidates(
    app: AppHandle,
    state: State<'_, DbState>,
    work_id: i64,
    query_override: Option<String>,
    media_type_hint: Option<String>,
) -> Result<Vec<TmdbCandidate>, String> {
    let api_key = get_api_key_internal(&state)?;
    let client = TmdbClient::new(api_key);

    let target = load_match_target_locked(&state, work_id)?;
    // 検索語と種別だけはユーザーの指定を優先する（自動照合の入力は変えない）
    let evidence = target
        .snapshot
        .local_evidence()
        .with_user_override(query_override.as_deref(), media_type_hint.as_deref());

    // 候補ダイアログにはタグ由来の検索で見つかった候補も見せる
    fetch_candidates(&client, &evidence)
        .await
        .map(|search| search.combined_ranked)
}

/// locked を解除する（固定解除）
/// メタデータはそのまま保持し、tmdb_id があれば matched、なければ unmatched に戻す
#[tauri::command]
pub fn unlock_tmdb_match(state: State<'_, DbState>, work_id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    unlock_work(&conn, work_id)?;
    Ok(())
}

/// 1作品を自動照合する。rules-safe で AUTO のときだけ反映し、
/// REVIEW は pending（レビュー待ち）、UNRESOLVED は unmatched として記録する
#[tauri::command]
pub async fn auto_match_work(
    app: AppHandle,
    state: State<'_, DbState>,
    work_id: i64,
) -> Result<AutoMatchResult, String> {
    let api_key = get_api_key_internal(&state)?;
    let client = TmdbClient::new(api_key);

    let target = load_match_target_locked(&state, work_id)?;

    if target.match_status == MatchStatus::Locked.as_str() {
        // 固定中の作品は run も作らない（照合そのものを行わない）
        return Ok(unapplied_result(
            work_id,
            MatchStatus::Locked,
            None,
            0,
            vec!["LOCKED".to_string()],
        ));
    }

    match_once(
        &app,
        &state,
        &client,
        &target.snapshot,
        TriggerKind::Single,
        None,
    )
    .await
}

/// ソース単位で未照合の作品を一括照合
#[tauri::command]
pub async fn auto_match_source(
    app: AppHandle,
    state: State<'_, DbState>,
    source_id: Option<i64>,
) -> Result<MetadataBatchProgress, String> {
    auto_match_source_inner(&app, &state, source_id).await
}

/// 手動選択した候補を反映（locked の作品は先に固定解除が必要）
#[tauri::command]
pub async fn apply_tmdb_match(
    app: AppHandle,
    state: State<'_, DbState>,
    work_id: i64,
    tmdb_id: i64,
    media_type: String,
    lock: bool,
    // method: "manual_apply"（候補を選んだ）か "manual_direct_id"（ID を直接指定した）
    method: Option<String>,
) -> Result<AutoMatchResult, String> {
    if media_type != "movie" && media_type != "tv" {
        return Err(format!("不正な media_type です: {media_type}"));
    }
    let method = match method.as_deref() {
        Some("manual_direct_id") => LabelMethod::ManualDirectId,
        _ => LabelMethod::ManualApply,
    };
    let api_key = get_api_key_internal(&state)?;
    let client = TmdbClient::new(api_key);
    let match_status = if lock { MatchStatus::Locked } else { MatchStatus::Matched };

    let previous = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        load_current_match(&conn, work_id)?
    };
    let result = apply_match_internal(
        &app,
        &state,
        &client,
        work_id,
        tmdb_id,
        &media_type,
        match_status,
        ConfidenceWrite::Clear,
    )
    .await?;

    // 人が確定した正解として残す。付け替えなら「前の ID ではない」も残す
    {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        if let (Some(previous_id), Some(previous_type)) = (previous.tmdb_id, previous.media_type.as_deref()) {
            if previous_id != tmdb_id || previous_type != media_type {
                match_history::record_rejection(
                    &conn,
                    work_id,
                    previous_id,
                    previous_type,
                    previous.match_source.as_deref(),
                    RejectionSource::Repick,
                )?;
            }
        }
        let run_id = match_history::latest_run_id(&conn, work_id);
        match_history::record_label(
            &conn,
            &LabelWrite {
                work_id,
                run_id,
                tmdb: Some((tmdb_id, &media_type)),
                method,
                note: None,
            },
        )?;
        match_history::set_match_source(&conn, work_id, Some(MatchSource::Manual))?;
    }
    Ok(result)
}

/// 既に tmdb_id がある作品のメタデータを再取得（locked の作品は先に固定解除が必要）
#[tauri::command]
pub async fn refresh_tmdb_metadata(
    app: AppHandle,
    state: State<'_, DbState>,
    work_id: i64,
) -> Result<AutoMatchResult, String> {
    let api_key = get_api_key_internal(&state)?;
    let client = TmdbClient::new(api_key);

    let (tmdb_id, media_type) = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT tmdb_id, COALESCE(tmdb_media_type, 'movie') FROM works WHERE id = ?1 AND tmdb_id IS NOT NULL",
            rusqlite::params![work_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "tmdb_id not set".to_string())?
    };

    apply_match_internal(
        &app,
        &state,
        &client,
        work_id,
        tmdb_id,
        &media_type,
        MatchStatus::Matched,
        ConfidenceWrite::Keep,
    )
    .await
}

/// 照合を解除する（poster 削除・match_status を unmatched に戻す）
/// locked の作品は先に固定解除が必要
#[tauri::command]
pub fn clear_tmdb_match(
    app: AppHandle,
    state: State<'_, DbState>,
    work_id: i64,
) -> Result<(), String> {
    {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        let previous = load_current_match(&conn, work_id)?;
        clear_match(&conn, work_id)?;
        // 「この ID ではない」という否定の記録。正解は分からないのでラベルは取り下げる
        if let (Some(previous_id), Some(previous_type)) =
            (previous.tmdb_id, previous.media_type.as_deref())
        {
            match_history::record_rejection(
                &conn,
                work_id,
                previous_id,
                previous_type,
                previous.match_source.as_deref(),
                RejectionSource::Clear,
            )?;
        }
        match_history::withdraw_active_label(&conn, work_id)?;
        match_history::set_match_source(&conn, work_id, None)?;
    }
    crate::services::poster_store::delete_poster_for_work(&app, work_id);
    Ok(())
}

// ─── inner（scan pipeline から呼べる） ───────────────────────────────────────

pub async fn auto_match_source_inner(
    app: &AppHandle,
    db: &DbState,
    source_id: Option<i64>,
) -> Result<MetadataBatchProgress, String> {
    let api_key = match get_api_key_internal(db) {
        Ok(k) => k,
        Err(_) => {
            // API キー未設定は静かにスキップ
            return Ok(MetadataBatchProgress {
                source_id,
                processed: 0,
                total: 0,
                matched: 0,
                skipped: 0,
                failed: 0,
                last_error: Some("API key not set".to_string()),
            });
        }
    };
    let client = TmdbClient::new(api_key);
    // 一括照合1回分の識別子（同じ実行の run をまとめて見るために使う）
    let batch_id = format!(
        "b{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );

    // 照合対象を取得
    let targets: Vec<(i64, bool)> = {
        let base_filter = if source_id.is_some() {
            "AND EXISTS (SELECT 1 FROM work_parts wp INNER JOIN files f ON f.id = wp.file_id WHERE wp.work_id = w.id AND f.source_id = ?1)"
        } else {
            ""
        };

        // 未照合（一度も試していない）に加えて、PR1 以前の pending も1回だけ付け直す。
        // run が1件でもあれば対象から外れるので、繰り返し実行されない。
        let sql = format!(
            "SELECT w.id,
                    CASE WHEN w.match_status = 'pending' THEN 1 ELSE 0 END AS legacy
             FROM works w
             WHERE w.tmdb_id IS NULL
               AND w.match_status <> 'locked'
               AND (
                    ((w.match_status IS NULL OR w.match_status = 'unmatched')
                      AND w.metadata_updated_at IS NULL)
                 OR (w.match_status = 'pending'
                      AND NOT EXISTS (SELECT 1 FROM metadata_match_runs r WHERE r.work_id = w.id))
               )
             {base_filter}
             ORDER BY w.id
             LIMIT 10"
        );

        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let read = |row: &rusqlite::Row| -> rusqlite::Result<(i64, bool)> {
            Ok((row.get(0)?, row.get::<_, i64>(1)? == 1))
        };
        let result: Vec<_> = if let Some(sid) = source_id {
            stmt.query_map(rusqlite::params![sid], read)
                .map_err(|e| e.to_string())?
                .flatten()
                .collect()
        } else {
            stmt.query_map([], read)
                .map_err(|e| e.to_string())?
                .flatten()
                .collect()
        };
        result
    };

    let total = targets.len();
    let mut processed = 0usize;
    let mut matched = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut last_error: Option<String> = None;

    let _ = app.emit(
        "metadata:batch_progress",
        MetadataBatchProgress {
            source_id,
            processed,
            total,
            matched,
            skipped,
            failed,
            last_error: None,
        },
    );

    for (work_id, legacy_pending) in targets {
        let target = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            load_match_target(&conn, work_id)
        };
        let target = match target {
            Ok(target) => target,
            Err(error) => {
                last_error = Some(format!("work {work_id}: {error}"));
                failed += 1;
                processed += 1;
                continue;
            }
        };
        let title = target.snapshot.derived_title.clone();
        let trigger_kind = if legacy_pending {
            TriggerKind::LegacyRescan
        } else {
            TriggerKind::Batch
        };

        match match_once(app, db, &client, &target.snapshot, trigger_kind, Some(&batch_id)).await {
            Ok(result) if result.matched => {
                eprintln!("[TMDb] MATCH: \"{}\" → {:?}", title, result.tmdb_id);
                matched += 1;
            }
            Ok(result) => {
                // 自動確定できない理由を診断情報として残す
                let msg = format!(
                    "\"{}\" → best score={} ({})",
                    title,
                    result.confidence,
                    result.reasons.join(", ")
                );
                eprintln!("[TMDb] {}: {}", result.status, msg);
                last_error = Some(msg);
                if result.decision.as_deref() == Some(SafeDecision::Review.as_str()) {
                    skipped += 1;
                } else {
                    failed += 1;
                }
            }
            Err(error) => {
                eprintln!("[TMDb] error for \"{}\": {}", title, error);
                last_error = Some(format!("\"{title}\": {error}"));
                failed += 1;
            }
        }

        processed += 1;
        let _ = app.emit(
            "metadata:batch_progress",
            MetadataBatchProgress {
                source_id,
                processed,
                total,
                matched,
                skipped,
                failed,
                last_error: last_error.clone(),
            },
        );

        // TMDb API レート制限対策（4 req/sec 以内に収める）
        tokio::time::sleep(std::time::Duration::from_millis(260)).await;
    }

    Ok(MetadataBatchProgress {
        source_id,
        processed,
        total,
        matched,
        skipped,
        failed,
        last_error,
    })
}

/// TMDb API の疎通テスト（"Unfaithful" を検索して最初の候補を返す）
#[tauri::command]
pub async fn test_tmdb_api(state: State<'_, DbState>) -> Result<String, String> {
    let api_key = get_api_key_internal(&state)?;
    let client = TmdbClient::new(api_key);
    match client.search_movie("Unfaithful", Some(2002)).await {
        Ok(results) => {
            if results.is_empty() {
                Ok("API OK - no results for 'Unfaithful 2002'".to_string())
            } else {
                Ok(format!(
                    "API OK - first result: '{}' (id={})",
                    results[0].title, results[0].id
                ))
            }
        }
        Err(e) => Err(format!("API FAIL: {}", e)),
    }
}

/// tmdb_id がある作品の人物情報（credits）だけ再取得して保存
#[tauri::command]
pub async fn repair_fetch_persons(
    app: AppHandle,
    state: State<'_, DbState>,
    work_id: i64,
) -> Result<(), String> {
    let api_key = get_api_key_internal(&state)?;
    let client = TmdbClient::new(api_key);

    let (tmdb_id, media_type) = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT tmdb_id, COALESCE(tmdb_media_type, 'movie') FROM works WHERE id = ?1 AND tmdb_id IS NOT NULL",
            rusqlite::params![work_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "tmdb_id not set for this work".to_string())?
    };

    let credits = if media_type == "movie" {
        client.get_movie_credits(tmdb_id).await?
    } else {
        client.get_tv_credits(tmdb_id).await?
    };

    crate::commands::persons::store_credits(&state, work_id, &credits)?;

    let _ = app.emit(
        "metadata:updated",
        serde_json::json!({ "work_id": work_id, "type": "persons" }),
    );

    Ok(())
}

#[derive(Serialize)]
pub struct PersonsSyncResult {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub last_error: Option<String>,
}

fn japanese_name_score(value: &str) -> u8 {
    let has_kana = value
        .chars()
        .any(|c| ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{31f0}'..='\u{31ff}').contains(&c));
    let has_kanji = value
        .chars()
        .any(|c| ('\u{3400}'..='\u{9fff}').contains(&c));
    if has_kana {
        2
    } else if has_kanji {
        1
    } else {
        0
    }
}

/// TMDb の人物別名から日本語表記を優先して persons.name を更新する。
#[tauri::command]
pub async fn localize_person_names(
    app: AppHandle,
    state: State<'_, DbState>,
) -> Result<PersonsSyncResult, String> {
    let api_key = get_api_key_internal(&state)?;
    let client = TmdbClient::new(api_key);
    let targets: Vec<(i64, i64, String)> = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, COALESCE(tmdb_id, tmdb_person_id), name
                 FROM persons
                 WHERE COALESCE(tmdb_id, tmdb_person_id) IS NOT NULL
                 ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    }
    .into_iter()
    .filter(|(_, _, name)| japanese_name_score(name) == 0)
    .collect();

    let total = targets.len();
    let mut succeeded = 0;
    let mut failed = 0;
    let mut last_error = None;

    for (person_id, tmdb_id, current_name) in targets {
        match client.get_person_detail(tmdb_id).await {
            Ok(detail) => {
                let localized = std::iter::once(detail.name.as_str())
                    .chain(detail.also_known_as.iter().map(String::as_str))
                    .filter(|name| japanese_name_score(name) > 0)
                    .max_by_key(|name| {
                        (
                            japanese_name_score(name),
                            std::cmp::Reverse(name.chars().count()),
                        )
                    })
                    .map(str::to_string);
                if let Some(localized) = localized {
                    let conn = state.0.lock().map_err(|e| e.to_string())?;
                    conn.execute(
                        "UPDATE persons
                         SET original_name = COALESCE(original_name, ?1), name = ?2, sort_name = ?2,
                             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                         WHERE id = ?3",
                        rusqlite::params![current_name, localized, person_id],
                    )
                    .map_err(|e| e.to_string())?;
                    succeeded += 1;
                }
            }
            Err(error) => {
                failed += 1;
                last_error = Some(error);
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }

    let _ = app.emit(
        "metadata:updated",
        serde_json::json!({ "type": "persons-localized" }),
    );
    Ok(PersonsSyncResult {
        total,
        succeeded,
        failed,
        last_error,
    })
}

/// TMDb 照合済みで人物情報が未取得の作品をまとめて同期する。
#[tauri::command]
pub async fn repair_fetch_all_persons(
    app: AppHandle,
    state: State<'_, DbState>,
) -> Result<PersonsSyncResult, String> {
    let api_key = get_api_key_internal(&state)?;
    let client = TmdbClient::new(api_key);
    let targets: Vec<(i64, i64, String)> = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT w.id, w.tmdb_id, COALESCE(w.tmdb_media_type, 'movie')
                 FROM works w
                 WHERE w.tmdb_id IS NOT NULL
                   AND NOT EXISTS (SELECT 1 FROM work_persons wp WHERE wp.work_id = w.id)
                 ORDER BY w.id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };

    let total = targets.len();
    let mut succeeded = 0;
    let mut failed = 0;
    let mut last_error = None;

    for (work_id, tmdb_id, media_type) in targets {
        let result = if media_type == "movie" {
            client.get_movie_credits(tmdb_id).await
        } else {
            client.get_tv_credits(tmdb_id).await
        };
        match result {
            Ok(credits) => match crate::commands::persons::store_credits(&state, work_id, &credits)
            {
                Ok(()) => succeeded += 1,
                Err(error) => {
                    failed += 1;
                    last_error = Some(error);
                }
            },
            Err(error) => {
                failed += 1;
                last_error = Some(error);
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(260)).await;
    }

    let _ = app.emit(
        "metadata:updated",
        serde_json::json!({ "type": "persons-batch" }),
    );
    Ok(PersonsSyncResult {
        total,
        succeeded,
        failed,
        last_error,
    })
}

/// TMDb 照合済み映画からコレクション情報を再取得する。
#[tauri::command]
pub async fn repair_fetch_all_movie_collections(
    app: AppHandle,
    state: State<'_, DbState>,
) -> Result<PersonsSyncResult, String> {
    let api_key = get_api_key_internal(&state)?;
    let client = TmdbClient::new(api_key);
    let targets: Vec<(i64, i64)> = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, tmdb_id FROM works
                 WHERE tmdb_id IS NOT NULL
                   AND COALESCE(tmdb_media_type, 'movie') = 'movie'
                   AND tmdb_collection_id IS NULL
                 ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };

    let total = targets.len();
    let mut succeeded = 0;
    let mut failed = 0;
    let mut last_error = None;
    for (work_id, tmdb_id) in targets {
        match client.get_movie_detail(tmdb_id).await {
            Ok(detail) => {
                if let Some(collection) = detail.belongs_to_collection {
                    let year = detail
                        .release_date
                        .as_deref()
                        .and_then(|date| date.split('-').next())
                        .and_then(|year| year.parse::<i32>().ok())
                        .unwrap_or(0);
                    match crate::commands::series::ensure_movie_collection(
                        &state,
                        collection.id,
                        &collection.name,
                        work_id,
                        year,
                    ) {
                        Ok(()) => succeeded += 1,
                        Err(error) => {
                            failed += 1;
                            last_error = Some(error);
                        }
                    }
                }
            }
            Err(error) => {
                failed += 1;
                last_error = Some(error);
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(260)).await;
    }

    let _ = app.emit(
        "metadata:updated",
        serde_json::json!({ "type": "series-batch" }),
    );
    Ok(PersonsSyncResult {
        total,
        succeeded,
        failed,
        last_error,
    })
}

// ─── ユーティリティ ───────────────────────────────────────────────────────────

fn genres_to_json(genres: Option<&[crate::models::tmdb::TmdbGenre]>) -> Option<String> {
    genres.map(|gs| {
        let names: Vec<&str> = gs.iter().map(|g| g.name.as_str()).collect();
        serde_json::to_string(&names).unwrap_or_default()
    })
}

fn countries_to_json(countries: Option<&[crate::models::tmdb::TmdbCountry]>) -> Option<String> {
    countries.map(|cs| {
        let codes: Vec<&str> = cs.iter().map(|c| c.iso_3166_1.as_str()).collect();
        serde_json::to_string(&codes).unwrap_or_default()
    })
}

// ─── 追加の型（ここに置くのが自然） ───────────────────────────────────────────

#[derive(Serialize)]
pub struct TmdbCandidatesResponse {
    pub work_id: i64,
    pub candidates: Vec<TmdbCandidate>,
    pub parsed_title: String,
    pub media_kind: String,
    pub year_hint: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::*;

    fn write<'a>(work_id: i64, tmdb_id: i64, poster: Option<&'a str>) -> TmdbMatchWrite<'a> {
        TmdbMatchWrite {
            work_id,
            tmdb_id,
            media_type: "movie",
            match_status: MatchStatus::Matched,
            confidence: ConfidenceWrite::Score(90),
            title: "エイリアン",
            original_title: Some("Alien"),
            overview: None,
            year: Some(1979),
            release_date: Some("1979-05-25"),
            genres_json: None,
            country_json: None,
            runtime_sec: None,
            imdb_id: None,
            poster_local_path: poster,
            external_rating: None,
            reading: None,
            country_type: None,
            media_category: "movie",
        }
    }

    fn row(conn: &Connection, work_id: i64) -> (String, Option<String>, Option<String>, Option<String>, Option<f64>) {
        conn.query_row(
            "SELECT title, title_guess, original_title, poster_path, match_confidence FROM works WHERE id = ?1",
            rusqlite::params![work_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap()
    }

    fn scanned_work(conn: &Connection) -> i64 {
        crate::commands::scan::insert_scanned_work(conn, "movie", "unknown", "Alien 1979", "foreign")
            .unwrap()
    }

    #[test]
    fn apply_keeps_title_guess_and_saves_original_title() {
        let conn = open_migrated();
        let work_id = scanned_work(&conn);
        assert!(write_tmdb_match(&conn, &write(work_id, 348, Some("/p/1.jpg"))).unwrap());

        let (title, title_guess, original_title, poster, confidence) = row(&conn, work_id);
        assert_eq!(title, "エイリアン");
        assert_eq!(title_guess.as_deref(), Some("Alien 1979"));
        assert_eq!(original_title.as_deref(), Some("Alien"));
        assert_eq!(poster.as_deref(), Some("/p/1.jpg"));
        assert_eq!(confidence, Some(90.0));
        assert_eq!(match_state(&conn, work_id), ("matched".into(), Some(348)));
    }

    #[test]
    fn poster_failure_keeps_poster_for_same_tmdb_work_only() {
        let conn = open_migrated();
        let work_id = scanned_work(&conn);
        write_tmdb_match(&conn, &write(work_id, 348, Some("/p/1.jpg"))).unwrap();

        // 再取得でポスター取得に失敗しても、同じ作品なら既存のポスターを残す
        write_tmdb_match(&conn, &write(work_id, 348, None)).unwrap();
        assert_eq!(row(&conn, work_id).3.as_deref(), Some("/p/1.jpg"));

        // 別の作品に付け替えて取得に失敗した場合は、古い作品のポスターを残さない
        write_tmdb_match(&conn, &write(work_id, 999, None)).unwrap();
        assert_eq!(row(&conn, work_id).3, None);
    }

    #[test]
    fn confidence_is_not_set_to_100_for_manual_or_refresh() {
        let conn = open_migrated();
        let work_id = scanned_work(&conn);
        write_tmdb_match(&conn, &write(work_id, 348, None)).unwrap();

        let mut refresh = write(work_id, 348, None);
        refresh.confidence = ConfidenceWrite::Keep;
        write_tmdb_match(&conn, &refresh).unwrap();
        assert_eq!(row(&conn, work_id).4, Some(90.0));

        let mut manual = write(work_id, 348, None);
        manual.confidence = ConfidenceWrite::Clear;
        write_tmdb_match(&conn, &manual).unwrap();
        assert_eq!(row(&conn, work_id).4, None);
    }

    #[test]
    fn locked_work_is_not_overwritten_by_apply() {
        let conn = open_migrated();
        let work_id = scanned_work(&conn);
        set_match(&conn, work_id, "locked", Some(11));

        let mut explicit_lock = write(work_id, 348, Some("/p/1.jpg"));
        explicit_lock.match_status = MatchStatus::Locked;
        assert!(!write_tmdb_match(&conn, &explicit_lock).unwrap());
        assert!(!write_tmdb_match(&conn, &write(work_id, 348, None)).unwrap());
        assert_eq!(match_state(&conn, work_id), ("locked".into(), Some(11)));
        assert_eq!(row(&conn, work_id).0, "Alien 1979");
    }

    #[test]
    fn unapplied_decisions_map_to_pending_and_unmatched() {
        let conn = open_migrated();
        let review = scanned_work(&conn);
        let unresolved = scanned_work(&conn);
        assert_eq!(
            record_unapplied_decision(&conn, review, SafeDecision::Review.match_status()).unwrap(),
            MatchStatus::Pending
        );
        assert_eq!(
            record_unapplied_decision(&conn, unresolved, SafeDecision::Unresolved.match_status())
                .unwrap(),
            MatchStatus::Unmatched
        );
        assert_eq!(match_state(&conn, review), ("pending".into(), None));
        assert_eq!(match_state(&conn, unresolved), ("unmatched".into(), None));
    }

    #[test]
    fn automatic_decisions_do_not_touch_locked_or_matched_works() {
        let conn = open_migrated();
        let locked = scanned_work(&conn);
        set_match(&conn, locked, "locked", Some(11));
        let matched = scanned_work(&conn);
        set_match(&conn, matched, "matched", Some(22));

        for status in [MatchStatus::Pending, MatchStatus::Unmatched] {
            assert_eq!(record_unapplied_decision(&conn, locked, status).unwrap(), MatchStatus::Locked);
            assert_eq!(record_unapplied_decision(&conn, matched, status).unwrap(), MatchStatus::Matched);
        }
        assert_eq!(match_state(&conn, locked), ("locked".into(), Some(11)));
        assert_eq!(match_state(&conn, matched), ("matched".into(), Some(22)));
    }

    #[test]
    fn clearing_a_locked_work_requires_unlock() {
        let conn = open_migrated();
        let work_id = scanned_work(&conn);
        set_match(&conn, work_id, "locked", Some(11));

        assert_eq!(clear_match(&conn, work_id), Err(LOCKED_ERROR.to_string()));
        assert_eq!(match_state(&conn, work_id), ("locked".into(), Some(11)));

        assert_eq!(unlock_work(&conn, work_id).unwrap(), 1);
        assert_eq!(match_state(&conn, work_id), ("matched".into(), Some(11)));
        clear_match(&conn, work_id).unwrap();
        assert_eq!(match_state(&conn, work_id), ("unmatched".into(), None));
    }

    #[test]
    fn unlock_without_tmdb_id_returns_to_unmatched() {
        let conn = open_migrated();
        let work_id = scanned_work(&conn);
        set_match(&conn, work_id, "locked", None);
        assert_eq!(unlock_work(&conn, work_id).unwrap(), 1);
        assert_eq!(match_state(&conn, work_id), ("unmatched".into(), None));
        // locked 以外には何もしない
        assert_eq!(unlock_work(&conn, work_id).unwrap(), 0);
    }
}
