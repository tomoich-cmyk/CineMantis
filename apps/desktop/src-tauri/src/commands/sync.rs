use crate::db::DbState;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::path::PathBuf;
use tauri::State;

#[derive(Debug, Serialize)]
pub struct SyncOutboxRow {
    pub id: i64,
    pub action_type: String,
    pub source_id: Option<i64>,
    pub work_id: Option<i64>,
    pub work_title: Option<String>,
    pub target_path: String,
    pub status: String,
    pub attempts: i64,
    pub error_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub processed_at: Option<String>,
}

#[derive(Debug, Serialize, Default)]
pub struct SyncProcessResult {
    pub processed: usize,
    pub deleted: usize,
    pub already_missing: usize,
    pub skipped_offline: usize,
    pub failed: usize,
}

struct PendingSyncRow {
    id: i64,
    source_root_path: Option<String>,
    target_path: String,
}

pub fn enqueue_file_delete(
    conn: &Connection,
    source_id: Option<i64>,
    work_id: i64,
    work_title: &str,
    target_path: &str,
) -> Result<i64, String> {
    if let Some(existing_id) = conn
        .query_row(
            "SELECT id
             FROM sync_outbox
             WHERE action_type = 'delete_file'
               AND target_path = ?1
               AND status IN ('pending', 'failed')
             ORDER BY id DESC
             LIMIT 1",
            rusqlite::params![target_path],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
    {
        conn.execute(
            "UPDATE sync_outbox
             SET source_id = ?1,
                 work_id = ?2,
                 work_title = ?3,
                 status = 'pending',
                 error_message = NULL,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?4",
            rusqlite::params![source_id, work_id, work_title, existing_id],
        )
        .map_err(|e| e.to_string())?;
        return Ok(existing_id);
    }

    conn.execute(
        "INSERT INTO sync_outbox
           (action_type, source_id, work_id, work_title, target_path, status)
         VALUES
           ('delete_file', ?1, ?2, ?3, ?4, 'pending')",
        rusqlite::params![source_id, work_id, work_title, target_path],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

#[tauri::command]
pub fn list_sync_outbox(state: State<'_, DbState>) -> Result<Vec<SyncOutboxRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, action_type, source_id, work_id, work_title, target_path,
                    status, attempts, error_message, created_at, updated_at, processed_at
             FROM sync_outbox
             WHERE status IN ('pending', 'failed')
             ORDER BY created_at ASC, id ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            Ok(SyncOutboxRow {
                id: row.get(0)?,
                action_type: row.get(1)?,
                source_id: row.get(2)?,
                work_id: row.get(3)?,
                work_title: row.get(4)?,
                target_path: row.get(5)?,
                status: row.get(6)?,
                attempts: row.get(7)?,
                error_message: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
                processed_at: row.get(11)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

#[tauri::command]
pub fn process_sync_outbox(
    state: State<'_, DbState>,
    source_id: Option<i64>,
) -> Result<SyncProcessResult, String> {
    process_sync_outbox_inner(&state, source_id)
}

pub fn process_sync_outbox_inner(
    db: &DbState,
    source_id: Option<i64>,
) -> Result<SyncProcessResult, String> {
    let rows = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT o.id, o.target_path, s.root_path
                 FROM sync_outbox o
                 LEFT JOIN sources s ON s.id = o.source_id
                 WHERE o.action_type = 'delete_file'
                   AND o.status IN ('pending', 'failed')
                   AND (?1 IS NULL OR o.source_id = ?1)
                 ORDER BY o.created_at ASC, o.id ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![source_id], |row| {
            Ok(PendingSyncRow {
                id: row.get(0)?,
                target_path: row.get(1)?,
                source_root_path: row.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
        rows
    };

    let mut result = SyncProcessResult::default();

    for row in rows {
        if let Some(root) = &row.source_root_path {
            if !PathBuf::from(root).exists() {
                let conn = db.0.lock().map_err(|e| e.to_string())?;
                conn.execute(
                    "UPDATE sync_outbox
                     SET status = 'pending',
                         error_message = 'source offline',
                         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                     WHERE id = ?1",
                    rusqlite::params![row.id],
                )
                .map_err(|e| e.to_string())?;
                result.skipped_offline += 1;
                continue;
            }
        }

        {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE sync_outbox
                 SET status = 'running',
                     attempts = attempts + 1,
                     error_message = NULL,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                 WHERE id = ?1",
                rusqlite::params![row.id],
            )
            .map_err(|e| e.to_string())?;
        }

        let target = PathBuf::from(&row.target_path);
        let outcome = if target.exists() {
            if !target.is_file() {
                Err(format!("削除対象がファイルではありません: {}", target.display()))
            } else {
                std::fs::remove_file(&target)
                    .map(|_| "deleted")
                    .map_err(|e| format!("ファイル削除に失敗しました: {} ({})", target.display(), e))
            }
        } else {
            Ok("already_missing")
        };

        result.processed += 1;
        match outcome {
            Ok("deleted") => {
                mark_done(db, row.id)?;
                result.deleted += 1;
            }
            Ok(_) => {
                mark_done(db, row.id)?;
                result.already_missing += 1;
            }
            Err(message) => {
                let conn = db.0.lock().map_err(|e| e.to_string())?;
                conn.execute(
                    "UPDATE sync_outbox
                     SET status = 'failed',
                         error_message = ?1,
                         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                     WHERE id = ?2",
                    rusqlite::params![message, row.id],
                )
                .map_err(|e| e.to_string())?;
                result.failed += 1;
            }
        }
    }

    Ok(result)
}

fn mark_done(db: &DbState, id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE sync_outbox
         SET status = 'done',
             error_message = NULL,
             processed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
