//! DB restore の安全な適用ロジック（REL-R1A）。
//!
//! Tauri に依存しない純粋なファイル操作 + rusqlite のみ。`app_dir` を引数に取るので、
//! テストでは一時ディレクトリの synthetic DB だけを対象にできる。
//!
//! 流れ（`apply_pending_restore`、アプリ起動時・DB 接続を開く前に実行）:
//!  0. 前回中断された適用の復旧（`.rollback` セットが残っていれば判定して戻す／片付ける）
//!  1. pending を staging にコピーし、**staging 上で**検証（open / integrity_check / 必須テーブル）
//!  2. 現在 DB（main + -wal + -shm）を `backups/pre-restore/` に退避（サイズ検証付き）
//!  3. 現在 DB セットを `.rollback` に rename で退避（古い -wal/-shm を新 DB に混ぜない）
//!  4. staging を rename で db_path に配置（同一ボリューム・原子的）
//!  5. 配置後の DB を開き直して integrity_check 再確認
//!  6. コミット = pending を削除（失敗時は `.applied-*` に rename、それも失敗なら rollback）
//!  7. `.rollback` セットを削除
//!
//! どの段階で失敗しても: 現在 DB は失われず（rollback で元に戻す）、pending は
//! 「再試行可能な失敗」なら保持、「内容が不良」なら `.rejected-*` に隔離して保存する（削除しない）。

use serde::Serialize;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

pub const DB_FILE: &str = "cinemantis.db";
pub const PENDING_FILE: &str = "cinemantis_restore_pending.db";
pub const RESULT_FILE: &str = "restore_last_result.json";
const STAGING_SUFFIX: &str = ".restore-staging";
const ROLLBACK_SUFFIX: &str = ".rollback";
const FAILED_SUFFIX: &str = ".failed-restore";
const MEMBERS: [&str; 3] = ["", "-wal", "-shm"];
const REQUIRED_TABLES: [&str; 7] = [
    "sources", "files", "works", "user_stats", "tags", "persons", "series",
];

// ─── 結果型 ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum FailureKind {
    /// pending の内容が不良。現在 DB は無変更。pending は `.rejected-*` に隔離保存。
    Rejected,
    /// 一時的な I/O 失敗。現在 DB は無変更（または rollback 済み）。pending は保持され次回起動で再試行。
    Deferred,
    /// swap 後の検証失敗などで rollback 済み。pending は隔離保存。
    RolledBack,
    /// rollback 自体に失敗。手動復旧が必要（`notes` に復旧元のパスを列挙）。
    RollbackFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status")]
pub enum RestoreOutcome {
    NoPending,
    /// 前回中断された適用の後始末のみ行った（pending は無い）
    RecoveredOnly { notes: Vec<String> },
    Applied {
        safety_backup: Option<String>,
        notes: Vec<String>,
    },
    Failed {
        kind: FailureKind,
        stage: String,
        reason: String,
        /// pending（またはその隔離先）の現在位置。無ければ None
        pending_path: Option<String>,
        notes: Vec<String>,
    },
}

/// フォールト注入用のステップ名（テストで失敗を再現する）
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    Stage,
    SafetyBackup,
    MoveAside,
    Place,
    Verify,
    Commit,
}

impl Step {
    fn name(self) -> &'static str {
        match self {
            Step::Stage => "stage-copy",
            Step::SafetyBackup => "safety-backup",
            Step::MoveAside => "move-aside",
            Step::Place => "place",
            Step::Verify => "reopen-verify",
            Step::Commit => "commit",
        }
    }
}

// ─── ヘルパー ─────────────────────────────────────────────────────────────────

fn with_suffix(p: &Path, s: &str) -> PathBuf {
    let mut o: OsString = p.as_os_str().to_owned();
    o.push(s);
    PathBuf::from(o)
}

fn member(base: &Path, m: &str, extra: &str) -> PathBuf {
    with_suffix(base, &format!("{m}{extra}"))
}

