//! 埋め込みメタデータの後埋め（PR2.5）。
//!
//! 020 以前から DB にあるファイルは、scan 時にタグを取っていない。
//! ユーザーが明示的に実行したときだけ ffprobe で読み直す（自動では走らせない）。
//! 中断しても、次に実行すれば残りから再開する。

use crate::db::DbState;
use crate::services::container_tags::ContainerTags;
use crate::services::match_history::{self, MetadataConflictDetails};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

/// 同時に走らせる ffprobe の数。NAS 越しは待ち時間が支配的なので並列にする
const DEFAULT_CONCURRENCY: usize = 6;
const MAX_CONCURRENCY: usize = 16;
/// 1回の呼び出しで処理する件数の既定値（お試し実行ができるよう limit を受け取る）
const DEFAULT_LIMIT: usize = 5000;

#[derive(Debug, Clone, Serialize)]
pub struct TagBackfillProgress {
    pub processed: usize,
    pub total: usize,
    pub with_tags: usize,
    pub failed: usize,
    pub current_file: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagBackfillResult {
    pub processed: usize,
    pub total: usize,
    pub with_tags: usize,
    pub failed: usize,
    /// まだタグを読んでいないファイル数（再開の目安）
    pub remaining: usize,
    /// 作ったレビュー課題（タグと既存照合の食い違い）
    pub conflicts_created: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagCoverage {
    pub files: usize,
    pub with_tags: usize,
    pub captured_at_scan: usize,
    pub provider_known: usize,
    pub pipeline_known: usize,
    pub unknown: usize,
    pub suspicious: usize,
}

/// タグの取得状況を返す（画面の表示用）
#[tauri::command]
pub fn container_tag_coverage(state: State<'_, DbState>) -> Result<TagCoverage, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let count = |sql: &str| -> Result<usize, String> {
        conn.query_row(sql, [], |row| row.get::<_, i64>(0))
            .map(|n| n as usize)
            .map_err(|e| e.to_string())
    };
    Ok(TagCoverage {
        files: count("SELECT COUNT(*) FROM files")?,
        with_tags: count("SELECT COUNT(*) FROM files WHERE container_tags_json IS NOT NULL")?,
        captured_at_scan: count("SELECT COUNT(*) FROM files WHERE tags_captured = 1")?,
        provider_known: count(
            "SELECT COUNT(*) FROM files WHERE tags_provenance = 'provider_known'",
        )?,
        pipeline_known: count(
            "SELECT COUNT(*) FROM files WHERE tags_provenance = 'pipeline_known'",
        )?,
        unknown: count("SELECT COUNT(*) FROM files WHERE tags_provenance = 'unknown'")?,
        suspicious: count("SELECT COUNT(*) FROM files WHERE tags_provenance = 'suspicious'")?,
    })
}

/// 埋め込みメタデータを後から読み込む。ユーザー操作でのみ実行する。
///
/// - `limit`: 今回処理する件数（お試し実行に使う。省略時は全件）
/// - `concurrency`: 同時に走らせる ffprobe の数（省略時 6、上限 16）
#[tauri::command]
pub async fn backfill_container_tags(
    app: AppHandle,
    state: State<'_, DbState>,
    limit: Option<usize>,
    concurrency: Option<usize>,
) -> Result<TagBackfillResult, String> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    let concurrency = concurrency.unwrap_or(DEFAULT_CONCURRENCY).clamp(1, MAX_CONCURRENCY);

