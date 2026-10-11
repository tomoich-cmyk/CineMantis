//! REL-R1ABC integration tests: R1A（restore）× R1B（migration fail-safe）の起動順序と相互作用。
//! すべて synthetic / 一時フォルダの DB。本番 DB・app_data・H2・live TMDB には触れない。
//!
//! 起動フロー（lib.rs の setup と同一順序）を `startup_flow` で再現する:
//!   restore::apply_pending_restore → migrate（検証済み backup → 1 transaction → verify_schema）

use crate::db::{migration_steps, Step, LATEST_SCHEMA_VERSION};
use crate::migrate::{backups_dir_for, migrate_database_with, FailureKind, MigrationError, MigrationReport};
use crate::restore::{self, RestoreOutcome};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

const M001: &str = include_str!("../../../../packages/db/migrations/001_initial.sql");
const M002: &str = include_str!("../../../../packages/db/migrations/002_tmdb_fields.sql");
const M003: &str = include_str!("../../../../packages/db/migrations/003_series.sql");
const M004: &str = include_str!("../../../../packages/db/migrations/004_persons.sql");

struct Dir(PathBuf);
impl Dir {
    fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("cm_r1abc_{}_{}_{}", tag, std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn db(&self) -> PathBuf {
        self.0.join(restore::DB_FILE)
    }
    fn pending(&self) -> PathBuf {
        self.0.join(restore::PENDING_FILE)
    }
    fn backups(&self) -> PathBuf {
        self.0.join("backups")
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sha(p: &Path) -> String {
    format!("{:x}", Sha256::digest(std::fs::read(p).unwrap()))
}

fn checkpoint(p: &Path) {
    let c = Connection::open(p).unwrap();
    let _ = c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
}

/// v1.0.0 リリース相当（001 + 002-004 をエラー無視で流す・user_version=0）の旧 schema DB。theme で出所を識別。
fn make_old_db(path: &Path, theme: &str) {
    {
        let c = Connection::open(path).unwrap();
        c.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;").unwrap();
        c.execute_batch(M001).unwrap();
        for m in [M002, M003, M004] {
            for s in m.split(';') {
                let t = s.trim();
                if !t.is_empty() {
                    let _ = c.execute_batch(t);
                }
            }
        }
        seed(&c, theme);
    }
    checkpoint(path);
}

fn seed(c: &Connection, theme: &str) {
    c.execute_batch(
        "INSERT INTO sources (name, root_path, source_type) VALUES ('NAS', 'Z:/movies', 'nas');
         INSERT INTO files (source_id, file_path, file_name) VALUES (1, 'Z:/movies/a.mkv', 'a.mkv');
         INSERT INTO files (source_id, file_path, file_name) VALUES (1, 'Z:/movies/b.mkv', 'b.mkv');
         INSERT INTO works (work_type, title, tmdb_id, match_status) VALUES ('movie', '七人の侍', 346, 'matched');
         INSERT INTO works (work_type, title) VALUES ('movie', 'Alien');
         INSERT INTO work_parts (work_id, file_id, part_no) VALUES (1, 1, 1);
         INSERT INTO work_parts (work_id, file_id, part_no) VALUES (2, 2, 1);
         INSERT INTO user_stats (work_id, user_rating, is_favorite) VALUES (1, 5, 1);
         INSERT INTO tags (name) VALUES ('名作');
         INSERT INTO work_tags (work_id, tag_id) VALUES (1, 1);",
    )
    .unwrap();
    c.execute("INSERT INTO app_settings (key, value) VALUES ('theme', ?1)", [theme]).unwrap();
}

/// 最新 schema の DB（migration で作成 → user data を入れる）。
fn make_latest_db(path: &Path, theme: &str) {
    let d = path.parent().unwrap().join("_mk_backups");
    migrate_database_with(path, &d, &migration_steps(true)).unwrap();
    let _ = std::fs::remove_dir_all(&d);
    {
        let c = Connection::open(path).unwrap();
        seed(&c, theme);
    }
    checkpoint(path);
}

type Snap = (i64, Vec<(String, String, String)>, BTreeMap<String, i64>, String);

/// 論理 snapshot: (user_version, schema SQL, 全テーブル行数, theme)
fn snap(path: &Path) -> Snap {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    let schema: Vec<(String, String, String)> = {
        let mut st = c.prepare("SELECT type, name, COALESCE(sql,'') FROM sqlite_master ORDER BY type, name").unwrap();
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        rows
    };
    let names: Vec<String> = schema.iter().filter(|s| s.0 == "table" && !s.1.starts_with("sqlite_")).map(|s| s.1.clone()).collect();
    let mut counts = BTreeMap::new();
    for n in names {
        if let Ok(k) = c.query_row(&format!("SELECT COUNT(*) FROM \"{n}\""), [], |r| r.get::<_, i64>(0)) {
            counts.insert(n, k);
        }
    }
    let theme = c.query_row("SELECT value FROM app_settings WHERE key='theme'", [], |r| r.get(0)).unwrap_or_default();
    (v, schema, counts, theme)
}

fn integrity_ok(path: &Path) -> bool {
    let c = Connection::open(path).unwrap();
    c.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)).unwrap() == "ok"
}

/// -wal が無い or 空であること（-shm は SQLite 管理なので問わない）
fn wal_clean(db: &Path) -> bool {
    let p = PathBuf::from(format!("{}-wal", db.display()));
    !p.exists() || std::fs::metadata(&p).map(|m| m.len() == 0).unwrap_or(true)
}

fn failing_step() -> Step {
    Step { name: "injected_fail", file: None, version: 0, data_changing: false, run: |_| Err(anyhow::anyhow!("injected failure")) }
}

fn sentinel_step() -> Step {
    Step {
        name: "sentinel",
        file: None,
        version: 0,
        data_changing: false,
        run: |c| {
            c.execute_batch("CREATE TABLE sentinel_ran (id INTEGER)")?;
            Ok(())
        },
    }
}

/// lib.rs setup と同じ順序: restore（→ record）→ migration。
fn startup_flow(
    app: &Dir,
    hook: &dyn Fn(restore::Step) -> std::io::Result<()>,
    steps: &[Step],
) -> (RestoreOutcome, Result<MigrationReport, MigrationError>) {
    let outcome = restore::apply_with_hook(&app.0, hook);
    restore::record_outcome(&app.0, &outcome);
    let r = migrate_database_with(&app.db(), &backups_dir_for(&app.db()), steps);
    (outcome, r)
}

fn ok_hook(_: restore::Step) -> std::io::Result<()> {
    Ok(())
}

fn premigration_backups(app: &Dir) -> Vec<PathBuf> {
    std::fs::read_dir(app.backups())
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("premigration_"))
                .collect()
        })
        .unwrap_or_default()
}

