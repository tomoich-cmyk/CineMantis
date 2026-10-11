//! migrate.rs のテスト（REL-R1B）。すべて synthetic / 一時フォルダの DB。本番 DB・app_data には触れない。

use super::*;
use crate::db::{migration_steps, UNWIRED_MIGRATION_FILES};
use std::sync::atomic::{AtomicUsize, Ordering};

// ─── helpers ──────────────────────────────────────────────────────────────

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cinemantis_r1b_{}_{}_{}",
            tag,
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn db(&self) -> PathBuf {
        self.0.join("cinemantis.db")
    }
    fn backups(&self) -> PathBuf {
        self.0.join("backups")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const M001: &str = include_str!("../../../../packages/db/migrations/001_initial.sql");
const M002: &str = include_str!("../../../../packages/db/migrations/002_tmdb_fields.sql");
const M003: &str = include_str!("../../../../packages/db/migrations/003_series.sql");
const M004: &str = include_str!("../../../../packages/db/migrations/004_persons.sql");

/// v1.0.0 リリース（main の db.rs: 001 + 002-004 を「エラー無視」で流す）と同じ状態の DB を作る
fn make_v1_release_db(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(M001).unwrap();
    for m in [M002, M003, M004] {
        for stmt in m.split(';') {
            let t = stmt.trim();
            if !t.is_empty() {
                let _ = conn.execute_batch(t);
            }
        }
    }
    seed_user_data(&conn);
    conn.execute_batch("INSERT INTO app_settings (key, value) VALUES ('theme', 'dark');").unwrap();
}

fn seed_user_data(conn: &Connection) {
    conn.execute_batch(
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
}

type Snapshot = (i64, Vec<(String, String, String)>, BTreeMap<String, i64>);

/// 論理スナップショット: (user_version, スキーマ SQL 全部, 全テーブル行数)
fn snapshot(path: &Path) -> Snapshot {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let version = read_user_version(&conn).unwrap();
    let mut stmt = conn
        .prepare("SELECT type, name, COALESCE(sql,'') FROM sqlite_master ORDER BY type, name")
        .unwrap();
    let schema = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    drop(stmt);
    let counts = table_row_counts(&conn).unwrap();
    (version, schema, counts)
}

fn count(path: &Path, sql: &str) -> i64 {
    Connection::open(path).unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
}

fn list_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default()
}

fn failing_step() -> Step {
    Step { name: "injected_fail", file: None, version: 0, data_changing: false, run: |_| Err(anyhow!("injected failure")) }
}

// ─── fresh / old fixture / re-run ───────────────────────────────────────────

#[test]
fn fresh_database_reaches_latest_without_backup() {
    let t = TempDir::new("fresh");
    let report = migrate_database(&t.db()).expect("fresh migrate");
    assert!(report.fresh);
    assert!(report.backup_path.is_none(), "新規 DB には守るデータが無いので backup 不要");
    assert_eq!(report.to_version, LATEST_SCHEMA_VERSION);
    assert!(!t.backups().exists());
    let conn = Connection::open(t.db()).unwrap();
    assert_eq!(read_user_version(&conn).unwrap(), LATEST_SCHEMA_VERSION);
    verify_schema(&conn).unwrap();
}

#[test]
fn v1_release_database_upgrades_with_verified_backup_and_keeps_data() {
    let t = TempDir::new("v1");
    make_v1_release_db(&t.db());
    let before = snapshot(&t.db());
    assert_eq!(before.0, 0, "v1.0 は user_version 未記録");

    let report = migrate_database(&t.db()).expect("upgrade");
    assert!(!report.fresh);
    assert_eq!(report.from_version, 0);
    let backup = report.backup_path.clone().expect("既存データがあるので backup が作られる");
    assert!(backup.starts_with(t.backups()));
    assert!(backup.exists());
    assert!(!PathBuf::from(format!("{}.partial", backup.display())).exists());

    // backup は適用前の状態そのもの（版・行数）
    let b = snapshot(&backup);
    assert_eq!(b.0, before.0);
    assert_eq!(b.2, before.2);
    verify_backup_file(&backup, None).unwrap();

    // 本体は最新版で、ユーザーデータが残っている
    assert_eq!(snapshot(&t.db()).0, LATEST_SCHEMA_VERSION);
    assert_eq!(count(&t.db(), "SELECT COUNT(*) FROM works"), 2);
    assert_eq!(count(&t.db(), "SELECT COUNT(*) FROM files"), 2);
    assert_eq!(count(&t.db(), "SELECT COUNT(*) FROM work_tags"), 1);
    assert_eq!(count(&t.db(), "SELECT COUNT(*) FROM user_stats WHERE is_favorite = 1"), 1);
    verify_schema(&Connection::open(t.db()).unwrap()).unwrap();
}

#[test]
fn legacy_v01_database_with_only_001_upgrades() {
    let t = TempDir::new("legacy");
    {
        let conn = Connection::open(t.db()).unwrap();
        conn.execute_batch(M001).unwrap();
        seed_user_data(&conn);
    }
    let report = migrate_database(&t.db()).expect("legacy upgrade");
    assert!(report.backup_path.is_some());
    assert_eq!(count(&t.db(), "SELECT COUNT(*) FROM works"), 2);
    assert_eq!(snapshot(&t.db()).0, LATEST_SCHEMA_VERSION);
}

#[test]
fn rerun_is_a_noop_without_new_backup_and_backup_survives_success() {
    let t = TempDir::new("rerun");
    make_v1_release_db(&t.db());
    let first = migrate_database(&t.db()).unwrap();
    let backup = first.backup_path.unwrap();

    // 004 の index は列が 006 で増えた後（006_deferred_004_indexes）に作るので、1 回目から決定的。
    let stable = snapshot(&t.db());
    for _ in 0..3 {
        let again = migrate_database(&t.db()).expect("再実行");
        assert_eq!(again.from_version, LATEST_SCHEMA_VERSION);
        assert!(again.backup_path.is_none(), "最新版なら backup は増やさない");
        assert_eq!(again.start_version, LATEST_SCHEMA_VERSION + 1);
        assert_eq!(again.executed, vec!["maintenance_backfill_reading"], "最新版では保守の後埋めしか走らない");
        assert_eq!(snapshot(&t.db()), stable, "再実行で DB は変わらない");
    }
    // 仕様: 成功後も適用前 backup は残る（自動削除しない）
    assert!(backup.exists());
    assert_eq!(list_files(&t.backups()).len(), 1);
}

// ─── failure injection / atomicity ──────────────────────────────────────────

#[test]
fn failure_after_every_step_rolls_back_everything_and_next_start_recovers() {
    let total = migration_steps(true).len();
    assert!(total >= 28, "step 数の取りこぼし検知: {total}");
    for n in 1..=total {
        let t = TempDir::new("inject");
        make_v1_release_db(&t.db());
        let before = snapshot(&t.db());

        let mut steps = migration_steps(true);
        steps.insert(n, failing_step());
        let e = migrate_database_with(&t.db(), &t.backups(), &steps)
            .expect_err(&format!("注入失敗 n={n} は Err になる"));
        assert_eq!(e.kind, FailureKind::MigrationFailed, "n={n}");
        assert_eq!(e.failed_step.as_deref(), Some("injected_fail"), "n={n}");
        assert!(e.db_unchanged, "n={n}");
        assert!(e.backup_path.as_ref().map(|p| p.exists()).unwrap_or(false), "n={n}: backup は残る");
        assert_eq!(snapshot(&t.db()), before, "n={n}: 巻き戻しで完全に元の状態");

        // 次の起動（本物の chain）で回復できる
        let ok = migrate_database(&t.db()).unwrap_or_else(|e| panic!("n={n}: 再起動で回復できない: {e}"));
        assert_eq!(ok.to_version, LATEST_SCHEMA_VERSION);
        assert_eq!(count(&t.db(), "SELECT COUNT(*) FROM works"), 2, "n={n}");
    }
}

#[test]
fn real_sql_error_in_the_middle_of_a_multi_statement_step_leaves_nothing_behind() {
    let t = TempDir::new("sqlerr");
    make_v1_release_db(&t.db());
    let before = snapshot(&t.db());
    let mut steps = migration_steps(true);
    steps.insert(
        10,
        Step {
            name: "bad_sql",
            file: None, version: 0, data_changing: false,
            run: |c| {
                crate::db::apply_migration_sql(
                    c,
                    "x.sql",
                    "CREATE TABLE half_applied (id INTEGER);
                     INSERT INTO half_applied VALUES (1);
                     ALTER TABLE no_such_table ADD COLUMN x TEXT;",
                )
            },
        },
    );
    let e = migrate_database_with(&t.db(), &t.backups(), &steps).unwrap_err();
    assert_eq!(e.kind, FailureKind::MigrationFailed);
    assert_eq!(e.failed_step.as_deref(), Some("bad_sql"));
    assert!(e.detail.contains("no_such_table"), "原因が detail に残る: {}", e.detail);
    assert_eq!(snapshot(&t.db()), before);
    assert_eq!(count(&t.db(), "SELECT COUNT(*) FROM sqlite_master WHERE name = 'half_applied'"), 0);
}

#[test]
fn migration_that_creates_new_foreign_key_violations_is_rejected() {
    let t = TempDir::new("fkviol");
    make_v1_release_db(&t.db());
    let before = snapshot(&t.db());
    let mut steps = migration_steps(true);
    steps.push(Step {
        name: "orphan_insert",
        file: None, version: 0, data_changing: false,
        run: |c| {
            c.execute("INSERT INTO work_parts (work_id, file_id, part_no) VALUES (9999, 9999, 1)", [])?;
            Ok(())
        },
    });
    let e = migrate_database_with(&t.db(), &t.backups(), &steps).unwrap_err();
    assert_eq!(e.failed_step.as_deref(), Some("foreign_key_check"));
    assert_eq!(snapshot(&t.db()), before);
}

#[test]
fn foreign_keys_pragma_is_restored_after_success_and_failure() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    let fk = |c: &Connection| c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0)).unwrap();
    crate::db::apply_migrations(&conn).unwrap();
    assert_eq!(fk(&conn), 1);
    let mut steps = migration_steps(true);
    steps.push(failing_step());
    assert!(run_steps_atomically(&conn, &steps, Some(LATEST_SCHEMA_VERSION)).is_err());
    assert_eq!(fk(&conn), 1);
}

