use crate::cache::CacheDir;
use crate::db::DbState;
use rusqlite::OptionalExtension;
use serde::Serialize;
use std::process::Command;
use tauri::{AppHandle, Emitter, State};

// ─── 公開イベント型 ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone)]
pub struct ThumbGenerated {
    pub work_id: i64,
    pub thumb_path: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct ThumbBatchProgress {
    pub total: usize,
    pub done: usize,
    pub failed: usize,
}

// ─── 内部ヘルパー ────────────────────────────────────────────────────────────

/// ffmpeg で動画の指定秒数からフレームを JPEG として書き出す
fn run_ffmpeg(input_path: &str, offset_sec: f64, output_path: &str) -> bool {
    Command::new("ffmpeg")
        .args([
            "-y",
            "-ss", &format!("{offset_sec:.3}"),
            "-i", input_path,
            "-vframes", "1",
            "-vf", "scale=480:-2",
            "-q:v", "3",
            output_path,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// duration × ratio で offset を求める。不明なら30秒固定
fn calc_offset(duration_sec: Option<f64>, ratio: f64) -> f64 {
    duration_sec
        .filter(|&d| d > 0.0)
        .map(|d| d * ratio)
        .unwrap_or(30.0)
        .max(1.0)
}

// ─── inner（spawn からも呼べる） ─────────────────────────────────────────────

/// バッチ生成の本体。Tauri コマンドと scan.rs バックグラウンドジョブの両方から呼ぶ。
pub async fn generate_thumbnails_batch_inner(
    app: &AppHandle,
    db: &DbState,
    source_id: Option<i64>,
) -> ThumbBatchProgress {
    let cache = match CacheDir::new(app) {
        Ok(c) => c,
        Err(_) => return ThumbBatchProgress { total: 0, done: 0, failed: 0 },
    };

    // サムネ未生成 works を取得
    let targets: Vec<(i64, String, Option<f64>)> = {
        let Ok(conn) = db.0.lock() else {
            return ThumbBatchProgress { total: 0, done: 0, failed: 0 };
        };

        let result = if let Some(sid) = source_id {
            let mut stmt = conn.prepare(
                "SELECT w.id, f.file_path, f.duration_sec
                 FROM works w
                 INNER JOIN work_parts wp ON wp.work_id = w.id
                 INNER JOIN files f ON f.id = wp.file_id
                 WHERE (w.thumb_path IS NULL OR w.thumb_path = '')
                   AND f.source_id = ?1
                   AND f.availability_status = 'available'
                 GROUP BY w.id ORDER BY w.id",
            ).ok();
            stmt.as_mut().and_then(|s| {
                s.query_map(rusqlite::params![sid], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                }).ok().map(|it| it.flatten().collect::<Vec<_>>())
            }).unwrap_or_default()
        } else {
            let mut stmt = conn.prepare(
                "SELECT w.id, f.file_path, f.duration_sec
                 FROM works w
                 INNER JOIN work_parts wp ON wp.work_id = w.id
                 INNER JOIN files f ON f.id = wp.file_id
                 WHERE (w.thumb_path IS NULL OR w.thumb_path = '')
                   AND f.availability_status = 'available'
                 GROUP BY w.id ORDER BY w.id",
            ).ok();
            stmt.as_mut().and_then(|s| {
                s.query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                }).ok().map(|it| it.flatten().collect::<Vec<_>>())
            }).unwrap_or_default()
        };
        result
    };

    let total = targets.len();
    let mut done = 0usize;
    let mut failed = 0usize;

    let _ = app.emit("thumb:batch_progress", ThumbBatchProgress { total, done, failed });

    for (work_id, file_path, duration_sec) in targets {
        let out_path = cache.thumb_path(work_id);
        let out_str = out_path.to_string_lossy().to_string();

        if out_path.exists() {
            if let Ok(conn) = db.0.lock() {
                let _ = conn.execute(
                    "UPDATE works SET thumb_path = ?1 WHERE id = ?2 AND (thumb_path IS NULL OR thumb_path = '')",
                    rusqlite::params![out_str, work_id],
                );
            }
            done += 1;
        } else {
            let offset = calc_offset(duration_sec, 0.30);
            let ok = run_ffmpeg(&file_path, offset, &out_str)
                || run_ffmpeg(&file_path, calc_offset(duration_sec, 0.10), &out_str);

            if ok {
                if let Ok(conn) = db.0.lock() {
                    let _ = conn.execute(
                        "UPDATE works SET thumb_path = ?1 WHERE id = ?2",
                        rusqlite::params![out_str, work_id],
                    );
                }
                let _ = app.emit("thumb:generated", ThumbGenerated {
                    work_id,
                    thumb_path: out_str,
                });
                done += 1;
            } else {
                failed += 1;
            }
        }

        let _ = app.emit("thumb:batch_progress", ThumbBatchProgress { total, done, failed });

        // CPU を独占しないよう 1 ms 待機
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }

    let progress = ThumbBatchProgress { total, done, failed };

    // サムネ完了後、メタデータバッチを自動起動（APIキーが設定済みの場合のみ）
    let app_clone = app.clone();
    let db_clone = db.clone();
    tauri::async_runtime::spawn(async move {
        // サムネとメタデータを同時に叩かないよう少し待つ
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let _ = crate::commands::tmdb::auto_match_source_inner(
            &app_clone,
            &db_clone,
            source_id,
        )
        .await;
    });

    progress
}

// ─── Tauri コマンド ──────────────────────────────────────────────────────────

/// 単体作品のサムネイルを生成する
#[tauri::command]
pub async fn generate_thumbnail(
    app: AppHandle,
    state: State<'_, DbState>,
    work_id: i64,
) -> Result<String, String> {
    let cache = CacheDir::new(&app)?;
    let out_path = cache.thumb_path(work_id);

    if out_path.exists() {
        return Ok(out_path.to_string_lossy().to_string());
    }

    let (file_path, duration_sec) = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT f.file_path, f.duration_sec
             FROM work_parts wp
             INNER JOIN files f ON f.id = wp.file_id
             WHERE wp.work_id = ?1
             ORDER BY wp.play_order ASC LIMIT 1",
            rusqlite::params![work_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<f64>>(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("work {work_id}: no file found"))?
    };

    let offset = calc_offset(duration_sec, 0.30);
    let out_str = out_path.to_string_lossy().to_string();

    let ok = run_ffmpeg(&file_path, offset, &out_str)
        || run_ffmpeg(&file_path, calc_offset(duration_sec, 0.10), &out_str);

    if !ok {
        return Err(format!("ffmpeg failed for work {work_id}"));
    }

    {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE works SET thumb_path = ?1 WHERE id = ?2",
            rusqlite::params![out_str, work_id],
        )
        .map_err(|e| e.to_string())?;
    }

    let _ = app.emit("thumb:generated", ThumbGenerated {
        work_id,
        thumb_path: out_str.clone(),
    });

    Ok(out_str)
}

/// ソース単位でサムネイル未生成の作品を一括生成する（Tauri コマンド版）
#[tauri::command]
pub async fn generate_thumbnails_batch(
    app: AppHandle,
    state: State<'_, DbState>,
    source_id: Option<i64>,
) -> Result<ThumbBatchProgress, String> {
    Ok(generate_thumbnails_batch_inner(&app, &state, source_id).await)
}

/// works.thumb_path を指定パスに上書き（手動差し替え用）
#[tauri::command]
pub fn set_custom_thumb(
    state: State<'_, DbState>,
    work_id: i64,
    path: String,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE works SET thumb_path = ?1 WHERE id = ?2",
        rusqlite::params![path, work_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
