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
    pub external_rating: Option<f64>,
    pub external_rating_source: Option<String>,
    pub tmdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub match_status: String,
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

#[tauri::command]
pub fn list_works(
    state: State<DbState>,
    work_type: Option<String>,
    watch_status: Option<String>,
    query: Option<String>,
    sort_field: Option<String>,
    sort_order: Option<String>,
) -> Result<Vec<WorkSummaryRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;

    // Build dynamic query — safe because sort_field is validated against allowlist
    let order_col = match sort_field.as_deref() {
        Some("year") => "w.year",
        Some("user_rating") => "us.user_rating",
        Some("external_rating") => "w.external_rating",
        Some("play_count") => "us.play_count",
        Some("last_played_at") => "us.last_played_at",
        Some("created_at") => "w.created_at",
        Some("runtime_sec") => "w.runtime_sec",
        _ => "w.sort_title",
    };
    let order_dir = match sort_order.as_deref() {
        Some("desc") => "DESC",
        _ => "ASC",
    };

    let sql = format!(
        "SELECT w.id, w.title, w.year, w.work_type, w.poster_path,
                us.user_rating, COALESCE(us.play_count, 0),
                COALESCE(us.watch_status, 'unwatched'),
                COALESCE(us.is_favorite, 0),
                w.runtime_sec, w.external_rating, w.match_status
         FROM works w
         LEFT JOIN user_stats us ON us.work_id = w.id
         WHERE (?1 IS NULL OR w.work_type = ?1)
           AND (?2 IS NULL OR us.watch_status = ?2)
           AND (?3 IS NULL OR w.title LIKE '%' || ?3 || '%'
                           OR w.original_title LIKE '%' || ?3 || '%')
         ORDER BY {order_col} {order_dir} NULLS LAST"
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(
            rusqlite::params![work_type, watch_status, query],
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
                    external_rating: row.get(10)?,
                    match_status: row.get(11)?,
                })
            },
        )
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(rows)
}

#[tauri::command]
pub fn get_work(state: State<DbState>, work_id: i64) -> Result<Option<WorkDetailRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT w.id, w.title, w.original_title, w.year, w.work_type,
                    w.synopsis, w.runtime_sec, w.genres_json,
                    w.poster_path, w.external_rating, w.external_rating_source,
                    w.tmdb_id, w.imdb_id, w.match_status, w.release_date,
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
                external_rating: row.get(9)?,
                external_rating_source: row.get(10)?,
                tmdb_id: row.get(11)?,
                imdb_id: row.get(12)?,
                match_status: row.get(13)?,
                release_date: row.get(14)?,
                user_rating: row.get(15)?,
                play_count: row.get(16)?,
                last_played_at: row.get(17)?,
                resume_position_sec: row.get(18)?,
                is_favorite: row.get::<_, i64>(19)? != 0,
                watch_status: row.get(20)?,
                personal_note: row.get(21)?,
            })
        })
        .optional()
        .map_err(|e| e.to_string())?;

    Ok(result)
}