fn stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{}{:03}", d.as_secs(), d.subsec_millis())
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// DB ファイルの検証: open 可否・integrity_check・必須テーブル。
/// 対象ファイルは変更しない（rw で開くが WAL でなければ副作用なし。WAL の場合も close で整理される）。
pub fn validate_db_file(path: &Path) -> Result<(), String> {
    use rusqlite::{Connection, OpenFlags};
    if !path.is_file() {
        return Err(format!("ファイルが存在しません: {}", path.display()));
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| format!("DB を開けません: {e}"))?;
    let result = (|| -> Result<(), String> {
        let mut stmt = conn
            .prepare("PRAGMA integrity_check")
            .map_err(|e| format!("integrity_check を実行できません: {e}"))?;
        let rows: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| format!("integrity_check を実行できません: {e}"))?
            .collect::<Result<_, _>>()
            .map_err(|e| format!("integrity_check を実行できません: {e}"))?;
        if rows.len() != 1 || rows[0] != "ok" {
            let head: Vec<_> = rows.iter().take(5).cloned().collect();
            return Err(format!("integrity_check 失敗: {}", head.join(" / ")));
        }
        drop(stmt);
        let mut missing = vec![];
        for t in REQUIRED_TABLES {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [t],
                    |r| r.get(0),
                )
                .map_err(|e| format!("スキーマ確認に失敗: {e}"))?;
            if n == 0 {
                missing.push(t);
            }
        }
        if !missing.is_empty() {
            return Err(format!("必須テーブルが不足: {}", missing.join(", ")));
        }
        Ok(())
    })();
    let _ = conn.close();
    result
}

fn remove_set(base: &Path, extra: &str) -> Vec<String> {
    let mut errs = vec![];
    for m in MEMBERS {
        let p = member(base, m, extra);
        if p.exists() {
            if let Err(e) = std::fs::remove_file(&p) {
                errs.push(format!("{} を削除できません: {e}", p.display()));
            }
        }
    }
    errs
}

/// base セットの存在するメンバーを `from_extra` → `to_extra` に rename。成功分を返し、失敗時は Err(成功済み分, エラー)
fn rename_set(
    base: &Path,
    from_extra: &str,
    to_extra: &str,
) -> Result<(), (Vec<&'static str>, String)> {
    let mut done: Vec<&'static str> = vec![];
    for m in MEMBERS {
        let from = member(base, m, from_extra);
        if !from.exists() {
            continue;
        }
        let to = member(base, m, to_extra);
        if to.exists() {
            let _ = std::fs::remove_file(&to);
        }
        if let Err(e) = std::fs::rename(&from, &to) {
            return Err((done, format!("{} → {}: {e}", from.display(), to.display())));
        }
        done.push(m);
    }
    Ok(())
}

/// 元の位置に戻す: db セットを failed-restore へ（任意）、rollback セットを元に戻す
fn revert(db_path: &Path, move_new_aside: bool) -> Result<Vec<String>, String> {
    let mut notes = vec![];
    if move_new_aside {
        // 新 DB（配置済みの可能性）を保存用にどける。失敗したら新 DB を削除してでも元を戻す必要はない→エラー扱い
        rename_set(db_path, "", FAILED_SUFFIX)
            .map_err(|(_, e)| format!("復旧: 新 DB を退避できません: {e}"))?;
        notes.push(format!(
            "検証失敗した DB を保存: {}",
            member(db_path, "", FAILED_SUFFIX).display()
        ));
    }
    rename_set(db_path, ROLLBACK_SUFFIX, "")
        .map_err(|(_, e)| format!("復旧: 元の DB を戻せません: {e}"))?;
    Ok(notes)
}

// ─── 本体 ─────────────────────────────────────────────────────────────────────

pub fn apply_pending_restore(app_dir: &Path) -> RestoreOutcome {
    apply_with_hook(app_dir, &|_| Ok(()))
}

