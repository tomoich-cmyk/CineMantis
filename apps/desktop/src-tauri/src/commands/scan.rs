use crate::db::DbState;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager, State};

// Video file extensions to scan
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "m4v", "ts", "m2ts",
    "mpg", "mpeg", "divx", "xvid", "vob", "iso", "rm", "rmvb",
    "webm", "3gp", "ogv",
];

static ACTIVE_SCANS: OnceLock<Mutex<HashSet<i64>>> = OnceLock::new();

struct ScanGuard(i64);

impl ScanGuard {
    fn acquire(source_id: i64) -> Result<Self, String> {
        let mut active = ACTIVE_SCANS
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .map_err(|e| e.to_string())?;
        if !active.is_empty() {
            return Err("別のスキャンが実行中です。完了してから再度実行してください".to_string());
        }
        active.insert(source_id);
        Ok(Self(source_id))
    }
}

impl Drop for ScanGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE_SCANS.get_or_init(|| Mutex::new(HashSet::new())).lock() {
            active.remove(&self.0);
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct ScanProgress {
    pub source_id: i64,
    pub phase: String,       // "walking" | "probing" | "done"
    pub total: usize,
    pub done: usize,
    pub current_file: Option<String>,
    pub new_files: usize,
    pub updated_files: usize,
}

#[derive(Debug, Serialize)]
pub struct ScanResult {
    pub source_id: i64,
    pub scanned: usize,
    pub new_files: usize,
    pub updated_files: usize,
    pub missing_files: usize,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
    size: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    streams: Option<Vec<FfprobeStream>>,
    format: Option<FfprobeFormat>,
}

/// FfprobeOutput から (duration, width, height, video_codec, audio_codec) を抽出
fn extract_probe_info(
    probe: Option<FfprobeOutput>,
) -> (Option<f64>, Option<i64>, Option<i64>, Option<String>, Option<String>) {
    if let Some(p) = probe {
        let dur = p
            .format
            .as_ref()
            .and_then(|f| f.duration.as_ref())
            .and_then(|d| d.parse::<f64>().ok());
        let (mut w, mut h, mut vc, mut ac) = (None, None, None, None);
        if let Some(streams) = &p.streams {
            for s in streams {
                match s.codec_type.as_deref() {
                    Some("video") if vc.is_none() => {
                        w = s.width;
                        h = s.height;
                        vc = s.codec_name.clone();
                    }
                    Some("audio") if ac.is_none() => {
                        ac = s.codec_name.clone();
                    }
                    _ => {}
                }
            }
        }
        (dur, w, h, vc, ac)
    } else {
        (None, None, None, None, None)
    }
}

fn probe_file(path: &str) -> Option<FfprobeOutput> {
    #[cfg(target_os = "windows")]
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let ffprobe = crate::ffmpeg_path::find_ffprobe();
    let mut cmd = Command::new(&ffprobe);
    cmd.args([
        "-v", "quiet",
        "-print_format", "json",
        "-show_streams",
        "-show_format",
        path,
    ])
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null());
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd.output().ok()?;

    if !output.status.success() {
        return None;
    }

    serde_json::from_slice(&output.stdout).ok()
}

fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn walk_video_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut results = Vec::new();
    walk_recursive(root, &mut results);
    results
}

fn walk_recursive(dir: &Path, results: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_recursive(&path, results);
        } else if is_video_file(&path) {
            results.push(path);
        }
    }
}

