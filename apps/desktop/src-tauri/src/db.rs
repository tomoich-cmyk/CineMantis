use anyhow::Result;
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, OptionalExtension};
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
const MIGRATION_MATCH_HISTORY: &str = include_str!("../../../../packages/db/migrations/021_match_history.sql");
const MIGRATION_CONTAINER_TAGS: &str = include_str!("../../../../packages/db/migrations/022_container_tags.sql");

/// 022 で CHECK 制約を広げるテーブル。SQLite は CHECK を後から変えられないので作り直す。
/// 毎起動で作り直さないよう、CHECK に目印の値が無いときだけ実行する。
const REVIEW_TASKS_MARKER: &str = "metadata_conflict";
const REVIEW_TASKS_NEW_SQL: &str = "
CREATE TABLE metadata_review_tasks_new (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  work_id INTEGER NOT NULL REFERENCES works(id) ON DELETE CASCADE,
  run_id INTEGER REFERENCES metadata_match_runs(id) ON DELETE SET NULL,
  reason TEXT NOT NULL CHECK (reason IN (
    'review_decision','legacy_pending','audit_sample','matcher_disagreement',
    'user_initiated','metadata_conflict')),
  sampling_json TEXT,
  details_json TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  resolved_at TEXT,
  resolution_label_id INTEGER,
  CHECK (reason <> 'audit_sample' OR sampling_json IS NOT NULL)
)";
const REVIEW_TASKS_COLUMNS: &str =
    "id, work_id, run_id, reason, sampling_json, created_at, resolved_at, resolution_label_id";
const REVIEW_TASKS_INDEX: &str =
    "CREATE UNIQUE INDEX IF NOT EXISTS ux_mrt_open ON metadata_review_tasks(work_id, reason) WHERE resolved_at IS NULL";

const VERDICTS_MARKER: &str = "rules-tags-shadow";
const VERDICTS_NEW_SQL: &str = "
CREATE TABLE metadata_match_verdicts_new (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id INTEGER NOT NULL REFERENCES metadata_match_runs(id) ON DELETE CASCADE,
  matcher TEXT NOT NULL CHECK (matcher IN (
    'rules-1','rules-safe','rules-tags-shadow','jev-policy','jev-choice-shuffled','jev-isolated')),
  matcher_version TEXT NOT NULL,
  tmdb_id INTEGER,
  media_type TEXT,
  decision TEXT NOT NULL CHECK (decision IN ('AUTO','REVIEW','UNRESOLVED')),
  score REAL,
  reasons_json TEXT,
  CHECK ((tmdb_id IS NULL AND media_type IS NULL) OR (tmdb_id IS NOT NULL AND media_type IN ('movie','tv'))),
  UNIQUE (run_id, matcher)
)";
const VERDICTS_COLUMNS: &str =
    "id, run_id, matcher, matcher_version, tmdb_id, media_type, decision, score, reasons_json";

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
    apply_lenient_migration(&conn, MIGRATION_MATCH_HISTORY)?;
    backfill_match_source(&conn)?;
    apply_lenient_migration(&conn, MIGRATION_CONTAINER_TAGS)?;
    rebuild_review_tasks_if_needed(&conn)?;
    rebuild_verdicts_if_needed(&conn)?;
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

/// sqlite_master に記録された CREATE 文（テーブルが無ければ None）
fn table_sql(conn: &Connection, table: &str) -> Result<Option<String>> {
    let sql = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
            rusqlite::params![table],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    Ok(sql)
}

