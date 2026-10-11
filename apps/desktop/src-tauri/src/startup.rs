//! 起動時の失敗を「無言で終わらせない」ための補助（REL-R1B）。
//!
//! release build は `windows_subsystem = "windows"` でコンソールが無く、setup の `?` で落ちると
//! 利用者には何も見えない。ここでは (1) 技術詳細を app_dir/startup.log へ書き、
//! (2) 利用者向けの短い日本語メッセージ（ダイアログ用）を作る。ダイアログ表示そのものは lib.rs。
//! log にもメッセージにも秘密値（API キー等）は出さない。

use crate::migrate::{FailureKind, MigrationError};
use std::io::Write;
use std::path::{Path, PathBuf};

const LOG_FILE: &str = "startup.log";
const LOG_MAX_BYTES: u64 = 512 * 1024;

/// UTC の現在時刻（外部 crate 不要。civil-from-days）
fn civil_now() -> (i64, u32, u32, u32, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d, (rem / 3_600) as u32, (rem % 3_600 / 60) as u32, (rem % 60) as u32)
}

/// YYYYMMDD_HHmmss（backup のファイル名用）
pub fn now_compact() -> String {
    let (y, mo, d, h, mi, s) = civil_now();
    format!("{y:04}{mo:02}{d:02}_{h:02}{mi:02}{s:02}")
}

pub fn now_iso() -> String {
    let (y, mo, d, h, mi, s) = civil_now();
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

pub fn log_path(app_dir: &Path) -> PathBuf {
    app_dir.join(LOG_FILE)
}

/// 秘密値らしいものを伏せる。log に書く文字列は必ずこれを通す。
/// 対象: `api_key=…` `token: …` 等のキー付き値、Bearer トークン、32桁以上の16進（TMDb v3 キー等）、JWT。
pub fn redact(text: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static RULES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        vec![
            (Regex::new(r"(?i)(bearer\s+)[A-Za-z0-9._~+/=-]+").unwrap(), "${1}[REDACTED]"),
            (
                Regex::new(r#"(?i)((?:api[_-]?key|apikey|access[_-]?token|token|secret|password|passwd|authorization)["']?\s*[:=]\s*["']?)[^\s"',;&]+"#).unwrap(),
                "${1}[REDACTED]",
            ),
                        (Regex::new(r"eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]*").unwrap(), "[REDACTED]"),
            (Regex::new(r"\b[0-9a-fA-F]{32,}\b").unwrap(), "[REDACTED]"),
        ]
    });
    let mut out = text.to_string();
    for (re, rep) in rules {
        out = re.replace_all(&out, *rep).into_owned();
    }
    out
}

/// startup.log へ 1 行追記する。失敗しても起動処理は止めない（log は補助）。
/// 512KB を超えていたら startup.log.old へ回して作り直す。
pub fn log_event(app_dir: &Path, level: &str, message: &str) {
    let path = log_path(app_dir);
    if std::fs::metadata(&path).map(|m| m.len() > LOG_MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(&path, app_dir.join(format!("{LOG_FILE}.old")));
    }
    let line = format!(
        "{} [{}] {}\n",
        now_iso(),
        level,
        redact(message).replace('\n', "\n    ")
    );
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// 利用者に見せる（ダイアログ用）メッセージ。技術詳細は含めず、log の場所を案内する。
pub fn user_message_for_migration(e: &MigrationError, app_dir: &Path, latest_backup: Option<&Path>) -> String {
    let log = log_path(app_dir);
    let head = match e.kind {
        FailureKind::Corrupted => "データベースファイルが壊れている可能性があるため、起動できませんでした。",
        FailureKind::NewerSchema => {
            "このデータベースは、より新しいバージョンの CineMantis で作成されています。\nこのバージョンでは安全に開けないため、起動を中止しました。新しいバージョンをお使いください。"
        }
        FailureKind::BackupFailed => {
            "データベース更新前のバックアップを作れなかったため、安全のため起動を中止しました。\n（データは変更していません。ディスクの空き容量などをご確認ください。）"
        }
        FailureKind::MigrationFailed => {
            "データベースの更新に失敗したため、起動を中止しました。"
        }
        FailureKind::OpenFailed => "データベースを開けなかったため、起動できませんでした。",
        FailureKind::InconsistentLegacySchema => {
            "データベースの構造が途中で欠けている（非連続な）状態のため、データ保護を優先して何も変更せずに起動を中止しました。\n自動では修復できません。修復には別途、専用の手順が必要です。"
        }
        FailureKind::UnknownSchema => "このデータベースは CineMantis のものと認識できないため、何も変更せずに起動を中止しました。",
    };
    let mut msg = String::from(head);
    if e.kind == FailureKind::MigrationFailed {
        if e.db_unchanged {
            msg.push_str("\n更新は取り消され、データは更新前の状態のままです。");
        } else {
            msg.push_str("\n更新後の状態を確認できませんでした。下記のバックアップからの復元をお勧めします。");
        }
    }
    if let Some(b) = e.backup_path.as_deref().or(latest_backup) {
        msg.push_str(&format!(
            "\n\n【復元用バックアップ】\n{}\n復元するには、このファイルを\n{}\nという名前で {} にコピーしてから、もう一度起動してください。",
            b.display(),
            app_dir.join("cinemantis_restore_pending.db").display(),
            app_dir.display()
        ));
    }
    msg.push_str(&format!("\n\n詳細は次のログをご確認ください:\n{}", log.display()));
    msg
}

pub fn user_message_for_restore_failure(app_dir: &Path) -> String {
    format!(
        "バックアップからの復元に失敗しました。現在のデータはそのままです。\n詳細は次のログをご確認ください:\n{}",
        log_path(app_dir).display()
    )
}

// ─── 致命エラーの通知（コンソールの無い release build 向け） ──────────────────

static APP_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn set_app_dir(dir: &Path) {
    let _ = APP_DIR.set(dir.to_path_buf());
}

/// app_dir が決まる前の失敗でも log を残せるよう、temp を代わりに使う
pub fn app_dir_or_temp() -> PathBuf {
    APP_DIR.get().cloned().unwrap_or_else(std::env::temp_dir)
}

/// OS 標準のエラーダイアログを出す（ブロックする）。webview / tauri の初期化に依存しないので、
/// setup の前後どちらの失敗でも使える。Windows 以外では stderr へ出すだけ。
#[cfg(windows)]
pub fn fatal_box(title: &str, message: &str) {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: *mut core::ffi::c_void, text: *const u16, caption: *const u16, utype: u32) -> i32;
    }
    let wide = |s: &str| -> Vec<u16> { std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect() };
    const MB_OK: u32 = 0x0;
    const MB_ICONERROR: u32 = 0x10;
    const MB_SETFOREGROUND: u32 = 0x10000;
    let (t, c) = (wide(message), wide(title));
    unsafe {
        MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONERROR | MB_SETFOREGROUND);
    }
}

#[cfg(not(windows))]
pub fn fatal_box(title: &str, message: &str) {
    eprintln!("{title}: {message}");
}

/// panic を log に残し、利用者にも知らせる（release は panic=abort なので、何も出さないと無言で消える）
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let dir = app_dir_or_temp();
        log_event(&dir, "PANIC", &info.to_string());
        fatal_box(
            "CineMantis",
            &format!(
                "予期しないエラーが発生したため、CineMantis を終了します。\n詳細は次のログをご確認ください:\n{}",
                log_path(&dir).display()
            ),
        );
    }));
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