pub fn apply_with_hook(
    app_dir: &Path,
    hook: &dyn Fn(Step) -> io::Result<()>,
) -> RestoreOutcome {
    let db_path = app_dir.join(DB_FILE);
    let pending = app_dir.join(PENDING_FILE);
    let staging = with_suffix(&db_path, STAGING_SUFFIX);
    let mut notes: Vec<String> = vec![];

    let fail = |kind: FailureKind,
                stage: &str,
                reason: String,
                pending_path: Option<String>,
                notes: Vec<String>| RestoreOutcome::Failed {
        kind,
        stage: stage.to_string(),
        reason,
        pending_path,
        notes,
    };

    // 0. 中断復旧
    let rollback_main = member(&db_path, "", ROLLBACK_SUFFIX);
    let any_rollback = MEMBERS
        .iter()
        .any(|m| member(&db_path, m, ROLLBACK_SUFFIX).exists());
    let mut recovered = false;
    if any_rollback {
        recovered = true;
        let completed = !pending.exists() && db_path.exists();
        if completed {
            // pending 削除（= コミット）済み。rollback の後始末のみ。
            let errs = remove_set(&db_path, ROLLBACK_SUFFIX);
            notes.push("前回の restore はコミット済み: rollback 退避を削除".into());
            notes.extend(errs);
        } else {
            let new_present = db_path.exists() && rollback_main.exists();
            match revert(&db_path, new_present) {
                Ok(n) => {
                    notes.push("前回中断された restore を rollback で元に戻した".into());
                    notes.extend(n);
                }
                Err(e) => {
                    return fail(
                        FailureKind::RollbackFailed,
                        "interrupted-recovery",
                        e,
                        pending.exists().then(|| s(&pending)),
                        vec![format!(
                            "元の DB は {} 等の .rollback ファイルに残っています",
                            db_path.display()
                        )],
                    )
                }
            }
        }
    }
    // 中断で残った staging は pending から再生成できるので破棄
    let _ = remove_set(&db_path, STAGING_SUFFIX);

    if !pending.exists() {
        return if recovered {
            RestoreOutcome::RecoveredOnly { notes }
        } else {
            RestoreOutcome::NoPending
        };
    }

    // 1. staging コピー + 検証
    let r = hook(Step::Stage).and_then(|_| std::fs::copy(&pending, &staging).map(|_| ()));
    if let Err(e) = r {
        let _ = remove_set(&db_path, STAGING_SUFFIX);
        return fail(
            FailureKind::Deferred,
            Step::Stage.name(),
            format!("pending を staging にコピーできません: {e}"),
            Some(s(&pending)),
            notes,
        );
    }
    if let Err(reason) = validate_db_file(&staging) {
        let _ = remove_set(&db_path, STAGING_SUFFIX);
        return quarantine_failure(&pending, FailureKind::Rejected, "pending-validate", reason, notes);
    }

    // 2. 現在 DB の退避（コピー）
    let mut safety_backup: Option<String> = None;
    let has_current = MEMBERS.iter().any(|m| member(&db_path, m, "").exists());
    if has_current {
        let dir = app_dir.join("backups").join("pre-restore");
        let base = dir.join(format!("cinemantis_prerestore_{}.db", stamp()));
        let r = hook(Step::SafetyBackup)
            .and_then(|_| std::fs::create_dir_all(&dir))
            .and_then(|_| copy_set_verified(&db_path, &base));
        if let Err(e) = r {
            let _ = remove_set(&base, "");
            let _ = remove_set(&db_path, STAGING_SUFFIX);
            return fail(
                FailureKind::Deferred,
                Step::SafetyBackup.name(),
                format!("現在 DB を退避できません（restore を中止、現在 DB は無変更）: {e}"),
                Some(s(&pending)),
                notes,
            );
        }
        safety_backup = Some(s(&base));
    } else {
        notes.push("現在 DB が存在しないため退避は省略".into());
    }

    // 3. 現在 DB セット（stale な -wal/-shm 含む）を rollback へ退避
    let r = hook(Step::MoveAside).map_err(|e| (vec![], e.to_string())).and_then(|_| {
        rename_set(&db_path, "", ROLLBACK_SUFFIX)
    });
    if let Err((_, e)) = r {
        return rollback_failure(
            &db_path, &pending, &staging, false, FailureKind::Deferred, Step::MoveAside, e, notes,
        );
    }

    // 4. 配置（rename）
    let r = hook(Step::Place).and_then(|_| std::fs::rename(&staging, &db_path));
    if let Err(e) = r {
        return rollback_failure(
            &db_path, &pending, &staging, false, FailureKind::Deferred, Step::Place,
            format!("staging を配置できません: {e}"), notes,
        );
    }

    // 5. reopen 検証
    let r = hook(Step::Verify)
        .map_err(|e| format!("検証前に失敗: {e}"))
        .and_then(|_| validate_db_file(&db_path));
    if let Err(reason) = r {
        return rollback_failure(
            &db_path, &pending, &staging, true, FailureKind::RolledBack, Step::Verify, reason, notes,
        );
    }

    // 6. コミット = pending を取り除く
    let r = hook(Step::Commit).and_then(|_| match std::fs::remove_file(&pending) {
        Ok(()) => Ok(()),
        Err(_) => std::fs::rename(&pending, with_suffix(&pending, &format!(".applied-{}", stamp()))),
    });
    if let Err(e) = r {
        return rollback_failure(
            &db_path, &pending, &staging, true, FailureKind::Deferred, Step::Commit,
            format!("pending を確定できません: {e}"), notes,
        );
    }

    // 7. rollback 後始末（失敗しても次回起動の中断復旧が「コミット済み」と判断して片付ける）
    notes.extend(remove_set(&db_path, ROLLBACK_SUFFIX));
    RestoreOutcome::Applied { safety_backup, notes }
}