#[tauri::command]
pub async fn scan_source(
    app: AppHandle,
    state: State<'_, DbState>,
    source_id: i64,
) -> Result<ScanResult, String> {
    let _scan_guard = ScanGuard::acquire(source_id)?;
    let window = app.get_webview_window("main")
        .ok_or("main window not found")?;
    // 1. Get source root path + media_kind
    let (root_path, source_media_kind) = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        let result: Option<(String, String)> = conn
            .query_row(
                "SELECT root_path, COALESCE(media_kind, 'unknown')
                 FROM sources WHERE id = ?1 AND is_enabled = 1",
                rusqlite::params![source_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        result.ok_or_else(|| format!("Source {} not found or disabled", source_id))?
    };
    // ソースの media_kind に応じた works の work_type / media_kind を決定
    // "unknown" のときはファイル名解析結果（title_parser）を使う
    let (source_work_type, source_work_media_kind): (Option<&str>, Option<&str>) =
        match source_media_kind.as_str() {
            "movie" => (Some("movie"), Some("movie")),
            "tv"    => (Some("drama"), Some("tv")),
            _       => (None, None),  // auto-detect from filename
        };

    let root = Path::new(&root_path);
    if !root.exists() {
        // Mark source offline
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE sources SET status = 'offline' WHERE id = ?1",
            rusqlite::params![source_id],
        )
        .map_err(|e| e.to_string())?;
        return Err(format!("Root path does not exist: {}", root_path));
    }

    // オンライン復帰時に、このソースに紐づく保留中の実ファイル操作を先に反映する。
    // 同期失敗はキューに残し、スキャン自体は継続する。
    let _ = crate::commands::sync::process_sync_outbox_inner(&state, Some(source_id));

    // 2. Walk files
    let _ = window.emit(
        "scan:progress",
        ScanProgress {
            source_id,
            phase: "walking".to_string(),
            total: 0,
            done: 0,
            current_file: None,
            new_files: 0,
            updated_files: 0,
        },
    );

    let files = walk_video_files(root);
    let total = files.len();

    // 3. Probe and upsert each file
    let mut new_files = 0usize;
    let mut updated_files = 0usize;
    let mut skipped_files = 0usize;

    for (i, file_path) in files.iter().enumerate() {
        let path_str = file_path.to_string_lossy().to_string();
        let file_name = file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let extension = file_path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        let _ = window.emit(
            "scan:progress",
            ScanProgress {
                source_id,
                phase: "probing".to_string(),
                total,
                done: i,
                current_file: Some(file_name.clone()),
                new_files,
                updated_files,
            },
        );

        // Get file metadata
        let meta = std::fs::metadata(file_path).ok();
        let file_size = meta.as_ref().map(|m| m.len() as i64);
        let mtime = meta.as_ref().and_then(|m| {
            m.modified().ok().map(|t| {
                let secs = t
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                chrono_from_unix(secs)
            })
        });

        let container = extension.to_string();

        // Check existing DB record (size + mtime で変更有無を判断)
        let existing: Option<(i64, i64, String, Option<i64>, Option<String>)> = {
            let conn = state.0.lock().map_err(|e| e.to_string())?;
            conn.query_row(
                "SELECT f.id, f.source_id, s.root_path, f.file_size, f.mtime
                 FROM files f JOIN sources s ON s.id = f.source_id
                 WHERE lower(f.file_path) = lower(?1)
                 ORDER BY length(s.root_path) DESC, f.id ASC LIMIT 1",
                rusqlite::params![path_str],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?
        };

        match existing {
            Some((file_id, existing_source_id, existing_root, db_size, db_mtime)) => {
                if existing_source_id != source_id && root_path.len() > existing_root.len() {
                    let conn = state.0.lock().map_err(|e| e.to_string())?;
                    conn.execute(
                        "UPDATE files SET source_id = ?1 WHERE id = ?2",
                        rusqlite::params![source_id, file_id],
                    )
                    .map_err(|e| e.to_string())?;
                }
                // ファイルが変化していなければ ffprobe をスキップ
                let unchanged = db_size == file_size && db_mtime == mtime;
                if unchanged {
                    // availability_status だけ更新（オフライン→オンライン復帰対応）
                    let conn = state.0.lock().map_err(|e| e.to_string())?;
                    conn.execute(
                        "UPDATE files SET availability_status = 'available' WHERE id = ?1",
                        rusqlite::params![file_id],
                    )
                    .map_err(|e| e.to_string())?;
                    skipped_files += 1;
                } else {
                    // サイズか更新日時が変わった → 再プローブ
                    let probe = probe_file(&path_str);
                    let (duration_sec, width, height, video_codec, audio_codec) =
                        extract_probe_info(probe);
                    let conn = state.0.lock().map_err(|e| e.to_string())?;
                    conn.execute(
                        "UPDATE files SET
                           file_size = ?1, mtime = ?2, duration_sec = ?3,
                           width = ?4, height = ?5, video_codec = ?6,
                           audio_codec = ?7, container = ?8,
                           availability_status = 'available'
                         WHERE id = ?9",
                        rusqlite::params![
                            file_size, mtime, duration_sec, width, height,
                            video_codec, audio_codec, container, file_id
                        ],
                    )
                    .map_err(|e| e.to_string())?;
                    updated_files += 1;
                }
            }
            None => {
                // 新規ファイル → プローブしてINSERT
                let probe = probe_file(&path_str);
                let (duration_sec, width, height, video_codec, audio_codec) =
                    extract_probe_info(probe);

                let conn = state.0.lock().map_err(|e| e.to_string())?;
                conn.execute(
                    "INSERT INTO files
                       (source_id, file_path, file_name, extension, file_size, mtime,
                        duration_sec, width, height, video_codec, audio_codec, container)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    rusqlite::params![
                        source_id, path_str, file_name, extension,
                        file_size, mtime, duration_sec, width, height,
                        video_codec, audio_codec, container
                    ],
                )
                .map_err(|e| e.to_string())?;

                let file_id = conn.last_insert_rowid();

                // Auto-create a Work from filename (title estimation)
                let title = estimate_title(&file_name);
                // ソースの media_kind が設定されていればそれを使い、
                // 未設定 (unknown) ならファイル名から TV パターンを推定する
                let (work_type, work_media_kind) = if let (Some(wt), Some(mk)) =
                    (source_work_type, source_work_media_kind)
                {
                    (wt.to_string(), mk.to_string())
                } else {
                    // title_parser でエピソードパターンを検出して判別
                    let parsed = crate::services::title_parser::parse_title(&title);
                    match parsed.media_kind.as_str() {
                        "tv" => ("drama".to_string(), "tv".to_string()),
                        _    => ("movie".to_string(), "unknown".to_string()),
                    }
                };
                let country_type = infer_country_type_from_path(file_path, root);
                let reading = crate::services::reading::infer_reading(&title);
                conn.execute(
                    "INSERT INTO works (work_type, media_kind, title, sort_title, reading, date_added, media_category, country_type)
                     VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%fZ','now'), ?6, ?7)",
                    rusqlite::params![
                        work_type,
                        work_media_kind,
                        title,
                        title.to_lowercase(),
                        reading,
                        if ["movie", "drama", "ova"].contains(&work_type.as_str()) { work_type.as_str() } else { "other" },
                        country_type,
                    ],
                )
                .map_err(|e| e.to_string())?;

                let work_id = conn.last_insert_rowid();

                // Link via work_parts
                conn.execute(
                    "INSERT INTO work_parts (work_id, file_id) VALUES (?1, ?2)",
                    rusqlite::params![work_id, file_id],
                )
                .map_err(|e| e.to_string())?;

                // Create empty user_stats row
                conn.execute(
                    "INSERT OR IGNORE INTO user_stats (work_id) VALUES (?1)",
                    rusqlite::params![work_id],
                )
                .map_err(|e| e.to_string())?;

                new_files += 1;
            }
        }
    }
    let _ = skipped_files; // suppress unused warning

    // 4. Detect missing files (files in DB not found on disk this scan)
    let missing_files = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        let scanned_paths: Vec<String> = files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        // Mark DB files not in current scan as missing
        let mut stmt = conn
            .prepare(
                "SELECT id, file_path FROM files
                 WHERE source_id = ?1 AND availability_status = 'available'",
            )
            .map_err(|e| e.to_string())?;

        let db_files: Vec<(i64, String)> = stmt
            .query_map(rusqlite::params![source_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?
            .flatten()
            .collect();

        let mut missing = 0usize;
        for (id, path) in &db_files {
            if !scanned_paths.contains(path) {
                conn.execute(
                    "UPDATE files SET availability_status = 'missing' WHERE id = ?1",
                    rusqlite::params![id],
                )
                .map_err(|e| e.to_string())?;
                missing += 1;
            }
        }
        missing
    };

    // 5. Update source scan timestamp
    {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE sources SET
               status = 'online',
               last_scan_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
               last_seen_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1",
            rusqlite::params![source_id],
        )
        .map_err(|e| e.to_string())?;
    }

    let _ = window.emit(
        "scan:progress",
        ScanProgress {
            source_id,
            phase: "done".to_string(),
            total,
            done: total,
            current_file: None,
            new_files,
            updated_files,
        },
    );

    let result = ScanResult {
        source_id,
        scanned: total,
        new_files,
        updated_files,
        missing_files,
    };

    // 6. サムネイル未生成の作品があればバックグラウンドでバッチ生成
    //    （新規ファイルがなくても、thumb_path が NULL の作品がある場合は実行）
    let has_unthumbed = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM works w
             INNER JOIN work_parts wp ON wp.work_id = w.id
             INNER JOIN files f ON f.id = wp.file_id
             WHERE (w.thumb_path IS NULL OR w.thumb_path = '')
               AND f.source_id = ?1
               AND f.availability_status = 'available'",
            rusqlite::params![source_id],
            |row| row.get(0),
        ).unwrap_or(0);
        count > 0
    };

    if has_unthumbed {
        let app_clone = app.clone();
        let db_clone: crate::db::DbState = state.inner().clone();
        tauri::async_runtime::spawn(async move {
            super::thumbnail::generate_thumbnails_batch_inner(
                &app_clone,
                &db_clone,
                Some(source_id),
            )
            .await;
        });
    }

    Ok(result)
}

