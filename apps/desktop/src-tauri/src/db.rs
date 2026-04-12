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

/// マイグレーション適用（起動時に一度だけ呼ぶ）
pub fn init(path: &Path) -> Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(MIGRATION_001)?;
    // 002: ALTER TABLE は "already exists" で失敗するため個別に試行
    for stmt in MIGRATION_002.split(';') {
        let trimmed = stmt.trim();
        if !trimmed.is_empty() {
            // duplicate column / table already exists は無視
            let _ = conn.execute_batch(trimmed);
        }
    }
    Ok(())
}
