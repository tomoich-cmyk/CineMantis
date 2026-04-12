use crate::db::DbState;
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Serialize)]
pub struct TagRow {
    pub id: i64,
    pub name: String,
    pub color: Option<String>,
    pub tag_type: String,
}

#[derive(Debug, Deserialize)]
pub struct AddTagPayload {
    pub name: String,
    pub color: Option<String>,
}

#[tauri::command]
pub fn list_tags(state: State<DbState>) -> Result<Vec<TagRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, name, color, tag_type FROM tags ORDER BY name")
        .map_err(|e| e.to_string())?;

    stmt.query_map([], |row| {
        Ok(TagRow {
            id: row.get(0)?,
            name: row.get(1)?,
            color: row.get(2)?,
            tag_type: row.get(3)?,
        })
    })
    .map_err(|e| e.to_string())?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_tag(state: State<DbState>, payload: AddTagPayload) -> Result<i64, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR IGNORE INTO tags (name, color) VALUES (?1, ?2)",
        rusqlite::params![payload.name, payload.color],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

#[tauri::command]
pub fn tag_work(state: State<DbState>, work_id: i64, tag_id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR IGNORE INTO work_tags (work_id, tag_id) VALUES (?1, ?2)",
        rusqlite::params![work_id, tag_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn untag_work(state: State<DbState>, work_id: i64, tag_id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM work_tags WHERE work_id = ?1 AND tag_id = ?2",
        rusqlite::params![work_id, tag_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn list_work_tags(state: State<DbState>, work_id: i64) -> Result<Vec<TagRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT t.id, t.name, t.color, t.tag_type
             FROM tags t
             INNER JOIN work_tags wt ON wt.tag_id = t.id
             WHERE wt.work_id = ?1
             ORDER BY t.name",
        )
        .map_err(|e| e.to_string())?;

    stmt.query_map(rusqlite::params![work_id], |row| {
        Ok(TagRow {
            id: row.get(0)?,
            name: row.get(1)?,
            color: row.get(2)?,
            tag_type: row.get(3)?,
        })
    })
    .map_err(|e| e.to_string())?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| e.to_string())
}
