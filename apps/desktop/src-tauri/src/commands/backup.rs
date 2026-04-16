use serde::Serialize;
use std::path::PathBuf;
use tauri::Manager;

// ─── BackupEntry ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct BackupEntry {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub created_at: String, // ISO 8601
}

// ─── backup_database ─────────────────────────────────────────────────────────

/// DB を backups/ ディレクトリへバックアップする（VACUUM INTO で整合コピー）。
/// label を指定するとファイル名に付加される。
#[tauri::command]
pub fn backup_database(
    app: tauri::AppHandle,
    state: tauri::State<crate::db::DbState>,
    label: Option<String>,
) -> Result<String, String> {
    let app_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let backups_dir = app_dir.join("backups");
    std::fs::create_dir_all(&backups_dir).map_err(|e| e.to_string())?;

    // タイムスタンプ付きファイル名
    let now = chrono_lite_now();
    let suffix = match &label {
        Some(l) if !l.trim().is_empty() => format!("_{}", l.trim().replace(' ', "_")),
        _ => String::new(),
    };
    let filename = format!("cinemantis_{}{}.db", now, suffix);
    let backup_path = backups_dir.join(&filename);

    // VACUUM INTO でクリーンコピー（接続を閉じずに済む）
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        &format!("VACUUM INTO '{}'", backup_path.to_string_lossy().replace('\'', "''")),
        [],
    )
    .map_err(|e| e.to_string())?;

    Ok(backup_path.to_string_lossy().into_owned())
}

// ─── list_backups ─────────────────────────────────────────────────────────────

#[tauri::command]
pub fn list_backups(app: tauri::AppHandle) -> Result<Vec<BackupEntry>, String> {
    let app_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let backups_dir = app_dir.join("backups");
    if !backups_dir.exists() {
        return Ok(vec![]);
    }

    let mut entries: Vec<BackupEntry> = std::fs::read_dir(&backups_dir)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension()?.to_str()? != "db" {
                return None;
            }
            let meta = std::fs::metadata(&path).ok()?;
            let created_at = meta
                .modified()
                .ok()
                .map(|t| {
                    let secs = t
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    unix_to_iso(secs)
                })
                .unwrap_or_default();
            Some(BackupEntry {
                name: path.file_name()?.to_string_lossy().into_owned(),
                path: path.to_string_lossy().into_owned(),
                size_bytes: meta.len(),
                created_at,
            })
        })
        .collect();

    // 新しい順
    entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(entries)
}

// ─── restore_database ─────────────────────────────────────────────────────────

/// 指定したバックアップを pending_restore としてマークする。
/// アプリを再起動すると自動的に適用される。
#[tauri::command]
pub fn restore_database(
    app: tauri::AppHandle,
    backup_path: String,
) -> Result<(), String> {
    // バックアップファイルの存在確認
    let src = PathBuf::from(&backup_path);
    if !src.exists() {
        return Err(format!("バックアップファイルが見つかりません: {backup_path}"));
    }

    let app_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let pending = app_dir.join("cinemantis_restore_pending.db");
    std::fs::copy(&src, &pending).map_err(|e| e.to_string())?;

    Ok(())
}

// ─── delete_backup ────────────────────────────────────────────────────────────

#[tauri::command]
pub fn delete_backup(app: tauri::AppHandle, backup_path: String) -> Result<(), String> {
    let app_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let backups_dir = app_dir.join("backups");
    let target = PathBuf::from(&backup_path);

    // セキュリティ: backups/ ディレクトリ内のファイルのみ削除可
    if !target.starts_with(&backups_dir) {
        return Err("バックアップディレクトリ外のファイルは削除できません".to_string());
    }
    std::fs::remove_file(&target).map_err(|e| e.to_string())?;
    Ok(())
}

// ─── ユーティリティ ───────────────────────────────────────────────────────────

/// YYYYMMDD_HHmmss 形式の現在時刻文字列
fn chrono_lite_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    unix_to_compact(secs)
}

fn unix_to_compact(secs: u64) -> String {
    // 簡易実装: Unix 秒 → YYYYMMDD_HHmmss
    let secs = secs as i64;
    let (y, mo, d, h, mi, s) = ymd_hms(secs);
    format!("{:04}{:02}{:02}_{:02}{:02}{:02}", y, mo, d, h, mi, s)
}

fn unix_to_iso(secs: u64) -> String {
    let secs = secs as i64;
    let (y, mo, d, h, mi, s) = ymd_hms(secs);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, mi, s)
}

/// Unix 秒 → (year, month, day, hour, min, sec) — UTC, no external crate
fn ymd_hms(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let s = secs % 60;
    let total_min = secs / 60;
    let mi = (total_min % 60) as u32;
    let total_h = total_min / 60;
    let h = (total_h % 24) as u32;
    let mut days = total_h / 24; // days since epoch

    // Gregorian calendar calculation
    let mut y: i32 = 1970;
    loop {
        let dy = if is_leap(y) { 366 } else { 365 };
        if days < dy { break; }
        days -= dy;
        y += 1;
    }
    let months = if is_leap(y) {
        [31i64, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31i64, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut mo: u32 = 1;
    for &m in &months {
        if days < m { break; }
        days -= m;
        mo += 1;
    }
    (y, mo, days as u32 + 1, h, mi, s as u32)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