fn copy_set_verified(from_base: &Path, to_base: &Path) -> io::Result<()> {
    for m in MEMBERS {
        let from = member(from_base, m, "");
        if !from.exists() {
            continue;
        }
        let to = member(to_base, m, "");
        std::fs::copy(&from, &to)?;
        if std::fs::metadata(&from)?.len() != std::fs::metadata(&to)?.len() {
            return Err(io::Error::new(io::ErrorKind::Other, format!("コピーのサイズ不一致: {}", to.display())));
        }
    }
    Ok(())
}

/// pending を `.rejected-<ts>` に隔離（削除しない）して Failed を返す
fn quarantine_failure(
    pending: &Path,
    kind: FailureKind,
    stage: &str,
    reason: String,
    mut notes: Vec<String>,
) -> RestoreOutcome {
    let dest = with_suffix(pending, &format!(".rejected-{}", stamp()));
    let pending_path = match std::fs::rename(pending, &dest) {
        Ok(()) => {
            notes.push("pending は隔離保存（削除していない）".into());
            Some(s(&dest))
        }
        Err(e) => {
            notes.push(format!("pending の隔離に失敗（その場に保持）: {e}"));
            Some(s(pending))
        }
    };
    RestoreOutcome::Failed { kind, stage: stage.into(), reason, pending_path, notes }
}

/// swap 途中の失敗: 元の DB を戻す。戻せたら kind に応じて pending を保持／隔離、戻せなければ RollbackFailed
#[allow(clippy::too_many_arguments)]
fn rollback_failure(
    db_path: &Path,
    pending: &Path,
    staging: &Path,
    new_may_be_placed: bool,
    kind: FailureKind,
    step: Step,
    reason: String,
    mut notes: Vec<String>,
) -> RestoreOutcome {
    let _ = std::fs::remove_file(staging);
    let new_present = new_may_be_placed && db_path.exists();
    match revert(db_path, new_present) {
        Ok(n) => {
            notes.extend(n);
            notes.push("元の DB を rollback で復元済み".into());
            if kind == FailureKind::RolledBack {
                quarantine_failure(pending, kind, step.name(), reason, notes)
            } else {
                RestoreOutcome::Failed {
                    kind,
                    stage: step.name().into(),
                    reason,
                    pending_path: pending.exists().then(|| s(pending)),
                    notes,
                }
            }
        }
        Err(e) => {
            notes.push(format!(
                "手動復旧: 元の DB は {}(.db/-wal/-shm) と backups/pre-restore に残っています",
                with_suffix(db_path, ROLLBACK_SUFFIX).display()
            ));
            RestoreOutcome::Failed {
                kind: FailureKind::RollbackFailed,
                stage: step.name().into(),
                reason: format!("{reason}; rollback 失敗: {e}"),
                pending_path: pending.exists().then(|| s(pending)),
                notes,
            }
        }
    }
}

// ─── restore 要求（pending の作成） ──────────────────────────────────────────

/// バックアップを検証して pending として原子的に配置する。
/// 検証は一時ファイル上で行い、失敗しても既存の pending には触れない。
pub fn stage_restore_request(app_dir: &Path, src: &Path) -> Result<(), String> {
    let pending = app_dir.join(PENDING_FILE);
    let db_path = app_dir.join(DB_FILE);
    if !src.is_file() {
        return Err(format!("バックアップファイルが見つかりません: {}", src.display()));
    }
    if src == pending || src == db_path {
        return Err("現在の DB / pending 自体は restore 元に指定できません".into());
    }
    let tmp = with_suffix(&pending, ".tmp");
    let _ = std::fs::remove_file(&tmp);
    std::fs::copy(src, &tmp).map_err(|e| format!("バックアップをコピーできません: {e}"))?;
    if let Err(e) = validate_db_file(&tmp) {
        let _ = remove_set(&tmp, "");
        return Err(format!("バックアップが不正のため restore を受け付けません: {e}"));
    }
    std::fs::rename(&tmp, &pending).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("pending を確定できません: {e}")
    })
}

