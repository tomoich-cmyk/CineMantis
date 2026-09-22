use anyhow::Result;
use rusqlite::functions::FunctionFlags;
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// DB 接続を Arc<Mutex> でラップすることで
/// tauri::async_runtime::spawn に渡せるようにする（Clone 可能）
#[derive(Clone)]
pub struct DbState(pub Arc<Mutex<Connection>>);

impl DbState {
    pub fn new(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        register_functions(&conn)?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }
}

fn register_functions(conn: &Connection) -> Result<()> {
    conn.create_scalar_function(
        "cm_kana_sort_key",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let value: Option<String> = ctx.get(0)?;
            Ok(value.map(|v| crate::services::reading::kana_sort_key(&v)))
        },
    )?;
    Ok(())
}

const MIGRATION_001: &str = include_str!("../../../../packages/db/migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("../../../../packages/db/migrations/002_tmdb_fields.sql");
const MIGRATION_003: &str = include_str!("../../../../packages/db/migrations/003_series.sql");
const MIGRATION_004: &str = include_str!("../../../../packages/db/migrations/004_persons.sql");
const MIGRATION_005: &str = include_str!("../../../../packages/db/migrations/005_source_media_kind.sql");
const MIGRATION_LIBRARY_FIELDS: &str = include_str!("../../../../packages/db/migrations/005_library_management_fields.sql");
const MIGRATION_LEGACY_COMPAT: &str = include_str!("../../../../packages/db/migrations/006_legacy_schema_compat.sql");
const MIGRATION_AWARDS: &str = include_str!("../../../../packages/db/migrations/007_awards.sql");
const MIGRATION_AWARD_CATEGORY_MASTER: &str = include_str!("../../../../packages/db/migrations/008_award_category_master.sql");
const MIGRATION_AWARD_SCHEDULE_ALERTS: &str = include_str!("../../../../packages/db/migrations/009_award_schedule_alerts.sql");
const MIGRATION_AWARD_IMPORT_FOUNDATION: &str = include_str!("../../../../packages/db/migrations/010_award_import_foundation.sql");
const MIGRATION_AWARD_WIKIDATA_QID_CORRECTIONS: &str = include_str!("../../../../packages/db/migrations/011_award_wikidata_qid_corrections.sql");
const MIGRATION_AWARD_CANNES_QID_FIX: &str = include_str!("../../../../packages/db/migrations/012_award_cannes_qid_fix.sql");
const MIGRATION_AWARD_FESTIVAL_QID_FIXES: &str = include_str!("../../../../packages/db/migrations/013_award_festival_qid_fixes.sql");
const MIGRATION_AWARD_PERSON_CATEGORY_QIDS: &str = include_str!("../../../../packages/db/migrations/014_award_person_category_qids.sql");
const MIGRATION_ACADEMY_CATEGORY_QIDS: &str = include_str!("../../../../packages/db/migrations/016_academy_category_qids.sql");
const MIGRATION_AWARD_CATEGORY_QID_EXPANSION: &str = include_str!("../../../../packages/db/migrations/017_award_category_qid_expansion.sql");
const MIGRATION_SYNC_OUTBOX: &str = include_str!("../../../../packages/db/migrations/018_sync_outbox.sql");
const MIGRATION_BACKFILL_COUNTRY_MEDIA: &str = include_str!("../../../../packages/db/migrations/019_backfill_country_type_media_category.sql");
const MIGRATION_PREMATCH_INPUTS: &str = include_str!("../../../../packages/db/migrations/020_prematch_inputs.sql");

/// 020 の files.original_* は一度値が入ったら変更させない。
/// トリガー本体に ';' を含むため、';' 区切りで流す lenient migration ではなくここで作成する。
const PREMATCH_INPUTS_GUARD: &str = "
CREATE TRIGGER IF NOT EXISTS trg_files_original_immutable
BEFORE UPDATE OF original_file_name, original_rel_path, original_captured ON files
FOR EACH ROW
WHEN OLD.original_file_name IS NOT NULL
  AND (NEW.original_file_name IS NOT OLD.original_file_name
       OR NEW.original_rel_path IS NOT OLD.original_rel_path
       OR NEW.original_captured IS NOT OLD.original_captured)
BEGIN
  SELECT RAISE(ABORT, 'files.original_* is immutable once captured');
END;
";

/// マイグレーション適用（起動時に一度だけ呼ぶ）
pub fn init(path: &Path) -> Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    apply_migrations(&conn)
}