    // 対象: まだタグを読んでいない、実体のあるファイル
    let targets: Vec<(i64, String)> = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        pending_tag_targets(&conn, limit)?
    };

    let total = targets.len();
    let mut processed = 0usize;
    let mut with_tags = 0usize;
    let mut failed = 0usize;

    let _ = app.emit(
        "tags:backfill_progress",
        TagBackfillProgress { processed, total, with_tags, failed, current_file: None },
    );

    // bounded concurrency: 同時に走らせる数を concurrency 件に抑えながら順に進める
    for chunk in targets.chunks(concurrency) {
        let chunk_targets = chunk.to_vec();
        let results = tokio::task::spawn_blocking(move || probe_chunk(&chunk_targets))
            .await
            .map_err(|e| e.to_string())?;

        let last_file = results.last().map(|(_, path, _)| path.clone());
        {
            let conn = state.0.lock().map_err(|e| e.to_string())?;
            let counts = store_chunk(&conn, &results);
            processed += counts.processed;
            with_tags += counts.with_tags;
            failed += counts.failed;
        }

        let _ = app.emit(
            "tags:backfill_progress",
            TagBackfillProgress {
                processed,
                total,
                with_tags,
                failed,
                current_file: last_file,
            },
        );
    }

    let (remaining, conflicts_created) = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files
                 WHERE container_tags_json IS NULL AND availability_status = 'available'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let conflicts = scan_metadata_conflicts(&conn)?;
        (remaining as usize, conflicts)
    };

    Ok(TagBackfillResult {
        processed,
        total,
        with_tags,
        failed,
        remaining,
        conflicts_created,
    })
}

/// 1 chunk 分のプローブ結果
type ProbeResult = (i64, String, Option<ContainerTags>);

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ChunkCounts {
    pub processed: usize,
    pub with_tags: usize,
    pub failed: usize,
}

/// chunk 内のファイルを並列に ffprobe する。
/// 同時実行数は chunk の大きさ（= concurrency）で抑える。
pub(crate) fn probe_chunk(targets: &[(i64, String)]) -> Vec<ProbeResult> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = targets
            .iter()
            .map(|(_, path)| scope.spawn(move || crate::commands::scan::probe_container_tags(path)))
            .collect();
        handles
            .into_iter()
            .zip(targets.iter())
            .map(|(handle, (file_id, path))| {
                let tags = handle.join().unwrap_or(None);
                (*file_id, path.clone(), tags)
            })
            .collect()
    })
}

/// プローブ結果を files に書く
pub(crate) fn store_chunk(conn: &rusqlite::Connection, results: &[ProbeResult]) -> ChunkCounts {
    let mut counts = ChunkCounts::default();
    for (file_id, path, tags) in results {
        counts.processed += 1;
        match tags {
            Some(tags) => {
                if !tags.is_empty() {
                    counts.with_tags += 1;
                }
                let _ = crate::commands::scan::store_container_tags(
                    conn,
                    *file_id,
                    tags,
                    Some(path),
                    false,
                );
            }
            None => counts.failed += 1,
        }
    }
    counts
}

/// まだタグを読んでいないファイル（中断後はここから再開する）
pub(crate) fn pending_tag_targets(
    conn: &rusqlite::Connection,
    limit: usize,
) -> Result<Vec<(i64, String)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, file_path FROM files
             WHERE container_tags_json IS NULL
               AND availability_status = 'available'
             ORDER BY id
             LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// 既存の照合とタグの年が食い違う作品にレビュー課題を作る。
/// works は一切変更しない（勝手に照合を外さない）。
#[tauri::command]
pub fn scan_metadata_conflicts_command(state: State<'_, DbState>) -> Result<usize, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    scan_metadata_conflicts(&conn)
}