// ─── schema version mismatch ────────────────────────────────────────────────

#[test]
fn database_from_a_newer_app_is_refused_and_untouched() {
    let t = TempDir::new("newer");
    make_v1_release_db(&t.db());
    Connection::open(t.db()).unwrap().execute_batch("PRAGMA user_version = 99").unwrap();
    let before = snapshot(&t.db());
    let e = migrate_database(&t.db()).unwrap_err();
    assert_eq!(e.kind, FailureKind::NewerSchema);
    assert_eq!(snapshot(&t.db()), before);
    assert!(!t.backups().exists(), "触らないので backup も作らない");
}

#[test]
fn version_stamp_is_refused_when_actual_schema_is_incomplete() {
    let conn = Connection::open_in_memory().unwrap();
    // 001 だけ流して「最新版」を刻もうとする → 実体不足なので刻まれず、全部巻き戻る
    let only_001 = vec![Step { name: "001_initial", file: None, version: 0, data_changing: false, run: |c| Ok(c.execute_batch(M001)?) }];
    let e = run_steps_atomically(&conn, &only_001, Some(LATEST_SCHEMA_VERSION)).unwrap_err();
    assert_eq!(e.downcast_ref::<StepError>().map(|s| s.step.as_str()), Some("verify_schema"));
    assert_eq!(read_user_version(&conn).unwrap(), 0);
    assert_eq!(user_table_count(&conn).unwrap(), 0, "001 の作成も巻き戻っている");
}