/// CHECK 制約を広げるためにテーブルを作り直す。
///
/// 目印（`marker`）が既に CHECK に入っていれば何もしない（起動のたびに作り直さない）。
/// 手順は次の順で行う。BEGIN より前に foreign_keys を切るのが要点。
///   1. foreign_keys の現在値を読む → 2. OFF → 3. BEGIN → 4. 新テーブル作成 →
///   5. コピー → 6. 旧テーブル DROP → 7. RENAME → 8. インデックス再作成 →
///   9. COMMIT → 10. foreign_keys を元に戻す → 11. foreign_key_check
fn rebuild_table_for_check(
    conn: &Connection,
    table: &str,
    marker: &str,
    create_new_sql: &str,
    columns: &str,
    indexes: &[&str],
    verify_tables: &[&str],
) -> Result<bool> {
    let Some(existing) = table_sql(conn, table)? else {
        return Ok(false); // まだ 021 を通していない DB
    };
    if existing.contains(marker) {
        return Ok(false);
    }

    let foreign_keys_on: bool = conn.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))? == 1;
    conn.execute_batch("PRAGMA foreign_keys=OFF")?;

    let rebuild = || -> Result<()> {
        conn.execute_batch("BEGIN")?;
        conn.execute_batch(create_new_sql)?;
        conn.execute_batch(&format!(
            "INSERT INTO {table}_new ({columns}) SELECT {columns} FROM {table}"
        ))?;
        conn.execute_batch(&format!("DROP TABLE {table}"))?;
        conn.execute_batch(&format!("ALTER TABLE {table}_new RENAME TO {table}"))?;
        for index in indexes {
            conn.execute_batch(index)?;
        }
        conn.execute_batch("COMMIT")?;
        Ok(())
    };

    let result = rebuild();
    if result.is_err() {
        let _ = conn.execute_batch("ROLLBACK");
        let _ = conn.execute_batch(&format!("DROP TABLE IF EXISTS {table}_new"));
    }
    if foreign_keys_on {
        conn.execute_batch("PRAGMA foreign_keys=ON")?;
    }
    result?;

    // 参照が壊れていないことを確かめる。
    // DB 全体ではなく作り直したテーブルと、それを参照するテーブルだけを見る
    // （awards など別機能に元からある不整合を、ここで失敗させないため）
    for target in std::iter::once(table).chain(verify_tables.iter().copied()) {
        let broken = foreign_key_violations(conn, target)?;
        if broken > 0 {
            anyhow::bail!("{table} の作り直しで {target} の外部キーが壊れました（{broken} 件）");
        }
    }
    Ok(true)
}

/// 指定したテーブルの外部キー違反の件数
pub(crate) fn foreign_key_violations(conn: &Connection, table: &str) -> Result<usize> {
    let mut stmt = conn.prepare(&format!("PRAGMA foreign_key_check({table})"))?;
    let rows = stmt.query_map([], |_| Ok(()))?;
    Ok(rows.count())
}

