//! 照合履歴（PR2）。
//!
//! 自動照合1回を `metadata_match_runs` に残し、その run が見た候補と
//! 判定器ごとの結論を子テーブルに書く。人間の確定は
//!   - 確定方法 … `metadata_match_labels.method`
//!   - レビュー対象になった理由 … `metadata_review_tasks.reason`
//! と別の概念として持つ。
//!
//! ここでは TMDB も Jev も呼ばない。DB への書き込みだけを担当する。

use crate::models::match_source::MatchSource;
use crate::models::match_status::MatchStatus;
use crate::models::tmdb::TmdbCandidate;
use crate::services::metadata_matcher::{
    self, SafeDecision, SafeMatchOutcome, THRESHOLD_AUTO, THRESHOLD_CANDIDATE, YEAR_TOLERANCE,
};
use crate::services::prematch_snapshot::PreMatchSnapshot;
use crate::services::title_parser::ParsedTitle;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

/// rules-safe のバージョン（閾値や矛盾ルールを変えたら上げる）
pub const POLICY_VERSION: &str = "rules-safe-1";
/// 比較基準（現行の score_movie / score_tv / best_candidate）。旧経路のまま凍結する
pub const RULES_V1_VERSION: &str = "rules-1";
/// 埋め込みメタデータを使う影判定（PR2.5 では記録・比較専用）
pub const SHADOW_VERSION: &str = "rules-tags-shadow-1";
/// 1 run に残す候補の最大件数
const MAX_STORED_CANDIDATES: usize = 20;

/// 照合が始まったきっかけ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKind {
    /// 一括照合
    Batch,
    /// 1作品の照合
    Single,
    /// PR1 以前の pending を付け直す（1作品につき1回だけ）
    LegacyRescan,
    /// 過去の run を再生する（works は変えない）。PR4 の再評価で使う
    #[cfg_attr(not(test), allow(dead_code))]
    Replay,
}

impl TriggerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TriggerKind::Batch => "batch",
            TriggerKind::Single => "single",
            TriggerKind::LegacyRescan => "legacy_rescan",
            TriggerKind::Replay => "replay",
        }
    }
}

/// レビュー対象になった理由
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewReason {
    ReviewDecision,
    LegacyPending,
    /// 判定器どうしの不一致（PR3 以降）
    #[cfg_attr(not(test), allow(dead_code))]
    MatcherDisagreement,
    /// ユーザーが自分で候補を探した（PR4）
    #[cfg_attr(not(test), allow(dead_code))]
    UserInitiated,
}

impl ReviewReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewReason::ReviewDecision => "review_decision",
            ReviewReason::LegacyPending => "legacy_pending",
            ReviewReason::MatcherDisagreement => "matcher_disagreement",
            ReviewReason::UserInitiated => "user_initiated",
        }
    }
}

/// 人がどうやって確定したか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelMethod {
    /// 候補ダイアログで選んで適用した
    ManualApply,
    /// TMDb ID を直接指定して適用した
    ManualDirectId,
    /// 固定した（一括操作を含むので弱い証拠）
    Lock,
}

impl LabelMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            LabelMethod::ManualApply => "manual_apply",
            LabelMethod::ManualDirectId => "manual_direct_id",
            LabelMethod::Lock => "lock",
        }
    }

    /// 精度の計算に使ってよい強さ。lock は「中身を確かめた」とは限らない
    pub fn strength(self) -> &'static str {
        match self {
            LabelMethod::Lock => "weak",
            _ => "strong",
        }
    }
}

/// 「この TMDB ID ではない」と分かった経緯
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectionSource {
    /// 照合を解除した
    Clear,
    /// 別の候補に付け替えた
    Repick,
}

impl RejectionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            RejectionSource::Clear => "clear",
            RejectionSource::Repick => "repick",
        }
    }
}

/// 実際に投げた TMDB 検索
#[derive(Debug, Clone, Serialize)]
pub struct SearchQuery {
    /// "movie" | "tv"
    pub kind: &'static str,
    pub query: String,
    pub year: Option<i32>,
    /// 検索語の出どころ（legacy / embedded）
    pub source: &'static str,
}

/// 候補がどの検索語から出てきたか（legacy / embedded / both）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateSource {
    pub tmdb_id: i64,
    pub media_type: String,
    pub query_source: String,
}

/// run に残す1回分の照合
pub struct RunInput<'a> {
    pub work_id: i64,
    pub batch_id: Option<&'a str>,
    pub trigger_kind: TriggerKind,
    pub snapshot: &'a PreMatchSnapshot,
    pub parsed: &'a ParsedTitle,
    pub queries: &'a [SearchQuery],
    /// merge_and_rank を通した候補（閾値以上・最大10件）
    pub ranked: &'a [TmdbCandidate],
    /// 落ちた候補も含む全候補（検索結果の順）
    pub all: &'a [TmdbCandidate],
    pub rules_safe: &'a SafeMatchOutcome<'a>,
    /// rules-1（比較基準）の結論。旧経路の候補だけから選ぶ
    pub rules_one: Option<&'a TmdbCandidate>,
    /// rules-tags-shadow（PR2.5 の記録専用判定）
    pub shadow: Option<&'a SafeMatchOutcome<'a>>,
    /// 候補ごとの検索語の出どころ
    pub candidate_sources: &'a [CandidateSource],
    pub tmdb_calls: usize,
    pub latency_ms: u64,
    /// TMDB 検索が失敗した run（decision は ERROR になる）
    pub error_text: Option<String>,
}