/// 全マイグレーションを適用する。起動ごとに再実行されても安全であること。
pub(crate) fn apply_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(MIGRATION_001)?;
    // 002-005: ALTER TABLE / CREATE TABLE が既存の場合はエラーを無視
    for migration in [MIGRATION_002, MIGRATION_003, MIGRATION_004, MIGRATION_005, MIGRATION_LIBRARY_FIELDS, MIGRATION_LEGACY_COMPAT] {
        for stmt in migration.split(';') {
            let trimmed = stmt.trim();
            if !trimmed.is_empty() {
                let _ = conn.execute_batch(trimmed);
            }
        }
    }
    conn.execute_batch(MIGRATION_AWARDS)?;
    conn.execute_batch(MIGRATION_AWARD_CATEGORY_MASTER)?;
    conn.execute_batch(MIGRATION_AWARD_SCHEDULE_ALERTS)?;
    apply_lenient_migration(&conn, MIGRATION_AWARD_IMPORT_FOUNDATION)?;
    conn.execute_batch(MIGRATION_AWARD_WIKIDATA_QID_CORRECTIONS)?;
    conn.execute_batch(MIGRATION_AWARD_CANNES_QID_FIX)?;
    conn.execute_batch(MIGRATION_AWARD_FESTIVAL_QID_FIXES)?;
    conn.execute_batch(MIGRATION_AWARD_PERSON_CATEGORY_QIDS)?;
    conn.execute_batch(MIGRATION_ACADEMY_CATEGORY_QIDS)?;
    conn.execute_batch(MIGRATION_AWARD_CATEGORY_QID_EXPANSION)?;
    conn.execute_batch(MIGRATION_SYNC_OUTBOX)?;
    conn.execute_batch(MIGRATION_BACKFILL_COUNTRY_MEDIA)?;
    apply_lenient_migration(&conn, MIGRATION_PREMATCH_INPUTS)?;
    conn.execute_batch(PREMATCH_INPUTS_GUARD)?;
    backfill_prematch_inputs(&conn)?;
    let mut stmt = conn.prepare("SELECT id, title FROM works WHERE reading IS NULL OR reading = ''")?;
    let works = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    for (work_id, title) in works {
        if let Some(reading) = crate::services::reading::infer_reading(&title) {
            conn.execute("UPDATE works SET reading = ?1 WHERE id = ?2", rusqlite::params![reading, work_id])?;
        }
    }
    Ok(())
}

