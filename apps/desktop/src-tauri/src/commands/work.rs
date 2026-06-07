use crate::db::DbState;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Serialize)]
pub struct WorkSummaryRow {
    pub id: i64,
    pub title: String,
    pub year: Option<i64>,
    pub release_year: Option<i64>,
    pub work_type: String,
    pub media_category: String,
    pub country_type: String,
    pub reading: Option<String>,
    pub genre_text: Option<String>,
    pub date_added: Option<String>,
    pub last_watched_at: Option<String>,
    pub storage_path: Option<String>,
    pub file_size: Option<i64>,
    pub poster_path: Option<String>,
    pub user_rating: Option<i64>,
    pub my_rating: Option<i64>,
    pub play_count: i64,
    pub watch_status: String,
    pub watched_status: String,
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
    pub release_year: Option<i64>,
    pub work_type: String,
    pub media_category: String,
    pub country_type: String,
    pub reading: Option<String>,
    pub synopsis: Option<String>,
    pub runtime_sec: Option<f64>,
    pub genres_json: Option<String>,
    pub genre_text: Option<String>,
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
    pub date_added: Option<String>,
    pub file_size: Option<i64>,
    // user_stats joined
    pub user_rating: Option<i64>,
    pub my_rating: Option<i64>,
    pub play_count: i64,
    pub last_played_at: Option<String>,
    pub last_watched_at: Option<String>,
    pub resume_position_sec: Option<f64>,
    pub is_favorite: bool,
    pub watch_status: String,
    pub watched_status: String,
    pub personal_note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FilterOptions {
    pub genres: Vec<String>,
    pub countries: Vec<String>,
    pub country_types: Vec<String>,
    pub media_categories: Vec<String>,
    pub year_min: Option<i64>,
    pub year_max: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateWorkLibraryPayload {
    pub work_id: i64,
    pub title: Option<String>,
    pub reading: Option<String>,
    pub country_type: Option<String>,
    pub media_category: Option<String>,
    pub release_year: Option<i64>,
    pub genre_text: Option<String>,
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
    country_type: Option<String>,
    media_category: Option<String>,
    date_added_from: Option<String>,
    date_added_to: Option<String>,
    person_id: Option<i64>,
    series_id: Option<i64>,
    min_user_rating: Option<i64>,
    is_favorite: Option<bool>,
    tag_ids_json: Option<String>,   // JSON 配列文字列 "[1,2,3]"
    min_play_count: Option<i64>,
    // ソート
    sort_field: Option<String>,
    sort_order: Option<String>,
    // 要確認フィルタ
    attention_filter: Option<String>,
    unorganized_only: Option<bool>,
) -> Result<Vec<WorkSummaryRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;

    // ソートカラム（allowlist 検証）
    let order_col = match sort_field.as_deref() {
        Some("year")           => "w.year",
        Some("release_year")   => "COALESCE(w.release_year, w.year)",
        Some("date_added")     => "w.date_added",
        Some("last_watched_at")=> "us.last_watched_at",
        Some("my_rating")      => "us.my_rating",
        Some("watched_status") => "us.watched_status",
        Some("media_category") => "w.media_category",
        Some("country_type")   => "w.country_type",
        Some("reading")        => "w.reading",
        Some("user_rating")    => "us.user_rating",
        Some("external_rating")=> "w.external_rating",
        Some("play_count")     => "us.play_count",
        Some("last_played_at") => "us.last_played_at",
        Some("created_at")     => "COALESCE(w.date_added, w.created_at)",
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
        Some("unorganized") => "AND (COALESCE(w.release_year, w.year) IS NULL OR COALESCE(w.country_type, 'unknown') = 'unknown' OR NULLIF(TRIM(COALESCE(w.reading, '')), '') IS NULL OR COALESCE(w.media_category, 'other') = 'other' OR NULLIF(TRIM(COALESCE(w.title, '')), '') IS NULL)",
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

    let unorganized_sql = if unorganized_only.unwrap_or(false) {
        "AND (COALESCE(w.release_year, w.year) IS NULL
              OR COALESCE(w.country_type, 'unknown') = 'unknown'
              OR NULLIF(TRIM(COALESCE(w.reading, '')), '') IS NULL
              OR COALESCE(w.media_category, 'other') = 'other'
              OR NULLIF(TRIM(COALESCE(w.title, '')), '') IS NULL)"
    } else {
        ""
    };

    let sql = format!(
        "SELECT w.id, w.title, w.year, COALESCE(w.release_year, w.year), w.work_type,
                COALESCE(w.media_category, CASE WHEN w.work_type IN ('movie', 'drama', 'ova') THEN w.work_type ELSE 'other' END),
                COALESCE(w.country_type, 'unknown'),
                w.reading,
                COALESCE(w.genre_text, w.genres_json),
                COALESCE(w.date_added, w.created_at),
                COALESCE(us.last_watched_at, us.last_played_at),
                (
                  SELECT MIN(s.root_path || CASE WHEN f.file_path IS NOT NULL THEN ' / ' || f.file_path ELSE '' END)
                  FROM work_parts wp0
                  JOIN files f ON f.id = wp0.file_id
                  JOIN sources s ON s.id = f.source_id
                  WHERE wp0.work_id = w.id
                ),
                (
                  SELECT SUM(COALESCE(f.file_size, 0))
                  FROM work_parts wp_size
                  JOIN files f ON f.id = wp_size.file_id
                  WHERE wp_size.work_id = w.id
                ),
                COALESCE(w.poster_path, w.thumb_path),
                us.user_rating, COALESCE(us.my_rating, us.user_rating), COALESCE(us.play_count, 0),
                COALESCE(us.watch_status, 'unwatched'),
                COALESCE(us.watched_status, CASE WHEN us.watch_status = 'skipped' THEN 'abandoned' ELSE us.watch_status END, 'unwatched'),
                COALESCE(us.is_favorite, 0),
                w.runtime_sec, us.resume_position_sec, w.external_rating, w.match_status
         FROM works w
         LEFT JOIN user_stats us ON us.work_id = w.id
         WHERE (?1  IS NULL OR w.work_type = ?1)
           AND (?2  IS NULL OR COALESCE(us.watched_status, CASE WHEN us.watch_status = 'skipped' THEN 'abandoned' ELSE us.watch_status END, 'unwatched') = ?2)
           AND (?3  IS NULL
                OR w.title LIKE '%' || ?3 || '%'
                OR w.original_title LIKE '%' || ?3 || '%')
           AND (?4  IS NULL
                OR w.match_status = ?4
                OR (?4 = 'unmatched' AND w.match_status IS NULL))
           AND (?5  IS NULL OR COALESCE(w.release_year, w.year) >= ?5)
           AND (?6  IS NULL OR COALESCE(w.release_year, w.year) <= ?6)
           AND (?7  IS NULL OR COALESCE(w.genre_text, w.genres_json) LIKE '%' || ?7  || '%')
           AND (?8  IS NULL OR w.country_json LIKE '%' || ?8  || '%')
           AND (?9  IS NULL OR w.country_type = ?9)
           AND (?10 IS NULL OR w.media_category = ?10)
           AND (?11 IS NULL OR COALESCE(w.date_added, w.created_at) >= ?11)
           AND (?12 IS NULL OR COALESCE(w.date_added, w.created_at) <= ?12)
           AND (?13 IS NULL OR EXISTS (
               SELECT 1 FROM work_persons wp
               WHERE wp.work_id = w.id AND wp.person_id = ?13))
           AND (?14 IS NULL OR EXISTS (
               SELECT 1 FROM series_items si
               WHERE si.work_id = w.id AND si.series_id = ?14))
           AND (?15 IS NULL OR COALESCE(us.my_rating, us.user_rating, 0) >= ?15)
           AND (?16 = 0 OR COALESCE(us.is_favorite, 0) = 1)
           AND (?17 IS NULL OR COALESCE(us.play_count, 0) >= ?17)
           AND (?18 IS NULL OR EXISTS (
               SELECT 1 FROM work_tags wt
               WHERE wt.work_id = w.id
                 AND wt.tag_id IN (SELECT CAST(value AS INTEGER) FROM json_each(?18))
           ))
           {attention_filter_sql}
           {unorganized_sql}
         ORDER BY {order_col} {order_dir} NULLS LAST"
    );

    let is_fav_int: i64 = if is_favorite.unwrap_or(false) { 1 } else { 0 };

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(
            rusqlite::params![
                work_type, watch_status, query, match_status,
                year_from, year_to, genre, country, country_type, media_category,
                date_added_from, date_added_to, person_id, series_id,
                min_user_rating, is_fav_int, min_play_count,
                tag_ids_json,
            ],
            |row| {
                Ok(WorkSummaryRow {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    year: row.get(2)?,
                    release_year: row.get(3)?,
                    work_type: row.get(4)?,
                    media_category: row.get(5)?,
                    country_type: row.get(6)?,
                    reading: row.get(7)?,
                    genre_text: row.get(8)?,
                    date_added: row.get(9)?,
                    last_watched_at: row.get(10)?,
                    storage_path: row.get(11)?,
                    file_size: row.get(12)?,
                    poster_path: row.get(13)?,
                    user_rating: row.get(14)?,
                    my_rating: row.get(15)?,
                    play_count: row.get(16)?,
                    watch_status: row.get(17)?,
                    watched_status: row.get(18)?,
                    is_favorite: row.get::<_, i64>(19)? != 0,
                    runtime_sec: row.get(20)?,
                    resume_position_sec: row.get(21)?,
                    external_rating: row.get(22)?,
                    match_status: row.get(23)?,
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
            "SELECT w.id, w.title, w.original_title, w.year, COALESCE(w.release_year, w.year),
                    w.work_type,
                    COALESCE(w.media_category, CASE WHEN w.work_type IN ('movie', 'drama', 'ova') THEN w.work_type ELSE 'other' END),
                    COALESCE(w.country_type, 'unknown'), w.reading,
                    w.synopsis, w.runtime_sec, w.genres_json, COALESCE(w.genre_text, w.genres_json),
                    w.poster_path, w.thumb_path,
                    w.external_rating, w.external_rating_source,
                    w.tmdb_id, w.tmdb_media_type, w.imdb_id,
                    w.match_status, w.match_confidence, w.release_date,
                    COALESCE(w.date_added, w.created_at),
                    (
                      SELECT SUM(COALESCE(f.file_size, 0))
                      FROM work_parts wp_size
                      JOIN files f ON f.id = wp_size.file_id
                      WHERE wp_size.work_id = w.id
                    ),
                    us.user_rating, COALESCE(us.my_rating, us.user_rating), COALESCE(us.play_count, 0),
                    us.last_played_at, COALESCE(us.last_watched_at, us.last_played_at), us.resume_position_sec,
                    COALESCE(us.is_favorite, 0),
                    COALESCE(us.watch_status, 'unwatched'),
                    COALESCE(us.watched_status, CASE WHEN us.watch_status = 'skipped' THEN 'abandoned' ELSE us.watch_status END, 'unwatched'),
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
                release_year: row.get(4)?,
                work_type: row.get(5)?,
                media_category: row.get(6)?,
                country_type: row.get(7)?,
                reading: row.get(8)?,
                synopsis: row.get(9)?,
                runtime_sec: row.get(10)?,
                genres_json: row.get(11)?,
                genre_text: row.get(12)?,
                poster_path: row.get(13)?,
                thumb_path: row.get(14)?,
                external_rating: row.get(15)?,
                external_rating_source: row.get(16)?,
                tmdb_id: row.get(17)?,
                tmdb_media_type: row.get(18)?,
                imdb_id: row.get(19)?,
                match_status: row.get(20)?,
                match_confidence: row.get(21)?,
                release_date: row.get(22)?,
                date_added: row.get(23)?,
                file_size: row.get(24)?,
                user_rating: row.get(25)?,
                my_rating: row.get(26)?,
                play_count: row.get(27)?,
                last_played_at: row.get(28)?,
                last_watched_at: row.get(29)?,
                resume_position_sec: row.get(30)?,
                is_favorite: row.get::<_, i64>(31)? != 0,
                watch_status: row.get(32)?,
                watched_status: row.get(33)?,
                personal_note: row.get(34)?,
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
                "SELECT DISTINCT COALESCE(genre_text, genres_json)
                 FROM works
                 WHERE NULLIF(TRIM(COALESCE(genre_text, genres_json, '')), '') IS NOT NULL
                 ORDER BY 1",
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

    let country_types: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT COALESCE(country_type, 'unknown')
                 FROM works
                 ORDER BY 1",
            )
            .map_err(|e| e.to_string())?;
        let x = stmt.query_map([], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .flatten()
            .collect();
        x
    };

    let media_categories: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT COALESCE(media_category, 'other')
                 FROM works
                 ORDER BY 1",
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
            "SELECT MIN(COALESCE(release_year, year)), MAX(COALESCE(release_year, year))
             FROM works WHERE COALESCE(release_year, year) IS NOT NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((None, None));

    Ok(FilterOptions { genres, countries, country_types, media_categories, year_min, year_max })
}

#[tauri::command]
pub fn update_work_library_fields(
    state: State<'_, DbState>,
    payload: UpdateWorkLibraryPayload,
) -> Result<(), String> {
    if let Some(country_type) = payload.country_type.as_deref() {
        if !["foreign", "domestic", "unknown"].contains(&country_type) {
            return Err("invalid country_type".to_string());
        }
    }
    if let Some(media_category) = payload.media_category.as_deref() {
        if !["movie", "drama", "ova", "other"].contains(&media_category) {
            return Err("invalid media_category".to_string());
        }
    }

    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE works SET
           title = COALESCE(?2, title),
           sort_title = COALESCE(lower(?2), sort_title),
           reading = COALESCE(?3, reading),
           country_type = COALESCE(?4, country_type),
           media_category = COALESCE(?5, media_category),
           release_year = COALESCE(?6, release_year),
           year = COALESCE(?6, year),
           genre_text = COALESCE(?7, genre_text)
         WHERE id = ?1",
        rusqlite::params![
            payload.work_id,
            payload.title,
            payload.reading,
            payload.country_type,
            payload.media_category,
            payload.release_year,
            payload.genre_text,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
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
    conn.execute("DELETE FROM series_items WHERE work_id = ?1", rusqlite::params![work_id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM works WHERE id = ?1", rusqlite::params![work_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn delete_work_files(
    state: State<'_, DbState>,
    work_id: i64,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;

    let files: Vec<(i64, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT f.id, f.file_path, s.root_path
                 FROM work_parts wp
                 JOIN files f ON f.id = wp.file_id
                 JOIN sources s ON s.id = f.source_id
                 WHERE wp.work_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![work_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };

    for (_, file_path, root_path) in &files {
        let mut path = std::path::PathBuf::from(file_path);
        if !path.is_absolute() {
            path = std::path::PathBuf::from(root_path).join(file_path);
        }
        if !path.exists() {
            continue;
        }
        if !path.is_file() {
            return Err(format!("削除対象がファイルではありません: {}", path.display()));
        }
        std::fs::remove_file(&path)
            .map_err(|e| format!("ファイル削除に失敗しました: {} ({})", path.display(), e))?;
    }

    conn.execute("DELETE FROM work_parts   WHERE work_id = ?1", rusqlite::params![work_id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM work_tags    WHERE work_id = ?1", rusqlite::params![work_id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM work_persons WHERE work_id = ?1", rusqlite::params![work_id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM user_stats   WHERE work_id = ?1", rusqlite::params![work_id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM series_items WHERE work_id = ?1", rusqlite::params![work_id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM works WHERE id = ?1", rusqlite::params![work_id])
        .map_err(|e| e.to_string())?;

    for (file_id, _, _) in files {
        conn.execute(
            "DELETE FROM files
             WHERE id = ?1
               AND NOT EXISTS (SELECT 1 FROM work_parts WHERE file_id = ?1)",
            rusqlite::params![file_id],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(())
}
