//! startup.rs のテスト（REL-R1B）

use super::*;
use crate::migrate::{FailureKind, MigrationError};

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cinemantis_r1b_startup_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn migration_error(kind: FailureKind) -> MigrationError {
    MigrationError {
        kind,
        detail: "technical detail: SQLITE_CONSTRAINT near token=abc123".into(),
        backup_path: None,
        failed_step: Some("022_rebuild".into()),
        db_unchanged: true,
    }
}

#[test]
fn redact_hides_api_keys_tokens_and_long_hex() {
    let key = "0123456789abcdef0123456789abcdef";
    for input in [
        format!("request failed: api_key={key}&language=ja"),
        format!("GET https://api.themoviedb.org/3/movie/1?api_key={key}"),
        format!("{{\"tmdb_api_key\": \"{key}\"}}"),
        "Authorization: Bearer abcDEF123456.token-value".to_string(),
        "headers: authorization=Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.sig".to_string(),
        format!("raw key {key} leaked"),
        "password: hunter2hunter2".to_string(),
    ] {
        let out = redact(&input);
        assert!(!out.contains(key), "{out}");
        assert!(!out.contains("hunter2"), "{out}");
        assert!(!out.contains("abcDEF123456"), "{out}");
        assert!(!out.contains("eyJzdWIi"), "{out}");
        assert!(out.contains("[REDACTED]"), "{out}");
    }
    // 秘密でない文はそのまま
    let plain = "migration step '022_rebuild' failed: no such table: works";
    assert_eq!(redact(plain), plain);
}

#[test]
fn log_event_appends_redacted_lines_and_rotates() {
    let dir = tmp("log");
    log_event(&dir, "ERROR", "boom api_key=0123456789abcdef0123456789abcdef\nsecond line");
    log_event(&dir, "INFO", "ok");
    let text = std::fs::read_to_string(log_path(&dir)).unwrap();
    assert!(text.contains("[ERROR] boom api_key=[REDACTED]"));
    assert!(text.contains("[INFO] ok"));
    assert!(!text.contains("0123456789abcdef0123456789abcdef"));
    assert_eq!(text.lines().count(), 3);

    // 上限超えは .old へ回して新しく書き直す
    std::fs::write(log_path(&dir), vec![b'x'; (LOG_MAX_BYTES + 10) as usize]).unwrap();
    log_event(&dir, "INFO", "after rotate");
    assert!(dir.join("startup.log.old").exists());
    assert!(std::fs::read_to_string(log_path(&dir)).unwrap().contains("after rotate"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn log_event_never_panics_on_unwritable_dir() {
    log_event(Path::new("Z:/definitely/not/here/xyz"), "ERROR", "x");
}

#[test]
fn user_messages_are_actionable_and_contain_no_technical_detail_or_secrets() {
    let dir = tmp("msg");
    let backup = dir.join("backups").join("premigration_v0_to_v25_20260101_000000.db");
    for kind in [
        FailureKind::Corrupted,
        FailureKind::NewerSchema,
        FailureKind::BackupFailed,
        FailureKind::MigrationFailed,
        FailureKind::OpenFailed,
        FailureKind::UnknownSchema,
        FailureKind::InconsistentLegacySchema,
    ] {
        let mut e = migration_error(kind);
        e.backup_path = Some(backup.clone());
        let msg = user_message_for_migration(&e, &dir, None);
        assert!(msg.contains("startup.log"), "{kind:?}: log の場所を案内する");
        assert!(msg.contains(&backup.display().to_string()), "{kind:?}: 復元用 backup を案内する");
        assert!(msg.contains("cinemantis_restore_pending.db"), "{kind:?}: 復元手順を案内する");
        assert!(!msg.contains("SQLITE_CONSTRAINT"), "{kind:?}: 技術詳細は log にだけ出す");
        assert!(!msg.contains("token=abc123"));
        assert!(!msg.contains("022_rebuild"));
    }
    let e = migration_error(FailureKind::MigrationFailed);
    assert!(user_message_for_migration(&e, &dir, None).contains("取り消され"), "巻き戻し済みであることを伝える");
    // backup が無くても、見つかった最新の正常 backup を案内できる
    let m = user_message_for_migration(&migration_error(FailureKind::Corrupted), &dir, Some(&backup));
    assert!(m.contains("premigration_v0_to_v25"));
    let _ = std::fs::remove_dir_all(&dir);
}
