use crate::db::DbState;
use rusqlite::OptionalExtension;
use serde::Serialize;
use tauri::State;

#[derive(Debug, Serialize)]
pub struct WorkSummaryRow {
    pub id: i64,
    pub title: String,
    pub year: Option<i64>,
    pub work_type: String,
    pub poster_path: Option<String>,
    pub user_rating: Option<i64>,
    pub play_count: i64,
    pub watch_status: String,
    pub is_favorite: bool,
    pub runtime_sec: Option<f64>,
    pub resume_position_sec: Option<f64>,
    pub external_rating: Option<f64>,
    pub match_status: String,
}

#[derive(Debug, Serialize)]
pub struct WorkDetailRow {
    pub id: i64,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i64>,
    pub work_type: String,
    pub synopsis: Option<String>,
    pub runtime_sec: Option<f64>,
    pub genres_json: Option<String>,
    pub poster_path: Option<String>,
    pub thumb_path: Option<String>,
    pub external_rating: Option<f64>,
    pub external_rating_source: Option<String>,
    pub tmdb_id: Option<i64>,
    pub tmdb_media_type: Option<String>,
    pub imdb_id: Option<String>,
    pub match_status: String,
    pub match_confidence: Option<f64>,
    pub release_date: Option<String>,
    // user_stats joined
    pub user_rating: Option<i64>,
    pub play_count: i64,
    pub last_played_at: Option<String>,
    pub resume_position_sec: Option<f64>,
    pub is_favorite: bool,
    pub watch_status: String,
    pub personal_note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FilterOptions {
    pub genres: Vec<String>,
    pub countries: Vec<String>,
    pub year_min: Option<i64>,
    pub year_max: Option<i64>,
}

// ─── list_works ───────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn list_works(
    state: State<'_, DbState>,
    // 基本フィルタ
    work_type: Option<String>,
    watch_status: Option<String>,
    query: Option<String>,
    match_status: Option<String>,
    // 拡張フィルタ
    year_from: Option<i64>,
    year_to: Option<i64>,
    genre: Option<String>,
    country: Option<String>,
    person_id: Option<i64>,
    min_user_rating: Option<i64>,
    is_favorite: Option<bool>,
    tag_ids_json: Option<String>,   // JSON 配列文字列 "[1,2,3]"
    min_play_count: Option<i64>,
    // ソート
    sort_field: Option<String>,
    sort_order: Option<String>,
    // 要確認フィルタ
    attention_filter: Option<String>,
) -> Result<Vec<WorkSummaryRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;

    // ソートカラム（allowlist 検証）
    let order_col = match sort_field.as_deref() {
        Some("year")           => "w.year",
        Some("user_rating")    => "us.user_rating",
        Some("external_rating")=> "w.external_rating",
        Some("play_count")     => "us.play_count",
        Some("last_played_at") => "us.last_played_at",
        Some("created_at")     => "w.created_at",
        Some("runtime_sec")    => "w.runtime_sec",
        Some("release_date")   => "w.release_date",
        _                      => "w.sort_title",
    };
    let order_dir = match sort_order.as_deref() {
        Some("desc") => "DESC",
        _ => "ASC",
    };

    // 要確認フィルタ (allowlist 検証してそのまま埋め込み)
    let attention_filter_sql: &str = match attention_filter.as_deref() {
        Some("no_poster")    => "AND w.poster_path IS NULL AND w.thumb_path IS NULL",
        Some("missing_meta") => "AND (w.year IS NULL OR w.genres_json IS NULL)",
        Some("no_persons")   =>
            "AND NOT EXISTS (SELECT 1 FROM work_persons wp2 WHERE wp2.work_id = w.id)",
        Some("file_missing") =>
            "AND EXISTS (
                SELECT 1 FROM work_parts wpt
                JOIN files ff ON ff.id = wpt.file_id
                WHERE wpt.work_id = w.id AND ff.availability_status = 'missing'
             )",
        Some("source_offline") =>
            "AND EXISTS (
                SELECT 1 FROM work_parts wpt
                JOIN files ff ON ff.id = wpt.file_id
                JOIN sources ss ON ss.id = ff.source_id
                WHERE wpt.work_id = w.id AND ss.status = 'offline'
             )",
        _ => "",
    };

    let sql = format!(
        "SELECT w.id, w.title, w.year, w.work_type,
                COALESCE(w.poster_path, w.thumb_path),
                us.user_rating, COALESCE(us.play_count, 0),
                COALESCE(us.watch_status, 'unwatched'),
                COALESCE(us.is_favorite, 0),
                w.runtime_sec, us.resume_position_sec, w.external_rating, w.match_status
         FROM works w
         LEFT JOIN user_stats us ON us.work_id = w.id
         WHERE (?1  IS NULL OR w.work_type = ?1)
           AND (?2  IS NULL OR us.watch_status = ?2)
           AND (?3  IS NULL
                OR w.title LIKE '%' || ?3 || '%'
                OR w.original_title LIKE '%' || ?3 || '%')
           AND (?4  IS NULL
                OR w.match_status = ?4
                OR (?4 = 'unmatched' AND w.match_status IS NULL))
           AND (?5  IS NULL OR w.year >= ?5)
           AND (?6  IS NULL OR w.year <= ?6)
           AND (?7  IS NULL OR w.genres_json  LIKE '%' || ?7  || '%')
           AND (?8  IS NULL OR w.country_json LIKE '%' || ?8  || '%')
           AND (?9  IS NULL OR EXISTS (
               SELECT 1 FROM work_persons wp
               WHERE wp.work_id = w.id AND wp.person_id = ?9))
           AND (?10 IS NULL OR COALESCE(us.user_rating, 0) >= ?10)
           AND (?11 = 0 OR COALESCE(us.is_favorite, 0) = 1)
           AND (?12 IS NULL OR COALESCE(us.play_count, 0) >= ?12)
           AND (?13 IS NULL OR EXISTS (
               SELECT 1 FROM work_tags wt
               WHERE wt.work_id = w.id
                 AND wt.tag_id IN (SELECT CAST(value AS INTEGER) FROM json_each(?13))
           ))
           {attention_filter_sql}
         ORDER BY {order_col} {order_dir} NULLS LAST"
    );

    let is_fav_int: i64 = if is_favorite.unwrap_or(false) { 1 } else { 0 };

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(
            rusqlite::params![
                work_type, watch_status, query, match_status,
                year_from, year_to, genre, country, person_id,
                min_user_rating, is_fav_int, min_play_count,
                tag_ids_json,
            ],
            |row| {
                Ok(WorkSummaryRow {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    year: row.get(2)?,
                    work_type: row.get(3)?,
                    poster_path: row.get(4)?,
                    user_rating: row.get(5)?,
                    play_count: row.get(6)?,
                    watch_status: row.get(7)?,
                    is_favorite: row.get::<_, i64>(8)? != 0,
                    runtime_sec: row.get(9)?,
                    resume_position_sec: row.get(10)?,
                    external_rating: row.get(11)?,
                    match_status: row.get(12)?,
                })
            },
        )
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(rows)
}

