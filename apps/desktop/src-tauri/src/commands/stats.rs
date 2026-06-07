use crate::db::DbState;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Deserialize)]
pub struct UpdateStatsPayload {
    pub work_id: i64,
    pub user_rating: Option<i64>,
    pub my_rating: Option<i64>,
    pub watch_status: Option<String>,
    pub watched_status: Option<String>,
    pub last_watched_at: Option<String>,
    pub is_favorite: Option<bool>,
    pub personal_note: Option<String>,
    pub resume_position_sec: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct UserStatsRow {
    pub work_id: i64,
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

fn legacy_watch_status(status: Option<&String>) -> Option<String> {
    status.map(|s| if s == "abandoned" { "skipped".to_string() } else { s.clone() })
}

#[tauri::command]
pub fn update_user_stats(
    state: State<DbState>,
    payload: UpdateStatsPayload,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let watched_status = payload.watched_status.clone().or(payload.watch_status.clone());
    let watch_status = legacy_watch_status(watched_status.as_ref());
    let rating = payload.my_rating.or(payload.user_rating);

    // Upsert user_stats row
    conn.execute(
        "INSERT INTO user_stats
           (work_id, user_rating, my_rating, watch_status, watched_status, last_watched_at,
            is_favorite, personal_note, resume_position_sec)
         VALUES
           (?1, ?2, ?2, COALESCE(?3, 'unwatched'), COALESCE(?4, 'unwatched'), ?5,
            COALESCE(?6, 0), ?7, ?8)
         ON CONFLICT(work_id) DO UPDATE SET
           user_rating         = COALESCE(?2, user_rating),
           my_rating           = COALESCE(?2, my_rating),
           watch_status        = COALESCE(?3, watch_status),
           watched_status      = COALESCE(?4, watched_status),
           last_watched_at     = COALESCE(?5, last_watched_at),
           is_favorite         = COALESCE(?6, is_favorite),
           personal_note       = COALESCE(?7, personal_note),
           resume_position_sec = COALESCE(?8, resume_position_sec)",
        rusqlite::params![
            payload.work_id,
            rating,
            watch_status,
            watched_status,
            payload.last_watched_at,
            payload.is_favorite.map(|b| b as i64),
            payload.personal_note,
            payload.resume_position_sec,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub fn get_user_stats(
    state: State<DbState>,
    work_id: i64,
) -> Result<Option<UserStatsRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = conn
        .query_row(
            "SELECT work_id, user_rating, play_count, last_played_at,
                    COALESCE(my_rating, user_rating), COALESCE(last_watched_at, last_played_at),
                    resume_position_sec, is_favorite, watch_status,
                    COALESCE(watched_status, CASE WHEN watch_status = 'skipped' THEN 'abandoned' ELSE watch_status END, 'unwatched'),
                    personal_note
             FROM user_stats WHERE work_id = ?1",
            rusqlite::params![work_id],
            |row| {
                Ok(UserStatsRow {
                    work_id: row.get(0)?,
                    user_rating: row.get(1)?,
                    my_rating: row.get(4)?,
                    play_count: row.get(2)?,
                    last_played_at: row.get(3)?,
                    last_watched_at: row.get(5)?,
                    resume_position_sec: row.get(6)?,
                    is_favorite: row.get::<_, i64>(7)? != 0,
                    watch_status: row.get(8)?,
                    watched_status: row.get(9)?,
                    personal_note: row.get(10)?,
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;

    Ok(result)
}

#[tauri::command]
pub fn record_play(state: State<DbState>, work_id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO user_stats (work_id, play_count, last_played_at, last_watched_at, watch_status, watched_status)
         VALUES (?1, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now'), strftime('%Y-%m-%dT%H:%M:%fZ','now'), 'watched', 'watched')
         ON CONFLICT(work_id) DO UPDATE SET
           play_count     = play_count + 1,
           last_played_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
           last_watched_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
           watch_status   = CASE WHEN watch_status = 'unwatched' THEN 'watched' ELSE watch_status END,
           watched_status = CASE WHEN watched_status = 'unwatched' THEN 'watched' ELSE watched_status END",
        rusqlite::params![work_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