// ─── I01 ──────────────────────────────────────────────────────────────────────
#[test]
fn i01_restore_current_latest() {
    let app = Dir::new("i01");
    make_latest_db(&app.db(), "current");
    make_latest_db(&app.pending(), "restored");
    let pend = snap(&app.pending());
    let (o, m) = startup_flow(&app, &ok_hook, &migration_steps(true));
    assert!(matches!(o, RestoreOutcome::Applied { .. }), "{o:?}");
    let rep = m.unwrap();
    assert!(rep.backup_path.is_none(), "最新版 → migration backup 不要");
    assert_eq!(rep.to_version, LATEST_SCHEMA_VERSION);
    let s = snap(&app.db());
    assert_eq!(s.3, "restored");
    assert_eq!(s.0, LATEST_SCHEMA_VERSION);
    assert_eq!(s.2["works"], pend.2["works"]);
    assert!(!app.pending().exists() && integrity_ok(&app.db()) && wal_clean(&app.db()));
    eprintln!("I01 final_sha={}", sha(&app.db()));
}

// ─── I02 / C ─────────────────────────────────────────────────────────────────
#[test]
fn i02_restore_old_schema_then_migrate() {
    let app = Dir::new("i02");
    make_latest_db(&app.db(), "current");
    make_old_db(&app.pending(), "restored-old");
    let old = snap(&app.pending());
    assert_eq!(old.0, 0);
    let (o, m) = startup_flow(&app, &ok_hook, &migration_steps(true));
    let RestoreOutcome::Applied { safety_backup, .. } = &o else { panic!("{o:?}") };
    // restore の safety backup は「置き換えられた current DB」（最新 schema / theme=current）
    let sb = PathBuf::from(safety_backup.clone().expect("current があるので退避される"));
    assert_eq!(snap(&sb).3, "current");
    assert!(sb.starts_with(app.0.join("backups").join("pre-restore")));
    let rep = m.unwrap();
    // migration 前 backup は「restore 後に migration 対象となる DB」（旧 schema / restored-old）
    let b = rep.backup_path.clone().expect("版が上がるので backup");
    let bs = snap(&b);
    assert_eq!(bs.0, 0);
    assert_eq!(bs.3, "restored-old");
    assert_eq!(bs.2["works"], old.2["works"]);
    assert_ne!(b, sb);
    // migration 後: user data 保持・版が最新・WAL 異常なし
    let s = snap(&app.db());
    assert_eq!(s.0, LATEST_SCHEMA_VERSION);
    assert_eq!(s.3, "restored-old");
    for t in ["works", "files", "user_stats", "tags", "work_tags", "sources"] {
        assert_eq!(s.2[t], old.2[t], "{t}");
    }
    assert!(integrity_ok(&app.db()) && wal_clean(&app.db()));
    assert_eq!(rep.start_basis, "legacy_probe");
    eprintln!("I02 backup_sha={} final_sha={}", sha(&b), sha(&app.db()));
}

