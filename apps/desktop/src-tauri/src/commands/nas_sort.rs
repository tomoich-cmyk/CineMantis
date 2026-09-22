//! 取り込みフォルダの作品を NAS の五十音フォルダへ振り分ける。
//!
//! 流れ:
//!   1. 取り込み元フォルダを source として登録し、スキャン（既存機能）
//!   2. TMDb 照合・よみ入力で未整理を解消（既存機能）
//!   3. plan_nas_sort で移動先を一覧表示 → ユーザーが確認
//!   4. execute_nas_sort で実際に移動
//!
//! 移動先は必ずサーバ側で再計算する（クライアントから渡されたパスは信用しない）。

use crate::db::DbState;
use crate::services::kana_bucket;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::State;

/// 既定の NAS ルート。app_settings の `nas_movie_root` で上書きできる。
const DEFAULT_NAS_ROOT: &str = r"\\TYNAS\home\Movie";
const SETTING_NAS_ROOT: &str = "nas_movie_root";

const DIR_FOREIGN: &str = "【01】洋画";
const DIR_DOMESTIC: &str = "【02】邦画";

// ─── 行 ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone)]
pub struct NasSortPlanRow {
    pub work_id: i64,
    pub file_id: i64,
    pub title: String,
    pub year: Option<i64>,
    pub reading: Option<String>,
    pub country_type: String,
    pub file_size: Option<i64>,
    /// 現在の絶対パス
    pub source_path: String,
    /// 移動先の絶対パス（決まらない場合は None）
    pub dest_path: Option<String>,
    /// 一覧表示用の相対パス。例: 【01】洋画\【あ-】\【あ】\タイトル (2023).mp4
    pub dest_display: Option<String>,
    /// ready | no_reading | no_country | dest_exists | file_missing | same_path
    pub status: String,
    pub note: Option<String>,
}

#[derive(Debug, Serialize, Default)]
pub struct NasSortResult {
    pub moved: i64,
    pub failed: i64,
    pub skipped: i64,
    pub errors: Vec<NasSortError>,
}

#[derive(Debug, Serialize, Clone)]
pub struct NasSortError {
    pub work_id: i64,
    pub title: String,
    pub message: String,
}

// ─── ファイル名の組み立て ─────────────────────────────────────────────────────

/// Windows で使えない文字を全角に置き換える。
/// 既存 NAS の『ANNA／アナ.mp4』と同じ流儀に合わせる。
fn sanitize_file_stem(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| match c {
            '\\' => '＼',
            '/' => '／',
            ':' => '：',
            '*' => '＊',
            '?' => '？',
            '"' => '”',
            '<' => '＜',
            '>' => '＞',
            '|' => '｜',
            // 制御文字は空白に落とす
            c if (c as u32) < 0x20 => ' ',
            c => c,
        })
        .collect();

    // 連続空白を畳み、Windows が嫌う末尾のドット・空白を削る
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_end_matches(['.', ' ']).to_string();

    // パス長対策。UTF-8 の途中で切らないように char 単位で詰める
    const MAX_CHARS: usize = 120;
    if trimmed.chars().count() > MAX_CHARS {
        trimmed.chars().take(MAX_CHARS).collect::<String>().trim_end().to_string()
    } else {
        trimmed
    }
}

/// `タイトル (年).拡張子` を作る。年が無ければタイトルのみ。
fn build_file_name(title: &str, year: Option<i64>, extension: &str) -> String {
    let stem = match year {
        Some(y) => sanitize_file_stem(&format!("{title} ({y})")),
        None => sanitize_file_stem(title),
    };
    let ext = extension.trim_start_matches('.');
    if ext.is_empty() {
        stem
    } else {
        format!("{stem}.{ext}")
    }
}

// ─── 設定 ─────────────────────────────────────────────────────────────────────

fn nas_root(conn: &rusqlite::Connection) -> String {
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?1",
        rusqlite::params![SETTING_NAS_ROOT],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .map(|v| v.trim().to_string())
    .filter(|v| !v.is_empty())
    .unwrap_or_else(|| DEFAULT_NAS_ROOT.to_string())
}