fn apply_lenient_migration(conn: &Connection, migration: &str) -> Result<()> {
    for stmt in migration.split(';') {
        let trimmed = stmt.trim();
        if trimmed.is_empty() {
            continue;
        }
        match conn.execute_batch(trimmed) {
            Ok(_) => {}
            Err(err) if is_expected_idempotent_error(&err.to_string()) => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

fn is_expected_idempotent_error(message: &str) -> bool {
    message.contains("duplicate column name")
}

/// 020 以前から存在する files 行の original_* を現在値から後埋めする。
/// original_file_name IS NULL の行だけが対象で、original_captured = 0 を付ける。
/// source root の外にある（相対化できない）パスは original_rel_path を NULL のままにする。
pub(crate) fn backfill_prematch_inputs(conn: &Connection) -> Result<usize> {
    let rows: Vec<(i64, String, String, Option<String>)> = {
        let mut stmt = conn.prepare(
            "SELECT f.id, f.file_name, f.file_path, s.root_path
             FROM files f LEFT JOIN sources s ON s.id = f.source_id
             WHERE f.original_file_name IS NULL",
        )?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    let mut filled = 0;
    for (file_id, file_name, file_path, root_path) in rows {
        let rel_path = root_path.as_deref().and_then(|root| {
            crate::services::prematch_inputs::relative_to_root(root, &file_path)
        });
        filled += conn.execute(
            "UPDATE files
             SET original_file_name = ?1, original_rel_path = ?2, original_captured = 0
             WHERE id = ?3 AND original_file_name IS NULL",
            rusqlite::params![file_name, rel_path, file_id],
        )?;
    }
    Ok(filled)
}

#[cfg(test)]
pub(crate) mod test_support {
    use rusqlite::Connection;

    /// 本番と同じマイグレーションを適用したインメモリ DB
    pub fn open_migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        super::register_functions(&conn).unwrap();
        super::apply_migrations(&conn).expect("migrations");
        conn
    }

    pub fn insert_source(conn: &Connection, root_path: &str) -> i64 {
        conn.execute(
            "INSERT INTO sources (name, root_path, source_type) VALUES ('test', ?1, 'local')",
            rusqlite::params![root_path],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    pub fn insert_work(conn: &Connection, title: &str) -> i64 {
        conn.execute(
            "INSERT INTO works (work_type, title) VALUES ('movie', ?1)",
            rusqlite::params![title],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    pub fn set_match(conn: &Connection, work_id: i64, status: &str, tmdb_id: Option<i64>) {
        conn.execute(
            "UPDATE works SET match_status = ?2, tmdb_id = ?3,
                    tmdb_media_type = CASE WHEN ?3 IS NULL THEN NULL ELSE 'movie' END
             WHERE id = ?1",
            rusqlite::params![work_id, status, tmdb_id],
        )
        .unwrap();
    }

    pub fn match_state(conn: &Connection, work_id: i64) -> (String, Option<i64>) {
        conn.query_row(
            "SELECT match_status, tmdb_id FROM works WHERE id = ?1",
            rusqlite::params![work_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    fn insert_legacy_file(conn: &Connection, source_id: i64, file_path: &str, file_name: &str) -> i64 {
        conn.execute(
            "INSERT INTO files (source_id, file_path, file_name) VALUES (?1, ?2, ?3)",
            rusqlite::params![source_id, file_path, file_name],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn originals(conn: &Connection, file_id: i64) -> (Option<String>, Option<String>, i64) {
        conn.query_row(
            "SELECT original_file_name, original_rel_path, original_captured FROM files WHERE id = ?1",
            rusqlite::params![file_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
    }

    #[test]
    fn migrations_are_idempotent_on_restart() {
        let conn = open_migrated();
        apply_migrations(&conn).expect("second run");
        apply_migrations(&conn).expect("third run");
    }

    /// 既存 DB のコピーにマイグレーションを流して壊れないことを確かめる（手動実行用）。
    /// CINEMANTIS_DB_COPY に「コピーした」DB のパスを渡して `cargo test -- --ignored` で実行する。
    #[test]
    #[ignore]
    fn migrate_copy_of_existing_db() {
        let Ok(path) = std::env::var("CINEMANTIS_DB_COPY") else {
            return;
        };
        let count = |conn: &Connection, sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        let before = {
            let conn = Connection::open(&path).unwrap();
            (
                count(&conn, "SELECT COUNT(*) FROM works"),
                count(&conn, "SELECT COUNT(*) FROM files"),
                count(&conn, "SELECT COUNT(*) FROM works WHERE tmdb_id IS NOT NULL"),
            )
        };
        init(Path::new(&path)).expect("first startup");
        init(Path::new(&path)).expect("second startup");

        let conn = Connection::open(&path).unwrap();
        let after = (
            count(&conn, "SELECT COUNT(*) FROM works"),
            count(&conn, "SELECT COUNT(*) FROM files"),
            count(&conn, "SELECT COUNT(*) FROM works WHERE tmdb_id IS NOT NULL"),
        );
        assert_eq!(before, after, "行数と照合件数が変わらないこと");
        let unfilled = count(&conn, "SELECT COUNT(*) FROM files WHERE original_file_name IS NULL");
        let no_rel = count(&conn, "SELECT COUNT(*) FROM files WHERE original_rel_path IS NULL");
        let captured = count(&conn, "SELECT COUNT(*) FROM files WHERE original_captured = 1");
        let absolute = count(
            &conn,
            "SELECT COUNT(*) FROM files WHERE original_rel_path LIKE '_:%' OR original_rel_path LIKE '/%' OR original_rel_path LIKE '\\%'",
        );
        let integrity: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
        println!(
            "works={} files={} matched={} unfilled={unfilled} rel_path_null={no_rel} captured={captured} absolute={absolute} integrity={integrity}",
            after.0, after.1, after.2
        );
        assert_eq!(unfilled, 0);
        assert_eq!(captured, 0);
        assert_eq!(absolute, 0);
        assert_eq!(integrity, "ok");
    }

    #[test]
    fn backfill_marks_existing_rows_as_not_captured() {
        let conn = open_migrated();
        let source_id = insert_source(&conn, r"D:\Movies");
        let file_id = insert_legacy_file(&conn, source_id, r"D:\Movies\sub\Alien.1979.mkv", "Alien.1979.mkv");
        let outside = insert_legacy_file(&conn, source_id, r"E:\Other\x.mkv", "x.mkv");

        assert_eq!(backfill_prematch_inputs(&conn).unwrap(), 2);
        assert_eq!(
            originals(&conn, file_id),
            (Some("Alien.1979.mkv".into()), Some("sub/Alien.1979.mkv".into()), 0)
        );
        assert_eq!(originals(&conn, outside), (Some("x.mkv".into()), None, 0));
    }

    #[test]
    fn backfill_never_overwrites_captured_rows() {
        let conn = open_migrated();
        let source_id = insert_source(&conn, r"D:\Movies");
        conn.execute(
            "INSERT INTO files (source_id, file_path, file_name, original_file_name, original_rel_path, original_captured)
             VALUES (?1, 'D:\\Movies\\洋画\\あ\\エイリアン (1979).mkv', 'エイリアン (1979).mkv', 'Alien.1979.mkv', 'Alien.1979.mkv', 1)",
            rusqlite::params![source_id],
        )
        .unwrap();
        let file_id = conn.last_insert_rowid();

        assert_eq!(backfill_prematch_inputs(&conn).unwrap(), 0);
        apply_migrations(&conn).unwrap();
        assert_eq!(
            originals(&conn, file_id),
            (Some("Alien.1979.mkv".into()), Some("Alien.1979.mkv".into()), 1)
        );
    }

    #[test]
    fn captured_originals_cannot_be_updated() {
        let conn = open_migrated();
        let source_id = insert_source(&conn, r"D:\Movies");
        let file_id = insert_legacy_file(&conn, source_id, r"D:\Movies\a.mkv", "a.mkv");
        backfill_prematch_inputs(&conn).unwrap();

        let err = conn.execute(
            "UPDATE files SET original_file_name = 'changed.mkv' WHERE id = ?1",
            rusqlite::params![file_id],
        );
        assert!(err.is_err());
        // 他の列の更新や、同じ値の再代入は妨げない
        conn.execute(
            "UPDATE files SET file_name = 'b.mkv', original_captured = 0 WHERE id = ?1",
            rusqlite::params![file_id],
        )
        .unwrap();
        assert_eq!(originals(&conn, file_id).0.as_deref(), Some("a.mkv"));
    }
}