#[test]
fn version_25_with_missing_pieces_is_a_failure_not_a_silent_repair() {
    // 版は最新だが実体が欠けている DB。版を信頼して何かを推測で補うことはせず、検証で失敗にする。
    let t = TempDir::new("mismatch");
    migrate_database(&t.db()).unwrap();
    Connection::open(t.db()).unwrap().execute_batch("DROP TABLE sync_outbox;").unwrap();
    let before = snapshot(&t.db());
    let e = migrate_database(&t.db()).unwrap_err();
    assert_eq!(e.kind, FailureKind::MigrationFailed);
    assert_eq!(e.failed_step.as_deref(), Some("verify_schema"));
    assert!(e.db_unchanged);
    assert_eq!(snapshot(&t.db()), before);
}

// ─── partially migrated fixtures ────────────────────────────────────────────

#[test]
fn partially_migrated_fixtures_complete_cleanly() {
    let total = migration_steps(true).len();
    for k in [1, 4, 7, 12, 19, 22, 26, total - 2] {
        let t = TempDir::new("partial");
        make_v1_release_db(&t.db());
        {
            // 旧実装の「途中まで進んだ（版は未記録）」状態を再現する: 先頭 k 段だけ適用
            let conn = Connection::open(t.db()).unwrap();
            conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
            run_steps_atomically(&conn, &migration_steps(true)[..k], None).unwrap();
        }
        let report = migrate_database(&t.db()).unwrap_or_else(|e| panic!("k={k}: {e}"));
        assert!(report.backup_path.is_some(), "k={k}");
        assert_eq!(count(&t.db(), "SELECT COUNT(*) FROM works"), 2, "k={k}");
        verify_schema(&Connection::open(t.db()).unwrap()).unwrap();
        migrate_database(&t.db()).unwrap();
    }
}

// ─── corrupted DB ───────────────────────────────────────────────────────────

#[test]
fn garbage_file_is_reported_as_corrupted_and_left_untouched() {
    let t = TempDir::new("garbage");
    std::fs::write(t.db(), b"this is definitely not a sqlite database, just text".repeat(50)).unwrap();
    let bytes = std::fs::read(t.db()).unwrap();
    let e = migrate_database(&t.db()).unwrap_err();
    assert_eq!(e.kind, FailureKind::Corrupted);
    assert_eq!(std::fs::read(t.db()).unwrap(), bytes, "壊れた DB は書き換えない");
    assert!(!t.backups().exists());
}

#[test]
fn truncated_database_is_reported_as_corrupted() {
    let t = TempDir::new("trunc");
    make_v1_release_db(&t.db());
    {
        let conn = Connection::open(t.db()).unwrap();
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).unwrap();
    }
    let len = std::fs::metadata(t.db()).unwrap().len();
    let f = std::fs::OpenOptions::new().write(true).open(t.db()).unwrap();
    f.set_len(len / 2 / 4096 * 4096).unwrap();
    drop(f);
    let _ = std::fs::remove_file(format!("{}-wal", t.db().display()));
    let _ = std::fs::remove_file(format!("{}-shm", t.db().display()));
    let e = migrate_database(&t.db()).unwrap_err();
    assert_eq!(e.kind, FailureKind::Corrupted, "{e}");
}

// ─── backup failure / verification ──────────────────────────────────────────

#[test]
fn backup_failure_stops_before_any_migration_step_runs() {
    let t = TempDir::new("bkfail");
    make_v1_release_db(&t.db());
    // backups の場所を「ファイル」にして作成を失敗させる
    std::fs::write(t.backups(), b"i am a file, not a folder").unwrap();
    let before = snapshot(&t.db());
    let mut steps = migration_steps(true);
    steps.insert(
        0,
        Step {
            name: "sentinel",
            file: None, version: 0, data_changing: false,
            run: |c| {
                c.execute_batch("CREATE TABLE sentinel_ran (id INTEGER)")?;
                Ok(())
            },
        },
    );
    let e = migrate_database_with(&t.db(), &t.backups(), &steps).unwrap_err();
    assert_eq!(e.kind, FailureKind::BackupFailed);
    assert!(e.db_unchanged);
    assert_eq!(snapshot(&t.db()), before, "backup が無いなら migration は 1 段も走らない");
}