/// DB 全体の外部キー違反（テーブル名, 子の rowid, 参照先, 外部キー番号）。
/// migration の前後で比べ、「新しい違反を増やしていない」ことを確かめるのに使う。
pub(crate) fn foreign_key_check_rows(conn: &Connection) -> Result<Vec<(String, Option<i64>, String, i64)>> {
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 022: metadata_review_tasks に metadata_conflict と details_json を足す
pub(crate) fn rebuild_review_tasks_if_needed(conn: &Connection) -> Result<bool> {
    rebuild_table_for_check(
        conn,
        "metadata_review_tasks",
        REVIEW_TASKS_MARKER,
        REVIEW_TASKS_NEW_SQL,
        REVIEW_TASKS_COLUMNS,
        &[REVIEW_TASKS_INDEX],
        &["metadata_match_labels"],
    )
}

/// 022: metadata_match_verdicts に rules-tags-shadow を足す
pub(crate) fn rebuild_verdicts_if_needed(conn: &Connection) -> Result<bool> {
    rebuild_table_for_check(
        conn,
        "metadata_match_verdicts",
        VERDICTS_MARKER,
        VERDICTS_NEW_SQL,
        VERDICTS_COLUMNS,
        &[],
        &[],
    )
}

/// 021 以前に照合された作品の出どころを `legacy` として印付ける。
/// 021 以降に照合した作品は match_source が入っているので触らない。
pub(crate) fn backfill_match_source(conn: &Connection) -> Result<usize> {
    let filled = conn.execute(
        "UPDATE works SET match_source = 'legacy'
         WHERE match_source IS NULL AND tmdb_id IS NOT NULL",
        [],
    )?;
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

    /// 022 のテーブル作り直しは、必要なときだけ・何度流しても同じ結果になること
    #[test]
    fn container_tags_migration_is_conditional_and_idempotent() {
        let conn = open_migrated();
        // open_migrated で1回目は済んでいる（021 の CHECK に無い値が使えるようになっている）
        assert!(!rebuild_review_tasks_if_needed(&conn).unwrap(), "2回目は作り直さない");
        assert!(!rebuild_verdicts_if_needed(&conn).unwrap(), "2回目は作り直さない");

        let work_id = insert_work(&conn, "A");
        conn.execute(
            "INSERT INTO metadata_review_tasks (work_id, reason, details_json)
             VALUES (?1, 'metadata_conflict', '{\"kind\":\"year_mismatch\"}')",
            rusqlite::params![work_id],
        )
        .unwrap();
        // 作り直したテーブルでも部分 UNIQUE インデックスが効いている
        assert!(conn
            .execute(
                "INSERT INTO metadata_review_tasks (work_id, reason) VALUES (?1, 'metadata_conflict')",
                rusqlite::params![work_id],
            )
            .is_err());

        // マイグレーションを再実行しても行が消えない・壊れない
        apply_migrations(&conn).unwrap();
        apply_migrations(&conn).unwrap();
        let tasks: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_review_tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tasks, 1);
        let broken = {
            let mut stmt = conn.prepare("PRAGMA foreign_key_check").unwrap();
            let rows = stmt.query_map([], |_| Ok(())).unwrap();
            rows.count()
        };
        assert_eq!(broken, 0);
        // 外部キーの設定は元に戻っている
        let foreign_keys: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
        assert_eq!(foreign_keys, 1);
    }

    /// 021 まで適用した状態の DB（022 の作り直しを試すため）
    fn open_021_database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        register_functions(&conn).unwrap();
        conn.execute_batch(MIGRATION_001).unwrap();
        for migration in [MIGRATION_002, MIGRATION_003, MIGRATION_004, MIGRATION_005, MIGRATION_LIBRARY_FIELDS, MIGRATION_LEGACY_COMPAT] {
            for stmt in migration.split(';') {
                let trimmed = stmt.trim();
                if !trimmed.is_empty() {
                    let _ = conn.execute_batch(trimmed);
                }
            }
        }
        apply_lenient_migration(&conn, MIGRATION_MATCH_HISTORY).unwrap();
        conn
    }

    /// 022 の作り直しが「新しい外部キー違反を増やさない」こと。
    /// 既存の違反（実 DB では awards 由来が6件ある）は残ってよいが、増えてはいけない。
    #[test]
    fn rebuild_does_not_add_foreign_key_violations() {
        let conn = open_021_database();
        let work_id = insert_work(&conn, "A");
        conn.execute(
            "INSERT INTO metadata_review_tasks (work_id, reason) VALUES (?1, 'review_decision')",
            rusqlite::params![work_id],
        )
        .unwrap();

        // 既存 DB にある「親のいない行」を再現する（外部キーを切って入れる）
        conn.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        conn.execute(
            "INSERT INTO work_parts (work_id, file_id, part_no) VALUES (?1, 999999, 1)",
            rusqlite::params![work_id],
        )
        .unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();

        let before = foreign_key_check_rows(&conn).unwrap();
        assert!(!before.is_empty(), "既存の違反を作れていること");

        assert!(rebuild_review_tasks_if_needed(&conn).unwrap());
        assert!(rebuild_verdicts_if_needed(&conn).unwrap());

        let after = foreign_key_check_rows(&conn).unwrap();
        let added: Vec<_> = after.iter().filter(|row| !before.contains(row)).collect();
        assert!(added.is_empty(), "増えた違反: {added:?}");
        assert_eq!(after.len(), before.len(), "post - pre = 0");
    }

    /// 021 のままの DB（CHECK が古い）に 022 を流すと作り直しが起きる
    #[test]
    fn rebuild_runs_once_on_a_021_database() {
        let conn = open_021_database();

        let work_id = insert_work(&conn, "A");
        conn.execute(
            "INSERT INTO metadata_review_tasks (work_id, reason) VALUES (?1, 'review_decision')",
            rusqlite::params![work_id],
        )
        .unwrap();
        // 古い CHECK では metadata_conflict を入れられない
        assert!(conn
            .execute(
                "INSERT INTO metadata_review_tasks (work_id, reason) VALUES (?1, 'metadata_conflict')",
                rusqlite::params![work_id],
            )
            .is_err());

        assert!(rebuild_review_tasks_if_needed(&conn).unwrap(), "1回目は作り直す");
        assert!(rebuild_verdicts_if_needed(&conn).unwrap());
        assert!(!rebuild_review_tasks_if_needed(&conn).unwrap(), "2回目は何もしない");

        // 既存行は残り、新しい値が使える
        let existing: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_review_tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(existing, 1);
        conn.execute(
            "INSERT INTO metadata_review_tasks (work_id, reason) VALUES (?1, 'metadata_conflict')",
            rusqlite::params![work_id],
        )
        .unwrap();
    }

    /// 021 より前からある照合は legacy として印を付け、その後は上書きしない
    #[test]
    fn match_source_backfill_only_touches_existing_matches() {
        let conn = open_migrated();
        let matched = insert_work(&conn, "A");
        set_match(&conn, matched, "matched", Some(10));
        let unmatched = insert_work(&conn, "B");
        let auto = insert_work(&conn, "C");
        set_match(&conn, auto, "matched", Some(20));
        conn.execute(
            "UPDATE works SET match_source = 'rules_safe_auto' WHERE id = ?1",
            rusqlite::params![auto],
        )
        .unwrap();

        assert_eq!(backfill_match_source(&conn).unwrap(), 1);
        assert_eq!(backfill_match_source(&conn).unwrap(), 0, "2回目は何もしない");

        let source = |id: i64| -> Option<String> {
            conn.query_row(
                "SELECT match_source FROM works WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(source(matched).as_deref(), Some("legacy"));
        assert_eq!(source(unmatched), None);
        assert_eq!(source(auto).as_deref(), Some("rules_safe_auto"));
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
        // migration 前の外部キー違反（awards 由来の既存分がある）
        let (before, violations_before) = {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
            let violations = foreign_key_check_rows(&conn).unwrap();
            (
                (
                    count(&conn, "SELECT COUNT(*) FROM works"),
                    count(&conn, "SELECT COUNT(*) FROM files"),
                    count(&conn, "SELECT COUNT(*) FROM works WHERE tmdb_id IS NOT NULL"),
                ),
                violations,
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
        // 021: 履歴テーブルと match_source の後埋め
        let history_tables = count(
            &conn,
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN
               ('metadata_match_runs','metadata_match_candidates','metadata_match_verdicts',
                'metadata_review_tasks','metadata_match_labels','metadata_match_rejections')",
        );
        let legacy = count(&conn, "SELECT COUNT(*) FROM works WHERE match_source = 'legacy'");
        // 022: 列の追加と、CHECK を広げるための作り直し
        let tag_columns = count(
            &conn,
            "SELECT COUNT(*) FROM pragma_table_info('files')
             WHERE name IN ('container_tags_json','tags_captured','tags_captured_at',
                            'tags_encoder','tags_provenance','tags_provider_hint','tags_truncated')",
        );
        let query_source_column = count(
            &conn,
            "SELECT COUNT(*) FROM pragma_table_info('metadata_match_candidates') WHERE name = 'query_source'",
        );
        let details_column = count(
            &conn,
            "SELECT COUNT(*) FROM pragma_table_info('metadata_review_tasks') WHERE name = 'details_json'",
        );
        let new_checks = count(
            &conn,
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table'
               AND ((name = 'metadata_review_tasks' AND sql LIKE '%metadata_conflict%')
                 OR (name = 'metadata_match_verdicts' AND sql LIKE '%rules-tags-shadow%'))",
        );
        let open_task_index = count(
            &conn,
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'ux_mrt_open'",
        );
        // 022 で触るテーブルだけを見る（awards には 021 以前からの不整合が残っている）
        let broken_foreign_keys: i64 = [
            "metadata_review_tasks",
            "metadata_match_labels",
            "metadata_match_verdicts",
            "metadata_match_candidates",
            "metadata_match_runs",
            "files",
        ]
        .iter()
        .map(|table| foreign_key_violations(&conn, table).unwrap() as i64)
        .sum();
        // 022 が新しい外部キー違反を増やしていないこと（既存分は許容するが、増えたら失敗）
        let violations_after = foreign_key_check_rows(&conn).unwrap();
        let added: Vec<_> = violations_after
            .iter()
            .filter(|row| !violations_before.contains(row))
            .collect();
        println!(
            "fk_violations before={} after={} added={} detail_before={:?}",
            violations_before.len(),
            violations_after.len(),
            added.len(),
            violations_before
        );
        assert!(added.is_empty(), "022 が増やした外部キー違反: {added:?}");
        assert_eq!(
            violations_after.len() as i64 - violations_before.len() as i64,
            0,
            "post - pre = 0 であること"
        );
        let source_without_match = count(
            &conn,
            "SELECT COUNT(*) FROM works WHERE match_source IS NOT NULL AND tmdb_id IS NULL",
        );
        let matched_without_source = count(
            &conn,
            "SELECT COUNT(*) FROM works WHERE tmdb_id IS NOT NULL AND match_source IS NULL",
        );
        println!(
            "works={} files={} matched={} unfilled={unfilled} rel_path_null={no_rel} captured={captured} absolute={absolute} history_tables={history_tables} legacy={legacy} tag_columns={tag_columns} query_source={query_source_column} details={details_column} new_checks={new_checks} open_task_index={open_task_index} broken_fk={broken_foreign_keys} integrity={integrity}",
            after.0, after.1, after.2
        );
        assert_eq!(tag_columns, 7);
        assert_eq!(query_source_column, 1);
        assert_eq!(details_column, 1);
        assert_eq!(new_checks, 2);
        assert_eq!(open_task_index, 1);
        assert_eq!(broken_foreign_keys, 0);
        assert_eq!(unfilled, 0);
        // captured = 1 は 020 以降にスキャンした行。後埋めした行が混ざっていてよい
        assert!(captured <= after.1);
        assert_eq!(absolute, 0);
        assert_eq!(history_tables, 6);
        // 021 より前からの照合は legacy、それ以降の照合は manual / rules_safe_auto になる
        assert_eq!(matched_without_source, 0, "照合済みの作品には必ず出どころが付く");
        assert!(legacy <= after.2);
        assert_eq!(source_without_match, 0);
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