// ─── I03 / D ─────────────────────────────────────────────────────────────────
#[test]
fn i03_restore_then_migration_failure() {
    for n in [1usize, 10, 20] {
        let app = Dir::new("i03");
        make_latest_db(&app.db(), "current");
        make_old_db(&app.pending(), "restored-old");
        let old = snap(&app.pending());
        let mut steps = migration_steps(true);
        steps.insert(n, failing_step());
        let (o, m) = startup_flow(&app, &ok_hook, &steps);
        assert!(matches!(o, RestoreOutcome::Applied { .. }));
        let e = m.expect_err("注入失敗");
        assert_eq!(e.kind, FailureKind::MigrationFailed, "n={n}");
        assert!(e.db_unchanged);
        let b = e.backup_path.clone().expect("migration 前 backup あり");
        assert_eq!(snap(&b).3, "restored-old");
        // restore 済み DB は保持（migration は全巻き戻し）
        assert_eq!(snap(&app.db()), old, "n={n}: restore 直後の状態のまま");
        assert!(!app.pending().exists());
        // 次の起動: restore は再実行されない。migration は成功する
        let (o2, m2) = startup_flow(&app, &ok_hook, &migration_steps(true));
        assert!(matches!(o2, RestoreOutcome::NoPending), "{o2:?}");
        let r = m2.unwrap();
        assert_eq!(r.to_version, LATEST_SCHEMA_VERSION);
        assert_eq!(snap(&app.db()).3, "restored-old", "current に巻き戻っていない");
        // user-visible error path
        let msg = crate::startup::user_message_for_migration(&e, &app.0, None);
        assert!(msg.contains("更新に失敗") && msg.contains(&b.display().to_string()), "{msg}");
    }
}

// ─── I04 / B ─────────────────────────────────────────────────────────────────
#[test]
fn i04_stale_wal_shm_then_restore() {
    let app = Dir::new("i04");
    make_latest_db(&app.db(), "current");
    std::fs::write(format!("{}-wal", app.db().display()), b"stale wal that must never be replayed").unwrap();
    std::fs::write(format!("{}-shm", app.db().display()), b"stale shm").unwrap();
    make_old_db(&app.pending(), "restored-old");
    let old = snap(&app.pending());
    let (o, m) = startup_flow(&app, &ok_hook, &migration_steps(true));
    assert!(matches!(o, RestoreOutcome::Applied { .. }), "{o:?}");
    let rep = m.unwrap();
    assert!(integrity_ok(&app.db()) && wal_clean(&app.db()));
    let s = snap(&app.db());
    assert_eq!((s.3.as_str(), s.0), ("restored-old", LATEST_SCHEMA_VERSION));
    assert_eq!(s.2["works"], old.2["works"]);
    // migration backup は stale WAL を含まない restore 後の内容
    assert_eq!(snap(&rep.backup_path.unwrap()).3, "restored-old");
    // leftover の rollback / staging なし
    let left: Vec<_> = std::fs::read_dir(&app.0)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|f| f.contains(".rollback") || f.contains(".restoring") || f.contains("staging"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn i04b_restore_failure_rolls_back_and_keeps_pending_then_migration_still_safe() {
    // Place 段で一時失敗 → rollback / pending 保持。migration は「現在 DB」に対して安全に走る。
    let app = Dir::new("i04b");
    make_old_db(&app.db(), "current-old");
    make_old_db(&app.pending(), "restored-old");
    let cur = snap(&app.db());
    let hook = |s: restore::Step| {
        if s == restore::Step::Place {
            Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "inject"))
        } else {
            Ok(())
        }
    };
    let (o, m) = startup_flow(&app, &hook, &migration_steps(true));
    assert!(matches!(&o, RestoreOutcome::Failed { .. }), "{o:?}");
    assert!(app.pending().exists(), "再試行可能な失敗は pending 保持");
    let rep = m.unwrap();
    // migration 前 backup の対象は restore に失敗した current DB（restored-old ではない）
    assert_eq!(snap(&rep.backup_path.unwrap()).3, "current-old");
    assert_eq!(snap(&app.db()).3, "current-old");
    assert_eq!(snap(&app.db()).2["works"], cur.2["works"]);
    // 次回起動: pending が再試行されて適用 → 再 migration（最新版へ）
    let (o2, m2) = startup_flow(&app, &ok_hook, &migration_steps(true));
    assert!(matches!(o2, RestoreOutcome::Applied { .. }), "{o2:?}");
    assert!(m2.is_ok());
    assert_eq!(snap(&app.db()).3, "restored-old");
}

