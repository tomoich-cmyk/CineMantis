use crate::commands::settings::get_api_key_internal;
use crate::db::DbState;
use crate::models::tmdb::*;
use crate::services::{
    metadata_matcher::{best_candidate, merge_and_rank, score_movie, score_tv},
    poster_store::store_poster_for_work,
    title_parser::parse_title,
    tmdb_client::TmdbClient,
};
use rusqlite::OptionalExtension;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

// ─── 内部ヘルパー ─────────────────────────────────────────────────────────────

/// works テーブルから 1件の照合用情報を取得
struct WorkForMatch {
    id: i64,
    title: String,
    title_guess: Option<String>,
    media_kind: String,
    year: Option<i32>,
    match_status: String,
}

fn get_work_for_match(db: &DbState, work_id: i64) -> Result<Option<WorkForMatch>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT id, title, title_guess, media_kind, year, match_status
         FROM works WHERE id = ?1",
        rusqlite::params![work_id],
        |row| {
            Ok(WorkForMatch {
                id: row.get(0)?,
                title: row.get(1)?,
                title_guess: row.get(2)?,
                media_kind: row.get(3)?,
                year: row.get(4)?,
                match_status: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// 候補を検索して scored candidates を返す（内部共通処理）
async fn fetch_candidates(
    client: &TmdbClient,
    work: &WorkForMatch,
) -> Vec<TmdbCandidate> {
    // 検索に使うタイトルを決定
    let raw_title = work.title_guess.as_deref().unwrap_or(&work.title);
    let parsed = parse_title(raw_title);
    let year = work.year.or(parsed.year_hint);

    let mut all: Vec<TmdbCandidate> = Vec::new();

    // media_kind に応じて検索種別を決める
    let search_movie = work.media_kind != "tv";
    let search_tv = work.media_kind != "movie";

    // 映画検索
    if search_movie {
        eprintln!("[TMDb] search_movie: '{}' year={:?}", &parsed.normalized_title, year);
        match client.search_movie(&parsed.normalized_title, year).await {
            Ok(results) => {
                eprintln!("[TMDb] got {} movie results", results.len());
                for r in &results {
                    let c = score_movie(r, &parsed);
                    eprintln!("[TMDb]   -> '{}' confidence={}", r.title, c.confidence);
                    all.push(c);
                }
                // 年ヒントがある場合は年なし検索も追加
                if year.is_some() {
                    if let Ok(results2) = client.search_movie(&parsed.normalized_title, None).await {
                        for r in &results2 {
                            let scored = score_movie(r, &parsed);
                            if !all.iter().any(|c| c.tmdb_id == scored.tmdb_id) {
                                all.push(scored);
                            }
                        }
                    }
                }
            }
            Err(e) => eprintln!("[TMDb] movie search error: {}", e),
        }
    }

    // TV 検索
    if search_tv {
        eprintln!("[TMDb] search_tv: '{}' year={:?}", &parsed.normalized_title, year);
        match client.search_tv(&parsed.normalized_title, year).await {
            Ok(results) => {
                eprintln!("[TMDb] got {} tv results", results.len());
                for r in &results {
                    let c = score_tv(r, &parsed);
                    eprintln!("[TMDb]   -> '{}' confidence={}", r.name, c.confidence);
                    all.push(c);
                }
            }
            Err(e) => eprintln!("[TMDb] tv search error: {}", e),
        }
    }

    merge_and_rank(all)
}

/// 詳細取得 + DB 更新 + ポスター保存（movie / tv 共通）
async fn apply_match_internal(
    app: &AppHandle,
    db: &DbState,
    client: &TmdbClient,
    work_id: i64,
    tmdb_id: i64,
    media_type: &str,
    new_match_status: &str,  // "auto" | "manual" | "locked"
) -> Result<AutoMatchResult, String> {
    // シリーズ解析用に照合前のオリジナルタイトルを先取り
    let orig_work_title: String = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT COALESCE(title_guess, title) FROM works WHERE id = ?1",
            rusqlite::params![work_id],
            |row| row.get(0),
        )
        .unwrap_or_default()
    };

    // 詳細取得
    let mut movie_collection: Option<crate::models::tmdb::TmdbCollection> = None;

    let (title, overview, release_date, genres_json, country_json, poster_remote, runtime_sec, imdb_id) =
        if media_type == "movie" {
            let d = client.get_movie_detail(tmdb_id).await?;
            movie_collection = d.belongs_to_collection.clone();
            let genres = genres_to_json(d.genres.as_deref());
            let countries = countries_to_json(d.production_countries.as_deref());
            (
                d.title,
                d.overview,
                d.release_date,
                genres,
                countries,
                d.poster_path,
                d.runtime.map(|r| r as f64 * 60.0),
                d.imdb_id,
            )
        } else {
            let d = client.get_tv_detail(tmdb_id).await?;
            let genres = genres_to_json(d.genres.as_deref());
            let countries = Some(
                serde_json::to_string(&d.origin_country.unwrap_or_default()).unwrap_or_default(),
            );
            let ep_runtime = d
                .episode_run_time
                .as_deref()
                .and_then(|r| r.first())
                .map(|&s| s as f64 * 60.0);
            (
                d.name,
                d.overview,
                d.first_air_date,
                genres,
                countries,
                d.poster_path,
                ep_runtime,
                None,
            )
        };

    let year = release_date
        .as_deref()
        .and_then(|d| d.split('-').next())
        .and_then(|y| y.parse::<i32>().ok());

    // ポスター保存
    let poster_local_path = if let Some(ref rp) = poster_remote {
        store_poster_for_work(app, client, work_id, rp).await
    } else {
        None
    };

    // DB 更新
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE works SET
               title            = ?1,
               synopsis         = COALESCE(?2, synopsis),
               year             = COALESCE(?3, year),
               release_date     = COALESCE(?4, release_date),
               genres_json      = COALESCE(?5, genres_json),
               country_json     = COALESCE(?6, country_json),
               runtime_sec      = COALESCE(?7, runtime_sec),
               tmdb_id          = ?8,
               tmdb_media_type  = ?9,
               imdb_id          = COALESCE(?10, imdb_id),
               poster_path      = COALESCE(?11, poster_path),
               media_kind       = ?9,
               match_status     = ?12,
               metadata_updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?13",
            rusqlite::params![
                title, overview, year, release_date, genres_json, country_json,
                runtime_sec, tmdb_id, media_type,
                imdb_id, poster_local_path,
                new_match_status, work_id,
            ],
        )
        .map_err(|e| e.to_string())?;
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
            db,
            coll.id,
            &coll.name,
            work_id,
            sort_order,
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
            status: new_match_status.to_string(),
            poster_local_path: poster_local_path.clone(),
            title: title.clone(),
            year,
        },
    );

    Ok(AutoMatchResult {
        work_id,
        matched: true,
        status: new_match_status.to_string(),
        confidence: 100, // 手動適用は 100
        tmdb_id: Some(tmdb_id),
        title: Some(title),
        poster_local_path,
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

    let mut work = get_work_for_match(&state, work_id)?
        .ok_or_else(|| format!("work {work_id} not found"))?;

    // ユーザーによるオーバーライドを反映
    if let Some(q) = query_override {
        if !q.trim().is_empty() {
            work.title_guess = Some(q.trim().to_string());
        }
    }
    if let Some(mt) = media_type_hint {
        if mt == "movie" || mt == "tv" {
            work.media_kind = mt;
        }
    }

    Ok(fetch_candidates(&client, &work).await)
}

/// locked → manual に変更（固定解除）
/// メタデータはそのまま保持し、locked フラグだけ外す
#[tauri::command]
pub fn unlock_tmdb_match(
    state: State<'_, DbState>,
    work_id: i64,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE works SET match_status = 'manual',
            metadata_updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1 AND match_status = 'locked'",
        rusqlite::params![work_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 1作品を自動照合して高信頼なら反映
#[tauri::command]
pub async fn auto_match_work(
    app: AppHandle,
    state: State<'_, DbState>,
    work_id: i64,
) -> Result<AutoMatchResult, String> {
    let api_key = get_api_key_internal(&state)?;
    let client = TmdbClient::new(api_key);

    let work = get_work_for_match(&state, work_id)?
        .ok_or_else(|| format!("work {work_id} not found"))?;

    if work.match_status == "locked" {
        return Ok(AutoMatchResult {
            work_id,
            matched: false,
            status: "locked".to_string(),
            confidence: 0,
            tmdb_id: None,
            title: None,
            poster_local_path: None,
        });
    }

    let candidates = fetch_candidates(&client, &work).await;

    match best_candidate(&candidates) {
        None => {
            // 低信頼 → unmatched に記録
            let conn = state.0.lock().map_err(|e| e.to_string())?;
            let _ = conn.execute(
                "UPDATE works SET match_status = 'unmatched', metadata_updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
                rusqlite::params![work_id],
            );
            Ok(AutoMatchResult {
                work_id,
                matched: false,
                status: "unmatched".to_string(),
                confidence: candidates.first().map(|c| c.confidence).unwrap_or(0),
                tmdb_id: None,
                title: None,
                poster_local_path: None,
            })
        }
        Some(best) => {
            let result = apply_match_internal(
                &app, &state, &client,
                work_id, best.tmdb_id, &best.media_type, "auto",
            )
            .await?;
            Ok(AutoMatchResult {
                confidence: best.confidence,
                ..result
            })
        }
    }
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

/// 手動選択した候補を反映
#[tauri::command]
pub async fn apply_tmdb_match(
    app: AppHandle,
    state: State<'_, DbState>,
    work_id: i64,
    tmdb_id: i64,
    media_type: String,
    lock: bool,
) -> Result<AutoMatchResult, String> {
    let api_key = get_api_key_internal(&state)?;
    let client = TmdbClient::new(api_key);
    let match_status = if lock { "locked" } else { "manual" };
    apply_match_internal(&app, &state, &client, work_id, tmdb_id, &media_type, match_status).await
}

/// 既に tmdb_id がある作品のメタデータを再取得
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

    apply_match_internal(&app, &state, &client, work_id, tmdb_id, &media_type, "auto").await
}

/// 照合を解除する（poster 削除・match_status を unmatched に戻す）
#[tauri::command]
pub fn clear_tmdb_match(
    app: AppHandle,
    state: State<'_, DbState>,
    work_id: i64,
) -> Result<(), String> {
    {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE works SET
               tmdb_id = NULL, tmdb_media_type = NULL,
               poster_path = NULL, match_status = 'unmatched',
               match_confidence = NULL, metadata_updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1",
            rusqlite::params![work_id],
        )
        .map_err(|e| e.to_string())?;
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
                processed: 0, total: 0, matched: 0, skipped: 0, failed: 0,
                last_error: Some("API key not set".to_string()),
            });
        }
    };
    let client = TmdbClient::new(api_key);

    // 照合対象を取得
    let targets: Vec<(i64, String, Option<String>, String, Option<i32>)> = {
        let base_filter = if source_id.is_some() {
            "AND EXISTS (SELECT 1 FROM work_parts wp INNER JOIN files f ON f.id = wp.file_id WHERE wp.work_id = w.id AND f.source_id = ?1)"
        } else {
            ""
        };

        let sql = format!(
            "SELECT w.id, w.title, w.title_guess, w.media_kind, w.year
             FROM works w
             WHERE (w.tmdb_id IS NULL)
               AND (w.match_status IS NULL OR w.match_status IN ('unmatched'))
               AND w.title IS NOT NULL AND w.title != ''
               AND w.match_status != 'locked'
             {base_filter}
             ORDER BY w.id
             LIMIT 10"
        );

        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let result: Vec<_> = if let Some(sid) = source_id {
            stmt.query_map(
                rusqlite::params![sid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .map_err(|e| e.to_string())?
            .flatten()
            .collect()
        } else {
            stmt.query_map(
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
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

    let _ = app.emit("metadata:batch_progress", MetadataBatchProgress {
        source_id, processed, total, matched, skipped, failed, last_error: None,
    });

    for (work_id, title, title_guess, media_kind, year) in targets {
        let work = WorkForMatch { id: work_id, title: title.clone(), title_guess, media_kind, year, match_status: "unmatched".to_string() };
        let candidates = fetch_candidates(&client, &work).await;

        match best_candidate(&candidates) {
            None => {
                // 候補なし or スコア不足の診断情報を記録
                if candidates.is_empty() {
                    let msg = format!("\"{}\" → no TMDb results", title);
                    eprintln!("[TMDb] FAIL: {}", msg);
                    last_error = Some(msg);
                } else {
                    let top = &candidates[0];
                    let msg = format!("\"{}\" → best={} score={} (< threshold)", title, top.title, top.confidence);
                    eprintln!("[TMDb] FAIL: {}", msg);
                    last_error = Some(msg);
                }
                let conn = db.0.lock().map_err(|e| e.to_string())?;
                let _ = conn.execute(
                    "UPDATE works SET match_status = 'unmatched' WHERE id = ?1",
                    rusqlite::params![work_id],
                );
                failed += 1;
            }
            Some(best) => {
                eprintln!("[TMDb] MATCH: \"{}\" → \"{}\" ({})", title, best.title, best.confidence);
                match apply_match_internal(
                    app, db, &client,
                    work_id, best.tmdb_id, &best.media_type, "auto",
                )
                .await
                {
                    Ok(_) => matched += 1,
                    Err(e) => {
                        eprintln!("[TMDb] apply_match_internal error: {}", e);
                        last_error = Some(e);
                        failed += 1;
                    }
                }
            }
        }

        processed += 1;
        let _ = app.emit("metadata:batch_progress", MetadataBatchProgress {
            source_id, processed, total, matched, skipped, failed,
            last_error: last_error.clone(),
        });

        // TMDb API レート制限対策（4 req/sec 以内に収める）
        tokio::time::sleep(std::time::Duration::from_millis(260)).await;
    }

    Ok(MetadataBatchProgress { source_id, processed, total, matched, skipped, failed, last_error })
}

/// TMDb API の疎通テスト（"Unfaithful" を検索して最初の候補を返す）
#[tauri::command]
pub async fn test_tmdb_api(
    state: State<'_, DbState>,
) -> Result<String, String> {
    let api_key = get_api_key_internal(&state)?;
    let client = TmdbClient::new(api_key);
    match client.search_movie("Unfaithful", Some(2002)).await {
        Ok(results) => {
            if results.is_empty() {
                Ok("API OK - no results for 'Unfaithful 2002'".to_string())
            } else {
                Ok(format!("API OK - first result: '{}' (id={})", results[0].title, results[0].id))
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

// ─── ユーティリティ ───────────────────────────────────────────────────────────

fn genres_to_json(genres: Option<&[crate::models::tmdb::TmdbGenre]>) -> Option<String> {
    genres.map(|gs| {
        let names: Vec<&str> = gs.iter().map(|g| g.name.as_str()).collect();
        serde_json::to_string(&names).unwrap_or_default()
    })
}

fn countries_to_json(
    countries: Option<&[crate::models::tmdb::TmdbCountry]>,
) -> Option<String> {
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