/// タグの年と TMDB の年が2年以上違う照合済み作品を洗い出す
pub(crate) fn scan_metadata_conflicts(conn: &rusqlite::Connection) -> Result<usize, String> {
    let rows: Vec<(i64, Option<i32>, Option<String>, Option<i64>, Option<String>, Option<String>, String, Option<String>, Option<String>)> = {
        let mut stmt = conn
            .prepare(
                "SELECT w.id, w.year, w.title, w.tmdb_id, w.tmdb_media_type, w.match_source,
                        f.container_tags_json, f.tags_provenance, f.tags_provider_hint
                 FROM works w
                 JOIN work_parts wp ON wp.work_id = w.id
                 JOIN files f ON f.id = wp.file_id
                 WHERE w.tmdb_id IS NOT NULL
                   AND w.year IS NOT NULL
                   AND f.container_tags_json IS NOT NULL",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };

    let mut created = 0usize;
    for (work_id, year, title, tmdb_id, media_type, match_source, tags_json, provenance, provider) in rows {
        let Some(tags) = ContainerTags::from_json(&tags_json) else {
            continue;
        };
        let (Some(embedded_year), Some(matched_year)) = (tags.year(), year) else {
            continue;
        };
        if (embedded_year - matched_year).abs() <= CONFLICT_YEAR_TOLERANCE {
            continue;
        }
        let details = MetadataConflictDetails {
            kind: "year_mismatch",
            embedded_year: Some(embedded_year),
            matched_year: Some(matched_year),
            embedded_title: tags.cleaned_title(),
            matched_title: title,
            tmdb_id,
            media_type,
            match_source,
            tags_provenance: provenance,
            tags_provider_hint: provider,
        };
        if match_history::open_metadata_conflict(conn, work_id, &details)?.is_some() {
            created += 1;
        }
    }
    Ok(created)
}

/// 年の食い違いをレビュー対象にする差（配信年のずれは ±1 まで許す）
const CONFLICT_YEAR_TOLERANCE: i32 = 1;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::scan::store_container_tags;
    use crate::db::test_support::*;
    use std::collections::BTreeMap;

    fn tags_with_year(year: &str) -> ContainerTags {
        let mut map = BTreeMap::new();
        map.insert("title".to_string(), "ナイアガラ".to_string());
        map.insert("date".to_string(), year.to_string());
        map.insert("encoder".to_string(), "Lavf58.76.100".to_string());
        ContainerTags::from_ffprobe(&map, &[])
    }

    fn work_with_tagged_file(conn: &rusqlite::Connection, title: &str, tags: &ContainerTags) -> i64 {
        let source_id = insert_source(conn, &format!(r"D:\Movies\{title}"));
        let work_id = insert_work(conn, title);
        conn.execute(
            "INSERT INTO files (source_id, file_path, file_name) VALUES (?1, ?2, ?3)",
            rusqlite::params![source_id, format!(r"D:\Movies\{title}\a.mkv"), "a.mkv"],
        )
        .unwrap();
        let file_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO work_parts (work_id, file_id) VALUES (?1, ?2)",
            rusqlite::params![work_id, file_id],
        )
        .unwrap();
        store_container_tags(conn, file_id, tags, None, false).unwrap();
        work_id
    }

    #[test]
    fn year_mismatch_creates_one_task_per_work() {
        let conn = open_migrated();
        let work_id = work_with_tagged_file(&conn, "ナイアガラ", &tags_with_year("1953"));
        conn.execute(
            "UPDATE works SET tmdb_id = 999, tmdb_media_type = 'movie', year = 2013,
                    match_status = 'matched', match_source = 'legacy' WHERE id = ?1",
            rusqlite::params![work_id],
        )
        .unwrap();

        assert_eq!(scan_metadata_conflicts(&conn).unwrap(), 1);
        // 2回流しても増えない
        assert_eq!(scan_metadata_conflicts(&conn).unwrap(), 0);

        let (reason, details): (String, Option<String>) = conn
            .query_row(
                "SELECT reason, details_json FROM metadata_review_tasks WHERE work_id = ?1",
                rusqlite::params![work_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(reason, "metadata_conflict");
        let details = details.expect("details_json");
        assert!(details.contains("\"embedded_year\":1953"), "{details}");
        assert!(details.contains("\"matched_year\":2013"), "{details}");
        assert!(details.contains("pipeline_known"), "{details}");

        // works は変更しない
        let (status, tmdb_id): (String, Option<i64>) = conn
            .query_row(
                "SELECT match_status, tmdb_id FROM works WHERE id = ?1",
                rusqlite::params![work_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((status.as_str(), tmdb_id), ("matched", Some(999)));
    }

    #[test]
    fn distribution_year_within_one_is_not_a_conflict() {
        let conn = open_migrated();
        let work_id = work_with_tagged_file(&conn, "モネ・ゲーム", &tags_with_year("2013"));
        conn.execute(
            "UPDATE works SET tmdb_id = 1, tmdb_media_type = 'movie', year = 2012,
                    match_status = 'matched' WHERE id = ?1",
            rusqlite::params![work_id],
        )
        .unwrap();
        assert_eq!(scan_metadata_conflicts(&conn).unwrap(), 0);
    }

    /// 後埋めは中断しても残りから再開する（読んだ行は対象から外れる）
    #[test]
    fn backfill_targets_skip_files_that_already_have_tags() {
        let conn = open_migrated();
        let source_id = insert_source(&conn, r"D:\Movies");
        let mut ids = Vec::new();
        for name in ["a.mkv", "b.mkv", "c.mkv"] {
            conn.execute(
                "INSERT INTO files (source_id, file_path, file_name) VALUES (?1, ?2, ?3)",
                rusqlite::params![source_id, format!(r"D:\Movies\{name}"), name],
            )
            .unwrap();
            ids.push(conn.last_insert_rowid());
        }
        // 実体が無いファイルは対象にしない
        conn.execute(
            "INSERT INTO files (source_id, file_path, file_name, availability_status)
             VALUES (?1, 'D:\\Movies\\gone.mkv', 'gone.mkv', 'missing')",
            rusqlite::params![source_id],
        )
        .unwrap();

        // お試し実行（1件だけ）
        let first = pending_tag_targets(&conn, 1).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].0, ids[0]);

        // 1件目を読んだことにすると、次は2件目から
        store_container_tags(&conn, ids[0], &ContainerTags::empty(), None, false).unwrap();
        let rest = pending_tag_targets(&conn, 100).unwrap();
        assert_eq!(rest.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![ids[1], ids[2]]);

        for id in &ids[1..] {
            store_container_tags(&conn, *id, &ContainerTags::empty(), None, false).unwrap();
        }
        assert!(pending_tag_targets(&conn, 100).unwrap().is_empty());
    }

    /// 実ファイルでの動作確認（手動実行）。
    /// CINEMANTIS_DB_COPY に「コピーした」DB を渡して
    /// `cargo test --lib -- --ignored backfill_smoke_test_on_db_copy --nocapture` で実行する。
    /// 実 NAS 上のファイルを ffprobe で読むだけで、works には一切書き込まない。
    #[test]
    #[ignore]
    fn backfill_smoke_test_on_db_copy() {
        let Ok(path) = std::env::var("CINEMANTIS_DB_COPY") else {
            return;
        };
        let limit: usize = std::env::var("CINEMANTIS_BACKFILL_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);
        let concurrency: usize = std::env::var("CINEMANTIS_BACKFILL_CONCURRENCY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);

        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        let works_before = works_fingerprint(&conn);
        let targets = pending_tag_targets(&conn, limit).unwrap();
        println!("targets={} concurrency={concurrency}", targets.len());

        let started = std::time::Instant::now();
        let mut counts = ChunkCounts::default();
        for chunk in targets.chunks(concurrency) {
            let results = probe_chunk(chunk);
            let chunk_counts = store_chunk(&conn, &results);
            counts.processed += chunk_counts.processed;
            counts.with_tags += chunk_counts.with_tags;
            counts.failed += chunk_counts.failed;
        }
        let elapsed = started.elapsed();
        println!(
            "processed={} with_tags={} failed={} elapsed={:.1}s ({:.2}s/file)",
            counts.processed,
            counts.with_tags,
            counts.failed,
            elapsed.as_secs_f64(),
            elapsed.as_secs_f64() / counts.processed.max(1) as f64
        );
        assert_eq!(counts.processed, targets.len());

        // 保存内容の要約
        for (index, (file_id, file_path)) in targets.iter().enumerate() {
            let row: (Option<String>, Option<String>, Option<String>, Option<i64>, i64) = conn
                .query_row(
                    "SELECT container_tags_json, tags_provenance, tags_provider_hint,
                            tags_truncated, tags_captured
                     FROM files WHERE id = ?1",
                    rusqlite::params![file_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .unwrap();
            let (json, provenance, provider, truncated, captured) = row;
            let json = json.expect("container_tags_json が書かれていること");
            assert_eq!(captured, 0, "後埋めは captured=0");
            let tags = ContainerTags::from_json(&json).expect("JSON を読み戻せること");
            if index < 5 {
                println!(
                    "--- {}\n  bytes={} provenance={:?} provider={:?} truncated={:?}\n  \
                     title={:?}\n  embedded_title={:?} year={:?}\n  audio={:?} sub={:?}\n  cast({})={:?}",
                    file_path,
                    json.len(),
                    provenance,
                    provider,
                    truncated,
                    tags.title,
                    tags.cleaned_title(),
                    tags.year(),
                    tags.audio_languages,
                    tags.subtitle_languages,
                    tags.cast().len(),
                    tags.cast().iter().take(3).collect::<Vec<_>>(),
                );
            }
            assert!(json.len() <= 8 * 1024, "JSON が上限を超えている: {}", json.len());
        }

        let sizes: Vec<usize> = targets
            .iter()
            .map(|(file_id, _)| {
                conn.query_row(
                    "SELECT LENGTH(container_tags_json) FROM files WHERE id = ?1",
                    rusqlite::params![file_id],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap() as usize
            })
            .collect();
        println!(
            "json_bytes min={} max={} avg={}",
            sizes.iter().min().copied().unwrap_or(0),
            sizes.iter().max().copied().unwrap_or(0),
            sizes.iter().sum::<usize>() / sizes.len().max(1)
        );

        // 2回目は同じファイルを処理しない
        let next = pending_tag_targets(&conn, limit).unwrap();
        let processed_ids: Vec<i64> = targets.iter().map(|(id, _)| *id).collect();
        assert!(
            next.iter().all(|(id, _)| !processed_ids.contains(id)),
            "処理済みのファイルが再度対象になっている"
        );

        // metadata_conflict の作成（works は変更しない）
        let conflicts = scan_metadata_conflicts(&conn).unwrap();
        println!("metadata_conflicts_created={conflicts}");
        if conflicts > 0 {
            let mut stmt = conn
                .prepare(
                    "SELECT work_id, details_json FROM metadata_review_tasks
                     WHERE reason = 'metadata_conflict' ORDER BY id DESC LIMIT 10",
                )
                .unwrap();
            let rows: Vec<(i64, Option<String>)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            for (work_id, details) in rows {
                println!("  conflict work_id={work_id} {}", details.unwrap_or_default());
            }
        }

        assert_eq!(
            works_fingerprint(&conn),
            works_before,
            "backfill は works の照合状態を変えない"
        );
    }

    /// works の照合状態（変わっていないことの確認用）
    #[cfg(test)]
    fn works_fingerprint(conn: &rusqlite::Connection) -> (i64, i64, i64, i64, i64) {
        let one = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        (
            one("SELECT COUNT(*) FROM works"),
            one("SELECT COUNT(*) FROM works WHERE tmdb_id IS NOT NULL"),
            one("SELECT COUNT(*) FROM works WHERE match_status = 'matched'"),
            one("SELECT COUNT(*) FROM works WHERE match_status = 'pending'"),
            one("SELECT COALESCE(SUM(tmdb_id), 0) FROM works"),
        )
    }

    #[test]
    fn unmatched_works_and_missing_years_are_skipped() {
        let conn = open_migrated();
        let unmatched = work_with_tagged_file(&conn, "未照合", &tags_with_year("1999"));
        let no_year = work_with_tagged_file(&conn, "年なし", &ContainerTags::empty());
        conn.execute(
            "UPDATE works SET tmdb_id = 5, tmdb_media_type = 'movie', year = 2020 WHERE id = ?1",
            rusqlite::params![no_year],
        )
        .unwrap();

        assert_eq!(scan_metadata_conflicts(&conn).unwrap(), 0);
        let tasks: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_review_tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tasks, 0);
        assert!(unmatched > 0);
    }
}
