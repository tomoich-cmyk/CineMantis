use crate::db::DbState;
use serde::Serialize;
use tauri::State;

// ─── AttentionStats ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct AttentionStats {
    pub unmatched: i64,      // 照合未完了
    pub no_poster: i64,      // ポスター/サムネなし
    pub missing_meta: i64,   // year or genres が空
    pub no_persons: i64,     // 人物情報なし
    pub file_missing: i64,   // ファイルが見つからない
    pub source_offline: i64, // ソースがオフライン
}

#[tauri::command]
pub fn get_attention_stats(state: State<DbState>) -> Result<AttentionStats, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;

    let row: AttentionStats = conn
        .query_row(
            "SELECT
               SUM(CASE WHEN w.match_status = 'unmatched' THEN 1 ELSE 0 END),
               SUM(CASE WHEN w.poster_path IS NULL AND w.thumb_path IS NULL THEN 1 ELSE 0 END),
               SUM(CASE WHEN w.year IS NULL OR w.genres_json IS NULL THEN 1 ELSE 0 END),
               (SELECT COUNT(DISTINCT w2.id) FROM works w2
                WHERE NOT EXISTS (SELECT 1 FROM work_persons wp WHERE wp.work_id = w2.id)),
               (SELECT COUNT(*) FROM files WHERE availability_status = 'missing'),
               (SELECT COUNT(*) FROM sources WHERE status = 'offline')
             FROM works w",
            [],
            |row| {
                Ok(AttentionStats {
                    unmatched:      row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                    no_poster:      row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    missing_meta:   row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    no_persons:     row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    file_missing:   row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    source_offline: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                })
            },
        )
        .map_err(|e| e.to_string())?;

    Ok(row)
}

// ─── bulk_set_watch_status ────────────────────────────────────────────────────

#[tauri::command]
pub fn bulk_set_watch_status(
    state: State<DbState>,
    work_ids_json: String, // "[1,2,3]"
    status: String,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let legacy_status = if status == "abandoned" { "skipped".to_string() } else { status.clone() };
    conn.execute(
        "INSERT INTO user_stats (work_id, watch_status, watched_status)
         SELECT CAST(je.value AS INTEGER), ?2, ?3 FROM json_each(?1) je
         ON CONFLICT(work_id) DO UPDATE SET
           watch_status = excluded.watch_status,
           watched_status = excluded.watched_status",
        rusqlite::params![work_ids_json, legacy_status, status],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── bulk_set_favorite ────────────────────────────────────────────────────────

#[tauri::command]
pub fn bulk_set_favorite(
    state: State<DbState>,
    work_ids_json: String,
    is_favorite: bool,
) -> Result<(), String> {
    let is_fav_int: i64 = if is_favorite { 1 } else { 0 };
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO user_stats (work_id, is_favorite)
         SELECT CAST(je.value AS INTEGER), ?2 FROM json_each(?1) je
         ON CONFLICT(work_id) DO UPDATE SET is_favorite = excluded.is_favorite",
        rusqlite::params![work_ids_json, is_fav_int],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── bulk_add_tag ─────────────────────────────────────────────────────────────

#[tauri::command]
pub fn bulk_add_tag(
    state: State<DbState>,
    work_ids_json: String,
    tag_id: i64,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR IGNORE INTO work_tags (work_id, tag_id)
         SELECT CAST(je.value AS INTEGER), ?2 FROM json_each(?1) je",
        rusqlite::params![work_ids_json, tag_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── bulk_remove_tag ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn bulk_remove_tag(
    state: State<DbState>,
    work_ids_json: String,
    tag_id: i64,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM work_tags
         WHERE tag_id = ?2
           AND work_id IN (SELECT CAST(je.value AS INTEGER) FROM json_each(?1) je)",
        rusqlite::params![work_ids_json, tag_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── bulk_set_match_status ────────────────────────────────────────────────────

/// match_status を一括変更する（locked / matched / unmatched 等）
#[tauri::command]
pub fn bulk_set_match_status(
    state: State<DbState>,
    work_ids_json: String,
    status: String,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE works SET match_status = ?2
         WHERE id IN (SELECT CAST(je.value AS INTEGER) FROM json_each(?1) je)",
        rusqlite::params![work_ids_json, status],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
