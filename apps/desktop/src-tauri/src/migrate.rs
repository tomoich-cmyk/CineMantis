//! 起動時 migration の fail-safe 層（REL-R1B）。
//!
//! 方針:
//!  - 既存 DB を書き換える前に、**検証済みの backup が完成していること**を必須にする。
//!  - migration 一式は 1 つの transaction（SAVEPOINT）で流す。途中で失敗したら全部巻き戻し、
//!    「一部成功・一部失敗」を DB に残さない。`user_version` も同じ transaction で刻むので、
//!    スキーマ版と実 DB の状態が食い違うことはない。
//!  - 失敗は握りつぶさず `MigrationError` で返す。呼び出し側（lib.rs）が log とダイアログに出す。
//!
//! 他の migration 機構（`PRAGMA user_version` 以外）には依存しない。
//! 本番 DB には触れない。ここにある関数はすべて、渡されたパス / 接続だけを対象にする。

use crate::db::{Step, LATEST_SCHEMA_VERSION};
use anyhow::{anyhow, Result};
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

// ─── 公開する型 ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MigrationReport {
    pub from_version: i64,
    pub to_version: i64,
    /// 適用前に作った検証済み backup（新規 DB・版が最新の DB では None）
    pub backup_path: Option<PathBuf>,
    /// 新規作成（既存テーブルなし）だったか
    pub fresh: bool,
    /// 開始版（この番号以上の migration だけを実行した）。fresh は 1、最新なら LATEST+1
    pub start_version: i64,
    /// 開始版を決めた根拠: "fresh" / "user_version" / "legacy_probe"
    pub start_basis: &'static str,
    /// 実行した段の名前（実行順）
    pub executed: Vec<String>,
    /// そのうち既存行を書き換える（data_changing）段
    pub data_steps_run: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// DB ファイルが壊れている（開けない / quick_check が ok でない）
    Corrupted,
    /// DB の user_version がこのビルドより新しい（ダウングレード）
    NewerSchema,
    /// 適用前 backup が作れない / 検証に通らない（DB は未変更）
    BackupFailed,
    /// migration の途中で失敗（transaction で巻き戻し済み）
    MigrationFailed,
    /// DB を開けない等の入出力エラー
    OpenFailed,
    /// user_version=0 でテーブルはあるが CineMantis の schema と認識できない
    UnknownSchema,
    /// user_version=0 の既存 DB で migration の証拠が非連続（途中が欠けて後ろがある）。修復せず停止する
    InconsistentLegacySchema,
}

#[derive(Debug)]
pub struct MigrationError {
    pub kind: FailureKind,
    /// 技術的な詳細（log 用。秘密値は含めない）
    pub detail: String,
    pub backup_path: Option<PathBuf>,
    pub failed_step: Option<String>,
    /// DB は失敗前の状態のまま（巻き戻し済み / 未着手）であることを確認できたか
    pub db_unchanged: bool,
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for MigrationError {}

/// どの段で失敗したかを anyhow 経由で運ぶ
#[derive(Debug)]
pub(crate) struct StepError {
    pub step: String,
    pub message: String,
}

impl std::fmt::Display for StepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "migration step '{}' failed: {}", self.step, self.message)
    }
}

impl std::error::Error for StepError {}

fn err(kind: FailureKind, detail: impl Into<String>) -> MigrationError {
    MigrationError { kind, detail: detail.into(), backup_path: None, failed_step: None, db_unchanged: true }
}

// ─── atomic 実行 ──────────────────────────────────────────────────────────────

/// 本番で必須のテーブル / 列。migration 完了後、`user_version` を刻む前に確認する。
/// 「版は最新なのに実体が足りない」状態を作らないための最終関門。
const REQUIRED_TABLES: &[&str] = &[
    "sources", "files", "works", "work_parts", "user_stats", "tags", "work_tags", "persons",
    "work_persons", "series", "series_items", "app_settings", "award_bodies", "award_categories",
    "sync_outbox", "metadata_match_runs", "metadata_review_tasks", "metadata_match_verdicts",
    "metadata_match_candidates", "metadata_match_candidate_scores", "metadata_match_jev_calls",
    "jev_contracts", "jev_eval_sessions",
];