#[test]
fn backup_verification_rejects_corrupt_empty_and_mismatched_backups() {
    let t = TempDir::new("bkverify");
    make_v1_release_db(&t.db());
    let conn = Connection::open(t.db()).unwrap();
    let backup = create_verified_backup(&conn, &t.backups(), 0, LATEST_SCHEMA_VERSION).unwrap();
    let counts = table_row_counts(&conn).unwrap();
    verify_backup_file(&backup, Some((&counts, 0))).unwrap();

    // 行数が違う / 版が違う → 不一致
    let mut wrong = counts.clone();
    *wrong.get_mut("works").unwrap() += 1;
    assert!(verify_backup_file(&backup, Some((&wrong, 0))).is_err());
    assert!(verify_backup_file(&backup, Some((&counts, 3))).is_err());

    // 空ファイル / 壊れたヘッダ / 無いファイル
    let empty = t.path().join("empty.db");
    std::fs::write(&empty, b"").unwrap();
    assert!(verify_backup_file(&empty, None).is_err());
    let mut bytes = std::fs::read(&backup).unwrap();
    for b in bytes.iter_mut().take(200) {
        *b = 0xFF;
    }
    let broken = t.path().join("broken.db");
    std::fs::write(&broken, &bytes).unwrap();
    assert!(verify_backup_file(&broken, None).is_err());
    assert!(verify_backup_file(&t.path().join("missing.db"), None).is_err());

    // ページの中身を壊す（ヘッダは無傷）
    let mut bytes = std::fs::read(&backup).unwrap();
    let n = bytes.len();
    for b in bytes[n / 2..n / 2 + 4096.min(n / 2)].iter_mut() {
        *b = 0xA5;
    }
    let damaged = t.path().join("damaged.db");
    std::fs::write(&damaged, &bytes).unwrap();
    assert!(verify_backup_file(&damaged, Some((&counts, 0))).is_err());

    // backups フォルダには確定済みの .db だけが残る（.partial は残らない）
    assert!(list_files(&t.backups()).iter().all(|p| p.extension().and_then(|e| e.to_str()) == Some("db")));
}

#[test]
fn find_latest_valid_backup_skips_broken_files() {
    let t = TempDir::new("bkfind");
    make_v1_release_db(&t.db());
    let conn = Connection::open(t.db()).unwrap();
    let good = create_verified_backup(&conn, &t.backups(), 0, 25).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    std::fs::write(t.backups().join("newer_but_broken.db"), b"garbage").unwrap();
    assert_eq!(find_latest_valid_backup(&t.backups()), Some(good));
}

// ─── migration chain の健全性 ───────────────────────────────────────────────

fn migrations_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../packages/db/migrations")
}

#[test]
fn migration_files_are_all_wired_or_explicitly_unwired() {
    let on_disk: std::collections::BTreeSet<String> = std::fs::read_dir(migrations_dir())
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".sql"))
        .collect();
    let wired: std::collections::BTreeSet<String> =
        migration_steps(true).iter().filter_map(|s| s.file.map(String::from)).collect();
    let unwired: std::collections::BTreeSet<String> = UNWIRED_MIGRATION_FILES.iter().map(|s| s.to_string()).collect();

    let missing: Vec<_> = on_disk.iter().filter(|f| !wired.contains(*f) && !unwired.contains(*f)).collect();
    assert!(missing.is_empty(), "chain に入っていない migration ファイル（配線漏れの疑い）: {missing:?}");
    let ghost: Vec<_> = wired.iter().chain(unwired.iter()).filter(|f| !on_disk.contains(*f)).collect();
    assert!(ghost.is_empty(), "存在しないファイルを参照: {ghost:?}");
    assert!(wired.is_disjoint(&unwired), "配線済みを UNWIRED に入れない");

    let max = on_disk.iter().filter_map(|n| n[..3].parse::<i64>().ok()).max().unwrap();
    assert_eq!(LATEST_SCHEMA_VERSION, max, "LATEST_SCHEMA_VERSION は最後の migration 番号と揃える");
}

#[test]
fn step_names_are_unique() {
    let steps = migration_steps(true);
    let names: std::collections::BTreeSet<_> = steps.iter().map(|s| s.name).collect();
    assert_eq!(names.len(), steps.len());
}

#[test]
fn fresh_database_gets_the_004_indexes_deterministically_on_first_run() {
    let t = TempDir::new("idx");
    migrate_database(&t.db()).unwrap();
    for index in ["idx_persons_tmdb_id", "idx_work_persons_role", "idx_persons_tmdb_id_compat", "idx_work_persons_role_compat", "idx_series_tmdb_id_compat"] {
        assert_eq!(
            count(&t.db(), &format!("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='{index}'")),
            1,
            "{index}"
        );
    }
}

#[test]
fn split_sql_statements_respects_comments_strings_and_triggers() {
    let parts = crate::db::split_sql_statements(
        "-- head; comment
CREATE TABLE a (x TEXT DEFAULT ';');
CREATE TRIGGER g AFTER INSERT ON a BEGIN SELECT 1; SELECT 2; END;
-- tail only
",
    );
    assert_eq!(parts.len(), 2, "{parts:?}");
    assert!(parts[1].contains("SELECT 2; END;"));
}