// ─── プラン作成 ───────────────────────────────────────────────────────────────

struct PlanInput {
    work_id: i64,
    file_id: i64,
    title: String,
    year: Option<i64>,
    reading: Option<String>,
    country_type: String,
    file_size: Option<i64>,
    source_path: String,
    extension: Option<String>,
}

fn build_plan_row(input: PlanInput, root: &str) -> NasSortPlanRow {
    let reading_trimmed = input
        .reading
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_string);

    let mut status = "ready";
    let mut note: Option<String> = None;
    let mut dest_path: Option<String> = None;
    let mut dest_display: Option<String> = None;

    if !Path::new(&input.source_path).exists() {
        status = "file_missing";
        note = Some("元ファイルが見つかりません".to_string());
    } else if input.country_type != "domestic" && input.country_type != "foreign" {
        status = "no_country";
        note = Some("洋邦が未設定です".to_string());
    } else if reading_trimmed.is_none() {
        status = "no_reading";
        note = Some("よみが未設定です".to_string());
    } else {
        let reading = reading_trimmed.as_deref().unwrap_or_default();
        match kana_bucket::bucket_for(reading, &input.country_type) {
            None => {
                status = "no_reading";
                note = Some(format!("よみ『{reading}』から行を判定できません"));
            }
            Some(bucket) => {
                let country_dir = if input.country_type == "domestic" {
                    DIR_DOMESTIC
                } else {
                    DIR_FOREIGN
                };
                let ext = input
                    .extension
                    .as_deref()
                    .map(str::to_string)
                    .or_else(|| {
                        Path::new(&input.source_path)
                            .extension()
                            .map(|e| e.to_string_lossy().to_string())
                    })
                    .unwrap_or_default();
                let file_name = build_file_name(&input.title, input.year, &ext);

                let relative = format!(
                    "{country_dir}\\{}\\{}\\{file_name}",
                    bucket.group_dir, bucket.leaf_dir
                );
                let absolute = PathBuf::from(root).join(&relative);
                let absolute_str = absolute.to_string_lossy().to_string();

                if paths_equal(&absolute_str, &input.source_path) {
                    status = "same_path";
                    note = Some("すでに正しい場所にあります".to_string());
                } else if absolute.exists() {
                    status = "dest_exists";
                    note = Some("移動先に同名ファイルが既にあります".to_string());
                }

                dest_display = Some(relative);
                dest_path = Some(absolute_str);
            }
        }
    }

    NasSortPlanRow {
        work_id: input.work_id,
        file_id: input.file_id,
        title: input.title,
        year: input.year,
        reading: reading_trimmed,
        country_type: input.country_type,
        file_size: input.file_size,
        source_path: input.source_path,
        dest_path,
        dest_display,
        status: status.to_string(),
        note,
    }
}

fn paths_equal(a: &str, b: &str) -> bool {
    // Windows なので大文字小文字とセパレータの違いを無視して比べる
    let norm = |s: &str| s.replace('/', "\\").to_lowercase();
    norm(a) == norm(b)
}

const PLAN_SQL: &str = "SELECT w.id, f.id, w.title, COALESCE(w.release_year, w.year),
            w.reading, COALESCE(w.country_type, 'unknown'),
            f.file_size, f.file_path, f.extension
     FROM works w
     JOIN work_parts wp ON wp.work_id = w.id
     JOIN files f       ON f.id = wp.file_id
     WHERE f.source_id = ?1
       AND f.availability_status = 'available'
     ORDER BY w.reading IS NULL, w.reading, w.title";