const REQUIRED_COLUMNS: &[(&str, &str)] = &[
    ("works", "reading"),
    ("works", "media_category"),
    ("works", "match_source"),
    ("files", "original_file_name"),
    ("files", "container_tags_json"),
    ("metadata_match_jev_calls", "eval_session_id"),
    ("metadata_match_jev_calls", "request_id"),
];

/// 全 step を 1 つの SAVEPOINT で流す。`stamp` が Some のときは、検証に通ったあとで
/// 同じ transaction 内で `user_version` を刻んでから確定する。
///
/// 失敗（step のエラー・検証失敗・外部キー違反の増加）したら **全部巻き戻して** Err を返す。
/// foreign_keys はテーブル作り直しのため transaction の外で一時的に OFF にし、必ず元へ戻す。
pub(crate) fn run_steps_atomically(conn: &Connection, steps: &[Step], stamp: Option<i64>) -> Result<()> {
    let fk_was_on = conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))? == 1;
    let outer_tx = !conn.is_autocommit();
    if fk_was_on && !outer_tx {
        conn.execute_batch("PRAGMA foreign_keys=OFF")?;
    }

    let result = (|| -> Result<()> {
        let fk_before = crate::db::foreign_key_check_rows(conn)?;
        conn.execute_batch("SAVEPOINT cm_migrate")?;
        let inner = (|| -> Result<()> {
            for step in steps {
                (step.run)(conn).map_err(|e| {
                    anyhow::Error::new(StepError { step: step.name.to_string(), message: format!("{e:#}") })
                })?;
            }
            if stamp.is_some() {
                verify_schema(conn).map_err(|e| {
                    anyhow::Error::new(StepError { step: "verify_schema".into(), message: format!("{e:#}") })
                })?;
            }
            let fk_after = crate::db::foreign_key_check_rows(conn)?;
            let added: Vec<_> = fk_after.iter().filter(|row| !fk_before.contains(row)).collect();
            if !added.is_empty() {
                return Err(anyhow::Error::new(StepError {
                    step: "foreign_key_check".into(),
                    message: format!("migration が新しい外部キー違反を作りました（{} 件）", added.len()),
                }));
            }
            if let Some(version) = stamp {
                conn.execute_batch(&format!("PRAGMA user_version = {version}"))?;
            }
            Ok(())
        })();
        match inner {
            Ok(()) => {
                conn.execute_batch("RELEASE cm_migrate")?;
                Ok(())
            }
            Err(e) => {
                // 巻き戻せなかった場合も元のエラーを優先して返す
                let _ = conn.execute_batch("ROLLBACK TO cm_migrate; RELEASE cm_migrate");
                Err(e)
            }
        }
    })();

    if fk_was_on && !outer_tx {
        conn.execute_batch("PRAGMA foreign_keys=ON")?;
    }
    result
}

/// 必須テーブル・列・作り直したテーブルの CHECK 目印が揃っていること
pub(crate) fn verify_schema(conn: &Connection) -> Result<()> {
    for table in REQUIRED_TABLES {
        if crate::db::table_sql(conn, table)?.is_none() {
            return Err(anyhow!("必須テーブルがありません: {table}"));
        }
    }
    for (table, column) in REQUIRED_COLUMNS {
        let found: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            rusqlite::params![table, column],
            |r| r.get(0),
        )?;
        if found == 0 {
            return Err(anyhow!("必須の列がありません: {table}.{column}"));
        }
    }
    for (table, marker) in [
        ("metadata_review_tasks", crate::db::REVIEW_TASKS_MARKER),
        ("metadata_match_verdicts", crate::db::VERDICTS_MARKER),
    ] {
        let sql = crate::db::table_sql(conn, table)?.unwrap_or_default();
        if !sql.contains(marker) {
            return Err(anyhow!("{table} の CHECK 制約が最新ではありません（'{marker}' がありません）"));
        }
    }
    Ok(())
}

// ─── ファイル DB の migration ────────────────────────────────────────────────

pub fn backups_dir_for(db_path: &Path) -> PathBuf {
    db_path.parent().map(|p| p.join("backups")).unwrap_or_else(|| PathBuf::from("backups"))
}

/// 起動時の入口。事前検査 → backup → atomic 適用。
pub fn migrate_database(db_path: &Path) -> std::result::Result<MigrationReport, MigrationError> {
    migrate_database_with(db_path, &backups_dir_for(db_path), &crate::db::migration_steps(true))
}