/// Simple title estimation from filename:
/// - Remove extension
/// - Replace . _ - with spaces
/// - Strip common release tags (1080p, BluRay, x264, …)
fn estimate_title(file_name: &str) -> String {
    let without_ext = file_name
        .rfind('.')
        .map(|i| &file_name[..i])
        .unwrap_or(file_name);

    // Replace separators
    let spaced = without_ext.replace(['.', '_', '-'], " ");

    // Strip known release tags (very basic)
    let junk = [
        "1080p", "720p", "4K", "2160p", "BluRay", "BDRip", "WEBRip", "WEB-DL",
        "HDRip", "DVDRip", "x264", "x265", "HEVC", "AAC", "AC3", "DTS",
        "H264", "H265", "10bit", "8bit", "HDR", "SDR", "REMUX",
    ];
    let mut result = spaced.clone();
    for tag in junk {
        // Case-insensitive replace with space
        let lower = result.to_lowercase();
        if let Some(pos) = lower.find(&tag.to_lowercase()) {
            result = format!("{} {}", &result[..pos], &result[pos + tag.len()..]);
        }
    }

    // Collapse multiple spaces and trim
    let collapsed: String = result
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    collapsed
}

fn infer_country_type_from_path(file_path: &Path, root: &Path) -> &'static str {
    let relative = file_path.strip_prefix(root).unwrap_or(file_path);
    let full = file_path.to_string_lossy().to_lowercase();
    let rel = relative.to_string_lossy().to_lowercase();
    let haystack = format!("{} {}", full, rel);

    let domestic_markers = [
        "邦画",
        "国内",
        "日本映画",
        "日本",
        "japanese",
        "japan",
        "jp-movie",
        "jp_movie",
        "domestic",
    ];
    let foreign_markers = [
        "洋画",
        "海外",
        "外国映画",
        "外国",
        "foreign",
        "western",
        "world cinema",
        "world_cinema",
        "korean",
        "chinese",
        "europe",
        "america",
    ];

    if domestic_markers.iter().any(|marker| haystack.contains(marker)) {
        return "domestic";
    }
    if foreign_markers.iter().any(|marker| haystack.contains(marker)) {
        return "foreign";
    }
    "unknown"
}

/// Format unix timestamp as ISO 8601 string (no chrono dependency)
fn chrono_from_unix(secs: u64) -> String {
    // Simple ISO 8601 UTC formatter without chrono
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;

    // Simplified Gregorian calendar calculation
    let (year, month, day) = days_to_ymd(days_since_epoch as i64);

    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, month, day, h, m, s)
}

fn days_to_ymd(days: i64) -> (i64, i64, i64) {
    // Gregorian calendar conversion (adapted from public domain algorithm)
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