// ─── I05 ──────────────────────────────────────────────────────────────────────
#[test]
fn i05_migration_backup_failure_after_restore() {
    let app = Dir::new("i05");
    make_latest_db(&app.db(), "current");
    make_old_db(&app.pending(), "restored-old");
    let old = snap(&app.pending());
    let o = restore::apply_pending_restore(&app.0);
    assert!(matches!(o, RestoreOutcome::Applied { .. }));
    checkpoint(&app.db());
    let after_restore = sha(&app.db());
    // backup 先を「ファイル」にして作成を失敗させる
    let bad = app.0.join("backups_is_a_file");
    std::fs::write(&bad, b"not a dir").unwrap();
    let mut steps = migration_steps(true);
    steps.insert(0, sentinel_step());
    let e = migrate_database_with(&app.db(), &bad, &steps).unwrap_err();
    assert_eq!(e.kind, FailureKind::BackupFailed);
    assert!(e.db_unchanged);
    assert_eq!(snap(&app.db()), old, "backup が無いなら migration は走らず restore 結果のまま");
    checkpoint(&app.db());
    assert_eq!(sha(&app.db()), after_restore, "DB 本体 byte 不変");
    let msg = crate::startup::user_message_for_migration(&e, &app.0, None);
    assert!(msg.contains("バックアップを作れなかった"), "{msg}");
}

// ─── I06 / F ─────────────────────────────────────────────────────────────────
#[test]
fn i06_migration_noncontiguous_legacy_stop_after_restore() {
    let app = Dir::new("i06");
    make_old_db(&app.db(), "current-old");
    // 非連続 legacy: 最新 schema から sync_outbox（018）だけ欠けた user_version=0 の DB を pending にする
    {
        make_latest_db(&app.pending(), "restored-gap");
        let c = Connection::open(app.pending()).unwrap();
        c.execute_batch("DROP TABLE sync_outbox; PRAGMA user_version = 0;").unwrap();
    }
    checkpoint(&app.pending());
    let mut steps = migration_steps(true);
    steps.insert(0, sentinel_step());
    let (o, m) = startup_flow(&app, &ok_hook, &steps);
    assert!(matches!(o, RestoreOutcome::Applied { .. }), "restore 自体は検証に通り適用される: {o:?}");
    checkpoint(&app.db());
    let before = sha(&app.db());
    let e = m.unwrap_err();
    assert_eq!(e.kind, FailureKind::InconsistentLegacySchema, "{e}");
    assert!(e.db_unchanged && e.backup_path.is_none() && e.failed_step.is_none());
    {
        let c = Connection::open(app.db()).unwrap();
        let ran: i64 = c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name='sentinel_ran'", [], |r| r.get(0)).unwrap();
        assert_eq!(ran, 0, "SQL は 1 本も流れていない");
    }
    checkpoint(&app.db());
    assert_eq!(sha(&app.db()), before, "DB byte 不変");
    assert!(premigration_backups(&app).is_empty());
    // 再起動しても restore は再実行されず、同じ停止になる（fail-closed）
    let (o2, m2) = startup_flow(&app, &ok_hook, &migration_steps(true));
    assert!(matches!(o2, RestoreOutcome::NoPending));
    assert_eq!(m2.unwrap_err().kind, FailureKind::InconsistentLegacySchema);
}