fn read_plan_rows(
    conn: &rusqlite::Connection,
    source_id: i64,
    only_work_ids: Option<&[i64]>,
) -> Result<Vec<NasSortPlanRow>, String> {
    let root = nas_root(conn);
    let mut stmt = conn.prepare(PLAN_SQL).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![source_id], |row| {
            Ok(PlanInput {
                work_id: row.get(0)?,
                file_id: row.get(1)?,
                title: row.get(2)?,
                year: row.get(3)?,
                reading: row.get(4)?,
                country_type: row.get(5)?,
                file_size: row.get(6)?,
                source_path: row.get(7)?,
                extension: row.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(rows
        .into_iter()
        .filter(|r| match only_work_ids {
            Some(ids) => ids.contains(&r.work_id),
            None => true,
        })
        .map(|r| build_plan_row(r, &root))
        .collect())
}

/// 振り分け計画を返す（移動はしない）
#[tauri::command]
pub fn plan_nas_sort(
    state: State<'_, DbState>,
    source_id: i64,
) -> Result<Vec<NasSortPlanRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    read_plan_rows(&conn, source_id, None)
}

// ─── 実行 ─────────────────────────────────────────────────────────────────────

/// 同一ボリューム内なら rename、跨ぐ場合は copy + delete でフォールバックする。
fn move_file(src: &Path, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("フォルダを作成できません: {} ({e})", parent.display()))?;
    }

    if std::fs::rename(src, dest).is_ok() {
        return Ok(());
    }

    // ボリュームを跨ぐ場合は rename が失敗するのでコピーしてから元を消す
    std::fs::copy(src, dest).map_err(|e| format!("コピーに失敗しました: {e}"))?;

    // コピー結果のサイズを検証してから元ファイルを消す（欠損コピーの取りこぼし防止）
    let src_len = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);
    let dest_len = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    if src_len != dest_len {
        let _ = std::fs::remove_file(dest);
        return Err(format!(
            "コピー後のサイズが一致しません（元 {src_len} / 先 {dest_len}）。移動を中止しました"
        ));
    }

    std::fs::remove_file(src)
        .map_err(|e| format!("移動後に元ファイルを削除できません: {e}"))?;
    Ok(())
}

/// 移動先パスを含む source を探す。見つからなければ None。
fn find_source_for_path(conn: &rusqlite::Connection, path: &str) -> Option<i64> {
    let mut stmt = conn
        .prepare("SELECT id, root_path FROM sources ORDER BY length(root_path) DESC")
        .ok()?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .ok()?;
    let needle = path.replace('/', "\\").to_lowercase();
    for row in rows.flatten() {
        let root = row.1.replace('/', "\\").to_lowercase();
        if needle.starts_with(&root) {
            return Some(row.0);
        }
    }
    None
}

