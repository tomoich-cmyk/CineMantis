use anyhow::Result;
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
        Ok(Self(Arc::new(Mutex::new(conn))))
    }
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

/// マイグレーション適用（起動時に一度だけ呼ぶ）
pub fn init(path: &Path) -> Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
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