#[test]
fn i06b_unknown_schema_is_byte_unchanged() {
    let app = Dir::new("i06b");
    {
        let c = Connection::open(app.db()).unwrap();
        c.execute_batch("PRAGMA journal_mode = WAL; CREATE TABLE foreign_app (id INTEGER); INSERT INTO foreign_app VALUES (1);").unwrap();
    }
    checkpoint(&app.db());
    let before = sha(&app.db());
    let (o, m) = startup_flow(&app, &ok_hook, &migration_steps(true));
    assert!(matches!(o, RestoreOutcome::NoPending));
    assert_eq!(m.unwrap_err().kind, FailureKind::UnknownSchema);
    assert_eq!(sha(&app.db()), before);
}

// ─── I07 ──────────────────────────────────────────────────────────────────────
#[test]
fn i07_restore_failure_leaves_current_db() {
    let app = Dir::new("i07");
    make_latest_db(&app.db(), "current");
    let cur = snap(&app.db());
    std::fs::write(app.pending(), b"garbage".repeat(2000)).unwrap();
    let (o, m) = startup_flow(&app, &ok_hook, &migration_steps(true));
    let RestoreOutcome::Failed { kind, .. } = &o else { panic!("{o:?}") };
    assert_eq!(*kind, restore::FailureKind::Rejected);
    assert!(!app.pending().exists(), "不良 pending は隔離され、次回起動で同じ失敗を繰り返さない");
    assert!(std::fs::read_dir(&app.0).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains(".rejected-")));
    m.unwrap();
    assert_eq!(snap(&app.db()), cur, "logical invariant（migration の maintenance 段が WAL を触るため byte 比較はしない）");
    let msg = crate::startup::user_message_for_restore_failure(&app.0);
    assert!(msg.contains("現在のデータはそのまま") && msg.contains("startup.log"), "{msg}");
}

// ─── I08 / E ─────────────────────────────────────────────────────────────────
#[test]
fn i08_migration_failure_keeps_restore_result_consistent() {
    let app = Dir::new("i08");
    make_latest_db(&app.db(), "current");
    make_old_db(&app.pending(), "restored-old");
    let old = snap(&app.pending());
    let mut steps = migration_steps(true);
    steps.insert(12, failing_step());
    let (_o, m) = startup_flow(&app, &ok_hook, &steps);
    assert!(m.is_err());
    // restore 結果ファイルは Applied のまま、DB は restore 直後 = 結果ファイルと矛盾しない
    let j = std::fs::read_to_string(app.0.join(restore::RESULT_FILE)).unwrap();
    assert!(j.contains("Applied"), "{j}");
    assert_eq!(snap(&app.db()), old);
    // 回復後も結果ファイルは restore の事実を保つ（migration は結果ファイルを書き換えない）
    let (_o2, m2) = startup_flow(&app, &ok_hook, &migration_steps(true));
    m2.unwrap();
    assert!(std::fs::read_to_string(app.0.join(restore::RESULT_FILE)).unwrap().contains("Applied"));
}

#[test]
fn i08b_migrated_db_then_later_restore_of_old_backup() {
    // E: 最新 schema に migrate 済みの DB に、後の起動で旧 schema の pending を置く
    let app = Dir::new("i08b");
    make_old_db(&app.db(), "first");
    let (_, m) = startup_flow(&app, &ok_hook, &migration_steps(true));
    let first = m.unwrap();
    assert_eq!(first.to_version, LATEST_SCHEMA_VERSION);
    // 旧 schema backup（migration 前 backup）を pending として置く
    let pre = first.backup_path.unwrap();
    std::fs::copy(&pre, app.pending()).unwrap();
    {
        // 現在 DB は migrate 後に追加編集されている
        let c = Connection::open(app.db()).unwrap();
        c.execute("UPDATE app_settings SET value='edited-after-migration' WHERE key='theme'", []).unwrap();
    }
    let (o, m2) = startup_flow(&app, &ok_hook, &migration_steps(true));
    let RestoreOutcome::Applied { safety_backup, .. } = &o else { panic!("{o:?}") };
    assert_eq!(snap(&PathBuf::from(safety_backup.clone().unwrap())).3, "edited-after-migration", "置換前の DB が退避される");
    let r = m2.unwrap();
    assert_eq!(r.start_basis, "legacy_probe", "旧版 backup は user_version 0 → legacy probe で再 migration");
    assert_eq!(r.to_version, LATEST_SCHEMA_VERSION);
    let s = snap(&app.db());
    assert_eq!((s.3.as_str(), s.0), ("first", LATEST_SCHEMA_VERSION));
    assert!(integrity_ok(&app.db()));
    // 二回目の起動: 何も起きない
    let (o3, m3) = startup_flow(&app, &ok_hook, &migration_steps(true));
    assert!(matches!(o3, RestoreOutcome::NoPending));
    assert!(m3.unwrap().backup_path.is_none());
}

