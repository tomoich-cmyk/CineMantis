use crate::db::DbState;
use rusqlite::OptionalExtension;
use tauri::State;
use tauri_plugin_shell::ShellExt;

// ─── open_work_file ──────────────────────────────────────────────────────────

/// 作品の主ファイルを OS の既定アプリで開き、再生履歴を記録する。
/// - play_count をインクリメント
/// - last_played_at を更新
/// - watch_status が "unwatched" であれば "watching" に自動変更
#[tauri::command]
pub fn open_work_file(
    state: State<'_, DbState>,
    app: tauri::AppHandle,
    work_id: i64,
) -> Result<(), String> {
    // ① ファイルパス取得（play_order 優先、最初のパートのみ）
    let file_path: Option<String> = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT f.file_path
             FROM work_parts wp
             JOIN files f ON f.id = wp.file_id
             WHERE wp.work_id = ?1
               AND f.availability_status = 'available'
             ORDER BY wp.play_order, wp.part_no
             LIMIT 1",
            rusqlite::params![work_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
    };

    let path = file_path.ok_or_else(|| "再生可能なファイルが見つかりません".to_string())?;

    // ② OS 既定アプリで開く
    app.shell()
        .open(&path, None)
        .map_err(|e| e.to_string())?;

    // ③ 再生記録: play_count++, watch_status を watching へ（unwatched の場合のみ）
    {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO user_stats (work_id, play_count, last_played_at, last_watched_at, watch_status, watched_status)
             VALUES (?1, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now'), strftime('%Y-%m-%dT%H:%M:%fZ','now'), 'watching', 'watching')
             ON CONFLICT(work_id) DO UPDATE SET
               play_count     = play_count + 1,
               last_played_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
               last_watched_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
               watch_status   = CASE
                                  WHEN watch_status = 'unwatched' THEN 'watching'
                                  ELSE watch_status
                                END,
               watched_status = CASE
                                  WHEN watched_status = 'unwatched' THEN 'watching'
                                  ELSE watched_status
                                END",
            rusqlite::params![work_id],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(())
}

// ─── set_watch_status ─────────────────────────────────────────────────────────

/// 視聴状態を直接変更する（unwatched / watching / watched / skipped）
#[tauri::command]
pub fn set_watch_status(
    state: State<'_, DbState>,
    work_id: i64,
    status: String,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let legacy_status = if status == "abandoned" { "skipped".to_string() } else { status.clone() };
    conn.execute(
        "INSERT INTO user_stats (work_id, watch_status, watched_status)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(work_id) DO UPDATE SET watch_status = ?2, watched_status = ?3",
        rusqlite::params![work_id, legacy_status, status],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── update_resume_position ───────────────────────────────────────────────────

/// 再生位置（秒）を保存する。外部プレーヤーから手動で入力する用途。
#[tauri::command]
pub fn update_resume_position(
    state: State<'_, DbState>,
    work_id: i64,
    position_sec: f64,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO user_stats (work_id, resume_position_sec)
         VALUES (?1, ?2)
         ON CONFLICT(work_id) DO UPDATE SET resume_position_sec = ?2",
        rusqlite::params![work_id, position_sec],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
