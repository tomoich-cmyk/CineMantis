use crate::db::DbState;
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Serialize)]
pub struct SourceRow {
    pub id: i64,
    pub name: String,
    pub root_path: String,
    pub source_type: String,
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
}

#[tauri::command]
pub fn list_sources(state: State<DbState>) -> Result<Vec<SourceRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, name, root_path, source_type, is_enabled, status, last_scan_at, last_seen_at
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
                is_enabled: row.get::<_, i64>(4)? != 0,
                status: row.get(5)?,
                last_scan_at: row.get(6)?,
                last_seen_at: row.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(rows)
}

#[tauri::command]
pub fn add_source(state: State<DbState>, payload: AddSourcePayload) -> Result<i64, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO sources (name, root_path, source_type) VALUES (?1, ?2, ?3)",
        rusqlite::params![payload.name, payload.root_path, payload.source_type],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
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
