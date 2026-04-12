use crate::db::DbState;
use serde::Serialize;
use tauri::State;

#[derive(Debug, Serialize)]
pub struct AppSetting {
    pub key: String,
    pub value: String,
}

/// 設定値を取得する
#[tauri::command]
pub fn get_setting(state: State<DbState>, key: String) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?1",
        rusqlite::params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// 設定値を保存する（upsert）
#[tauri::command]
pub fn set_setting(state: State<DbState>, key: String, value: String) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO app_settings (key, value)
         VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        rusqlite::params![key, value],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// TMDb API キーをマスクして返す（UI 表示用）
#[tauri::command]
pub fn get_tmdb_api_key_masked(state: State<DbState>) -> Result<Option<String>, String> {
    let full = get_setting(state, "tmdb_api_key".to_string())?;
    Ok(full.map(|k| {
        if k.len() > 8 {
            format!("{}…{}", &k[..4], &k[k.len() - 4..])
        } else {
            "****".to_string()
        }
    }))
}

// ─── 内部ヘルパー：他 Rust モジュールから使う ─────────────────────────────────

pub fn get_api_key_internal(db: &DbState) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = 'tmdb_api_key'",
        [],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "TMDb API キーが設定されていません。設定画面から登録してください。".to_string())
}
