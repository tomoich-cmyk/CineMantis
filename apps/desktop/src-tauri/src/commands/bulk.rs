use crate::commands::tmdb::{CLEAR_MATCH_ASSIGNMENTS, UNLOCKED_STATUS_SQL};
use crate::db::DbState;
use crate::models::match_status::MatchStatus;
use crate::services::match_history::{self, LabelMethod, LabelWrite, RejectionSource};
use serde::Serialize;
use tauri::{AppHandle, State};

// ─── AttentionStats ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct AttentionStats {
    pub unmatched: i64,      // 照合未完了
    pub pending: i64,        // レビュー待ち（候補はあるが自動確定できなかった）
    pub no_poster: i64,      // legacy: ポスター/サムネなし
    pub missing_meta: i64,   // year or genres が空
    pub no_persons: i64,     // 人物情報なし
    pub file_missing: i64,   // ファイルが見つからない
    pub source_offline: i64, // legacy: ソースがオフライン
}

#[tauri::command]
pub fn get_attention_stats(state: State<DbState>) -> Result<AttentionStats, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;

    let row: AttentionStats = conn
        .query_row(
            "SELECT
               SUM(CASE WHEN COALESCE(w.match_status, 'unmatched') = 'unmatched' THEN 1 ELSE 0 END),
               SUM(CASE WHEN w.match_status = 'pending' THEN 1 ELSE 0 END),
               SUM(CASE WHEN w.poster_path IS NULL AND w.thumb_path IS NULL THEN 1 ELSE 0 END),
               SUM(CASE WHEN w.year IS NULL OR w.genres_json IS NULL THEN 1 ELSE 0 END),
               (SELECT COUNT(DISTINCT w2.id) FROM works w2
                WHERE NOT EXISTS (SELECT 1 FROM work_persons wp WHERE wp.work_id = w2.id)),
               (SELECT COUNT(DISTINCT wpt_missing.work_id)
                  FROM work_parts wpt_missing
                  JOIN files ff_missing ON ff_missing.id = wpt_missing.file_id
                 WHERE ff_missing.availability_status = 'missing'),
               (SELECT COUNT(*) FROM sources WHERE status = 'offline')
             FROM works w",
            [],
            |row| {
                Ok(AttentionStats {
                    unmatched:      row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                    pending:        row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    no_poster:      row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    missing_meta:   row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    no_persons:     row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    file_missing:   row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                    source_offline: row.get::<_, Option<i64>>(6)?.unwrap_or(0),
                })
            },
        )
        .map_err(|e| e.to_string())?;

    Ok(row)
}

// ─── bulk_set_watch_status ────────────────────────────────────────────────────