// ─── I09 ──────────────────────────────────────────────────────────────────────
#[test]
fn i09_fresh_install_no_restore() {
    let app = Dir::new("i09");
    let (o, m) = startup_flow(&app, &ok_hook, &migration_steps(true));
    assert!(matches!(o, RestoreOutcome::NoPending));
    let r = m.unwrap();
    assert!(r.fresh && r.backup_path.is_none());
    assert_eq!(snap(&app.db()).0, LATEST_SCHEMA_VERSION);
    assert!(!app.backups().exists(), "backup も pre-restore も作られない");
    assert!(!app.0.join(restore::RESULT_FILE).exists(), "restore していないので結果ファイルも無い");
    assert!(integrity_ok(&app.db()));
}

// ─── I10 ──────────────────────────────────────────────────────────────────────
#[test]
fn i10_startup_error_surface_path() {
    let app = Dir::new("i10");
    for kind in [
        FailureKind::Corrupted,
        FailureKind::NewerSchema,
        FailureKind::BackupFailed,
        FailureKind::MigrationFailed,
        FailureKind::OpenFailed,
        FailureKind::UnknownSchema,
        FailureKind::InconsistentLegacySchema,
    ] {
        let e = MigrationError {
            kind,
            detail: "boom api_key=0123456789abcdef0123456789abcdef".into(),
            backup_path: None,
            failed_step: None,
            db_unchanged: true,
        };
        let msg = crate::startup::user_message_for_migration(&e, &app.0, None);
        assert!(msg.contains("startup.log"), "{kind:?}: ログの場所を案内する");
        assert!(!msg.contains("0123456789abcdef"), "利用者向け文言に秘密を出さない");
        crate::startup::log_event(&app.0, "ERROR", &e.detail);
    }
    let log = std::fs::read_to_string(crate::startup::log_path(&app.0)).unwrap();
    assert!(!log.contains("0123456789abcdef0123456789abcdef") && log.contains("[REDACTED]"));
    // restore の失敗結果は get_last_restore_result が読む JSON として残る
    std::fs::write(app.pending(), b"garbage".repeat(500)).unwrap();
    let o = restore::apply_pending_restore(&app.0);
    restore::record_outcome(&app.0, &o);
    let j = std::fs::read_to_string(app.0.join(restore::RESULT_FILE)).unwrap();
    assert!(j.contains("Failed") && j.contains("Rejected"), "{j}");
}

// ─── 追加（final audit）: 旧 R1B restore テストの未包含分 ─────────────────────────────
fn leftovers(app: &Dir) -> Vec<String> {
    std::fs::read_dir(&app.0)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|f| f.contains(".rollback") || f.contains(".restoring") || f.contains("staging"))
        .collect()
}

/// 旧 R1B `restore_can_recover_a_corrupted_db_and_saves_the_raw_file_first` 相当
#[test]
fn i11_restore_over_corrupted_current_db_saves_raw_file_and_recovers() {
    let app = Dir::new("i11");
    make_old_db(&app.pending(), "restored-old");
    let old = snap(&app.pending());
    let corrupt = b"corrupted!".repeat(500);
    std::fs::write(app.db(), &corrupt).unwrap();
    let (o, m) = startup_flow(&app, &ok_hook, &migration_steps(true));
    let RestoreOutcome::Applied { safety_backup, .. } = &o else { panic!("{o:?}") };
    let sb = PathBuf::from(safety_backup.clone().expect("壊れた DB も退避される"));
    assert_eq!(std::fs::read(&sb).unwrap(), corrupt, "壊れた current DB は raw のまま退避");
    m.unwrap();
    let s = snap(&app.db());
    assert_eq!((s.3.as_str(), s.0, s.2["works"]), ("restored-old", LATEST_SCHEMA_VERSION, old.2["works"]));
    assert!(integrity_ok(&app.db()) && leftovers(&app).is_empty());
}