pub(crate) fn migrate_database_with(
    db_path: &Path,
    backups_dir: &Path,
    steps: &[Step],
) -> std::result::Result<MigrationReport, MigrationError> {
    let existed = std::fs::metadata(db_path).map(|m| m.len() > 0).unwrap_or(false);
    let conn = Connection::open(db_path)
        .map_err(|e| err(FailureKind::OpenFailed, format!("DB を開けません: {e}")))?;
    let _ = conn.busy_timeout(Duration::from_secs(5));

    if existed {
        preflight_check(&conn)?;
    }
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
        .map_err(|e| err(FailureKind::OpenFailed, format!("PRAGMA 設定に失敗: {e}")))?;

    let from_version = read_user_version(&conn)
        .map_err(|e| err(FailureKind::Corrupted, format!("user_version を読めません: {e}")))?;
    if from_version > LATEST_SCHEMA_VERSION {
        return Err(err(
            FailureKind::NewerSchema,
            format!("DB のスキーマ版 {from_version} は、このアプリが対応する版 {LATEST_SCHEMA_VERSION} より新しい"),
        ));
    }

    let has_tables = user_table_count(&conn)
        .map_err(|e| err(FailureKind::Corrupted, format!("sqlite_master を読めません: {e}")))?
        > 0;
    let fresh = !has_tables;

    // 開始版の決定（明示的な契約）:
    //  - user_version > 0: その次の番号から（版を信頼する。実体の不足は最後の verify_schema が検出して失敗にする）
    //  - user_version = 0 かつテーブル無し: fresh。001 から全部
    //  - user_version = 0 かつテーブル有り（版を使っていなかった既存 DB）: migration ごとの「適用済みの証拠」
    //    （表・列・トリガー）を 001 から順に確かめ、連続して揃っている最後の番号の次から。
    //    CineMantis と認識できない DB（001 の証拠が無い）は何も触らず拒否する。
    let (start_version, start_basis) = if from_version > 0 {
        (from_version + 1, "user_version")
    } else if fresh {
        (1, "fresh")
    } else if crate::db::legacy_probe(&conn, 1) != Some(true) {
        return Err(err(
            FailureKind::UnknownSchema,
            "user_version=0 のテーブルがあるが、CineMantis の基本テーブルが揃っていないため扱えません",
        ));
    } else {
        match crate::db::legacy_start_version(&conn) {
            Ok(v) => (v, "legacy_probe"),
            // fail-closed: migration SQL を 1 本も流さず、版・データ・DB 本体・backup も作らずに止める
            Err(detail) => return Err(err(FailureKind::InconsistentLegacySchema, detail)),
        }
    };
    let to_run: Vec<Step> = steps.iter().filter(|s| s.version == 0 || s.version >= start_version).copied().collect();

    // 既存データがあり、これから版が上がる（または版未記録の既存 DB）なら、backup が先。
    let mut backup_path = None;
    if has_tables && from_version < LATEST_SCHEMA_VERSION {
        match create_verified_backup(&conn, backups_dir, from_version, LATEST_SCHEMA_VERSION) {
            Ok(path) => backup_path = Some(path),
            Err(e) => {
                return Err(err(
                    FailureKind::BackupFailed,
                    format!("適用前 backup を作れなかったため migration を開始しません: {e:#}"),
                ));
            }
        }
    }

    match run_steps_atomically(&conn, &to_run, Some(LATEST_SCHEMA_VERSION)) {
        Ok(()) => Ok(MigrationReport {
            from_version,
            to_version: LATEST_SCHEMA_VERSION,
            backup_path,
            fresh,
            start_version,
            start_basis,
            executed: to_run.iter().map(|s| s.name.to_string()).collect(),
            data_steps_run: to_run.iter().filter(|s| s.data_changing).map(|s| s.name.to_string()).collect(),
        }),
        Err(e) => {
            let failed_step = e.downcast_ref::<StepError>().map(|s| s.step.clone());
            // 巻き戻しの確認: 版が変わっていないこと
            let unchanged = read_user_version(&conn).map(|v| v == from_version).unwrap_or(false);
            Err(MigrationError {
                kind: FailureKind::MigrationFailed,
                detail: format!("{e:#}"),
                backup_path,
                failed_step,
                db_unchanged: unchanged,
            })
        }
    }
}