#[tauri::command]
pub fn bulk_set_watch_status(
    state: State<DbState>,
    work_ids_json: String, // "[1,2,3]"
    status: String,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let legacy_status = if status == "abandoned" { "skipped".to_string() } else { status.clone() };
    conn.execute(
        "INSERT INTO user_stats (work_id, watch_status, watched_status)
         SELECT CAST(je.value AS INTEGER), ?2, ?3 FROM json_each(?1) je
         ON CONFLICT(work_id) DO UPDATE SET
           watch_status = excluded.watch_status,
           watched_status = excluded.watched_status",
        rusqlite::params![work_ids_json, legacy_status, status],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── bulk_set_favorite ────────────────────────────────────────────────────────

#[tauri::command]
pub fn bulk_set_favorite(
    state: State<DbState>,
    work_ids_json: String,
    is_favorite: bool,
) -> Result<(), String> {
    let is_fav_int: i64 = if is_favorite { 1 } else { 0 };
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO user_stats (work_id, is_favorite)
         SELECT CAST(je.value AS INTEGER), ?2 FROM json_each(?1) je
         ON CONFLICT(work_id) DO UPDATE SET is_favorite = excluded.is_favorite",
        rusqlite::params![work_ids_json, is_fav_int],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── bulk_add_tag ─────────────────────────────────────────────────────────────

#[tauri::command]
pub fn bulk_add_tag(
    state: State<DbState>,
    work_ids_json: String,
    tag_id: i64,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR IGNORE INTO work_tags (work_id, tag_id)
         SELECT CAST(je.value AS INTEGER), ?2 FROM json_each(?1) je",
        rusqlite::params![work_ids_json, tag_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── bulk_remove_tag ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn bulk_remove_tag(
    state: State<DbState>,
    work_ids_json: String,
    tag_id: i64,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM work_tags
         WHERE tag_id = ?2
           AND work_id IN (SELECT CAST(je.value AS INTEGER) FROM json_each(?1) je)",
        rusqlite::params![work_ids_json, tag_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── bulk_set_match_status ────────────────────────────────────────────────────

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct BulkMatchStatusResult {
    /// 状態を変更した件数
    pub updated: usize,
    /// 条件に合わず変更しなかった件数（tmdb_id なしでの固定、固定中の照合解除など）
    pub skipped: usize,
}

/// match_status を一括変更する。
///   locked    : tmdb_id がある作品だけを固定する
///   matched   : 固定を解除する（locked の作品だけが対象。tmdb_id がなければ unmatched に戻す）
///   unmatched : 照合を解除する（固定中の作品は対象外。tmdb_id やポスターも消す）
///   pending   : tmdb_id がなく固定されていない作品だけをレビュー待ちにする
#[tauri::command]
pub fn bulk_set_match_status(
    app: AppHandle,
    state: State<DbState>,
    work_ids_json: String,
    status: String,
) -> Result<BulkMatchStatusResult, String> {
    let status = MatchStatus::parse(&status)?;
    let work_ids: Vec<i64> =
        serde_json::from_str(&work_ids_json).map_err(|_| "作品 ID の指定が不正です".to_string())?;
    let (result, cleared) = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        bulk_set_match_status_inner(&conn, &work_ids, status)?
    };
    for work_id in cleared {
        crate::services::poster_store::delete_poster_for_work(&app, work_id);
    }
    Ok(result)
}

/// bulk_set_match_status の DB 処理。照合を解除した作品 ID も返す（ポスター削除用）。
pub(crate) fn bulk_set_match_status_inner(
    conn: &rusqlite::Connection,
    work_ids: &[i64],
    status: MatchStatus,
) -> Result<(BulkMatchStatusResult, Vec<i64>), String> {
    let mut unique_ids = work_ids.to_vec();
    unique_ids.sort_unstable();
    unique_ids.dedup();
    let ids_json = serde_json::to_string(&unique_ids).map_err(|e| e.to_string())?;
    const TARGET: &str = "id IN (SELECT CAST(je.value AS INTEGER) FROM json_each(?1) je)";

    // 固定・解除の記録に使うので、変更前の照合内容を先に読む
    let previous = read_previous_matches(conn, &ids_json)?;

    let sql = match status {
        MatchStatus::Locked => format!(
            "UPDATE works SET match_status = 'locked'
             WHERE {TARGET} AND tmdb_id IS NOT NULL AND match_status <> 'locked'
             RETURNING id"
        ),
        MatchStatus::Matched => format!(
            "UPDATE works SET match_status = {UNLOCKED_STATUS_SQL},
                    metadata_updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE {TARGET} AND match_status = 'locked'
             RETURNING id"
        ),
        MatchStatus::Unmatched => format!(
            "UPDATE works SET {CLEAR_MATCH_ASSIGNMENTS}
             WHERE {TARGET} AND match_status <> 'locked'
             RETURNING id"
        ),
        MatchStatus::Pending => format!(
            "UPDATE works SET match_status = 'pending'
             WHERE {TARGET} AND match_status <> 'locked' AND tmdb_id IS NULL
             RETURNING id"
        ),
    };

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let changed: Vec<i64> = stmt
        .query_map(rusqlite::params![ids_json], |row| row.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;

    drop(stmt);
    record_bulk_history(conn, &changed, &previous, status)?;

    let result = BulkMatchStatusResult {
        updated: changed.len(),
        skipped: unique_ids.len() - changed.len(),
    };
    let cleared = if status == MatchStatus::Unmatched { changed } else { Vec::new() };
    Ok((result, cleared))
}

/// 変更前の (work_id, tmdb_id, media_type, match_source)
type PreviousMatch = (i64, Option<i64>, Option<String>, Option<String>);

fn read_previous_matches(
    conn: &rusqlite::Connection,
    ids_json: &str,
) -> Result<Vec<PreviousMatch>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, tmdb_id, tmdb_media_type, match_source FROM works
             WHERE id IN (SELECT CAST(je.value AS INTEGER) FROM json_each(?1) je)",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![ids_json], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// 一括操作を照合履歴に残す。
///   固定   … 弱いラベル（中身を確かめたとは限らないので strength = weak）
///   解除   … 「この ID ではない」という否定の記録とラベルの取り下げ
/// 固定解除とレビュー待ちは、正解についての情報を持たないので何も書かない。
fn record_bulk_history(
    conn: &rusqlite::Connection,
    changed: &[i64],
    previous: &[PreviousMatch],
    status: MatchStatus,
) -> Result<(), String> {
    if !matches!(status, MatchStatus::Locked | MatchStatus::Unmatched) {
        return Ok(());
    }
    for work_id in changed {
        let Some((_, tmdb_id, media_type, match_source)) =
            previous.iter().find(|(id, ..)| id == work_id)
        else {
            continue;
        };
        let (Some(tmdb_id), Some(media_type)) = (tmdb_id, media_type.as_deref()) else {
            continue;
        };
        match status {
            MatchStatus::Locked => {
                match_history::record_label(
                    conn,
                    &LabelWrite {
                        work_id: *work_id,
                        run_id: match_history::latest_safe_run_id(conn, *work_id),
                        tmdb: Some((*tmdb_id, media_type)),
                        method: LabelMethod::Lock,
                        note: None,
                    },
                )?;
            }
            MatchStatus::Unmatched => {
                match_history::record_rejection(
                    conn,
                    *work_id,
                    *tmdb_id,
                    media_type,
                    match_source.as_deref(),
                    RejectionSource::Clear,
                )?;
                match_history::withdraw_active_label(conn, *work_id)?;
                match_history::set_match_source(conn, *work_id, None)?;
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::*;

    #[test]
    fn invalid_status_strings_are_rejected() {
        for value in ["auto", "manual", "MATCHED", "", "matched; DROP TABLE works"] {
            assert!(MatchStatus::parse(value).is_err(), "{value}");
        }
    }

    #[test]
    fn lock_requires_tmdb_id() {
        let conn = open_migrated();
        let matched = insert_work(&conn, "A");
        set_match(&conn, matched, "matched", Some(10));
        let unmatched = insert_work(&conn, "B");

        let (result, _) =
            bulk_set_match_status_inner(&conn, &[matched, unmatched], MatchStatus::Locked).unwrap();
        assert_eq!(result, BulkMatchStatusResult { updated: 1, skipped: 1 });
        assert_eq!(match_state(&conn, matched), ("locked".into(), Some(10)));
        assert_eq!(match_state(&conn, unmatched), ("unmatched".into(), None));
    }

    #[test]
    fn unlock_returns_to_a_status_consistent_with_tmdb_id() {
        let conn = open_migrated();
        let with_id = insert_work(&conn, "A");
        set_match(&conn, with_id, "locked", Some(10));
        let without_id = insert_work(&conn, "B");
        set_match(&conn, without_id, "locked", None); // 旧コードで作られ得た不整合
        let unmatched = insert_work(&conn, "C");

        let (result, _) = bulk_set_match_status_inner(
            &conn,
            &[with_id, without_id, unmatched],
            MatchStatus::Matched,
        )
        .unwrap();
        assert_eq!(result, BulkMatchStatusResult { updated: 2, skipped: 1 });
        assert_eq!(match_state(&conn, with_id), ("matched".into(), Some(10)));
        assert_eq!(match_state(&conn, without_id), ("unmatched".into(), None));
        assert_eq!(match_state(&conn, unmatched), ("unmatched".into(), None));
    }

    #[test]
    fn clearing_skips_locked_works_and_removes_tmdb_id() {
        let conn = open_migrated();
        let matched = insert_work(&conn, "A");
        set_match(&conn, matched, "matched", Some(10));
        let locked = insert_work(&conn, "B");
        set_match(&conn, locked, "locked", Some(20));

        let (result, cleared) =
            bulk_set_match_status_inner(&conn, &[matched, locked, matched], MatchStatus::Unmatched)
                .unwrap();
        assert_eq!(result, BulkMatchStatusResult { updated: 1, skipped: 1 });
        assert_eq!(cleared, vec![matched]);
        assert_eq!(match_state(&conn, matched), ("unmatched".into(), None));
        assert_eq!(match_state(&conn, locked), ("locked".into(), Some(20)));
    }

    /// 一括固定は「中身を確かめた」とは限らないので弱いラベルとして残す
    #[test]
    fn bulk_lock_records_a_weak_label() {
        let conn = open_migrated();
        let matched = insert_work(&conn, "A");
        set_match(&conn, matched, "matched", Some(10));
        let unmatched = insert_work(&conn, "B");

        bulk_set_match_status_inner(&conn, &[matched, unmatched], MatchStatus::Locked).unwrap();

        let labels: Vec<(i64, i64, String, String)> = {
            let mut stmt = conn
                .prepare("SELECT work_id, tmdb_id, method, strength FROM metadata_match_labels")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(labels, vec![(matched, 10, "lock".to_string(), "weak".to_string())]);
    }

    /// 一括解除は「この ID ではない」だけが分かる。ラベルは取り下げる
    #[test]
    fn bulk_clear_records_a_rejection_and_withdraws_the_label() {
        let conn = open_migrated();
        let work_id = insert_work(&conn, "A");
        set_match(&conn, work_id, "matched", Some(10));
        conn.execute(
            "UPDATE works SET match_source = 'rules_safe_auto' WHERE id = ?1",
            rusqlite::params![work_id],
        )
        .unwrap();
        match_history::record_label(
            &conn,
            &LabelWrite {
                work_id,
                run_id: None,
                tmdb: Some((10, "movie")),
                method: LabelMethod::ManualApply,
                note: None,
            },
        )
        .unwrap();

        bulk_set_match_status_inner(&conn, &[work_id], MatchStatus::Unmatched).unwrap();

        let (tmdb_id, source, previous): (i64, String, Option<String>) = conn
            .query_row(
                "SELECT tmdb_id, source, previous_match_source FROM metadata_match_rejections
                 WHERE work_id = ?1",
                rusqlite::params![work_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((tmdb_id, source.as_str(), previous.as_deref()), (10, "clear", Some("rules_safe_auto")));

        let active: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata_match_labels
                 WHERE work_id = ?1 AND superseded_at IS NULL",
                rusqlite::params![work_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active, 0);
        let match_source: Option<String> = conn
            .query_row(
                "SELECT match_source FROM works WHERE id = ?1",
                rusqlite::params![work_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(match_source, None);
    }

    /// 固定解除とレビュー待ちは、正解についての情報を持たないので履歴を書かない
    #[test]
    fn unlock_and_pending_do_not_write_history() {
        let conn = open_migrated();
        let locked = insert_work(&conn, "A");
        set_match(&conn, locked, "locked", Some(10));
        let unmatched = insert_work(&conn, "B");

        bulk_set_match_status_inner(&conn, &[locked], MatchStatus::Matched).unwrap();
        bulk_set_match_status_inner(&conn, &[unmatched], MatchStatus::Pending).unwrap();

        for table in ["metadata_match_labels", "metadata_match_rejections"] {
            let rows: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(rows, 0, "{table}");
        }
    }

    #[test]
    fn pending_requires_no_tmdb_id() {
        let conn = open_migrated();
        let matched = insert_work(&conn, "A");
        set_match(&conn, matched, "matched", Some(10));
        let unmatched = insert_work(&conn, "B");

        let (result, _) =
            bulk_set_match_status_inner(&conn, &[matched, unmatched], MatchStatus::Pending).unwrap();
        assert_eq!(result, BulkMatchStatusResult { updated: 1, skipped: 1 });
        assert_eq!(match_state(&conn, matched), ("matched".into(), Some(10)));
        assert_eq!(match_state(&conn, unmatched), ("pending".into(), None));
    }
}