/// 旧 R1B `restore_from_a_bad_backup_fails_without_touching_the_current_db` /
/// `pending_restore_failure_is_reported_keeps_db_and_does_not_loop` の byte 水準・非隠蔽の補強
#[test]
fn i12_rejected_restore_keeps_current_bytes_and_failure_is_not_hidden_by_migration() {
    let app = Dir::new("i12");
    make_old_db(&app.db(), "current-old"); // migration 対象の旧 schema
    let bad = b"garbage".repeat(2000);
    std::fs::write(app.pending(), &bad).unwrap();
    let before = sha(&app.db());
    let cur = snap(&app.db());
    // restore 段のみ
    let o = restore::apply_pending_restore(&app.0);
    restore::record_outcome(&app.0, &o);
    assert!(matches!(&o, RestoreOutcome::Failed { kind: restore::FailureKind::Rejected, .. }), "{o:?}");
    assert_eq!(sha(&app.db()), before, "restore 失敗直後、current DB は byte 不変（migration 前）");
    let rej: Vec<_> = std::fs::read_dir(&app.0).unwrap().map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().contains(".rejected-")).collect();
    assert_eq!(rej.len(), 1);
    assert_eq!(std::fs::read(&rej[0]).unwrap(), bad, "不良 pending は削除されず原文のまま隔離");
    assert!(leftovers(&app).is_empty());
    // その後 migration へ進む（現在の startup flow）
    let rep = migrate_database_with(&app.db(), &backups_dir_for(&app.db()), &migration_steps(true)).unwrap();
    assert_eq!(snap(&rep.backup_path.unwrap()).3, "current-old", "backup 対象は current DB");
    assert_eq!(snap(&app.db()).3, "current-old");
    assert_eq!(snap(&app.db()).2["works"], cur.2["works"]);
    // restore 失敗の記録は migration 成功後も残り、次の起動（NoPending）でも上書きされない
    let j = std::fs::read_to_string(app.0.join(restore::RESULT_FILE)).unwrap();
    assert!(j.contains("Failed") && j.contains("Rejected"), "{j}");
    let (o2, _) = startup_flow(&app, &ok_hook, &migration_steps(true));
    assert!(matches!(o2, RestoreOutcome::NoPending));
    assert!(std::fs::read_to_string(app.0.join(restore::RESULT_FILE)).unwrap().contains("Rejected"));
}

/// 一時失敗（Deferred/RolledBack）: pending は byte 不変で保持、current も byte 不変
#[test]
fn i12b_transient_restore_failure_keeps_pending_and_current_bytes() {
    for failing in [restore::Step::Stage, restore::Step::SafetyBackup, restore::Step::MoveAside, restore::Step::Place] {
        let app = Dir::new("i12b");
        make_old_db(&app.db(), "current-old");
        make_old_db(&app.pending(), "restored-old");
        let (db0, pend0) = (sha(&app.db()), sha(&app.pending()));
        let hook = move |s: restore::Step| {
            if s == failing { Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "inject")) } else { Ok(()) }
        };
        let o = restore::apply_with_hook(&app.0, &hook);
        assert!(matches!(&o, RestoreOutcome::Failed { .. }), "{failing:?}: {o:?}");
        assert_eq!(sha(&app.db()), db0, "{failing:?}: current byte 不変");
        if app.pending().exists() {
            assert_eq!(sha(&app.pending()), pend0, "{failing:?}: pending byte 不変");
        } else {
            let q: Vec<_> = std::fs::read_dir(&app.0).unwrap().map(|e| e.unwrap().path())
                .filter(|p| p.to_string_lossy().contains(".rejected-")).collect();
            assert!(q.iter().any(|p| sha(p) == pend0), "{failing:?}: pending は削除されず隔離で保存: {o:?}");
        }
        assert!(leftovers(&app).is_empty(), "{failing:?}: {:?}", leftovers(&app));
    }
}
