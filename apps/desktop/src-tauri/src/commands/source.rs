use crate::db::DbState;
use serde::{Deserialize, Serialize};
use tauri::State;

fn normalized_path(value: &str) -> String {
    value.trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase()
}

fn paths_overlap(a: &str, b: &str) -> bool {
    let a = normalized_path(a);
    let b = normalized_path(b);
    a == b || a.starts_with(&(b.clone() + "\\")) || b.starts_with(&(a + "\\"))
}

#[derive(Debug, Serialize)]
pub struct SourceRow {
    pub id: i64,
    pub name: String,
    pub root_path: String,
    pub source_type: String,
    /// "movie" | "tv" | "unknown"
    pub media_kind: String,
    pub is_enabled: bool,
    pub status: String,
    pub last_scan_at: Option<String>,
    pub last_seen_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddSourcePayload {
    pub name: String,
    pub root_path: String,
    pub source_type: String,
    /// "movie" | "tv" | "unknown"
    #[serde(default = "default_media_kind")]
    pub media_kind: String,
}

fn default_media_kind() -> String {
    "unknown".to_string()
}

#[tauri::command]
pub fn list_sources(state: State<DbState>) -> Result<Vec<SourceRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, name, root_path, source_type, media_kind, is_enabled, status,
                    last_scan_at, last_seen_at
             FROM sources ORDER BY name",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            Ok(SourceRow {
                id: row.get(0)?,
                name: row.get(1)?,
                root_path: row.get(2)?,
                source_type: row.get(3)?,
                media_kind: row.get::<_, Option<String>>(4)?.unwrap_or_else(|| "unknown".to_string()),
                is_enabled: row.get::<_, i64>(5)? != 0,
                status: row.get(6)?,
                last_scan_at: row.get(7)?,
                last_seen_at: row.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(rows)
}

#[tauri::command]
pub fn add_source(state: State<DbState>, payload: AddSourcePayload) -> Result<i64, String> {
    // media_kind の値を検証
    let media_kind = match payload.media_kind.as_str() {
        "movie" | "tv" | "unknown" => payload.media_kind.clone(),
        _ => "unknown".to_string(),
    };
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let existing_paths = conn
        .prepare("SELECT root_path FROM sources")
        .map_err(|e| e.to_string())?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if let Some(existing) = existing_paths.iter().find(|path| paths_overlap(path, &payload.root_path)) {
        return Err(format!(
            "登録済みソースと範囲が重複しています: {}。親または子のどちらか一方だけを登録してください",
            existing
        ));
    }
    conn.execute(
        "INSERT INTO sources (name, root_path, source_type, media_kind) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![payload.name, payload.root_path, payload.source_type, media_kind],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

#[tauri::command]
pub fn delete_source(state: State<DbState>, source_id: i64) -> Result<(), String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let work_ids: Vec<i64> = {
        let mut stmt = tx
            .prepare(
                "SELECT DISTINCT wp.work_id
                 FROM work_parts wp JOIN files f ON f.id = wp.file_id
                 WHERE f.source_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![source_id], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    tx.execute("DELETE FROM sources WHERE id = ?1", rusqlite::params![source_id])
        .map_err(|e| e.to_string())?;
    for work_id in work_ids {
        tx.execute(
            "DELETE FROM works WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM work_parts WHERE work_id = ?1)",
            rusqlite::params![work_id],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}

#[derive(Debug, Serialize)]
pub struct DeduplicateResult {
    pub removed_files: usize,
    pub removed_works: usize,
}

#[tauri::command]
pub fn deduplicate_library_files(state: State<DbState>) -> Result<DeduplicateResult, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let duplicate_paths: Vec<String> = {
        let mut stmt = tx
            .prepare("SELECT file_path FROM files GROUP BY lower(file_path) HAVING COUNT(*) > 1")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    let mut removed_files = 0usize;
    let mut removed_works = 0usize;

    for path in duplicate_paths {
        let rows: Vec<(i64, i64, i64, usize)> = {
            let mut stmt = tx
                .prepare(
                    "SELECT f.id, wp.work_id, f.source_id,
                            (CASE WHEN w.tmdb_id IS NOT NULL THEN 100 ELSE 0 END +
                             CASE WHEN w.match_status IN ('matched','locked') THEN 50 ELSE 0 END +
                             CASE WHEN w.poster_path IS NOT NULL THEN 20 ELSE 0 END +
                             CASE WHEN w.year IS NOT NULL THEN 10 ELSE 0 END +
                             length(s.root_path)) AS score
                     FROM files f
                     JOIN work_parts wp ON wp.file_id = f.id
                     JOIN works w ON w.id = wp.work_id
                     JOIN sources s ON s.id = f.source_id
                     WHERE lower(f.file_path) = lower(?1)
                     ORDER BY score DESC, f.id ASC",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt.query_map(rusqlite::params![path], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get::<_, i64>(3)? as usize))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
            rows
        };
        let Some(&(keep_file, keep_work, _, _)) = rows.first() else { continue };
        let preferred_source = rows
            .iter()
            .max_by_key(|(_, _, source_id, _)| {
                tx.query_row("SELECT length(root_path) FROM sources WHERE id = ?1", [source_id], |r| r.get::<_, i64>(0)).unwrap_or(0)
            })
            .map(|row| row.2);
        for &(file_id, work_id, _, _) in rows.iter().skip(1) {
            tx.execute("DELETE FROM files WHERE id = ?1", [file_id]).map_err(|e| e.to_string())?;
            removed_files += 1;
            if work_id != keep_work {
                removed_works += tx.execute(
                    "DELETE FROM works WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM work_parts WHERE work_id = ?1)",
                    [work_id],
                ).map_err(|e| e.to_string())?;
            }
        }
        if let Some(source_id) = preferred_source {
            tx.execute("UPDATE files SET source_id = ?1 WHERE id = ?2", rusqlite::params![source_id, keep_file])
                .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(DeduplicateResult { removed_files, removed_works })
}

#[tauri::command]
pub fn update_source_status(
    state: State<DbState>,
    source_id: i64,
    status: String,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE sources SET status = ?1, last_seen_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?2",
        rusqlite::params![status, source_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
