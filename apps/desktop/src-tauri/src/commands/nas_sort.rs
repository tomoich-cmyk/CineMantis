//! 取り込みフォルダの作品を NAS の五十音フォルダへ振り分ける。
//!
//! 流れ:
//!   1. 取り込み元フォルダを含むフォルダを source として登録し、スキャン（既存機能）
//!   2. TMDb 照合・よみ入力で未整理を解消（既存機能）
//!   3. plan_nas_sort で、ユーザーが選んだ取り込み元フォルダ配下の作品の移動先を一覧表示
//!   4. execute_nas_sort で実際に移動
//!
//! 移動先は必ずサーバ側で再計算する（クライアントから渡されたパスは信用しない）。

use crate::db::DbState;
use crate::services::kana_bucket;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::State;

/// NAS ルートは app_settings の `nas_movie_root` だけが持つ。既定値は無い（未設定なら振り分けは実行しない）。
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
    /// works.match_status（振り分け画面から個別照合するときの表示用）
    pub match_status: String,
}

#[derive(Debug, Serialize, Default)]
pub struct NasSortResult {
    pub moved: i64,
    pub failed: i64,
    pub skipped: i64,
    pub errors: Vec<NasSortError>,
    /// 移動先を含むソースが無かったため新しく登録したソースのルート
    pub registered_sources: Vec<String>,
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

/// NAS ルートの設定値を検証する。空・空白のみ・相対パス・`..` を含むパスは拒否する
/// （カレントディレクトリや相対パスとして解釈させない）。
/// 受け付けるのは UNC（`\\server\share[\…]`）かドライブ絶対パス（`X:\…`）だけ。
fn validate_nas_root(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("保存先が未設定です。設定画面で NAS の保存先を設定してください".to_string());
    }
    let invalid = || {
        format!(
            "保存先が絶対パスではありません。UNC（\\\\server\\share）またはドライブ指定（D:\\…）で設定してください: {trimmed}"
        )
    };
    let norm = trimmed.replace('/', "\\");
    let ok = match norm.strip_prefix("\\\\") {
        Some(rest) => {
            // server と share が必須。`\\?\` `\\.\` のデバイス名前空間は受け付けない
            let mut it = rest.split('\\').filter(|p| !p.is_empty());
            let server = it.next().unwrap_or("");
            let share = it.next().unwrap_or("");
            !server.is_empty() && !share.is_empty() && server != "?" && server != "."
        }
        None => {
            let b = norm.as_bytes();
            b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
        }
    };
    if !ok || norm.split('\\').any(|p| p == "..") {
        return Err(invalid());
    }
    Ok(trimmed.trim_end_matches(['\\', '/']).to_string())
}

/// 保存済みの NAS ルートを読む。未設定・不正なら Err（呼び出し側はファイル操作へ進まない）。
fn nas_root(conn: &rusqlite::Connection) -> Result<String, String> {
    let raw = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = ?1",
            rusqlite::params![SETTING_NAS_ROOT],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_default();
    validate_nas_root(&raw)
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
    match_status: String,
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
        match_status: input.match_status,
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
            f.file_size, f.file_path, f.extension, w.match_status
     FROM works w
     JOIN work_parts wp ON wp.work_id = w.id
     JOIN files f       ON f.id = wp.file_id
     WHERE f.availability_status = 'available'
     ORDER BY w.reading IS NULL, w.reading, w.title";

/// ユーザーが選んだ取り込み元フォルダ（絶対パス）を検証する
fn validate_folder(folder: &str) -> Result<String, String> {
    let folder = folder.trim().trim_end_matches(['\\', '/']).to_string();
    // UNC（\\server\share）かドライブ指定（C:\…）だけを受け付ける
    let absolute = folder.starts_with(r"\\") || folder.as_bytes().get(1) == Some(&b':');
    if folder.is_empty() || !absolute {
        return Err("取り込み元フォルダを絶対パスで指定してください".to_string());
    }
    Ok(folder)
}

/// 取り込み元フォルダ配下（サブフォルダを含む）にあるファイルの振り分け計画を作る
fn read_plan_rows(
    conn: &rusqlite::Connection,
    folder: &str,
    only_work_ids: Option<&[i64]>,
    root: &str,
) -> Result<Vec<NasSortPlanRow>, String> {
    let mut stmt = conn.prepare(PLAN_SQL).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
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
                match_status: row.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(rows
        .into_iter()
        .filter(|r| {
            crate::services::prematch_inputs::relative_to_root(folder, &r.source_path).is_some()
        })
        .filter(|r| match only_work_ids {
            Some(ids) => ids.contains(&r.work_id),
            None => true,
        })
        .map(|r| build_plan_row(r, root))
        .collect())
}

#[derive(Debug, Serialize)]
pub struct NasSortPlan {
    pub rows: Vec<NasSortPlanRow>,
    /// 取り込み元フォルダを含む登録済みソースのルート。未登録なら None（先にソース登録とスキャンが必要）
    pub source_root: Option<String>,
    /// 移動先の NAS ルート（未設定・不正なら空文字）
    pub nas_root: String,
    /// NAS ルートが設定済みで有効な絶対パスか。false なら計画は空で、移動は実行しない
    pub nas_configured: bool,
    /// 未設定・不正のときの理由（UI 表示用）
    pub nas_config_error: Option<String>,
    /// NAS に到達できるか。false なら移動は実行しない
    pub nas_available: bool,
}

/// NAS ルートに到達できるか確かめる。
/// NAS が落ちているときに1件ずつ移動を試して全部失敗する（os error 53）のを防ぐため、
/// 計画の表示時と実行前に1回だけ確認する。
fn check_nas_reachable(root: &str) -> Result<(), String> {
    match std::fs::metadata(root) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(format!("NAS のルートがフォルダではありません: {root}")),
        Err(error) => Err(format!(
            "NAS に接続できません（{root}）。接続を確認してからやり直してください: {error}"
        )),
    }
}