#[test]
fn nested_table_rebuild_inside_the_chain_is_rolled_back_with_it() {
    // 022 の作り直し（SAVEPOINT）が成功したあとで後続が失敗しても、作り直し自体が巻き戻る
    let t = TempDir::new("rebuild");
    make_v1_release_db(&t.db());
    let conn = Connection::open(t.db()).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    let all = migration_steps(true);
    let idx = all.iter().position(|s| s.name == "022_rebuild_review_tasks").unwrap();
    run_steps_atomically(&conn, &all[..idx], None).unwrap();
    let sql_of = |c: &Connection| {
        c.query_row("SELECT sql FROM sqlite_master WHERE name='metadata_review_tasks'", [], |r| r.get::<_, String>(0)).unwrap()
    };
    let before = sql_of(&conn);
    assert!(!before.contains(crate::db::REVIEW_TASKS_MARKER));

    let mut upto: Vec<Step> = migration_steps(true).into_iter().skip(idx).take(2).collect();
    upto.push(failing_step());
    assert!(run_steps_atomically(&conn, &upto, None).is_err());
    assert_eq!(before, sql_of(&conn), "作り直しも巻き戻る");
}

/// 手動 dry-run（既定では実行しない）。実 DB を**読み取り専用で開いて**一時フォルダへ複製し、
/// 複製だけに migration をかける。元の DB・WAL・shm には一切書き込まない。
///
///   $env:CINEMANTIS_R1B_DRYRUN_DB = "<DB のパス>"
///   cargo test --lib dry_run_on_a_copy_of_a_real_database -- --ignored --nocapture
#[test]
#[ignore = "手動 dry-run 用。環境変数 CINEMANTIS_R1B_DRYRUN_DB に DB のパスを指定する"]
fn dry_run_on_a_copy_of_a_real_database() {
    let Ok(src) = std::env::var("CINEMANTIS_R1B_DRYRUN_DB") else {
        println!("CINEMANTIS_R1B_DRYRUN_DB が未指定のためスキップ");
        return;
    };
    let t = TempDir::new("dryrun");
    {
        let ro = Connection::open_with_flags(&src, OpenFlags::SQLITE_OPEN_READ_ONLY).expect("読み取り専用で開く");
        ro.execute(&format!("VACUUM INTO '{}'", t.db().to_string_lossy().replace('\'', "''")), []).expect("複製");
    }
    let before = snapshot(&t.db());
    let report = migrate_database(&t.db()).expect("複製への migration");
    let after = snapshot(&t.db());
    println!("from v{} -> v{} / backup={:?}", report.from_version, report.to_version, report.backup_path);
    println!("start v{} ({}) / executed={:?} / data steps={:?}", report.start_version, report.start_basis, report.executed, report.data_steps_run);
    for (table, n) in &before.2 {
        let now = after.2.get(table).copied().unwrap_or(-1);
        if *n != now {
            println!("  行数の変化: {table}: {n} -> {now}");
        }
    }
    verify_schema(&Connection::open(t.db()).unwrap()).unwrap();
}

// ═══ legacy bootstrap 契約（user_version = 0 の既存 DB） ═══════════════════════

fn step_version(name: &str) -> i64 {
    migration_steps(true).iter().find(|s| s.name == name).unwrap_or_else(|| panic!("step {name}")).version
}

const SENTINEL: &str = "Q_USER_EDIT_SENTINEL";

/// A: 現行相当 schema + user_version=0（実ユーザーの既存 DB と同じ状態）。
/// 012 が上書きする値（Palme d'Or の QID）をユーザー編集値にして、データ更新 migration の再実行を検知する。
fn make_current_schema_unversioned_db(path: &Path) {
    migrate_database(path).unwrap();
    let conn = Connection::open(path).unwrap();
    seed_user_data(&conn);
    let n = conn
        .execute(
            &format!("UPDATE award_categories SET wikidata_entity_id = '{SENTINEL}' WHERE name = 'Palme d''Or'"),
            [],
        )
        .unwrap();
    assert_eq!(n, 1, "seed に Palme d'Or がある");
    conn.execute_batch("PRAGMA user_version = 0").unwrap();
}

fn sentinel_count(path: &Path) -> i64 {
    count(path, &format!("SELECT COUNT(*) FROM award_categories WHERE wikidata_entity_id = '{SENTINEL}'"))
}

#[test]
fn bootstrap_a_current_schema_with_user_version_0_runs_no_migration_and_no_data_change() {
    let t = TempDir::new("boot_a");
    make_current_schema_unversioned_db(&t.db());
    let before = snapshot(&t.db());
    assert_eq!(before.0, 0);

    let report = migrate_database(&t.db()).expect("bootstrap");
    assert_eq!(report.start_basis, "legacy_probe");
    assert_eq!(report.start_version, LATEST_SCHEMA_VERSION + 1, "証拠が全部揃っている → 実行する migration は無い");
    assert_eq!(report.executed, vec!["maintenance_backfill_reading"]);
    assert_eq!(report.data_steps_run, vec!["maintenance_backfill_reading"], "再実行されるデータ更新は、WHERE で絞った後埋めだけ");
    assert_eq!(sentinel_count(&t.db()), 1, "012 等のデータ補正が再実行されていない（ユーザー編集値が残る）");
    // 適用前 backup は legacy DB の状態そのもの（版 0）
    let b = snapshot(report.backup_path.as_ref().expect("版が上がるので backup"));
    assert_eq!(b.0, 0);
    assert_eq!(b.2, before.2);
    // 版は検証（verify_schema）に通った後で初めて刻まれる
    assert_eq!(snapshot(&t.db()).0, LATEST_SCHEMA_VERSION);
    assert_eq!(snapshot(&t.db()).2, before.2, "行数も変わらない");
}

