use crate::db::DbState;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use tauri::State;

#[derive(Debug, Serialize)]
pub struct AppSetting {
    pub key: String,
    pub value: String,
}

// ─── secret key policy ───────────────────────────────────────────────────────
//
// API キーなどの秘密値は汎用 get_setting でフロントエンドへ返さない。
// 保存（set_setting）は従来どおり可能で、表示はマスク済み API だけを使う。
// 新しい秘密値を app_settings に入れる場合は SECRET_SETTING_KEYS に追加するか、
// 末尾規則（_api_key / _secret / _token / _password）に合う名前にすること。

const SECRET_SETTING_KEYS: &[&str] = &["tmdb_api_key"];
const SECRET_SETTING_SUFFIXES: &[&str] = &["_api_key", "_secret", "_token", "_password"];

pub fn is_secret_setting_key(key: &str) -> bool {
    let key = key.trim().to_ascii_lowercase();
    SECRET_SETTING_KEYS.contains(&key.as_str())
        || SECRET_SETTING_SUFFIXES.iter().any(|suffix| key.ends_with(suffix))
}

/// 秘密値を画面表示用にマスクする（先頭4文字…末尾4文字）
fn mask_secret(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() > 8 {
        let head: String = chars[..4].iter().collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{head}…{tail}")
    } else {
        "****".to_string()
    }
}

fn read_setting_raw(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?1",
        rusqlite::params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn get_setting_inner(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    if is_secret_setting_key(key) {
        return Err(format!("設定値「{key}」は秘密情報のため取得できません"));
    }
    read_setting_raw(conn, key)
}

/// 設定値を取得する（秘密値は取得不可）
#[tauri::command]
pub fn get_setting(state: State<DbState>, key: String) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    get_setting_inner(&conn, &key)
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
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    Ok(read_setting_raw(&conn, "tmdb_api_key")?.map(|k| mask_secret(&k)))
}

// ─── 内部ヘルパー：他 Rust モジュールから使う ─────────────────────────────────

pub fn get_api_key_internal(db: &DbState) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    read_setting_raw(&conn, "tmdb_api_key")?
        .ok_or_else(|| "TMDb API キーが設定されていません。設定画面から登録してください。".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::open_migrated;

    fn put(conn: &Connection, key: &str, value: &str) {
        conn.execute(
            "INSERT INTO app_settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )
        .unwrap();
    }

    #[test]
    fn secret_keys_are_not_readable_through_get_setting() {
        let conn = open_migrated();
        put(&conn, "tmdb_api_key", "abcd1234efgh5678");
        put(&conn, "typesafe_api_key", "ts-secret-value");
        put(&conn, "nas_root", r"\\nas\movies");

        for key in ["tmdb_api_key", "TMDB_API_KEY", " tmdb_api_key ", "typesafe_api_key", "some_token"] {
            let result = get_setting_inner(&conn, key);
            assert!(result.is_err(), "{key} must be rejected");
            assert!(!result.unwrap_err().contains("abcd1234efgh5678"));
        }
        assert_eq!(
            get_setting_inner(&conn, "nas_root").unwrap().as_deref(),
            Some(r"\\nas\movies")
        );
    }

    #[test]
    fn masked_value_hides_the_middle() {
        assert_eq!(mask_secret("abcd1234efgh5678"), "abcd…5678");
        assert_eq!(mask_secret("short"), "****");
        assert_eq!(mask_secret("あいうえおかきくけこ"), "あいうえ…きくけこ");
    }
}
