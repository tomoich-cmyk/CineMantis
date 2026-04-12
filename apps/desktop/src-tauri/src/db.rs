use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

pub struct DbState(pub Mutex<Connection>);

impl DbState {
    pub fn new(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Ok(Self(Mutex::new(conn)))
    }
}

const MIGRATION_001: &str = include_str!("../../../../packages/db/migrations/001_initial.sql");

pub fn init(path: &Path) -> Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(MIGRATION_001)?;
    Ok(())
}