/// 結果をファイルに記録し、失敗は stderr にも出す（silent failure にしない）
pub fn record_outcome(app_dir: &Path, outcome: &RestoreOutcome) {
    if matches!(outcome, RestoreOutcome::NoPending) {
        return;
    }
    if let RestoreOutcome::Failed { .. } = outcome {
        eprintln!("[restore] FAILED: {outcome:?}");
    } else {
        eprintln!("[restore] {outcome:?}");
    }
    if let Ok(json) = serde_json::to_string_pretty(outcome) {
        let _ = std::fs::write(app_dir.join(RESULT_FILE), json);
    }
}

// ─── テスト ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "cm_r1a_{}_{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn make_db(path: &Path, marker: &str, wal: bool) {
        let c = Connection::open(path).unwrap();
        c.execute_batch(include_str!("../../../../packages/db/migrations/001_initial.sql")).unwrap();
        if wal {
            c.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        }
        c.execute_batch("CREATE TABLE IF NOT EXISTS marker(v TEXT);").unwrap();
        c.execute("INSERT INTO marker(v) VALUES (?1)", [marker]).unwrap();
        let _ = c.close();
    }

    fn marker(path: &Path) -> String {
        let c = Connection::open(path).unwrap();
        c.query_row("SELECT v FROM marker", [], |r| r.get(0)).unwrap()
    }

    fn setup(cur: Option<&str>, pend: Option<&str>) -> Tmp {
        let t = Tmp::new();
        if let Some(m) = cur {
            make_db(&t.0.join(DB_FILE), m, false);
        }
        if let Some(m) = pend {
            make_db(&t.0.join(PENDING_FILE), m, false);
        }
        t
    }

    fn failed(o: &RestoreOutcome) -> (&FailureKind, &str) {
        match o {
            RestoreOutcome::Failed { kind, stage, .. } => (kind, stage.as_str()),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn inject(step: Step) -> impl Fn(Step) -> io::Result<()> {
        move |s| {
            if s == step {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "injected"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn normal_restore() {
        let t = setup(Some("current"), Some("restored"));
        let o = apply_pending_restore(&t.0);
        let RestoreOutcome::Applied { safety_backup, .. } = &o else { panic!("{o:?}") };
        assert_eq!(marker(&t.0.join(DB_FILE)), "restored");
        assert!(!t.0.join(PENDING_FILE).exists());
        // 現在 DB は退避されている
        let sb = PathBuf::from(safety_backup.clone().unwrap());
        assert_eq!(marker(&sb), "current");
        // 作業用ファイルが残っていない
        assert_eq!(files(&t.0), vec!["backups", DB_FILE]);
    }

    #[test]
    fn no_pending_is_noop() {
        let t = setup(Some("current"), None);
        assert_eq!(apply_pending_restore(&t.0), RestoreOutcome::NoPending);
        assert_eq!(marker(&t.0.join(DB_FILE)), "current");
    }

    #[test]
    fn current_db_missing() {
        let t = setup(None, Some("restored"));
        let o = apply_pending_restore(&t.0);
        let RestoreOutcome::Applied { safety_backup, .. } = &o else { panic!("{o:?}") };
        assert!(safety_backup.is_none());
        assert_eq!(marker(&t.0.join(DB_FILE)), "restored");
    }

    #[test]
    fn corrupt_pending_rejected_and_quarantined() {
        let t = setup(Some("current"), None);
        std::fs::write(t.0.join(PENDING_FILE), b"this is not a sqlite database at all, just garbage bytes").unwrap();
        let o = apply_pending_restore(&t.0);
        assert_eq!(failed(&o).0, &FailureKind::Rejected);
        assert_eq!(marker(&t.0.join(DB_FILE)), "current");
        assert!(!t.0.join(PENDING_FILE).exists());
        let q: Vec<_> = files(&t.0).into_iter().filter(|f| f.contains(".rejected-")).collect();
        assert_eq!(q.len(), 1, "pending は隔離保存される: {:?}", files(&t.0));
        // 再起動しても同じ失敗を繰り返さない
        assert_eq!(apply_pending_restore(&t.0), RestoreOutcome::NoPending);
    }

    #[test]
    fn pending_missing_required_tables_rejected() {
        let t = setup(Some("current"), None);
        let c = Connection::open(t.0.join(PENDING_FILE)).unwrap();
        c.execute_batch("CREATE TABLE x(a); INSERT INTO x VALUES(1);").unwrap();
        let _ = c.close();
        let o = apply_pending_restore(&t.0);
        assert_eq!(failed(&o).0, &FailureKind::Rejected);
        assert_eq!(marker(&t.0.join(DB_FILE)), "current");
    }

    #[test]
    fn integrity_check_failure_rejected() {
        let t = setup(Some("current"), None);
        // 多数行で複数ページにし、内部ページを破壊
        let p = t.0.join(PENDING_FILE);
        let c = Connection::open(&p).unwrap();
        c.execute_batch(include_str!("../../../../packages/db/migrations/001_initial.sql")).unwrap();
        c.execute_batch("CREATE TABLE marker(v TEXT); CREATE INDEX mi ON marker(v);").unwrap();
        for i in 0..3000 {
            c.execute("INSERT INTO marker(v) VALUES (?1)", [format!("row-{i}-{}", "x".repeat(40))]).unwrap();
        }
        let _ = c.close();
        let mut bytes = std::fs::read(&p).unwrap();
        let len = bytes.len();
        for b in bytes[len / 2..len / 2 + 4096].iter_mut() {
            *b = 0xFF;
        }
        std::fs::write(&p, bytes).unwrap();
        let o = apply_pending_restore(&t.0);
        assert_eq!(failed(&o).0, &FailureKind::Rejected);
        assert_eq!(marker(&t.0.join(DB_FILE)), "current");
    }

    #[test]
    fn stale_wal_shm_not_mixed_into_new_db() {
        let t = setup(None, Some("restored"));
        // 現在 DB: WAL モードで、未チェックポイントの内容を持つ状態を作る
        let db = t.0.join(DB_FILE);
        let c = Connection::open(&db).unwrap();
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;").unwrap();
        c.execute_batch(include_str!("../../../../packages/db/migrations/001_initial.sql")).unwrap();
        c.execute_batch("CREATE TABLE marker(v TEXT); INSERT INTO marker VALUES('current-wal');").unwrap();
        // 接続を開いたまま sidecar をコピーしてクラッシュ状態を再現（close すると消える）
        let wal = member(&db, "-wal", "");
        assert!(wal.exists());
        let snap = t.0.join("snap");
        std::fs::create_dir_all(&snap).unwrap();
        for m in MEMBERS {
            let f = member(&db, m, "");
            if f.exists() {
                std::fs::copy(&f, snap.join(f.file_name().unwrap())).unwrap();
            }
        }
        drop(c);
        for m in MEMBERS {
            let _ = std::fs::remove_file(member(&db, m, ""));
            let f = snap.join(format!("{DB_FILE}{m}"));
            if f.exists() {
                std::fs::copy(&f, member(&db, m, "")).unwrap();
            }
        }
        std::fs::remove_dir_all(&snap).unwrap();
        assert!(member(&db, "-wal", "").exists(), "stale wal を再現");
        let o = apply_pending_restore(&t.0);
        let RestoreOutcome::Applied { safety_backup, .. } = &o else { panic!("{o:?}") };
        assert_eq!(marker(&db), "restored");
        // 新 DB の sidecar に古い WAL が混入していない（検証で close 済み → 無いか空）
        let w = member(&db, "-wal", "");
        assert!(!w.exists() || std::fs::metadata(&w).unwrap().len() == 0);
        // 退避セットから WAL 内容が復元できる
        let sb = PathBuf::from(safety_backup.clone().unwrap());
        assert!(member(&sb, "-wal", "").exists());
        assert_eq!(marker(&sb), "current-wal");
    }

    fn assert_current_intact_and_clean(t: &Tmp) {
        assert_eq!(marker(&t.0.join(DB_FILE)), "current");
        let leftovers: Vec<_> = files(&t.0)
            .into_iter()
            .filter(|f| f.contains(".rollback") || f.contains("staging"))
            .collect();
        assert!(leftovers.is_empty(), "作業ファイルが残っている: {leftovers:?}");
    }

    #[test]
    fn transient_failures_keep_pending_and_current() {
        for step in [Step::Stage, Step::SafetyBackup, Step::MoveAside, Step::Place, Step::Commit] {
            let t = setup(Some("current"), Some("restored"));
            let o = apply_with_hook(&t.0, &inject(step));
            let (kind, stage) = failed(&o);
            assert_eq!(kind, &FailureKind::Deferred, "{step:?}");
            assert_eq!(stage, step.name());
            assert_current_intact_and_clean(&t);
            assert_eq!(marker(&t.0.join(PENDING_FILE)), "restored", "pending 保持 {step:?}");
            // 障害解消後の再試行で適用できる（冪等・再試行可能）
            let o2 = apply_pending_restore(&t.0);
            assert!(matches!(o2, RestoreOutcome::Applied { .. }), "{step:?}: {o2:?}");
            assert_eq!(marker(&t.0.join(DB_FILE)), "restored");
        }
    }

    #[test]
    fn reopen_failure_rolls_back() {
        let t = setup(Some("current"), Some("restored"));
        let db = t.0.join(DB_FILE);
        let hook = |s: Step| {
            if s == Step::Verify {
                // 配置直後に DB を壊す
                std::fs::write(&db, b"garbage garbage garbage garbage garbage").unwrap();
            }
            Ok(())
        };
        let o = apply_with_hook(&t.0, &hook);
        assert_eq!(failed(&o).0, &FailureKind::RolledBack);
        assert_current_intact_and_clean(&t);
        // pending は隔離保存、壊れた DB も保存
        let fs = files(&t.0);
        assert!(fs.iter().any(|f| f.contains(".rejected-")), "{fs:?}");
        assert!(fs.iter().any(|f| f.ends_with(".failed-restore")), "{fs:?}");
        assert!(!t.0.join(PENDING_FILE).exists());
    }

    #[test]
    fn verify_step_injected_error_rolls_back() {
        let t = setup(Some("current"), Some("restored"));
        let o = apply_with_hook(&t.0, &inject(Step::Verify));
        assert_eq!(failed(&o).0, &FailureKind::RolledBack);
        assert_current_intact_and_clean(&t);
    }

    #[test]
    fn interrupted_before_place_is_recovered() {
        // MoveAside 後に落ちた状態: db 無し・.rollback 有り・pending 有り
        let t = setup(Some("current"), Some("restored"));
        let db = t.0.join(DB_FILE);
        rename_set(&db, "", ROLLBACK_SUFFIX).unwrap();
        std::fs::copy(t.0.join(PENDING_FILE), with_suffix(&db, STAGING_SUFFIX)).unwrap();
        let o = apply_pending_restore(&t.0);
        // 復旧後に pending が再適用される
        assert!(matches!(o, RestoreOutcome::Applied { .. }), "{o:?}");
        assert_eq!(marker(&db), "restored");
        assert_eq!(files(&t.0), vec!["backups", DB_FILE]);
        // 退避には元の current が入っている
        let dir = t.0.join("backups").join("pre-restore");
        let b = files(&dir).into_iter().find(|f| f.ends_with(".db")).unwrap();
        assert_eq!(marker(&dir.join(b)), "current");
    }

    #[test]
    fn interrupted_after_place_before_commit_reverts_then_reapplies() {
        // 新 DB 配置済み・.rollback 有り・pending 有り（コミット前クラッシュ）
        let t = setup(Some("current"), Some("restored"));
        let db = t.0.join(DB_FILE);
        rename_set(&db, "", ROLLBACK_SUFFIX).unwrap();
        std::fs::copy(t.0.join(PENDING_FILE), &db).unwrap();
        let o = apply_pending_restore(&t.0);
        assert!(matches!(o, RestoreOutcome::Applied { .. }), "{o:?}");
        assert_eq!(marker(&db), "restored");
    }

    #[test]
    fn interrupted_after_commit_only_cleans_rollback() {
        // pending 削除済み・新 DB 配置済み・.rollback 残存
        let t = setup(Some("current"), None);
        let db = t.0.join(DB_FILE);
        rename_set(&db, "", ROLLBACK_SUFFIX).unwrap();
        make_db(&db, "restored", false);
        let o = apply_pending_restore(&t.0);
        assert!(matches!(o, RestoreOutcome::RecoveredOnly { .. }), "{o:?}");
        assert_eq!(marker(&db), "restored", "コミット済みの restore を巻き戻してはならない");
        assert_eq!(files(&t.0), vec![DB_FILE]);
    }

    #[test]
    fn rollback_failure_is_reported_loudly() {
        // MoveAside は成功し Place が失敗 → rollback の rename も失敗させる:
        // db_path に「ディレクトリ」を置いて rename を失敗させる
        let t = setup(Some("current"), Some("restored"));
        let db = t.0.join(DB_FILE);
        let hook = |s: Step| {
            if s == Step::Place {
                std::fs::create_dir_all(&db).unwrap(); // rollback 先をふさぐ
                return Err(io::Error::new(io::ErrorKind::Other, "injected"));
            }
            Ok(())
        };
        let o = apply_with_hook(&t.0, &hook);
        let (kind, _) = failed(&o);
        assert_eq!(kind, &FailureKind::RollbackFailed);
        // 元 DB は .rollback と pre-restore に残っており、pending も残る
        assert!(with_suffix(&db, ROLLBACK_SUFFIX).exists());
        assert!(t.0.join(PENDING_FILE).exists());
    }

    #[test]
    fn idempotent_double_run() {
        let t = setup(Some("current"), Some("restored"));
        assert!(matches!(apply_pending_restore(&t.0), RestoreOutcome::Applied { .. }));
        let db = t.0.join(DB_FILE);
        // 復元後に新しいデータを書いた想定
        let c = Connection::open(&db).unwrap();
        c.execute("INSERT INTO marker VALUES('later')", []).unwrap();
        let _ = c.close();
        assert_eq!(apply_pending_restore(&t.0), RestoreOutcome::NoPending);
        let c = Connection::open(&db).unwrap();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM marker", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn stage_request_validates_and_preserves_existing_pending() {
        let t = setup(Some("current"), Some("older-request"));
        let bad = t.0.join("bad.db");
        std::fs::write(&bad, b"not a db not a db not a db").unwrap();
        let e = stage_restore_request(&t.0, &bad).unwrap_err();
        assert!(e.contains("不正"), "{e}");
        assert_eq!(marker(&t.0.join(PENDING_FILE)), "older-request");
        assert!(!with_suffix(&t.0.join(PENDING_FILE), ".tmp").exists());

        let good = t.0.join("good.db");
        make_db(&good, "newer-request", false);
        stage_restore_request(&t.0, &good).unwrap();
        assert_eq!(marker(&t.0.join(PENDING_FILE)), "newer-request");

        assert!(stage_restore_request(&t.0, &t.0.join("nope.db")).is_err());
        assert!(stage_restore_request(&t.0, &t.0.join(DB_FILE)).is_err());
    }

    /// 69d 互換: 現行の全 migration 適用後の DB が validate を通り、restore がエンドツーエンドで動く
    #[test]
    fn fully_migrated_schema_passes_validation_and_restores() {
        let t = Tmp::new();
        let mk = |p: &Path, m: &str| {
            let c = Connection::open(p).unwrap();
            crate::db::apply_migrations(&c).unwrap();
            c.execute_batch("CREATE TABLE IF NOT EXISTS marker(v TEXT);").unwrap();
            c.execute("INSERT INTO marker VALUES (?1)", [m]).unwrap();
            let _ = c.close();
        };
        mk(&t.0.join(DB_FILE), "current");
        mk(&t.0.join(PENDING_FILE), "restored");
        validate_db_file(&t.0.join(PENDING_FILE)).unwrap();
        let c = Connection::open(t.0.join(DB_FILE)).unwrap();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table'", [], |r| r.get(0)).unwrap();
        println!("tables after full migration: {n}");
        let _ = c.close();
        assert!(matches!(apply_pending_restore(&t.0), RestoreOutcome::Applied { .. }));
        assert_eq!(marker(&t.0.join(DB_FILE)), "restored");
    }

    /// 監査用: R1A_AUDIT_DIR / R1A_AUDIT_PHASE=prepare|apply（通常実行では ignore）
    #[test]
    #[ignore]
    fn audit_fixture() {
        let dir = PathBuf::from(std::env::var("R1A_AUDIT_DIR").expect("R1A_AUDIT_DIR"));
        match std::env::var("R1A_AUDIT_PHASE").unwrap().as_str() {
            "prepare" => {
                std::fs::create_dir_all(&dir).unwrap();
                make_db(&dir.join(DB_FILE), "current", false);
                make_db(&dir.join(PENDING_FILE), "restored", false);
            }
            _ => {
                let o = apply_pending_restore(&dir);
                println!("{o:#?}");
            }
        }
    }
}