#[derive(Serialize)]
struct PolicySnapshot {
    threshold_auto: i32,
    threshold_candidate: i32,
    year_tolerance: i32,
    explicit_conflicts: [&'static str; 2],
}

#[derive(Serialize)]
struct CandidateSnapshot<'a> {
    title: &'a str,
    original_title: Option<&'a str>,
    year: Option<i32>,
    poster_path: Option<&'a str>,
}

fn policy_snapshot_json() -> String {
    serde_json::to_string(&PolicySnapshot {
        threshold_auto: THRESHOLD_AUTO,
        threshold_candidate: THRESHOLD_CANDIDATE,
        year_tolerance: YEAR_TOLERANCE,
        explicit_conflicts: [
            metadata_matcher::reason::EPISODE_MARKER_VS_MOVIE,
            metadata_matcher::reason::PART_MISMATCH,
        ],
    })
    .unwrap_or_else(|_| "{}".to_string())
}

fn json_array(values: &[impl Serialize]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
}

/// 保存する候補を選ぶ。順位の付いた候補を優先し、落ちた候補も上位から足す
fn candidates_to_store<'a>(
    ranked: &'a [TmdbCandidate],
    all: &'a [TmdbCandidate],
) -> Vec<(&'a TmdbCandidate, Option<usize>, Option<usize>)> {
    let mut stored: Vec<(&TmdbCandidate, Option<usize>, Option<usize>)> = Vec::new();
    let search_rank = |c: &TmdbCandidate| {
        all.iter()
            .position(|a| a.tmdb_id == c.tmdb_id && a.media_type == c.media_type)
            .map(|i| i + 1)
    };
    for (index, candidate) in ranked.iter().enumerate() {
        stored.push((candidate, search_rank(candidate), Some(index + 1)));
    }
    let mut dropped: Vec<&TmdbCandidate> = all
        .iter()
        .filter(|c| {
            !ranked
                .iter()
                .any(|r| r.tmdb_id == c.tmdb_id && r.media_type == c.media_type)
        })
        .collect();
    dropped.sort_by(|a, b| b.confidence.cmp(&a.confidence));
    for candidate in dropped {
        if stored.len() >= MAX_STORED_CANDIDATES {
            break;
        }
        stored.push((candidate, search_rank(candidate), None));
    }
    stored
}

/// run・候補・判定・（REVIEW なら）レビュー課題を1トランザクションで書く。
/// works は変更しない（適用の記録は [`mark_run_applied`] / [`finish_run`]）。
pub fn record_run(conn: &Connection, input: &RunInput) -> Result<i64, String> {
    let decision = match (&input.error_text, input.rules_safe.decision) {
        (Some(_), _) => "ERROR".to_string(),
        (None, decision) => decision.as_str().to_string(),
    };
    let reasons: Vec<&str> = input.rules_safe.reasons.to_vec();

    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO metadata_match_runs
           (work_id, batch_id, trigger_kind, mode, evidence_class, state_schema_version,
            input_snapshot_json, search_queries_json, policy_version, policy_snapshot_json,
            governing_matcher, decision, decision_reasons_json, tmdb_calls, latency_ms, error_text)
         VALUES (?1, ?2, ?3, 'safe', ?4, ?5, ?6, ?7, ?8, ?9, 'rules-safe', ?10, ?11, ?12, ?13, ?14)",
        rusqlite::params![
            input.work_id,
            input.batch_id,
            input.trigger_kind.as_str(),
            input.snapshot.evidence_class.as_str(),
            input.snapshot.state_schema_version,
            input.snapshot.to_json(),
            json_array(input.queries),
            POLICY_VERSION,
            policy_snapshot_json(),
            decision,
            json_array(&reasons),
            input.tmdb_calls as i64,
            input.latency_ms as i64,
            input.error_text,
        ],
    )
    .map_err(|e| e.to_string())?;
    let run_id = tx.last_insert_rowid();

    for (index, (candidate, search_rank, rules_rank)) in
        candidates_to_store(input.ranked, input.all).into_iter().enumerate()
    {
        let conflicts = metadata_matcher::explicit_conflicts(candidate, input.parsed);
        let query_source = input
            .candidate_sources
            .iter()
            .find(|s| s.tmdb_id == candidate.tmdb_id && s.media_type == candidate.media_type)
            .map(|s| s.query_source.clone());
        tx.execute(
            "INSERT INTO metadata_match_candidates
               (run_id, cand_key, tmdb_id, media_type, search_rank, rules_rank,
                rules_score, rules_reasons_json, tmdb_snapshot_json, explicit_conflicts_json,
                query_source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                run_id,
                format!("c{}", index + 1),
                candidate.tmdb_id,
                candidate.media_type,
                search_rank.map(|r| r as i64),
                rules_rank.map(|r| r as i64),
                candidate.confidence,
                json_array(&candidate.reasons),
                serde_json::to_string(&CandidateSnapshot {
                    title: &candidate.title,
                    original_title: candidate.original_title.as_deref(),
                    year: candidate.year,
                    poster_path: candidate.poster_path.as_deref(),
                })
                .unwrap_or_else(|_| "{}".to_string()),
                json_array(&conflicts),
                query_source,
            ],
        )
        .map_err(|e| e.to_string())?;
    }

    // rules-safe（本番の判定）
    insert_verdict(
        &tx,
        run_id,
        "rules-safe",
        POLICY_VERSION,
        input.rules_safe.top,
        input.rules_safe.decision,
        &reasons,
    )?;
    // rules-1（比較基準）。閾値以上の最良候補が無ければ UNRESOLVED
    let rules_one_decision = if input.rules_one.is_some() {
        SafeDecision::Auto
    } else {
        SafeDecision::Unresolved
    };
    insert_verdict(
        &tx,
        run_id,
        "rules-1",
        RULES_V1_VERSION,
        input.rules_one,
        rules_one_decision,
        &[],
    )?;
    // rules-tags-shadow（記録専用。works には反映しない）
    if let Some(shadow) = input.shadow {
        let shadow_reasons: Vec<&str> = shadow.reasons.to_vec();
        insert_verdict(
            &tx,
            run_id,
            "rules-tags-shadow",
            SHADOW_VERSION,
            shadow.top,
            shadow.decision,
            &shadow_reasons,
        )?;
    }

    if input.error_text.is_none() && input.rules_safe.decision == SafeDecision::Review {
        let reason = match input.trigger_kind {
            TriggerKind::LegacyRescan => ReviewReason::LegacyPending,
            _ => ReviewReason::ReviewDecision,
        };
        open_review_task_in(&tx, input.work_id, Some(run_id), reason)?;
    }

    tx.commit().map_err(|e| e.to_string())?;
    Ok(run_id)
}