fn preflight_check(conn: &Connection) -> std::result::Result<(), MigrationError> {
    match conn.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0)) {
        Ok(v) if v == "ok" => Ok(()),
        Ok(v) => Err(err(FailureKind::Corrupted, format!("quick_check が ok ではありません: {v}"))),
        Err(e) => Err(err(FailureKind::Corrupted, format!("DB を検査できません（破損の可能性）: {e}"))),
    }
}

fn read_user_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

fn user_tables(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let names = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(names)
}

fn user_table_count(conn: &Connection) -> Result<usize> {
    Ok(user_tables(conn)?.len())
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn table_row_counts(conn: &Connection) -> Result<BTreeMap<String, i64>> {
    let mut counts = BTreeMap::new();
    for table in user_tables(conn)? {
        let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {}", quote_ident(&table)), [], |r| r.get(0))?;
        counts.insert(table, n);
    }
    Ok(counts)
}

// ─── backup ───────────────────────────────────────────────────────────────────

/// 適用前 backup を作って検証し、検証に通ってから最終名へ rename する。
/// 途中のファイルは `.partial` で、失敗時は消す。最終名のファイルは「検証済み」のみ。
pub(crate) fn create_verified_backup(
    conn: &Connection,
    backups_dir: &Path,
    from_version: i64,
    to_version: i64,
) -> Result<PathBuf> {
    std::fs::create_dir_all(backups_dir)
        .map_err(|e| anyhow!("backup フォルダを作れません ({}): {e}", backups_dir.display()))?;

    let stamp = crate::startup::now_compact();
    let base = format!("premigration_v{from_version}_to_v{to_version}_{stamp}");
    let mut final_path = backups_dir.join(format!("{base}.db"));
    let mut n = 2;
    while final_path.exists() {
        final_path = backups_dir.join(format!("{base}_{n}.db"));
        n += 1;
    }
    let partial = PathBuf::from(format!("{}.partial", final_path.display()));
    let _ = std::fs::remove_file(&partial);

    let build = || -> Result<()> {
        conn.execute(&format!("VACUUM INTO '{}'", partial.to_string_lossy().replace('\'', "''")), [])?;
        let expected = table_row_counts(conn)?;
        verify_backup_file(&partial, Some((&expected, from_version)))?;
        Ok(())
    };
    if let Err(e) = build() {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&partial, &final_path) {
        let _ = std::fs::remove_file(&partial);
        return Err(anyhow!("backup の確定（rename）に失敗: {e}"));
    }
    Ok(final_path)
}

/// backup ファイルが復旧に使えることを確認する。
/// - integrity_check が ok
/// - user_version がこのビルドの対応範囲内
/// - `expected` があれば、全テーブルの行数と版が元 DB と一致
pub(crate) fn verify_backup_file(path: &Path, expected: Option<(&BTreeMap<String, i64>, i64)>) -> Result<()> {
    let meta = std::fs::metadata(path).map_err(|e| anyhow!("backup が読めません: {e}"))?;
    if meta.len() == 0 {
        return Err(anyhow!("backup が空です"));
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| anyhow!("backup を開けません: {e}"))?;
    let check: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|e| anyhow!("backup の integrity_check に失敗: {e}"))?;
    if check != "ok" {
        return Err(anyhow!("backup の integrity_check が ok ではありません: {check}"));
    }
    let version = read_user_version(&conn)?;
    if version > LATEST_SCHEMA_VERSION {
        return Err(anyhow!("backup のスキーマ版 {version} は対応範囲外です"));
    }
    if let Some((counts, from_version)) = expected {
        if version != from_version {
            return Err(anyhow!("backup の user_version が元と一致しません（{version} != {from_version}）"));
        }
        let actual = table_row_counts(&conn)?;
        if &actual != counts {
            return Err(anyhow!("backup のテーブル構成 / 行数が元の DB と一致しません"));
        }
    }
    Ok(())
}

/// backups フォルダ内で、検証に通る最も新しい backup
pub fn find_latest_valid_backup(backups_dir: &Path) -> Option<PathBuf> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(backups_dir)
        .ok()?
        .filter_map(|e| {
            let p = e.ok()?.path();
            if p.extension()?.to_str()? != "db" {
                return None;
            }
            Some((std::fs::metadata(&p).ok()?.modified().ok()?, p))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.into_iter().map(|(_, p)| p).find(|p| verify_backup_file(p, None).is_ok())
}

#[cfg(test)]
#[path = "migrate_tests.rs"]
mod tests;