/// 選択された作品を NAS へ移動する。
/// 移動先はサーバ側で再計算するので、クライアントは work_id だけ渡せばよい。
#[tauri::command]
pub fn execute_nas_sort(
    state: State<'_, DbState>,
    source_id: i64,
    work_ids: Vec<i64>,
) -> Result<NasSortResult, String> {
    if work_ids.is_empty() {
        return Ok(NasSortResult::default());
    }

    let plans = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        read_plan_rows(&conn, source_id, Some(&work_ids))?
    };

    let mut result = NasSortResult::default();

    for plan in plans {
        // ready 以外は触らない（dest_exists や no_reading をここで握り潰さない）
        if plan.status != "ready" {
            result.skipped += 1;
            continue;
        }
        let Some(dest) = plan.dest_path.clone() else {
            result.skipped += 1;
            continue;
        };

        let dest_path = PathBuf::from(&dest);
        // 計画時から状況が変わっていないか直前に再確認する
        if dest_path.exists() {
            result.failed += 1;
            result.errors.push(NasSortError {
                work_id: plan.work_id,
                title: plan.title.clone(),
                message: "移動先に同名ファイルが既にあります".to_string(),
            });
            continue;
        }

        match move_file(Path::new(&plan.source_path), &dest_path) {
            Ok(()) => {
                let conn = state.0.lock().map_err(|e| e.to_string())?;
                let new_source_id =
                    find_source_for_path(&conn, &dest).unwrap_or(source_id);
                let file_name = dest_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                conn.execute(
                    "UPDATE files
                     SET source_id = ?1,
                         file_path = ?2,
                         file_name = ?3,
                         availability_status = 'available',
                         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                     WHERE id = ?4",
                    rusqlite::params![new_source_id, dest, file_name, plan.file_id],
                )
                .map_err(|e| e.to_string())?;
                result.moved += 1;
            }
            Err(message) => {
                result.failed += 1;
                result.errors.push(NasSortError {
                    work_id: plan.work_id,
                    title: plan.title.clone(),
                    message,
                });
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_illegal_characters_with_fullwidth() {
        assert_eq!(sanitize_file_stem("ANNA/アナ"), "ANNA／アナ");
        assert_eq!(sanitize_file_stem("A:B*C?D"), "A：B＊C？D");
    }

    #[test]
    fn trims_trailing_dots_and_spaces() {
        assert_eq!(sanitize_file_stem("タイトル... "), "タイトル");
        assert_eq!(sanitize_file_stem("  空白  畳み  "), "空白 畳み");
    }

    #[test]
    fn builds_name_with_and_without_year() {
        assert_eq!(
            build_file_name("エクソシスト 信じる者", Some(2023), "mp4"),
            "エクソシスト 信じる者 (2023).mp4"
        );
        assert_eq!(build_file_name("タイトル", None, ".mkv"), "タイトル.mkv");
    }

    #[test]
    fn path_comparison_ignores_case_and_separator() {
        assert!(paths_equal(r"\\NAS\A\b.mp4", r"//nas/a/B.MP4"));
        assert!(!paths_equal(r"\\NAS\A\b.mp4", r"\\NAS\A\c.mp4"));
    }

    fn input(title: &str, reading: Option<&str>, country: &str) -> PlanInput {
        PlanInput {
            work_id: 1,
            file_id: 2,
            title: title.to_string(),
            year: Some(2023),
            reading: reading.map(str::to_string),
            country_type: country.to_string(),
            file_size: Some(100),
            // 存在しないパスなので file_missing になる。分岐の確認用。
            source_path: r"C:\does-not-exist\x.mp4".to_string(),
            extension: Some("mp4".to_string()),
        }
    }

    #[test]
    fn missing_source_file_is_flagged_first() {
        let row = build_plan_row(input("あ", Some("あい"), "foreign"), DEFAULT_NAS_ROOT);
        assert_eq!(row.status, "file_missing");
        assert!(row.dest_path.is_none());
    }

    #[test]
    fn unknown_country_is_flagged() {
        // file_missing が先に出ないよう、存在するパスを使う
        let mut i = input("あ", Some("あい"), "unknown");
        i.source_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let row = build_plan_row(i, DEFAULT_NAS_ROOT);
        assert_eq!(row.status, "no_country");
    }

    #[test]
    fn missing_reading_is_flagged() {
        let mut i = input("戦争と平和", None, "domestic");
        i.source_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let row = build_plan_row(i, DEFAULT_NAS_ROOT);
        assert_eq!(row.status, "no_reading");
    }

    #[test]
    fn builds_destination_under_country_and_kana_dirs() {
        let mut i = input("エクソシスト 信じる者", Some("えくそしすと"), "foreign");
        i.source_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let row = build_plan_row(i, r"\\NAS\Movie");
        assert_eq!(
            row.dest_display.as_deref(),
            Some(r"【01】洋画\【あ-】\【え】\エクソシスト 信じる者 (2023).mp4")
        );
    }

    #[test]
    fn domestic_goes_under_second_dir() {
        let mut i = input("わたしの話", Some("わたしのはなし"), "domestic");
        i.source_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let row = build_plan_row(i, r"\\NAS\Movie");
        // 邦画のわ行は 【わ-】（洋画は 【わ】）
        assert_eq!(
            row.dest_display.as_deref(),
            Some(r"【02】邦画\【わ-】\【わ】\わたしの話 (2023).mp4")
        );
    }
}