fn insert_verdict(
    conn: &Connection,
    run_id: i64,
    matcher: &str,
    matcher_version: &str,
    candidate: Option<&TmdbCandidate>,
    decision: SafeDecision,
    reasons: &[&str],
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO metadata_match_verdicts
           (run_id, matcher, matcher_version, tmdb_id, media_type, decision, score, reasons_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            run_id,
            matcher,
            matcher_version,
            candidate.map(|c| c.tmdb_id),
            candidate.map(|c| c.media_type.clone()),
            decision.as_str(),
            candidate.map(|c| c.confidence as f64),
            json_array(reasons),
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 自動適用が works に反映できた後に呼ぶ
pub fn mark_run_applied(
    conn: &Connection,
    run_id: i64,
    work_id: i64,
    tmdb_id: i64,
    media_type: &str,
    status_after: MatchStatus,
    source: MatchSource,
) -> Result<(), String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE metadata_match_runs
         SET applied = 1, applied_tmdb_id = ?2, applied_media_type = ?3, status_after = ?4
         WHERE id = ?1",
        rusqlite::params![run_id, tmdb_id, media_type, status_after.as_str()],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE works SET last_match_run_id = ?2, match_source = ?3 WHERE id = ?1",
        rusqlite::params![work_id, run_id, source.as_str()],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

/// 適用しなかった run の後始末（works の状態を記録する）
pub fn finish_run(conn: &Connection, run_id: i64, status_after: MatchStatus) -> Result<(), String> {
    conn.execute(
        "UPDATE metadata_match_runs SET status_after = ?2 WHERE id = ?1",
        rusqlite::params![run_id, status_after.as_str()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// その作品の直近の run（手動確定をどの run の候補から選んだかを辿るため）
pub fn latest_run_id(conn: &Connection, work_id: i64) -> Option<i64> {
    conn.query_row(
        "SELECT id FROM metadata_match_runs WHERE work_id = ?1 ORDER BY id DESC LIMIT 1",
        rusqlite::params![work_id],
        |row| row.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// 未解決のレビュー課題を作る。同じ理由の未解決課題があれば作らない
/// （PR2 の本番経路は record_run 経由。監査サンプルの生成は PR4）
#[cfg_attr(not(test), allow(dead_code))]
pub fn open_review_task(
    conn: &Connection,
    work_id: i64,
    run_id: Option<i64>,
    reason: ReviewReason,
) -> Result<i64, String> {
    open_review_task_in(conn, work_id, run_id, reason)
}

fn open_review_task_in(
    conn: &Connection,
    work_id: i64,
    run_id: Option<i64>,
    reason: ReviewReason,
) -> Result<i64, String> {
    if let Some(existing) = conn
        .query_row(
            "SELECT id FROM metadata_review_tasks
             WHERE work_id = ?1 AND reason = ?2 AND resolved_at IS NULL",
            rusqlite::params![work_id, reason.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
    {
        return Ok(existing);
    }
    conn.execute(
        "INSERT INTO metadata_review_tasks (work_id, run_id, reason) VALUES (?1, ?2, ?3)",
        rusqlite::params![work_id, run_id, reason.as_str()],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// タグと既存照合の食い違い（PR2.5）。works は一切変更しない。
/// 同じ作品に未解決の metadata_conflict があれば作らない。
pub fn open_metadata_conflict(
    conn: &Connection,
    work_id: i64,
    details: &MetadataConflictDetails,
) -> Result<Option<i64>, String> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM metadata_review_tasks
             WHERE work_id = ?1 AND reason = 'metadata_conflict' AND resolved_at IS NULL",
            rusqlite::params![work_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if existing.is_some() {
        return Ok(None);
    }
    conn.execute(
        "INSERT INTO metadata_review_tasks (work_id, reason, details_json)
         VALUES (?1, 'metadata_conflict', ?2)",
        rusqlite::params![
            work_id,
            serde_json::to_string(details).unwrap_or_else(|_| "{}".to_string())
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(Some(conn.last_insert_rowid()))
}

/// metadata_conflict の理由を後から確認できるようにする
#[derive(Debug, Clone, Serialize)]
pub struct MetadataConflictDetails {
    pub kind: &'static str,
    pub embedded_year: Option<i32>,
    pub matched_year: Option<i32>,
    pub embedded_title: Option<String>,
    pub matched_title: Option<String>,
    pub tmdb_id: Option<i64>,
    pub media_type: Option<String>,
    pub match_source: Option<String>,
    pub tags_provenance: Option<String>,
    pub tags_provider_hint: Option<String>,
}

/// 人が確定した正解
pub struct LabelWrite<'a> {
    pub work_id: i64,
    /// 判断したときに見ていた run
    pub run_id: Option<i64>,
    /// 確定した TMDB 作品。None は「該当なし」
    pub tmdb: Option<(i64, &'a str)>,
    pub method: LabelMethod,
    pub note: Option<&'a str>,
}

/// ラベルを1件書く。既存の active ラベルは superseded にし、
/// 未解決のレビュー課題があれば同じトランザクションで解決する。
/// 固定（lock）は弱い証拠なので、すでに active ラベルがあるときは何もしない。
pub fn record_label(conn: &Connection, write: &LabelWrite) -> Result<i64, String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let active: Option<i64> = tx
        .query_row(
            "SELECT id FROM metadata_match_labels WHERE work_id = ?1 AND superseded_at IS NULL",
            rusqlite::params![write.work_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if write.method == LabelMethod::Lock {
        if let Some(existing) = active {
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(existing);
        }
    }
    if active.is_some() {
        tx.execute(
            "UPDATE metadata_match_labels
             SET superseded_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE work_id = ?1 AND superseded_at IS NULL",
            rusqlite::params![write.work_id],
        )
        .map_err(|e| e.to_string())?;
    }

    let open_task: Option<i64> = tx
        .query_row(
            "SELECT id FROM metadata_review_tasks
             WHERE work_id = ?1 AND resolved_at IS NULL ORDER BY id LIMIT 1",
            rusqlite::params![write.work_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    tx.execute(
        "INSERT INTO metadata_match_labels
           (work_id, run_id, review_task_id, label, tmdb_id, media_type, method, strength, note)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            write.work_id,
            write.run_id,
            open_task,
            if write.tmdb.is_some() { "tmdb" } else { "none" },
            write.tmdb.map(|(id, _)| id),
            write.tmdb.map(|(_, media_type)| media_type),
            write.method.as_str(),
            write.method.strength(),
            write.note,
        ],
    )
    .map_err(|e| e.to_string())?;
    let label_id = tx.last_insert_rowid();

    tx.execute(
        "UPDATE metadata_review_tasks
         SET resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'), resolution_label_id = ?2
         WHERE work_id = ?1 AND resolved_at IS NULL",
        rusqlite::params![write.work_id, label_id],
    )
    .map_err(|e| e.to_string())?;

    tx.commit().map_err(|e| e.to_string())?;
    Ok(label_id)
}

/// 照合を解除したときに、active ラベルを取り下げる（正解が分からない状態に戻す）
pub fn withdraw_active_label(conn: &Connection, work_id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE metadata_match_labels
         SET superseded_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE work_id = ?1 AND superseded_at IS NULL",
        rusqlite::params![work_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 「この TMDB ID ではない」を記録する
pub fn record_rejection(
    conn: &Connection,
    work_id: i64,
    tmdb_id: i64,
    media_type: &str,
    previous_match_source: Option<&str>,
    source: RejectionSource,
) -> Result<(), String> {
    if media_type != "movie" && media_type != "tv" {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO metadata_match_rejections
           (work_id, tmdb_id, media_type, previous_match_source, source, run_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            work_id,
            tmdb_id,
            media_type,
            previous_match_source,
            source.as_str(),
            latest_run_id(conn, work_id),
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// works.match_source を書き換える（照合解除は None）
pub fn set_match_source(
    conn: &Connection,
    work_id: i64,
    source: Option<MatchSource>,
) -> Result<(), String> {
    conn.execute(
        "UPDATE works SET match_source = ?2 WHERE id = ?1",
        rusqlite::params![work_id, source.map(|s| s.as_str())],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::scan::{insert_scanned_file, insert_scanned_work, ScannedFile};
    use crate::db::test_support::*;
    use crate::services::title_parser::parse_title;

    fn work_with_file(conn: &Connection, title: &str, file_name: &str) -> i64 {
        let root = r"D:\Import";
        let source_id = conn
            .query_row(
                "SELECT id FROM sources WHERE root_path = ?1",
                rusqlite::params![root],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or_else(|_| insert_source(conn, root));
        let work_id = insert_scanned_work(conn, "movie", "unknown", title, "unknown").unwrap();
        let file_id = insert_scanned_file(
            conn,
            &ScannedFile {
                source_id,
                root_path: root,
                file_path: &format!(r"D:\Import\{file_name}"),
                file_name,
                extension: "mkv",
                file_size: None,
                mtime: None,
                duration_sec: None,
                width: None,
                height: None,
                video_codec: None,
                audio_codec: None,
                container: "mkv",
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO work_parts (work_id, file_id) VALUES (?1, ?2)",
            rusqlite::params![work_id, file_id],
        )
        .unwrap();
        work_id
    }

    fn candidate(tmdb_id: i64, title: &str, year: Option<i32>, confidence: i32) -> TmdbCandidate {
        TmdbCandidate {
            tmdb_id,
            media_type: "movie".to_string(),
            title: title.to_string(),
            original_title: None,
            year,
            poster_path: None,
            overview: Some("記録しない".to_string()),
            original_language: None,
            confidence,
            reasons: vec!["title match(60)".to_string()],
        }
    }

    struct Fixture {
        conn: Connection,
        work_id: i64,
        parsed: ParsedTitle,
    }

    fn fixture(file_name: &str) -> Fixture {
        let conn = open_migrated();
        let work_id = work_with_file(&conn, "Alien 1979", file_name);
        let parsed = parse_title(file_name);
        Fixture { conn, work_id, parsed }
    }

    fn run_for<'a>(
        f: &'a Fixture,
        snapshot: &'a PreMatchSnapshot,
        ranked: &'a [TmdbCandidate],
        all: &'a [TmdbCandidate],
        outcome: &'a SafeMatchOutcome<'a>,
        trigger: TriggerKind,
    ) -> RunInput<'a> {
        RunInput {
            work_id: f.work_id,
            batch_id: None,
            trigger_kind: trigger,
            snapshot,
            parsed: &f.parsed,
            queries: &[],
            ranked,
            all,
            rules_safe: outcome,
            rules_one: metadata_matcher::best_candidate(ranked),
            shadow: None,
            candidate_sources: &[],
            tmdb_calls: 1,
            latency_ms: 12,
            error_text: None,
        }
    }

    fn run_row(conn: &Connection, run_id: i64) -> (String, i64, Option<String>, String, String) {
        conn.query_row(
            "SELECT decision, applied, status_after, evidence_class, decision_reasons_json
             FROM metadata_match_runs WHERE id = ?1",
            rusqlite::params![run_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap()
    }

    fn count(conn: &Connection, sql: &str, run_id: i64) -> i64 {
        conn.query_row(sql, rusqlite::params![run_id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn auto_run_records_candidates_and_both_verdicts() {
        let f = fixture("Alien.1979.mkv");
        let snapshot = PreMatchSnapshot::capture(&f.conn, f.work_id).unwrap();
        let ranked = vec![candidate(348, "エイリアン", Some(1979), 95)];
        let outcome = metadata_matcher::rules_safe_best_candidate(&ranked, &f.parsed);
        assert_eq!(outcome.decision, SafeDecision::Auto);

        let run_id = record_run(
            &f.conn,
            &run_for(&f, &snapshot, &ranked, &ranked, &outcome, TriggerKind::Single),
        )
        .unwrap();

        let (decision, applied, status_after, evidence, reasons) = run_row(&f.conn, run_id);
        assert_eq!(decision, "AUTO");
        assert_eq!(applied, 0, "works へ反映できてから applied を立てる");
        assert_eq!(status_after, None);
        assert_eq!(evidence, "live");
        assert_eq!(reasons, "[]");
        assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM metadata_match_candidates WHERE run_id = ?1", run_id), 1);
        assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM metadata_match_verdicts WHERE run_id = ?1", run_id), 2);
        assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM metadata_review_tasks WHERE run_id = ?1", run_id), 0);

        mark_run_applied(
            &f.conn,
            run_id,
            f.work_id,
            348,
            "movie",
            MatchStatus::Matched,
            MatchSource::RulesSafeAuto,
        )
        .unwrap();
        let (_, applied, status_after, _, _) = run_row(&f.conn, run_id);
        assert_eq!(applied, 1);
        assert_eq!(status_after.as_deref(), Some("matched"));
        let (run, source): (Option<i64>, Option<String>) = f
            .conn
            .query_row(
                "SELECT last_match_run_id, match_source FROM works WHERE id = ?1",
                rusqlite::params![f.work_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(run, Some(run_id));
        assert_eq!(source.as_deref(), Some("rules_safe_auto"));
    }

    /// rules-1 は AUTO、rules-safe は REVIEW という食い違いが2行とも残ること
    #[test]
    fn disagreement_between_rules_one_and_rules_safe_is_kept() {
        let f = fixture("Suspiria.mkv"); // 年ヒントなし
        let snapshot = PreMatchSnapshot::capture(&f.conn, f.work_id).unwrap();
        let ranked = vec![candidate(361292, "サスペリア", Some(2018), 80)];
        let outcome = metadata_matcher::rules_safe_best_candidate(&ranked, &f.parsed);
        assert_eq!(outcome.decision, SafeDecision::Review);

        let run_id = record_run(
            &f.conn,
            &run_for(&f, &snapshot, &ranked, &ranked, &outcome, TriggerKind::Batch),
        )
        .unwrap();

        let verdicts: Vec<(String, String, Option<i64>)> = {
            let mut stmt = f
                .conn
                .prepare(
                    "SELECT matcher, decision, tmdb_id FROM metadata_match_verdicts
                     WHERE run_id = ?1 ORDER BY matcher",
                )
                .unwrap();
            stmt.query_map(rusqlite::params![run_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            verdicts,
            vec![
                ("rules-1".to_string(), "AUTO".to_string(), Some(361292)),
                ("rules-safe".to_string(), "REVIEW".to_string(), Some(361292)),
            ]
        );

        // REVIEW は理由付きのレビュー課題になる
        let (reason, resolved): (String, Option<String>) = f
            .conn
            .query_row(
                "SELECT reason, resolved_at FROM metadata_review_tasks WHERE work_id = ?1",
                rusqlite::params![f.work_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(reason, "review_decision");
        assert_eq!(resolved, None);
    }

    /// PR2.5: 影判定と候補の出どころが残ること
    #[test]
    fn shadow_verdict_and_query_sources_are_recorded() {
        let f = fixture("Alien.mkv"); // 年ヒントなし
        let snapshot = PreMatchSnapshot::capture(&f.conn, f.work_id).unwrap();
        let ranked = vec![candidate(348, "エイリアン", Some(1979), 95)];
        let outcome = metadata_matcher::rules_safe_best_candidate(&ranked, &f.parsed);
        let evidence = metadata_matcher::EmbeddedEvidence {
            embedded_year: Some(1979),
            ..Default::default()
        };
        let shadow =
            metadata_matcher::rules_tags_shadow_best_candidate(&ranked, &f.parsed, &evidence);
        assert_eq!(outcome.decision, SafeDecision::Review, "本番はタグで AUTO にしない");
        assert_eq!(shadow.decision, SafeDecision::Auto, "影判定はタグの年を使う");

        let sources = vec![CandidateSource {
            tmdb_id: 348,
            media_type: "movie".to_string(),
            query_source: "both".to_string(),
        }];
        let mut input = run_for(&f, &snapshot, &ranked, &ranked, &outcome, TriggerKind::Single);
        input.shadow = Some(&shadow);
        input.candidate_sources = &sources;
        let run_id = record_run(&f.conn, &input).unwrap();

        let verdicts: Vec<(String, String)> = {
            let mut stmt = f
                .conn
                .prepare(
                    "SELECT matcher, decision FROM metadata_match_verdicts
                     WHERE run_id = ?1 ORDER BY matcher",
                )
                .unwrap();
            stmt.query_map(rusqlite::params![run_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            verdicts,
            vec![
                ("rules-1".to_string(), "AUTO".to_string()),
                ("rules-safe".to_string(), "REVIEW".to_string()),
                ("rules-tags-shadow".to_string(), "AUTO".to_string()),
            ]
        );
        // 実際に works へ反映されるのは rules-safe の判定（run の decision）
        assert_eq!(run_row(&f.conn, run_id).0, "REVIEW");

        let query_source: Option<String> = f
            .conn
            .query_row(
                "SELECT query_source FROM metadata_match_candidates WHERE run_id = ?1",
                rusqlite::params![run_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(query_source.as_deref(), Some("both"));
    }

    #[test]
    fn legacy_rescan_opens_a_legacy_pending_task_once() {
        let f = fixture("Unknown.mkv");
        let snapshot = PreMatchSnapshot::capture(&f.conn, f.work_id).unwrap();
        let ranked = vec![candidate(1, "アンノウン", Some(2011), 80)];
        let outcome = metadata_matcher::rules_safe_best_candidate(&ranked, &f.parsed);

        for _ in 0..2 {
            record_run(
                &f.conn,
                &run_for(&f, &snapshot, &ranked, &ranked, &outcome, TriggerKind::LegacyRescan),
            )
            .unwrap();
        }
        assert_eq!(
            count(&f.conn, "SELECT COUNT(*) FROM metadata_review_tasks WHERE work_id = ?1", f.work_id),
            1
        );
        let reason: String = f
            .conn
            .query_row(
                "SELECT reason FROM metadata_review_tasks WHERE work_id = ?1",
                rusqlite::params![f.work_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason, "legacy_pending");
    }

    #[test]
    fn search_errors_are_kept_as_error_runs() {
        let f = fixture("Alien.1979.mkv");
        let snapshot = PreMatchSnapshot::capture(&f.conn, f.work_id).unwrap();
        let outcome = metadata_matcher::rules_safe_best_candidate(&[], &f.parsed);
        let mut input = run_for(&f, &snapshot, &[], &[], &outcome, TriggerKind::Batch);
        input.error_text = Some("TMDb search failed: 429".to_string());

        let run_id = record_run(&f.conn, &input).unwrap();
        assert_eq!(run_row(&f.conn, run_id).0, "ERROR");
        // ERROR の run はレビュー課題を作らない
        assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM metadata_review_tasks WHERE work_id = ?1", f.work_id), 0);
    }

    #[test]
    fn dropped_candidates_are_kept_up_to_the_limit() {
        let f = fixture("Alien.1979.mkv");
        let snapshot = PreMatchSnapshot::capture(&f.conn, f.work_id).unwrap();
        let all: Vec<TmdbCandidate> = (0..30)
            .map(|i| candidate(1000 + i, &format!("候補{i}"), Some(1979), 90 - i as i32 * 3))
            .collect();
        let ranked = metadata_matcher::merge_and_rank(all.clone());
        let outcome = metadata_matcher::rules_safe_best_candidate(&ranked, &f.parsed);

        let run_id = record_run(
            &f.conn,
            &run_for(&f, &snapshot, &ranked, &all, &outcome, TriggerKind::Batch),
        )
        .unwrap();

        assert_eq!(
            count(&f.conn, "SELECT COUNT(*) FROM metadata_match_candidates WHERE run_id = ?1", run_id),
            MAX_STORED_CANDIDATES as i64
        );
        // 順位の付いた候補には rules_rank、落ちた候補には NULL が入る
        assert_eq!(
            count(
                &f.conn,
                "SELECT COUNT(*) FROM metadata_match_candidates WHERE run_id = ?1 AND rules_rank IS NOT NULL",
                run_id
            ),
            ranked.len() as i64
        );
        let (score, conflicts): (i64, Option<String>) = f
            .conn
            .query_row(
                "SELECT rules_score, explicit_conflicts_json FROM metadata_match_candidates
                 WHERE run_id = ?1 AND cand_key = 'c1'",
                rusqlite::params![run_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(score, 90);
        assert_eq!(conflicts.as_deref(), Some("[]"));
    }

    #[test]
    fn labels_supersede_the_previous_one_and_resolve_open_tasks() {
        let f = fixture("Alien.1979.mkv");
        let task = open_review_task(&f.conn, f.work_id, None, ReviewReason::ReviewDecision).unwrap();

        let first = record_label(
            &f.conn,
            &LabelWrite {
                work_id: f.work_id,
                run_id: None,
                tmdb: Some((348, "movie")),
                method: LabelMethod::ManualApply,
                note: None,
            },
        )
        .unwrap();
        let second = record_label(
            &f.conn,
            &LabelWrite {
                work_id: f.work_id,
                run_id: None,
                tmdb: Some((999, "movie")),
                method: LabelMethod::ManualDirectId,
                note: None,
            },
        )
        .unwrap();

        let active: Vec<(i64, i64, String, String)> = {
            let mut stmt = f
                .conn
                .prepare(
                    "SELECT id, tmdb_id, method, strength FROM metadata_match_labels
                     WHERE work_id = ?1 AND superseded_at IS NULL",
                )
                .unwrap();
            stmt.query_map(rusqlite::params![f.work_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
        };
        assert_eq!(active, vec![(second, 999, "manual_direct_id".to_string(), "strong".to_string())]);
        assert_ne!(first, second);

        let (resolved, resolution): (Option<String>, Option<i64>) = f
            .conn
            .query_row(
                "SELECT resolved_at, resolution_label_id FROM metadata_review_tasks WHERE id = ?1",
                rusqlite::params![task],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(resolved.is_some());
        assert_eq!(resolution, Some(first), "最初のラベルで解決済みになる");
    }

    /// 固定は弱い証拠。すでに人が選んでいれば上書きしない
    #[test]
    fn lock_does_not_overwrite_an_existing_label() {
        let f = fixture("Alien.1979.mkv");
        let manual = record_label(
            &f.conn,
            &LabelWrite {
                work_id: f.work_id,
                run_id: None,
                tmdb: Some((348, "movie")),
                method: LabelMethod::ManualApply,
                note: None,
            },
        )
        .unwrap();
        let locked = record_label(
            &f.conn,
            &LabelWrite {
                work_id: f.work_id,
                run_id: None,
                tmdb: Some((348, "movie")),
                method: LabelMethod::Lock,
                note: None,
            },
        )
        .unwrap();
        assert_eq!(manual, locked);
        assert_eq!(
            count(&f.conn, "SELECT COUNT(*) FROM metadata_match_labels WHERE work_id = ?1", f.work_id),
            1
        );

        // ラベルが無い作品の固定は weak ラベルとして残る
        let other = work_with_file(&f.conn, "B", "B.2000.mkv");
        record_label(
            &f.conn,
            &LabelWrite {
                work_id: other,
                run_id: None,
                tmdb: Some((10, "movie")),
                method: LabelMethod::Lock,
                note: None,
            },
        )
        .unwrap();
        let strength: String = f
            .conn
            .query_row(
                "SELECT strength FROM metadata_match_labels WHERE work_id = ?1",
                rusqlite::params![other],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(strength, "weak");
    }

    #[test]
    fn clearing_records_a_rejection_and_withdraws_the_label() {
        let f = fixture("Alien.1979.mkv");
        record_label(
            &f.conn,
            &LabelWrite {
                work_id: f.work_id,
                run_id: None,
                tmdb: Some((348, "movie")),
                method: LabelMethod::ManualApply,
                note: None,
            },
        )
        .unwrap();
        record_rejection(&f.conn, f.work_id, 348, "movie", Some("rules_safe_auto"), RejectionSource::Clear)
            .unwrap();
        withdraw_active_label(&f.conn, f.work_id).unwrap();
        set_match_source(&f.conn, f.work_id, None).unwrap();

        let (tmdb_id, source, previous): (i64, String, Option<String>) = f
            .conn
            .query_row(
                "SELECT tmdb_id, source, previous_match_source FROM metadata_match_rejections
                 WHERE work_id = ?1",
                rusqlite::params![f.work_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((tmdb_id, source.as_str(), previous.as_deref()), (348, "clear", Some("rules_safe_auto")));
        assert_eq!(
            count(
                &f.conn,
                "SELECT COUNT(*) FROM metadata_match_labels WHERE work_id = ?1 AND superseded_at IS NULL",
                f.work_id
            ),
            0
        );
    }

    /// DB 側の歯止め（CHECK と部分 UNIQUE）が効いていること
    #[test]
    fn database_rejects_inconsistent_history_rows() {
        let f = fixture("Alien.1979.mkv");

        // 該当なしラベルに tmdb_id は入れられない
        assert!(f
            .conn
            .execute(
                "INSERT INTO metadata_match_labels (work_id, label, tmdb_id, media_type, method, strength)
                 VALUES (?1, 'none', 348, 'movie', 'manual_apply', 'strong')",
                rusqlite::params![f.work_id],
            )
            .is_err());
        // lock は strong にできない
        assert!(f
            .conn
            .execute(
                "INSERT INTO metadata_match_labels (work_id, label, tmdb_id, media_type, method, strength)
                 VALUES (?1, 'tmdb', 348, 'movie', 'lock', 'strong')",
                rusqlite::params![f.work_id],
            )
            .is_err());
        // active ラベルは作品につき1件
        let insert_active = "INSERT INTO metadata_match_labels (work_id, label, tmdb_id, media_type, method, strength)
             VALUES (?1, 'tmdb', 348, 'movie', 'manual_apply', 'strong')";
        f.conn.execute(insert_active, rusqlite::params![f.work_id]).unwrap();
        assert!(f.conn.execute(insert_active, rusqlite::params![f.work_id]).is_err());

        // 未解決のレビュー課題は理由ごとに1件
        let insert_task = "INSERT INTO metadata_review_tasks (work_id, reason) VALUES (?1, 'review_decision')";
        f.conn.execute(insert_task, rusqlite::params![f.work_id]).unwrap();
        assert!(f.conn.execute(insert_task, rusqlite::params![f.work_id]).is_err());

        // audit_sample は抽出条件が必須
        assert!(f
            .conn
            .execute(
                "INSERT INTO metadata_review_tasks (work_id, reason) VALUES (?1, 'audit_sample')",
                rusqlite::params![f.work_id],
            )
            .is_err());
    }

    /// replay の run は works へ反映できない
    #[test]
    fn replay_runs_can_never_be_applied() {
        let f = fixture("Alien.1979.mkv");
        let snapshot = PreMatchSnapshot::capture(&f.conn, f.work_id).unwrap();
        let ranked = vec![candidate(348, "エイリアン", Some(1979), 95)];
        let outcome = metadata_matcher::rules_safe_best_candidate(&ranked, &f.parsed);
        let run_id = record_run(
            &f.conn,
            &run_for(&f, &snapshot, &ranked, &ranked, &outcome, TriggerKind::Replay),
        )
        .unwrap();

        assert!(mark_run_applied(
            &f.conn,
            run_id,
            f.work_id,
            348,
            "movie",
            MatchStatus::Matched,
            MatchSource::RulesSafeAuto,
        )
        .is_err());
        assert_eq!(run_row(&f.conn, run_id).1, 0);
    }
}
