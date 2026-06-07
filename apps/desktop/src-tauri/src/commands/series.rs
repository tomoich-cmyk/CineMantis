use crate::db::DbState;
use rusqlite::OptionalExtension;
use serde::Serialize;
use tauri::State;

// ─── 公開型 ──────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone)]
pub struct SeriesSummaryRow {
    pub id: i64,
    pub title: String,
    pub series_type: String,
    pub tmdb_id: Option<i64>,
    pub poster_path: Option<String>,
    pub work_count: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct SeriesDetailRow {
    pub id: i64,
    pub title: String,
    pub series_type: String,
    pub tmdb_id: Option<i64>,
    pub poster_path: Option<String>,
    pub overview: Option<String>,
}

// ─── Tauri コマンド ──────────────────────────────────────────────────────────

/// シリーズ一覧（作品数付き、先頭作品のポスターをフォールバック）
#[tauri::command]
pub fn list_series(state: State<'_, DbState>) -> Result<Vec<SeriesSummaryRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.title, s.series_type, s.tmdb_id,
                    COALESCE(s.poster_path,
                        (SELECT COALESCE(w.poster_path, w.thumb_path)
                         FROM series_items si2
                         JOIN works w ON w.id = si2.work_id
                         WHERE si2.series_id = s.id
                         ORDER BY si2.sort_order ASC LIMIT 1)
                    ) AS poster_path,
                    COUNT(si.work_id) AS work_count
             FROM series s
             LEFT JOIN series_items si ON si.series_id = s.id
             GROUP BY s.id
             HAVING COUNT(si.work_id) > 0
             ORDER BY s.sort_title ASC NULLS LAST, s.title ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            Ok(SeriesSummaryRow {
                id: row.get(0)?,
                title: row.get(1)?,
                series_type: row.get(2)?,
                tmdb_id: row.get(3)?,
                poster_path: row.get(4)?,
                work_count: row.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(rows)
}

/// シリーズ詳細1件
#[tauri::command]
pub fn get_series(
    state: State<'_, DbState>,
    series_id: i64,
) -> Result<Option<SeriesDetailRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT id, title, series_type, tmdb_id, poster_path, overview FROM series WHERE id = ?1",
        rusqlite::params![series_id],
        |row| {
            Ok(SeriesDetailRow {
                id: row.get(0)?,
                title: row.get(1)?,
                series_type: row.get(2)?,
                tmdb_id: row.get(3)?,
                poster_path: row.get(4)?,
                overview: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// シリーズに含まれる作品一覧（sort_order → season_no → episode_no → year 順）
#[tauri::command]
pub fn get_series_works(
    state: State<'_, DbState>,
    series_id: i64,
) -> Result<Vec<crate::commands::work::WorkSummaryRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT w.id, w.title, w.year, w.work_type,
                    COALESCE(w.poster_path, w.thumb_path),
                    us.user_rating, COALESCE(us.play_count, 0),
                    COALESCE(us.watch_status, 'unwatched'),
                    COALESCE(us.is_favorite, 0),
                    w.runtime_sec, us.resume_position_sec, w.external_rating, w.match_status
             FROM series_items si
             JOIN works w ON w.id = si.work_id
             LEFT JOIN user_stats us ON us.work_id = w.id
             WHERE si.series_id = ?1
             ORDER BY si.sort_order ASC,
                      si.season_no ASC NULLS LAST,
                      si.episode_no ASC NULLS LAST,
                      w.year ASC NULLS LAST,
                      w.sort_title ASC NULLS LAST",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(rusqlite::params![series_id], |row| {
            Ok(crate::commands::work::WorkSummaryRow {
                id: row.get(0)?,
                title: row.get(1)?,
                year: row.get(2)?,
                release_year: row.get(2)?,
                work_type: row.get(3)?,
                media_category: row.get(3)?,
                country_type: "unknown".to_string(),
                reading: None,
                genre_text: None,
                date_added: None,
                last_watched_at: None,
                storage_path: None,
                poster_path: row.get(4)?,
                user_rating: row.get(5)?,
                my_rating: row.get(5)?,
                play_count: row.get(6)?,
                watch_status: row.get(7)?,
                watched_status: row.get(7)?,
                is_favorite: row.get::<_, i64>(8)? != 0,
                runtime_sec: row.get(9)?,
                resume_position_sec: row.get(10)?,
                external_rating: row.get(11)?,
                match_status: row.get(12)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(rows)
}

/// 手動シリーズを作成して id を返す
#[tauri::command]
pub fn create_series(
    state: State<'_, DbState>,
    title: String,
    series_type: Option<String>,
) -> Result<i64, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO series (title, sort_title, series_type) VALUES (?1, ?1, ?2)",
        rusqlite::params![title, series_type.as_deref().unwrap_or("manual")],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// 作品をシリーズに追加
#[tauri::command]
pub fn add_to_series(
    state: State<'_, DbState>,
    series_id: i64,
    work_id: i64,
    sort_order: Option<i64>,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR IGNORE INTO series_items (series_id, work_id, sort_order)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![series_id, work_id, sort_order.unwrap_or(0)],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 作品をシリーズから削除
#[tauri::command]
pub fn remove_from_series(
    state: State<'_, DbState>,
    series_id: i64,
    work_id: i64,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM series_items WHERE series_id = ?1 AND work_id = ?2",
        rusqlite::params![series_id, work_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// シリーズを削除（items は ON DELETE CASCADE で連動削除）
#[tauri::command]
pub fn delete_series(state: State<'_, DbState>, series_id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM series WHERE id = ?1",
        rusqlite::params![series_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── 内部ヘルパー（tmdb.rs から呼ぶ） ─────────────────────────────────────────

/// TMDb 映画コレクションのシリーズを確保し、作品をリンクする
pub fn ensure_movie_collection(
    db: &DbState,
    collection_id: i64,
    collection_name: &str,
    work_id: i64,
    sort_order: i32, // 通常は公開年
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // シリーズが存在しなければ INSERT IGNORE
    conn.execute(
        "INSERT OR IGNORE INTO series (title, sort_title, series_type, tmdb_id)
         VALUES (?1, ?1, 'movie_collection', ?2)",
        rusqlite::params![collection_name, collection_id],
    )
    .map_err(|e| e.to_string())?;

    // series_id 取得
    let series_id: i64 = conn
        .query_row(
            "SELECT id FROM series WHERE tmdb_id = ?1 AND series_type = 'movie_collection'",
            rusqlite::params![collection_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    // 作品リンク（sort_order = 公開年）
    conn.execute(
        "INSERT OR REPLACE INTO series_items (series_id, work_id, sort_order)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![series_id, work_id, sort_order],
    )
    .map_err(|e| e.to_string())?;

    // works.tmdb_collection_id を更新
    let _ = conn.execute(
        "UPDATE works SET tmdb_collection_id = ?1 WHERE id = ?2",
        rusqlite::params![collection_id, work_id],
    );

    Ok(())
}

/// TVシリーズのシリーズを確保し、作品をリンクする
pub fn ensure_tv_series(
    db: &DbState,
    tv_tmdb_id: i64,
    tv_title: &str,
    overview: Option<&str>,
    work_id: i64,
    season_no: Option<i32>,
    episode_no: Option<i32>,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // シリーズが存在しなければ INSERT IGNORE
    conn.execute(
        "INSERT OR IGNORE INTO series (title, sort_title, series_type, tmdb_id, overview)
         VALUES (?1, ?1, 'tv_show', ?2, ?3)",
        rusqlite::params![tv_title, tv_tmdb_id, overview],
    )
    .map_err(|e| e.to_string())?;

    // series_id 取得
    let series_id: i64 = conn
        .query_row(
            "SELECT id FROM series WHERE tmdb_id = ?1 AND series_type = 'tv_show'",
            rusqlite::params![tv_tmdb_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    // sort_order = season * 10000 + episode（ゼロでも OK）
    let sort_order =
        season_no.unwrap_or(0) as i64 * 10_000 + episode_no.unwrap_or(0) as i64;

    conn.execute(
        "INSERT OR REPLACE INTO series_items (series_id, work_id, sort_order, season_no, episode_no)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![series_id, work_id, sort_order, season_no, episode_no],
    )
    .map_err(|e| e.to_string())?;

    // works に season_no / episode_no を記録
    let _ = conn.execute(
        "UPDATE works SET
             season_no  = COALESCE(?1, season_no),
             episode_no = COALESCE(?2, episode_no)
         WHERE id = ?3",
        rusqlite::params![season_no, episode_no, work_id],
    );

    Ok(())
}