#[test]
fn bootstrap_b1_v1_release_schema_starts_at_005_and_never_reruns_001_to_004() {
    let t = TempDir::new("boot_b1");
    make_v1_release_db(&t.db());
    let report = migrate_database(&t.db()).unwrap();
    assert_eq!((report.start_version, report.start_basis), (5, "legacy_probe"));
    let all = migration_steps(true);
    let expected: Vec<String> =
        all.iter().filter(|s| s.version == 0 || s.version >= 5).map(|s| s.name.to_string()).collect();
    assert_eq!(report.executed, expected);
    for name in ["001_initial", "002_tmdb_fields", "003_series", "004_persons"] {
        assert!(!report.executed.iter().any(|n| n == name), "{name} は再実行しない");
    }
    assert_eq!(count(&t.db(), "SELECT COUNT(*) FROM works"), 2);
    verify_schema(&Connection::open(t.db()).unwrap()).unwrap();
}

#[test]
fn bootstrap_b2_old_intermediate_schema_starts_right_after_the_last_proven_migration() {
    let t = TempDir::new("boot_b2");
    make_v1_release_db(&t.db());
    {
        // 古い中間版: 010 までを適用した状態（版は未記録）
        let conn = Connection::open(t.db()).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        let upto10: Vec<Step> = migration_steps(true).into_iter().filter(|s| s.version != 0 && s.version <= 10).collect();
        run_steps_atomically(&conn, &upto10, None).unwrap();
        conn.execute_batch(&format!("UPDATE award_categories SET wikidata_entity_id = '{SENTINEL}' WHERE name = 'Palme d''Or'")).unwrap();
        assert_eq!(read_user_version(&conn).unwrap(), 0);
    }
    let report = migrate_database(&t.db()).unwrap();
    assert_eq!(report.start_version, 11);
    assert!(report.executed.iter().all(|n| n == "maintenance_backfill_reading" || step_version(n) >= 11), "{:?}", report.executed);
    // 011 以降のデータ補正は、まだ適用されていないので実行される（意図どおり）
    assert!(report.data_steps_run.iter().any(|n| n == "012_award_cannes_qid_fix"));
    assert_eq!(sentinel_count(&t.db()), 0);
    assert_eq!(count(&t.db(), "SELECT COUNT(*) FROM works"), 2);
}

fn file_bytes(t: &TempDir) -> (Vec<u8>, bool) {
    let wal = PathBuf::from(format!("{}-wal", t.db().display()));
    (std::fs::read(t.db()).unwrap(), wal.exists() && std::fs::metadata(&wal).map(|m| m.len() > 0).unwrap_or(false))
}

/// gap のある legacy schema は fail-closed で拒否する: SQL を 1 本も流さず、版・データ・DB 本体を変えない
fn assert_rejected_as_inconsistent(t: &TempDir) {
    // WAL が残っていると byte 比較がぶれるので、先に畳んでおく（これは fixture 作成の一部）
    Connection::open(t.db()).unwrap().query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).unwrap();
    let bytes_before = file_bytes(t);
    let snap_before = snapshot(&t.db());
    let mut steps = migration_steps(true);
    // 1 本でも流れたら検知できる番兵（version 0 = 常に対象）
    steps.insert(
        0,
        Step {
            name: "sentinel",
            file: None,
            version: 0,
            data_changing: false,
            run: |c| {
                c.execute_batch("CREATE TABLE sentinel_ran (id INTEGER)")?;
                Ok(())
            },
        },
    );
    let e = migrate_database_with(&t.db(), &t.backups(), &steps).unwrap_err();
    assert_eq!(e.kind, FailureKind::InconsistentLegacySchema, "{e}");
    assert!(e.detail.contains("INCONSISTENT_LEGACY_SCHEMA"));
    assert!(e.db_unchanged && e.backup_path.is_none() && e.failed_step.is_none());
    assert_eq!(file_bytes(t), bytes_before, "DB 本体が byte 単位で不変");
    assert_eq!(snapshot(&t.db()), snap_before);
    assert_eq!(snap_before.0, 0, "user_version は 0 のまま");
    assert_eq!(count(&t.db(), "SELECT COUNT(*) FROM sqlite_master WHERE name = 'sentinel_ran'"), 0);
    assert!(!t.backups().exists());
    // 利用者向けメッセージは「自動修復できない・別手順が必要」を伝え、ログの場所を案内する
    let msg = crate::startup::user_message_for_migration(&e, t.path(), None);
    assert!(msg.contains("修復") && msg.contains("startup.log"), "{msg}");
}

#[test]
fn bootstrap_b3_gap_at_011_with_later_evidence_is_rejected_fail_closed() {
    // 001〜010 あり / 018（sync_outbox）なし / 020 以降あり → 非連続。011〜019 を再実行して直さない
    let t = TempDir::new("boot_b3");
    make_current_schema_unversioned_db(&t.db());
    Connection::open(t.db()).unwrap().execute_batch("DROP TABLE sync_outbox").unwrap();
    assert_rejected_as_inconsistent(&t);
    assert_eq!(sentinel_count(&t.db()), 1, "ユーザー編集値も不変");
}