// ─── get_work ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_work(state: State<'_, DbState>, work_id: i64) -> Result<Option<WorkDetailRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT w.id, w.title, w.original_title, w.year, w.work_type,
                    w.synopsis, w.runtime_sec, w.genres_json,
                    w.poster_path, w.thumb_path,
                    w.external_rating, w.external_rating_source,
                    w.tmdb_id, w.tmdb_media_type, w.imdb_id,
                    w.match_status, w.match_confidence, w.release_date,
                    us.user_rating, COALESCE(us.play_count, 0),
                    us.last_played_at, us.resume_position_sec,
                    COALESCE(us.is_favorite, 0),
                    COALESCE(us.watch_status, 'unwatched'),
                    us.personal_note
             FROM works w
             LEFT JOIN user_stats us ON us.work_id = w.id
             WHERE w.id = ?1",
        )
        .map_err(|e| e.to_string())?;

    let result = stmt
        .query_row(rusqlite::params![work_id], |row| {
            Ok(WorkDetailRow {
                id: row.get(0)?,
                title: row.get(1)?,
                original_title: row.get(2)?,
                year: row.get(3)?,
                work_type: row.get(4)?,
                synopsis: row.get(5)?,
                runtime_sec: row.get(6)?,
                genres_json: row.get(7)?,
                poster_path: row.get(8)?,
                thumb_path: row.get(9)?,
                external_rating: row.get(10)?,
                external_rating_source: row.get(11)?,
                tmdb_id: row.get(12)?,
                tmdb_media_type: row.get(13)?,
                imdb_id: row.get(14)?,
                match_status: row.get(15)?,
                match_confidence: row.get(16)?,
                release_date: row.get(17)?,
                user_rating: row.get(18)?,
                play_count: row.get(19)?,
                last_played_at: row.get(20)?,
                resume_position_sec: row.get(21)?,
                is_favorite: row.get::<_, i64>(22)? != 0,
                watch_status: row.get(23)?,
                personal_note: row.get(24)?,
            })
        })
        .optional()
        .map_err(|e| e.to_string())?;

    Ok(result)
}

// ─── get_filter_options ───────────────────────────────────────────────────────

/// フィルタバー用の選択肢を返す（genres, countries, 年代レンジ）
#[tauri::command]
pub fn get_filter_options(state: State<'_, DbState>) -> Result<FilterOptions, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;

    // genres: genres_json は ["アクション","ドラマ"] 形式の JSON 配列
    let genres: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT je.value
                 FROM works, json_each(works.genres_json) AS je
                 WHERE works.genres_json IS NOT NULL
                   AND je.value != ''
                 ORDER BY je.value",
            )
            .map_err(|e| e.to_string())?;
        let x = stmt.query_map([], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .flatten()
            .collect();
        x
    };

    // countries: country_json は ["JP","US"] 形式
    let countries: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT je.value
                 FROM works, json_each(works.country_json) AS je
                 WHERE works.country_json IS NOT NULL
                   AND je.value != ''
                 ORDER BY je.value",
            )
            .map_err(|e| e.to_string())?;
        let x = stmt.query_map([], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .flatten()
            .collect();
        x
    };

    // 年代レンジ
    let (year_min, year_max): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT MIN(year), MAX(year) FROM works WHERE year IS NOT NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((None, None));

    Ok(FilterOptions { genres, countries, year_min, year_max })
}

/// 作品をライブラリから削除する（関連データも削除）
#[tauri::command]
pub fn delete_work(
    state: State<'_, DbState>,
    work_id: i64,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    // 関連テーブルを手動クリーンアップ（FK が有効でない環境でも安全）
    conn.execute("DELETE FROM work_parts   WHERE work_id = ?1", rusqlite::params![work_id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM work_tags    WHERE work_id = ?1", rusqlite::params![work_id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM work_persons WHERE work_id = ?1", rusqlite::params![work_id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM user_stats   WHERE work_id = ?1", rusqlite::params![work_id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM series_works WHERE work_id = ?1", rusqlite::params![work_id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM works WHERE id = ?1", rusqlite::params![work_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}