/// 振り分け計画を返す（移動はしない）
#[tauri::command]
pub fn plan_nas_sort(state: State<'_, DbState>, folder: String) -> Result<NasSortPlan, String> {
    let folder = validate_folder(&folder)?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let root = match nas_root(&conn) {
        Ok(root) => root,
        Err(message) => {
            return Ok(NasSortPlan {
                rows: Vec::new(),
                source_root: None,
                nas_root: String::new(),
                nas_configured: false,
                nas_config_error: Some(message),
                nas_available: false,
            });
        }
    };
    let rows = read_plan_rows(&conn, &folder, None, &root)?;
    let source_root = find_source_for_path(&conn, &folder).and_then(|id| {
        conn.query_row(
            "SELECT root_path FROM sources WHERE id = ?1",
            rusqlite::params![id],
            |row| row.get::<_, String>(0),
        )
        .ok()
    });
    let nas_available = check_nas_reachable(&root).is_ok();
    Ok(NasSortPlan {
        rows,
        source_root,
        nas_root: root,
        nas_configured: true,
        nas_config_error: None,
        nas_available,
    })
}

// ─── 実行 ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
thread_local! {
    /// テスト用: ファイル操作（rename / copy / remove_file）の唯一の入口 move_file の呼び出し回数
    static MOVE_FILE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 同一ボリューム内なら rename、跨ぐ場合は copy + delete でフォールバックする。
fn move_file(src: &Path, dest: &Path) -> Result<(), String> {
    #[cfg(test)]
    MOVE_FILE_CALLS.with(|c| c.set(c.get() + 1));
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
    let needle = path.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    for row in rows.flatten() {
        let root = row.1.replace('/', "\\").trim_end_matches('\\').to_lowercase();
        // 「【わ】」と「【わ-】」のような前方一致の取り違えを防ぐため、区切りまで一致させる
        if needle == root || needle.starts_with(&format!("{root}\\")) {
            return Some(row.0);
        }
    }
    None
}

/// 移動先の行フォルダ（例: 【02】邦画\【さ-】）をソースとして登録し、その ID とルートを返す。
/// 既存の洋画側と同じ「国\行」単位にそろえる。既存ソースと範囲が重なる、
/// または NAS ルートの外などで登録できない場合は None。
fn ensure_bucket_source(
    conn: &rusqlite::Connection,
    nas_root: &str,
    dest: &str,
) -> Option<(i64, String)> {
    let relative = crate::services::prematch_inputs::relative_to_root(nas_root, dest)?;
    let parts: Vec<&str> = relative.split('/').collect();
    // 国\行\段\ファイル名 の形でなければ登録しない
    if parts.len() < 4 {
        return None;
    }
    let group_root = PathBuf::from(nas_root)
        .join(parts[0])
        .join(parts[1])
        .to_string_lossy()
        .to_string();

    let existing: Vec<String> = conn
        .prepare("SELECT root_path FROM sources")
        .ok()?
        .query_map([], |row| row.get::<_, String>(0))
        .ok()?
        .collect::<Result<_, _>>()
        .ok()?;
    if existing
        .iter()
        .any(|root| crate::commands::source::paths_overlap(root, &group_root))
    {
        return None;
    }

    conn.execute(
        "INSERT INTO sources (name, root_path, source_type, media_kind) VALUES (?1, ?2, 'local', 'unknown')",
        rusqlite::params![parts[1], group_root],
    )
    .ok()?;
    Some((conn.last_insert_rowid(), group_root))
}

/// 選択された作品を NAS へ移動する。
/// 移動先はサーバ側で再計算するので、クライアントは work_id だけ渡せばよい。
#[tauri::command]
pub fn execute_nas_sort(
    state: State<'_, DbState>,
    folder: String,
    work_ids: Vec<i64>,
) -> Result<NasSortResult, String> {
    execute_nas_sort_impl(&state.0, folder, work_ids)
}

fn execute_nas_sort_impl(
    db: &std::sync::Mutex<rusqlite::Connection>,
    folder: String,
    work_ids: Vec<i64>,
) -> Result<NasSortResult, String> {
    if work_ids.is_empty() {
        return Ok(NasSortResult::default());
    }
    let folder = validate_folder(&folder)?;

    let plans = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        // NAS ルート未設定・不正ならファイル操作へ進まない（最初のガード）
        let root = nas_root(&conn)?;
        // NAS が落ちているまま1件ずつ移動を試すと、全件が同じエラーで失敗する
        check_nas_reachable(&root)?;
        read_plan_rows(&conn, &folder, Some(&work_ids), &root)?
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
                let conn = db.lock().map_err(|e| e.to_string())?;
                // 移動先を含むソースが無ければ、今のソースに残す
                let current_source_id: i64 = conn
                    .query_row(
                        "SELECT source_id FROM files WHERE id = ?1",
                        rusqlite::params![plan.file_id],
                        |row| row.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                // 移動先を含むソースが無ければ、移動先の行フォルダをソースとして登録する
                // （ライブラリから外れて次回スキャンで「実体なし」にならないように）
                let new_source_id = match find_source_for_path(&conn, &dest) {
                    Some(id) => id,
                    None => match nas_root(&conn)
                        .ok()
                        .and_then(|root| ensure_bucket_source(&conn, &root, &dest))
                    {
                        Some((id, root)) => {
                            result.registered_sources.push(root);
                            id
                        }
                        None => current_source_id,
                    },
                };
                let file_name = dest_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                record_moved_file(&conn, plan.file_id, new_source_id, &dest, &file_name)
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

/// 移動後の場所を files に反映する。照合前入力の original_* は変更しない。
/// ファイル名を CineMantis が書き換えた場合は renamed_by_app に印を残す
/// （元ファイル名が失われた作品を、照合の評価から外すため）。
fn record_moved_file(
    conn: &rusqlite::Connection,
    file_id: i64,
    source_id: i64,
    dest: &str,
    file_name: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE files
         SET source_id = ?1,
             file_path = ?2,
             file_name = ?3,
             renamed_by_app = CASE
                                WHEN original_file_name IS NOT NULL AND original_file_name <> ?3
                                  THEN 'nas_sort'
                                ELSE renamed_by_app
                              END,
             availability_status = 'available',
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?4",
        rusqlite::params![source_id, dest, file_name, file_id],
    )
}

#[cfg(test)]
mod tests {
    const TEST_ROOT: &str = r"\\nas-host\media\Movie";
    use super::*;

    #[test]
    fn source_lookup_requires_a_path_boundary() {
        use crate::db::test_support::*;
        let conn = open_migrated();
        let wa = insert_source(&conn, r"\\nas-host\media\Movie\【01】洋画\【わ】");
        let a = insert_source(&conn, r"\\nas-host\media\Movie\【01】洋画\【あ-】");
        assert_eq!(
            find_source_for_path(&conn, r"\\nas-host\media\Movie\【01】洋画\【わ】\【わ】\x.mp4"),
            Some(wa)
        );
        assert_eq!(find_source_for_path(&conn, r"\\nas-host\MEDIA\Movie\【01】洋画\【あ-】"), Some(a));
        assert_eq!(
            find_source_for_path(&conn, r"\\nas-host\media\Movie\【01】洋画\【わ-】\【わ】\x.mp4"),
            None
        );
    }

    #[test]
    fn moving_to_an_unregistered_bucket_registers_it_once() {
        use crate::db::test_support::*;
        let conn = open_migrated();
        let root = r"\\nas-host\media\Movie";
        insert_source(&conn, r"\\nas-host\media\Movie\【01】洋画\【さ-】");
        let dest = r"\\nas-host\media\Movie\【02】邦画\【さ-】\【さ】\THE FIRST SLAM DUNK (2022).mp4";

        let (id, group_root) = ensure_bucket_source(&conn, root, dest).expect("registered");
        assert_eq!(group_root, r"\\nas-host\media\Movie\【02】邦画\【さ-】");
        assert_eq!(find_source_for_path(&conn, dest), Some(id));
        // 2回目は既存ソースと重なるので登録しない
        assert!(ensure_bucket_source(&conn, root, dest).is_none());
        // NAS ルートの外は登録しない
        assert!(ensure_bucket_source(&conn, root, r"D:\Other\a\b\c.mp4").is_none());
    }

    /// NAS が落ちているときは、移動を1件も試さずに止める
    #[test]
    fn unreachable_nas_root_is_detected_before_moving() {
        let existing = std::env::temp_dir();
        check_nas_reachable(&existing.to_string_lossy()).expect("temp dir is reachable");

        let missing = existing.join("cinemantis-nas-not-there-4f2a");
        let error = check_nas_reachable(&missing.to_string_lossy()).unwrap_err();
        assert!(error.contains("NAS に接続できません"), "{error}");
    }

    #[test]
    fn folder_must_be_absolute() {
        assert_eq!(validate_folder(r"D:\Import\").unwrap(), r"D:\Import");
        assert_eq!(validate_folder(r"\\nas-host\media\Movie").unwrap(), r"\\nas-host\media\Movie");
        assert!(validate_folder("").is_err());
        assert!(validate_folder(r"Import\sub").is_err());
    }

    #[test]
    fn plan_only_includes_files_under_the_chosen_folder() {
        use crate::commands::scan::{insert_scanned_file, insert_scanned_work, ScannedFile};
        use crate::db::test_support::*;

        let conn = open_migrated();
        let root = r"D:\Import";
        let source_id = insert_source(&conn, root);
        let add = |path: &str, name: &str| {
            let file_id = insert_scanned_file(
                &conn,
                &ScannedFile {
                    source_id,
                    root_path: root,
                    file_path: path,
                    file_name: name,
                    extension: "mkv",
                    file_size: None,
                    mtime: None,
                    duration_sec: None,
                    width: None,
                    height: None,
                    video_codec: None,
                    audio_codec: None,
                    container: "mkv",
                },
            )
            .unwrap();
            let work_id = insert_scanned_work(&conn, "movie", "unknown", name, "foreign").unwrap();
            conn.execute(
                "INSERT INTO work_parts (work_id, file_id) VALUES (?1, ?2)",
                rusqlite::params![work_id, file_id],
            )
            .unwrap();
            work_id
        };
        let in_folder = add(r"D:\Import\新着\a.mkv", "a.mkv");
        let in_subfolder = add(r"D:\Import\新着\sub\b.mkv", "b.mkv");
        let outside = add(r"D:\Import\保留\c.mkv", "c.mkv");
        let similar_name = add(r"D:\Import\新着2\d.mkv", "d.mkv");

        let ids: Vec<i64> = read_plan_rows(&conn, r"d:\import\新着", None, r"D:\NasRoot")
            .unwrap()
            .iter()
            .map(|r| r.work_id)
            .collect();
        assert!(ids.contains(&in_folder));
        assert!(ids.contains(&in_subfolder));
        assert!(!ids.contains(&outside));
        assert!(!ids.contains(&similar_name));
    }

    #[test]
    fn moving_a_file_keeps_original_inputs() {
        use crate::commands::scan::{insert_scanned_file, ScannedFile};
        use crate::db::test_support::*;

        let conn = open_migrated();
        let root = r"D:\Movies";
        let source_id = insert_source(&conn, root);
        let file_id = insert_scanned_file(
            &conn,
            &ScannedFile {
                source_id,
                root_path: root,
                file_path: r"D:\Movies\Alien.1979.1080p.mkv",
                file_name: "Alien.1979.1080p.mkv",
                extension: "mkv",
                file_size: None,
                mtime: None,
                duration_sec: None,
                width: None,
                height: None,
                video_codec: None,
                audio_codec: None,
                container: "mkv",
            },
        )
        .unwrap();

        let dest = r"D:\Movies\洋画\あ\エイリアン (1979).mkv";
        record_moved_file(&conn, file_id, source_id, dest, &build_file_name("エイリアン", Some(1979), "mkv"))
            .unwrap();

        let row: (String, String, String, String, i64, Option<String>) = conn
            .query_row(
                "SELECT file_name, file_path, original_file_name, original_rel_path,
                        original_captured, renamed_by_app
                 FROM files WHERE id = ?1",
                rusqlite::params![file_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!(row.0, "エイリアン (1979).mkv");
        assert_eq!(row.1, dest);
        assert_eq!(row.2, "Alien.1979.1080p.mkv");
        assert_eq!(row.3, "Alien.1979.1080p.mkv");
        assert_eq!(row.4, 1);
        // 改名したので、この作品は照合の評価に使えない
        assert_eq!(row.5.as_deref(), Some("nas_sort"));
    }

    /// 名前が変わらない移動では改名の印を付けない
    #[test]
    fn moving_without_renaming_keeps_the_file_usable_for_evaluation() {
        use crate::commands::scan::{insert_scanned_file, ScannedFile};
        use crate::db::test_support::*;

        let conn = open_migrated();
        let root = r"D:\Movies";
        let source_id = insert_source(&conn, root);
        let file_id = insert_scanned_file(
            &conn,
            &ScannedFile {
                source_id,
                root_path: root,
                file_path: r"D:\Movies\エイリアン (1979).mkv",
                file_name: "エイリアン (1979).mkv",
                extension: "mkv",
                file_size: None,
                mtime: None,
                duration_sec: None,
                width: None,
                height: None,
                video_codec: None,
                audio_codec: None,
                container: "mkv",
            },
        )
        .unwrap();

        record_moved_file(
            &conn,
            file_id,
            source_id,
            r"D:\Movies\洋画\あ\エイリアン (1979).mkv",
            "エイリアン (1979).mkv",
        )
        .unwrap();

        let renamed: Option<String> = conn
            .query_row(
                "SELECT renamed_by_app FROM files WHERE id = ?1",
                rusqlite::params![file_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(renamed, None);
    }

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
            match_status: "unmatched".to_string(),
        }
    }

    #[test]
    fn missing_source_file_is_flagged_first() {
        let row = build_plan_row(input("あ", Some("あい"), "foreign"), TEST_ROOT);
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
        let row = build_plan_row(i, TEST_ROOT);
        assert_eq!(row.status, "no_country");
    }

    #[test]
    fn missing_reading_is_flagged() {
        let mut i = input("戦争と平和", None, "domestic");
        i.source_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let row = build_plan_row(i, TEST_ROOT);
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

    // ─── NAS ルート未設定の fail-closed ───────────────────────────────────────

    #[test]
    fn nas_root_rejects_empty_blank_and_relative() {
        for bad in [
            "",
            "   ",
            "\t\n",
            ".",
            "..",
            "Movie",
            r"Movie\sub",
            r".\Movie",
            r"..\Movie",
            r"\Movie",
            "/Movie",
            "C:",
            "C:Movie",
            r"D:\a\..\b",
            r"\\",
            r"\\server",
            r"\\server\",
            r"\\?\C:\x",
            r"\\.\pipe\x",
            r"\\server\share\..\x",
        ] {
            assert!(validate_nas_root(bad).is_err(), "must reject {bad:?}");
        }
    }

    #[test]
    fn nas_root_accepts_unc_and_drive_absolute() {
        assert_eq!(validate_nas_root(r"  \\srv\share\Movie\ ").unwrap(), r"\\srv\share\Movie");
        assert_eq!(validate_nas_root(r"\\srv\share").unwrap(), r"\\srv\share");
        assert_eq!(validate_nas_root(r"D:\Movie").unwrap(), r"D:\Movie");
        assert_eq!(validate_nas_root("D:/Movie/").unwrap(), "D:/Movie");
    }

    fn set_nas_root(conn: &rusqlite::Connection, v: &str) {
        conn.execute(
            "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![SETTING_NAS_ROOT, v],
        )
        .unwrap();
    }

    /// Fresh install（設定行なし）は未設定。空白・相対パスの保存値も未設定扱い
    #[test]
    fn fresh_install_has_no_nas_root() {
        use crate::db::test_support::*;
        let conn = open_migrated();
        assert!(nas_root(&conn).is_err());
        set_nas_root(&conn, "   ");
        assert!(nas_root(&conn).is_err());
        set_nas_root(&conn, r"relative\dir");
        assert!(nas_root(&conn).is_err());
    }

    /// 既存ユーザーが保存済みの NAS ルート（キー nas_movie_root）はそのまま読まれる
    #[test]
    fn saved_nas_root_is_preserved() {
        use crate::db::test_support::*;
        let conn = open_migrated();
        set_nas_root(&conn, r"\\saved-host\share\Movie");
        assert_eq!(nas_root(&conn).unwrap(), r"\\saved-host\share\Movie");
        assert_eq!(SETTING_NAS_ROOT, "nas_movie_root");
    }

    /// 未設定・不正な NAS ルートでは execute はファイル操作を1回も呼ばず、元ファイルも不変
    #[test]
    fn execute_without_nas_root_never_touches_files() {
        use crate::commands::scan::{insert_scanned_file, insert_scanned_work, ScannedFile};
        use crate::db::test_support::*;

        let tmp = std::env::temp_dir().join(format!("cm-r9-nas-{}", std::process::id()));
        let import = tmp.join("import");
        std::fs::create_dir_all(&import).unwrap();
        let src = import.join("a.mkv");
        std::fs::write(&src, b"payload").unwrap();
        let import_s = import.to_string_lossy().to_string();
        let src_s = src.to_string_lossy().to_string();

        let conn = open_migrated();
        let source_id = insert_source(&conn, &import_s);
        let file_id = insert_scanned_file(
            &conn,
            &ScannedFile {
                source_id,
                root_path: &import_s,
                file_path: &src_s,
                file_name: "a.mkv",
                extension: "mkv",
                file_size: None,
                mtime: None,
                duration_sec: None,
                width: None,
                height: None,
                video_codec: None,
                audio_codec: None,
                container: "mkv",
            },
        )
        .unwrap();
        let work_id = insert_scanned_work(&conn, "movie", "unknown", "a", "foreign").unwrap();
        conn.execute(
            "INSERT INTO work_parts (work_id, file_id) VALUES (?1, ?2)",
            rusqlite::params![work_id, file_id],
        )
        .unwrap();
        conn.execute("UPDATE works SET reading = 'あ' WHERE id = ?1", rusqlite::params![work_id])
            .unwrap();

        fn walk(d: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                out.push(p.to_string_lossy().to_string());
                if p.is_dir() {
                    walk(&p, out);
                }
            }
        }
        let snapshot = |dir: &Path| -> Vec<String> {
            let mut v = Vec::new();
            walk(dir, &mut v);
            v.sort();
            v
        };
        let before = snapshot(&tmp);

        let db = std::sync::Mutex::new(conn);
        for value in [None, Some(""), Some("   "), Some("relative"), Some(r"..\x"), Some(".")] {
            if let Some(v) = value {
                set_nas_root(&db.lock().unwrap(), v);
            }
            MOVE_FILE_CALLS.with(|c| c.set(0));
            let err = execute_nas_sort_impl(&db, import_s.clone(), vec![work_id]).unwrap_err();
            assert!(err.contains("保存先"), "{err}");
            assert_eq!(MOVE_FILE_CALLS.with(|c| c.get()), 0, "no file op for {value:?}");
            assert_eq!(std::fs::read(&src).unwrap(), b"payload");
            assert_eq!(snapshot(&tmp), before, "nothing created for {value:?}");
        }

        // 対照: 設定済みの NAS ルートなら move_file まで進む（カウンタが機能していることの証明）
        let nas = tmp.join("nas");
        std::fs::create_dir_all(&nas).unwrap();
        set_nas_root(&db.lock().unwrap(), &nas.to_string_lossy());
        MOVE_FILE_CALLS.with(|c| c.set(0));
        let ok = execute_nas_sort_impl(&db, import_s.clone(), vec![work_id]).unwrap();
        assert_eq!(ok.moved, 1, "{:?}", ok.errors);
        assert_eq!(MOVE_FILE_CALLS.with(|c| c.get()), 1);
        assert!(!src.exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