#[test]
fn bootstrap_multiple_gaps_are_rejected() {
    let t = TempDir::new("boot_multigap");
    make_current_schema_unversioned_db(&t.db());
    Connection::open(t.db())
        .unwrap()
        .execute_batch("DROP TABLE sync_outbox; DROP TABLE award_body_schedule_rules; DROP TABLE metadata_match_jev_run_links;")
        .unwrap();
    assert_rejected_as_inconsistent(&t);
}

#[test]
fn bootstrap_gap_in_the_middle_of_old_schema_is_rejected() {
    // v1.0 DB に、005 の証拠を欠いたまま 007（awards）だけ入っている
    let t = TempDir::new("boot_oldgap");
    make_v1_release_db(&t.db());
    let awards = include_str!("../../../../packages/db/migrations/007_awards.sql").replace("PRAGMA", "-- PRAGMA");
    Connection::open(t.db()).unwrap().execute_batch(&awards).unwrap();
    assert_rejected_as_inconsistent(&t);
}

#[test]
fn bootstrap_contiguous_prefix_never_reruns_data_changing_migrations_below_the_start() {
    // 連続 prefix（010 まで）→ 011 から。010 以前の段（データ更新を含む）は実行されない
    let t = TempDir::new("boot_prefix");
    make_v1_release_db(&t.db());
    {
        let conn = Connection::open(t.db()).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        let upto10: Vec<Step> = migration_steps(true).into_iter().filter(|s| s.version != 0 && s.version <= 10).collect();
        run_steps_atomically(&conn, &upto10, None).unwrap();
    }
    let report = migrate_database(&t.db()).unwrap();
    assert_eq!(report.start_version, 11);
    let below: Vec<_> = migration_steps(true).into_iter().filter(|s| s.version != 0 && s.version < 11).map(|s| s.name).collect();
    for name in below {
        assert!(!report.executed.iter().any(|n| n == name), "{name} は再実行しない");
    }
}

#[test]
fn bootstrap_c_fresh_database_runs_everything_from_001() {
    let t = TempDir::new("boot_c");
    let report = migrate_database(&t.db()).unwrap();
    assert_eq!((report.start_version, report.start_basis), (1, "fresh"));
    let all: Vec<String> = migration_steps(true).iter().map(|s| s.name.to_string()).collect();
    assert_eq!(report.executed, all);
    assert_eq!(snapshot(&t.db()).0, LATEST_SCHEMA_VERSION);
}

#[test]
fn bootstrap_rejects_an_unrecognised_database_without_touching_it() {
    let t = TempDir::new("boot_unknown");
    {
        let conn = Connection::open(t.db()).unwrap();
        conn.execute_batch("CREATE TABLE somebody_elses_table (id INTEGER); INSERT INTO somebody_elses_table VALUES (1);").unwrap();
    }
    let before = snapshot(&t.db());
    let e = migrate_database(&t.db()).unwrap_err();
    assert_eq!(e.kind, FailureKind::UnknownSchema);
    assert_eq!(snapshot(&t.db()), before);
    assert!(!t.backups().exists());
}

#[test]
fn bootstrap_failure_leaves_the_legacy_database_and_its_version_untouched() {
    // A: 現行相当 + 版 0 に、最後で失敗する段を足す → 版は 0 のまま、データも不変、backup は残る
    let t = TempDir::new("boot_fail_a");
    make_current_schema_unversioned_db(&t.db());
    let before = snapshot(&t.db());
    let mut steps = migration_steps(true);
    steps.push(failing_step());
    let e = migrate_database_with(&t.db(), &t.backups(), &steps).unwrap_err();
    assert_eq!(e.kind, FailureKind::MigrationFailed);
    assert!(e.db_unchanged && e.backup_path.is_some());
    assert_eq!(snapshot(&t.db()), before);
    assert_eq!(sentinel_count(&t.db()), 1);
}

#[test]
fn bootstrap_stops_on_a_unique_index_data_conflict_and_rolls_back() {
    // v1.0 DB に tmdb_person_id が重複した persons がある → 006 の UNIQUE index が作れない。
    // 旧実装は黙って index 無しで続行していたが、今は STOP（巻き戻し）。
    let t = TempDir::new("boot_unique");
    make_v1_release_db(&t.db());
    Connection::open(t.db())
        .unwrap()
        .execute_batch(
            "INSERT INTO persons (name, tmdb_person_id) VALUES ('A', 5);
             INSERT INTO persons (name, tmdb_person_id) VALUES ('A dup', 5);",
        )
        .unwrap();
    let before = snapshot(&t.db());
    let e = migrate_database(&t.db()).unwrap_err();
    assert_eq!(e.kind, FailureKind::MigrationFailed);
    assert_eq!(e.failed_step.as_deref(), Some("006_legacy_schema_compat"));
    assert!(e.db_unchanged);
    assert_eq!(snapshot(&t.db()), before);
}

#[test]
fn explicit_user_version_is_trusted_for_the_start_version() {
    let t = TempDir::new("boot_uv");
    migrate_database(&t.db()).unwrap();
    Connection::open(t.db()).unwrap().execute_batch("PRAGMA user_version = 22").unwrap();
    let report = migrate_database(&t.db()).unwrap();
    assert_eq!((report.start_version, report.start_basis), (23, "user_version"));
    assert!(report.executed.iter().all(|n| n == "maintenance_backfill_reading" || step_version(n) >= 23));
}

// ═══ SQL エラー許容の厳密化 ═════════════════════════════════════════════════

fn conn_with_001() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::apply_strict_migration(&conn, M001).unwrap();
    conn
}

#[test]
fn benign_known_duplicate_add_column_passes() {
    let conn = conn_with_001();
    crate::db::apply_migration_sql(&conn, "002_tmdb_fields.sql", M002).unwrap();
    // 適用済みの DB へもう一度（全 ALTER が既知の重複列として通る）
    crate::db::apply_migration_sql(&conn, "002_tmdb_fields.sql", M002).expect("既知の idempotent ケース");
}

#[test]
fn unexpected_duplicate_column_stops() {
    let conn = conn_with_001();
    // 一覧に無い列
    assert!(crate::db::apply_migration_sql(&conn, "002_tmdb_fields.sql", "ALTER TABLE works ADD COLUMN title TEXT;").is_err());
    // 一覧にある列でも、別 migration からの重複は不可
    crate::db::apply_migration_sql(&conn, "002_tmdb_fields.sql", M002).unwrap();
    assert!(crate::db::apply_migration_sql(&conn, "003_series.sql", "ALTER TABLE works ADD COLUMN title_guess TEXT;").is_err());
    // 型が違う既存列
    let other = Connection::open_in_memory().unwrap();
    other.execute_batch("CREATE TABLE works (title_guess INTEGER);").unwrap();
    assert!(crate::db::apply_migration_sql(&other, "002_tmdb_fields.sql", "ALTER TABLE works ADD COLUMN title_guess TEXT;").is_err());
    // ALTER 以外の「既に存在する」エラー
    assert!(crate::db::apply_migration_sql(&conn, "x.sql", "CREATE TABLE works (id INTEGER);").is_err());
}

#[test]
fn existing_identical_index_passes_but_every_other_index_failure_stops() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE t (a INTEGER, b INTEGER); INSERT INTO t VALUES (1, 1), (1, 2);").unwrap();
    let ok = "CREATE INDEX IF NOT EXISTS idx_t_a ON t(a);";
    crate::db::apply_migration_sql(&conn, "x.sql", ok).unwrap();
    crate::db::apply_migration_sql(&conn, "x.sql", ok).expect("同じ index が既にあるケース");
    // UNIQUE index がデータ衝突で作れない → STOP
    assert!(crate::db::apply_migration_sql(&conn, "x.sql", "CREATE UNIQUE INDEX u_a ON t(a);").is_err());
    // 任意の CREATE INDEX 失敗 → STOP（存在しない列 / 表 / IF NOT EXISTS 無しの同名）
    assert!(crate::db::apply_migration_sql(&conn, "x.sql", "CREATE INDEX IF NOT EXISTS i1 ON t(nope);").is_err());
    assert!(crate::db::apply_migration_sql(&conn, "x.sql", "CREATE INDEX IF NOT EXISTS i2 ON missing(a);").is_err());
    assert!(crate::db::apply_migration_sql(&conn, "x.sql", "CREATE INDEX idx_t_a ON t(a);").is_err());
    assert!(crate::db::apply_migration_sql(&conn, "x.sql", "THIS IS NOT SQL;").is_err());
}

#[test]
fn only_the_two_known_004_indexes_are_deferred_and_only_while_their_column_is_missing() {
    let conn = conn_with_001();
    // 001 の persons には tmdb_id が無い。004 は該当 index だけ後回しにして成功する
    crate::db::apply_migration_sql_with(&conn, "004_persons.sql", M004, &crate::db::defer_004_index_if_column_missing).unwrap();
    let idx = |name: &str| -> i64 {
        conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = ?1", [name], |r| r.get(0)).unwrap()
    };
    assert_eq!((idx("idx_persons_tmdb_id"), idx("idx_work_persons_role")), (0, 0));
    // 後回し対象以外の index は、列が無ければ失敗する
    assert!(crate::db::apply_migration_sql_with(
        &conn,
        "004_persons.sql",
        "CREATE INDEX IF NOT EXISTS idx_other ON persons(no_such_column);",
        &crate::db::defer_004_index_if_column_missing
    )
    .is_err());
    // 列が揃えば後回しにせず普通に作る
    conn.execute_batch("ALTER TABLE persons ADD COLUMN tmdb_id INTEGER;").unwrap();
    crate::db::apply_migration_sql_with(&conn, "004_persons.sql", M004, &crate::db::defer_004_index_if_column_missing).unwrap();
    assert_eq!(idx("idx_persons_tmdb_id"), 1);
}

#[test]
fn known_add_columns_match_the_migration_files_exactly() {
    let re = regex::Regex::new(r"(?is)ALTER\s+TABLE\s+(\w+)\s+ADD\s+COLUMN\s+(\w+)\s+(\w+)").unwrap();
    let mut from_files = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(migrations_dir()).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.ends_with(".sql") {
            continue;
        }
        let text: String = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter(|l| !l.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        for c in re.captures_iter(&text) {
            from_files.insert((name.clone(), c[1].to_string(), c[2].to_string(), c[3].to_uppercase()));
        }
    }
    let listed: std::collections::BTreeSet<_> = crate::db::KNOWN_ADD_COLUMNS
        .iter()
        .map(|(f, t, c, ty)| (f.to_string(), t.to_string(), c.to_string(), ty.to_string()))
        .collect();
    assert_eq!(listed, from_files, "KNOWN_ADD_COLUMNS は migration ファイルの ADD COLUMN と一致させる");
}
