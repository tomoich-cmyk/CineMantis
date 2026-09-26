//! ground truth 用の run 生成（PR3 C5c.2）。
//!
//! 凍結済みの `audit_sample` 課題を 1 件ずつ処理し、**候補と判定を記録するところで止める**。
//!
//! ```text
//! 凍結済み audit_sample 課題
//!   → PreMatchSnapshot::capture
//!   → 抽出時の cohort と一致するか確認
//!   → 候補検索（production と同じ経路）
//!   → rules-1 / rules-safe / rules-tags-shadow
//!   → safe run + 候補 + 判定を保存
//!   → 課題を run へ繋いで state='ready'
//!   → STOP
//! ```
//!
//! # 触らないもの
//!
//! `works` / `files` / `match_status` / poster / TMDB detail / credits / persons /
//! series / `run.applied` / production の review 課題 / Jev。
//! production の `match_once` はここから呼ばない（AUTO で works を書き換えてしまうため）。
//!
//! # 抽出との関係
//!
//! 対象集合は C5c.2 の抽出時点で DB に凍結されている。生成が途中で失敗しても、
//! **同じ集合を再開するだけで、別の work に差し替えない**。成功件数に合わせて
//! 後から補充もしない。

use std::future::Future;
use std::pin::Pin;

use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::Connection;
use serde_json::Value;

use crate::commands::tmdb::CandidateSearch;
use crate::services::gt_sampling::{self, SampleManifest, STATE_SELECTED};
use crate::services::match_history::{self, RunInput, TriggerKind};
use crate::services::metadata_matcher::{
    rules_safe_with_embedded, rules_tags_shadow_best_candidate,
};
use crate::services::prematch_snapshot::{LocalEvidence, PreMatchSnapshot};
use crate::services::tmdb_client::{TmdbClient, MAX_HTTP_ATTEMPTS};

/// 生成が終わった課題の状態
pub const STATE_READY: &str = "ready";
/// 抽出時と証拠の種別が変わってしまった
pub const STATE_STALE: &str = "stale";
/// 候補検索などに失敗した（同じ work を後でやり直せる）
pub const STATE_GENERATION_ERROR: &str = "generation_error";

/// 候補検索の差し替え口。
///
/// production は TMDB を叩く実装を渡す。テストは通信せずに用意した結果を返す。
/// **テストのために production の検索そのものを変えない。**
pub trait CandidateProvider {
    fn search<'a>(
        &'a self,
        evidence: &'a LocalEvidence,
    ) -> Pin<Box<dyn Future<Output = Result<CandidateSearch, String>> + 'a>>;
}

/// production で使う候補検索。TMDB を実際に叩く。
///
/// production の `match_once` と同じ [`crate::commands::tmdb::search_candidates`] を
/// 呼ぶだけにしてあるので、「タイトルが無ければ 0 call」といった振る舞いが
/// audit 側だけずれることはない。
pub struct TmdbCandidateProvider<'a> {
    client: &'a TmdbClient,
}

impl<'a> TmdbCandidateProvider<'a> {
    pub fn new(client: &'a TmdbClient) -> Self {
        TmdbCandidateProvider { client }
    }
}

impl CandidateProvider for TmdbCandidateProvider<'_> {
    fn search<'b>(
        &'b self,
        evidence: &'b LocalEvidence,
    ) -> Pin<Box<dyn Future<Output = Result<CandidateSearch, String>> + 'b>> {
        Box::pin(crate::commands::tmdb::search_candidates(self.client, evidence))
    }
}

/// 1 回の生成で使ってよい上限。
///
/// `max_outbound_http_attempts` は **実際に送る HTTP の本数**の上限。
/// TMDB クライアントは 1 回の論理的な検索につき最大 [`MAX_HTTP_ATTEMPTS`] 回まで
/// 送り直すので、work を始める前に「論理 call 数 × 再送上限」を予約し、
/// 残りが足りなければ **検索そのものを呼ばない**。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GenerationBudget {
    pub max_works: usize,
    pub max_outbound_http_attempts: usize,
}

impl Default for GenerationBudget {
    fn default() -> Self {
        GenerationBudget { max_works: 50, max_outbound_http_attempts: 600 }
    }
}

/// 生成の結果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GenerationReport {
    pub sample_id: String,
    /// 新しく run を作れた件数
    pub ready: usize,
    /// 抽出時と証拠が変わっていた件数
    pub stale: usize,
    /// 検索などに失敗した件数
    pub failed: usize,
    /// 予算で打ち切った件数
    pub skipped_budget: usize,
    /// 既に run がある（再開時に飛ばした）件数
    pub already_ready: usize,
    /// 実際に記録した論理 TMDB call 数（run に残る値の合計）
    pub logical_tmdb_calls: usize,
    /// 予約した outbound HTTP の上限本数（実際の送信数はこれ以下）
    pub reserved_http_attempts: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuditRunError {
    Sampling(String),
    /// 抽出済みの sample が無い
    SampleNotFound(String),
    Db(String),
}

impl std::fmt::Display for AuditRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditRunError::Sampling(reason) => write!(f, "抽出の読み出しに失敗: {reason}"),
            AuditRunError::SampleNotFound(id) => write!(f, "sample {id} がありません"),
            AuditRunError::Db(reason) => write!(f, "DB エラー: {reason}"),
        }
    }
}

impl std::error::Error for AuditRunError {}

impl From<rusqlite::Error> for AuditRunError {
    fn from(error: rusqlite::Error) -> Self {
        AuditRunError::Db(error.to_string())
    }
}

/// 凍結済み sample について、まだ run の無い課題を処理する。
///
/// **DB のロックを HTTP の待ち時間に握らない。** 受け取るのは `Connection` ではなく
/// `Arc<Mutex<Connection>>`（production の [`crate::db::DbState`] が持っているもの）で、
/// DB を触るたびに lock し、`await` の前に必ず落とす。lock を持ったまま
/// `provider.search(...).await` に入ると、その間 UI も scan も DB を待つことになる。
///
/// `await` を跨いで transaction を開いたままにしないのも同じ理由。
pub async fn generate_for_sample(
    db: &Arc<Mutex<Connection>>,
    provider: &dyn CandidateProvider,
    sample_id: &str,
    budget: GenerationBudget,
) -> Result<GenerationReport, AuditRunError> {
    let manifest: SampleManifest = with_conn(db, |conn| {
        gt_sampling::load_manifest(conn, sample_id)
            .map_err(|error| AuditRunError::Sampling(error.to_string()))
    })??
    .ok_or_else(|| AuditRunError::SampleNotFound(sample_id.to_string()))?;

    let mut report = GenerationReport { sample_id: sample_id.to_string(), ..Default::default() };

    for entry in &manifest.entries {
        // ① ここまでは同期の DB 作業。lock はこのブロックの中だけ
        let prepared = {
            let conn = lock(db)?;

            // 既に run がある課題は触らない（二重に run を作らない）
            if entry.state == STATE_READY || task_has_run(&conn, entry.task_id)? {
                report.already_ready += 1;
                continue;
            }
            if entry.state != STATE_SELECTED && entry.state != STATE_GENERATION_ERROR {
                // stale などはここでは触らない
                continue;
            }
            if report.ready + report.failed >= budget.max_works {
                report.skipped_budget += 1;
                continue;
            }

            // 抽出時の cohort と現在の証拠が一致するか
            let sampling = gt_sampling::task_sampling(&conn, entry.task_id)
                .map_err(|error| AuditRunError::Sampling(error.to_string()))?
                .unwrap_or(Value::Null);
            let expected_cohort = sampling
                .get("cohort")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            let snapshot = match PreMatchSnapshot::capture(&conn, entry.work_id) {
                Ok(snapshot) => snapshot,
                Err(_) => {
                    mark(&conn, entry.task_id, STATE_GENERATION_ERROR, None)?;
                    report.failed += 1;
                    continue;
                }
            };
            if snapshot.evidence_class.as_str() != expected_cohort {
                // 抽出時と違う証拠で作った観測を、同じ cohort の結果として混ぜない
                mark(&conn, entry.task_id, STATE_STALE, Some("evidence_class_changed"))?;
                report.stale += 1;
                continue;
            }
            snapshot
        };

        // ② 送信前に予算を予約する。
        // ここで足りなければ検索を **呼ばない**（送ってから捨てると、
        // ネットワーク上は既に上限を超えている）
        let evidence = prepared.local_evidence();
        let planned = crate::commands::tmdb::planned_logical_calls(&evidence);
        let worst_case = planned.saturating_mul(MAX_HTTP_ATTEMPTS);
        if report.reserved_http_attempts + worst_case > budget.max_outbound_http_attempts {
            report.skipped_budget += 1;
            continue;
        }
        report.reserved_http_attempts += worst_case;

        // ③ 候補検索。**ここでは DB の lock を持っていない**
        let started = std::time::Instant::now();
        let search = provider.search(&evidence).await;

        // ④ 記録。ふたたび lock を取り直す
        let conn = lock(db)?;
        match search {
            Ok(search) => {
                report.logical_tmdb_calls += search.tmdb_calls;

                let embedded = prepared.embedded_evidence();
                let outcome =
                    rules_safe_with_embedded(&search.legacy_ranked, &search.parsed, &embedded);
                let shadow = rules_tags_shadow_best_candidate(
                    &search.combined_ranked,
                    &search.parsed,
                    &embedded,
                );

                let result = match_history::record_audit_run(
                    &conn,
                    &RunInput {
                        work_id: entry.work_id,
                        batch_id: Some(sample_id),
                        // 新しく検索して候補を作るので replay ではない。
                        // audit かどうかは audit_sample 課題と sampling_json で判別する
                        trigger_kind: TriggerKind::Batch,
                        snapshot: &prepared,
                        parsed: &search.parsed,
                        queries: &search.queries,
                        ranked: &search.combined_ranked,
                        all: &search.combined_all,
                        rules_safe: &outcome,
                        rules_one: search.rules_one.clone(),
                        shadow: Some(&shadow),
                        candidate_sources: &search.sources,
                        tmdb_calls: search.tmdb_calls,
                        latency_ms: started.elapsed().as_millis() as u64,
                        error_text: None,
                    },
                    entry.task_id,
                );
                match result {
                    Ok(_) => report.ready += 1,
                    Err(_) => {
                        // 失敗の原因が「別の generator に先を越された」ことなら、
                        // 向こうが繋いだ run を失敗で塗り潰さない
                        if already_finished(&conn, entry.task_id)? {
                            report.already_ready += 1;
                        } else {
                            mark(&conn, entry.task_id, STATE_GENERATION_ERROR, None)?;
                            report.failed += 1;
                        }
                    }
                }
            }
            Err(_) => {
                // 失敗しても sample から外さない。同じ work を後でやり直す
                if already_finished(&conn, entry.task_id)? {
                    report.already_ready += 1;
                } else {
                    mark(&conn, entry.task_id, STATE_GENERATION_ERROR, None)?;
                    report.failed += 1;
                }
            }
        }
    }

    Ok(report)
}

/// DB の lock。毒された lock は握りつぶさずエラーにする
fn lock(db: &Arc<Mutex<Connection>>) -> Result<MutexGuard<'_, Connection>, AuditRunError> {
    db.lock().map_err(|error| AuditRunError::Db(error.to_string()))
}

/// 短い同期作業のための lock。戻ったときには必ず外れている
fn with_conn<T>(
    db: &Arc<Mutex<Connection>>,
    work: impl FnOnce(&Connection) -> T,
) -> Result<T, AuditRunError> {
    let conn = lock(db)?;
    Ok(work(&conn))
}

fn task_has_run(conn: &Connection, task_id: i64) -> Result<bool, AuditRunError> {
    let run_id: Option<i64> = conn.query_row(
        "SELECT run_id FROM metadata_review_tasks WHERE id = ?1",
        rusqlite::params![task_id],
        |row| row.get(0),
    )?;
    Ok(run_id.is_some())
}

/// 未着手（selected / generation_error）の課題だけを次の状態へ進める。
///
/// 既に別の generator が run を繋いで ready にしていたら書き換えない。
/// 戻り値が `false` なら「負けた」ということ。
fn mark(
    conn: &Connection,
    task_id: i64,
    state: &str,
    stale_reason: Option<&str>,
) -> Result<bool, AuditRunError> {
    gt_sampling::set_task_state(
        conn,
        task_id,
        &[STATE_SELECTED, STATE_GENERATION_ERROR],
        state,
        stale_reason,
    )
    .map(|change| change == gt_sampling::TaskStateChange::Updated)
    .map_err(|error| AuditRunError::Sampling(error.to_string()))
}

/// 記録に失敗したあと、実際には別の generator が先に成功していなかったかを見る。
fn already_finished(conn: &Connection, task_id: i64) -> Result<bool, AuditRunError> {
    let progress = gt_sampling::task_progress(conn, task_id)
        .map_err(|error| AuditRunError::Sampling(error.to_string()))?;
    Ok(matches!(progress, Some((Some(_), state)) if state == STATE_READY))
}

// ─── 集計の定義（C5c.4 以降で使う）────────────────────────────────────────
//
// candidate recall@3
//   = GT が top3 に居た件数 / GT が tmdb の全件
//
// diagnostic conditional AUTO precision
//   = GT が top3 に居た部分集合での「正しい AUTO」/ その部分集合の AUTO
//   （candidate 生成の失敗を隠すので、これは診断用であって promotion には使わない）
//
// FINAL end-to-end AUTO precision
//   = holdout 全体での「正しい AUTO」/ **すべての AUTO**
//   candidate set の外に正解があるのに別候補を AUTO した場合も、誤り 1 件として
//   分子から外し分母には残す。promotion の判断に使うのはこれ。
//
// AUTO coverage    = AUTO / holdout 全件
// abstention rate  = (REVIEW + UNRESOLVED) / holdout 全件
//
// 「Jev 正解 / GT 総数」を precision と呼ばない。

/// 他モジュールのテストからも使う指紋（production 側が動いていないことの確認用）
#[cfg(test)]
pub(crate) mod tests_support {
    use rusqlite::Connection;

    /// production 側のテーブルをまとめて文字列化する。
    /// audit 経路の前後でこれが 1 文字でも変われば、どこかを書いている。
    pub fn production_fingerprint(conn: &Connection) -> String {
        let mut parts = Vec::new();
        for sql in [
            "SELECT id || ':' || title || ':' || COALESCE(original_title,'') || ':' ||
                    COALESCE(year,'') || ':' || COALESCE(tmdb_id,'') || ':' ||
                    COALESCE(imdb_id,'') || ':' || COALESCE(match_status,'') || ':' ||
                    COALESCE(match_confidence,'') || ':' || COALESCE(match_source,'') || ':' ||
                    COALESCE(poster_path,'') || ':' || COALESCE(thumb_path,'') || ':' ||
                    COALESCE(last_match_run_id,'')
               FROM works ORDER BY id",
            "SELECT id || ':' || file_name || ':' || file_path || ':' ||
                    COALESCE(renamed_by_app,'') || ':' || COALESCE(original_file_name,'')
               FROM files ORDER BY id",
            "SELECT id || ':' || COALESCE(name,'') FROM persons ORDER BY id",
            "SELECT work_id || ':' || person_id FROM work_persons ORDER BY work_id, person_id",
            "SELECT id || ':' || COALESCE(title,'') FROM series ORDER BY id",
            "SELECT series_id || ':' || work_id FROM series_items ORDER BY series_id, work_id",
        ] {
            let mut stmt = conn.prepare(sql).unwrap();
            let rows: Vec<String> = stmt
                .query_map([], |row| row.get::<_, Option<String>>(0))
                .unwrap()
                .map(|value| value.unwrap().unwrap_or_default())
                .collect();
            parts.push(rows.join("|"));
        }
        parts.join("#")
    }
}

// ─── テスト ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::tests_support::production_fingerprint;
    use super::*;
    use crate::commands::scan::insert_scanned_work;
    use crate::db::test_support::*;
    use crate::models::tmdb::TmdbCandidate;
    use crate::services::gt_sampling::{
        freeze_sample, SamplePurpose, SampleRequest, STATE_SELECTED,
    };
    use crate::services::match_history::{CandidateSource, RulesOneVerdict, SearchQuery};
    use crate::services::title_parser::parse_title;
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex, MutexGuard};

    const SHA: &str = "3946af66200053309c45fd45bc57743fd9345b7c";

    // ─── 差し替え可能な候補検索 ──────────────────────────────────────────

    /// 通信しない候補提供。呼ばれた回数を数える
    struct FakeProvider {
        calls: RefCell<usize>,
        fail: bool,
        tmdb_calls: usize,
    }

    impl FakeProvider {
        fn ok() -> Self {
            FakeProvider { calls: RefCell::new(0), fail: false, tmdb_calls: 2 }
        }

        fn failing() -> Self {
            FakeProvider { calls: RefCell::new(0), fail: true, tmdb_calls: 0 }
        }

        fn calls(&self) -> usize {
            *self.calls.borrow()
        }
    }

    fn candidate(tmdb_id: i64, title: &str, confidence: i32) -> TmdbCandidate {
        TmdbCandidate {
            tmdb_id,
            media_type: "movie".to_string(),
            title: title.to_string(),
            original_title: Some(title.to_string()),
            year: Some(2021),
            poster_path: Some("/p.jpg".to_string()),
            overview: None,
            original_language: Some("ja".to_string()),
            confidence,
            reasons: vec!["title match(60)".to_string()],
        }
    }

    impl CandidateProvider for FakeProvider {
        fn search<'a>(
            &'a self,
            evidence: &'a LocalEvidence,
        ) -> Pin<Box<dyn Future<Output = Result<CandidateSearch, String>> + 'a>> {
            *self.calls.borrow_mut() += 1;
            let fail = self.fail;
            let tmdb_calls = self.tmdb_calls;
            let title = evidence.title().to_string();
            Box::pin(async move {
                if fail {
                    return Err("search failed".to_string());
                }
                let ranked = vec![candidate(1001, "候補 A", 80), candidate(1002, "候補 B", 55)];
                Ok(CandidateSearch {
                    legacy_ranked: ranked.clone(),
                    legacy_all: ranked.clone(),
                    combined_ranked: ranked.clone(),
                    combined_all: ranked,
                    sources: vec![CandidateSource {
                        tmdb_id: 1001,
                        media_type: "movie".to_string(),
                        query_source: "legacy".to_string(),
                    }],
                    rules_one: RulesOneVerdict::default(),
                    parsed: parse_title(&title),
                    queries: vec![SearchQuery {
                        kind: "movie",
                        query: title,
                        year: None,
                        source: "legacy",
                    }],
                    tmdb_calls,
                })
            })
        }
    }

    // ─── fixtures ────────────────────────────────────────────────────────

    fn insert_reconstructed(conn: &Connection, index: usize) -> i64 {
        let root = r"D:\Import";
        let source_id = conn
            .query_row(
                "SELECT id FROM sources WHERE root_path = ?1",
                rusqlite::params![root],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or_else(|_| insert_source(conn, root));
        let file_name = format!("audit-{index}.mkv");
        let work_id =
            insert_scanned_work(conn, "movie", "unknown", &format!("作品 {index}"), "unknown")
                .unwrap();
        // 020 以前の行として入れ、backfill に original_* を埋めさせる
        conn.execute(
            "INSERT INTO files (source_id, file_path, file_name, extension, container)
             VALUES (?1, ?2, ?3, 'mkv', 'mkv')",
            rusqlite::params![source_id, format!(r"D:\Import\{file_name}"), file_name],
        )
        .unwrap();
        let file_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO work_parts (work_id, file_id) VALUES (?1, ?2)",
            rusqlite::params![work_id, file_id],
        )
        .unwrap();
        crate::db::backfill_prematch_inputs(conn).unwrap();
        work_id
    }

    fn request(sample_id: &str, count: usize) -> SampleRequest {
        SampleRequest {
            sample_id: sample_id.to_string(),
            cohort: "reconstructed_clean".to_string(),
            purpose: SamplePurpose::Development,
            target_count: count,
            source_git_sha: SHA.to_string(),
            state_schema_version: "cm-prematch-2".to_string(),
            rules_policy_version: "rules-safe-2".to_string(),
        }
    }

    /// production の `DbState` と同じ形で持つ。
    /// generator が lock を await 越しに握らないことを、テストでも同じ型で確かめる
    fn seed(count: usize, sample: usize) -> (Arc<Mutex<Connection>>, SampleManifest) {
        let mut conn = open_migrated();
        for index in 1..=count {
            insert_reconstructed(&conn, index);
        }
        let manifest = freeze_sample(&mut conn, &request("sample-a", sample)).unwrap();
        (Arc::new(Mutex::new(conn)), manifest)
    }

    /// 短い読み書きのための lock
    fn c(db: &Arc<Mutex<Connection>>) -> MutexGuard<'_, Connection> {
        db.lock().unwrap()
    }

    fn task_state(conn: &Connection, task_id: i64) -> String {
        conn.query_row(
            "SELECT json_extract(details_json,'$.state') FROM metadata_review_tasks WHERE id = ?1",
            rusqlite::params![task_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    // ─── 生成 ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn an_audit_run_is_recorded_without_touching_production() {
        let (db, manifest) = seed(10, 3);
        let before = production_fingerprint(&c(&db));
        let provider = FakeProvider::ok();

        let report = generate_for_sample(&db, &provider, "sample-a", GenerationBudget::default())
            .await
            .unwrap();
        assert_eq!(report.ready, 3);
        assert_eq!(report.stale, 0);
        assert_eq!(report.failed, 0);
        assert_eq!(report.logical_tmdb_calls, 6);
        // 論理 2 call × 再送上限 3 × 3 work
        assert_eq!(report.reserved_http_attempts, 18);
        assert_eq!(provider.calls(), 3);

        // production 側は 1 文字も動いていない
        assert_eq!(production_fingerprint(&c(&db)), before, "works / files などが変わっている");

        // run の形
        let runs: Vec<(String, i64, Option<i64>, Option<String>, Option<String>, String, Option<String>, String)> = {
        let guard = c(&db);
        let mut stmt = guard
            .prepare(
                "SELECT mode, applied, applied_tmdb_id, applied_media_type, status_after,
                        trigger_kind, batch_id, evidence_class
                   FROM metadata_match_runs ORDER BY id",
            )
            .unwrap();
            stmt.query_map([], |row| {
                Ok((
                    row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                    row.get(5)?, row.get(6)?, row.get(7)?,
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
        };
        assert_eq!(runs.len(), 3);
        for run in &runs {
            assert_eq!(run.0, "safe");
            assert_eq!(run.1, 0, "applied が立っている");
            assert_eq!(run.2, None);
            assert_eq!(run.3, None);
            assert_eq!(run.4, None);
            assert_eq!(run.5, "batch", "replay を使っている");
            assert_eq!(run.6.as_deref(), Some("sample-a"));
            assert_eq!(run.7, "reconstructed_clean");
        }

        // 候補と判定が残っている
        let (candidates, verdicts): (i64, i64) = c(&db)
            .query_row(
                "SELECT (SELECT COUNT(*) FROM metadata_match_candidates),
                        (SELECT COUNT(*) FROM metadata_match_verdicts)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(candidates, 6, "候補 2 件 × 3 run");
        assert_eq!(verdicts, 9, "rules-1 / rules-safe / rules-tags-shadow × 3 run");

        // 課題が run に繋がって ready
        for entry in &manifest.entries {
            let (run_id, state): (Option<i64>, String) = c(&db)
                .query_row(
                    "SELECT run_id, json_extract(details_json,'$.state')
                       FROM metadata_review_tasks WHERE id = ?1",
                    rusqlite::params![entry.task_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert!(run_id.is_some(), "run に繋がっていない");
            assert_eq!(state, STATE_READY);
        }

        // production の review 課題を作っていない
        let production_tasks: i64 = c(&db)
            .query_row(
                "SELECT COUNT(*) FROM metadata_review_tasks WHERE reason <> 'audit_sample'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(production_tasks, 0, "review_decision 課題を作っている");

        // Jev 側は空のまま
        let (calls, links, shadow): (i64, i64, i64) = c(&db)
            .query_row(
                "SELECT (SELECT COUNT(*) FROM metadata_match_jev_calls),
                        (SELECT COUNT(*) FROM metadata_match_jev_run_links),
                        (SELECT COUNT(*) FROM metadata_match_runs WHERE mode='shadow')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((calls, links, shadow), (0, 0, 0));
    }

    /// production の record_run は従来どおり review 課題を作る（挙動を変えていない）
    #[test]
    fn production_record_run_still_opens_review_tasks() {
        let conn = open_migrated();
        let work_id = insert_reconstructed(&conn, 1);
        let snapshot = PreMatchSnapshot::capture(&conn, work_id).unwrap();
        let ranked = vec![candidate(1001, "候補 A", 70)];
        let parsed = parse_title("audit-1.mkv");
        let outcome =
            crate::services::metadata_matcher::rules_safe_best_candidate(&ranked, &parsed);
        assert_eq!(
            outcome.decision,
            crate::services::metadata_matcher::SafeDecision::Review
        );

        match_history::record_run(
            &conn,
            &RunInput {
                work_id,
                batch_id: None,
                trigger_kind: TriggerKind::Single,
                snapshot: &snapshot,
                parsed: &parsed,
                queries: &[],
                ranked: &ranked,
                all: &ranked,
                rules_safe: &outcome,
                rules_one: RulesOneVerdict::default(),
                shadow: None,
                candidate_sources: &[],
                tmdb_calls: 1,
                latency_ms: 1,
                error_text: None,
            },
        )
        .unwrap();

        let tasks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata_review_tasks WHERE reason = 'review_decision'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tasks, 1, "production の挙動が変わっている");
    }

    // ─── 証拠の変化 ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_changed_evidence_class_stops_before_searching() {
        let (db, manifest) = seed(6, 2);
        // 抽出後に 1 件が historical_audit_only になった
        let target = manifest.entries[0].work_id;
        c(&db).execute(
            "UPDATE files SET renamed_by_app = 'nas_sort'
              WHERE id IN (SELECT file_id FROM work_parts WHERE work_id = ?1)",
            rusqlite::params![target],
        )
        .unwrap();

        let provider = FakeProvider::ok();
        let report =
            generate_for_sample(&db, &provider, "sample-a", GenerationBudget::default())
                .await
                .unwrap();

        assert_eq!(report.stale, 1);
        assert_eq!(report.ready, 1);
        assert_eq!(provider.calls(), 1, "変化した work でも検索している");

        assert_eq!(task_state(&c(&db), manifest.entries[0].task_id), STATE_STALE);
        let (run_id, reason): (Option<i64>, Option<String>) = c(&db)
            .query_row(
                "SELECT run_id, json_extract(details_json,'$.stale_reason')
                   FROM metadata_review_tasks WHERE id = ?1",
                rusqlite::params![manifest.entries[0].task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(run_id, None, "stale なのに run を作っている");
        assert_eq!(reason.as_deref(), Some("evidence_class_changed"));

        // 課題は消さない
        let tasks: i64 = c(&db)
            .query_row(
                "SELECT COUNT(*) FROM metadata_review_tasks WHERE reason = 'audit_sample'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tasks, 2);
    }

    // ─── 再開 ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn resuming_processes_only_unfinished_tasks() {
        let (db, manifest) = seed(10, 3);
        let provider = FakeProvider::ok();
        generate_for_sample(
            &db,
            &provider,
            "sample-a",
            GenerationBudget { max_works: 2, max_outbound_http_attempts: 600 },
        )
            .await
            .unwrap();
        let first_runs: i64 = c(&db)
            .query_row("SELECT COUNT(*) FROM metadata_match_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(first_runs, 2);

        let provider = FakeProvider::ok();
        let report =
            generate_for_sample(&db, &provider, "sample-a", GenerationBudget::default())
                .await
                .unwrap();
        assert_eq!(report.already_ready, 2, "済みの課題を作り直している");
        assert_eq!(report.ready, 1);
        assert_eq!(provider.calls(), 1);

        let runs: i64 = c(&db)
            .query_row("SELECT COUNT(*) FROM metadata_match_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(runs, 3, "二重に run を作っている");
        assert_eq!(manifest.entries.len(), 3, "sample を補充している");
    }

    #[tokio::test]
    async fn the_budget_stops_generation_without_changing_the_sample() {
        let (db, manifest) = seed(10, 4);
        let provider = FakeProvider::ok();
        let report = generate_for_sample(
            &db,
            &provider,
            "sample-a",
            GenerationBudget { max_works: 2, max_outbound_http_attempts: 600 },
        )
        .await
        .unwrap();
        assert_eq!(report.ready, 2);
        assert_eq!(report.skipped_budget, 2);
        assert_eq!(provider.calls(), 2);

        // sample は変わらない
        let tasks: Vec<i64> = {
            let guard = c(&db);
            let mut stmt = guard
                .prepare(
                    "SELECT work_id FROM metadata_review_tasks WHERE reason = 'audit_sample'
                      ORDER BY id",
                )
                .unwrap();
            stmt.query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect()
        };
        assert_eq!(
            tasks,
            manifest.entries.iter().map(|e| e.work_id).collect::<Vec<_>>()
        );
    }

    /// 予算は「送った HTTP の本数」の上限。足りなければ検索を呼ばない
    #[tokio::test]
    async fn the_outbound_budget_stops_before_sending() {
        let (db, _) = seed(10, 3);
        let provider = FakeProvider::ok();
        // 1 work の最悪値は 論理 2 call × 再送 3 = 6 attempts。7 なら 1 件だけ通る
        let report = generate_for_sample(
            &db,
            &provider,
            "sample-a",
            GenerationBudget { max_works: 10, max_outbound_http_attempts: 7 },
        )
        .await
        .unwrap();
        assert_eq!(report.ready, 1, "予約を超えて記録している");
        assert_eq!(report.skipped_budget, 2);
        assert_eq!(provider.calls(), 1, "予算が足りないのに検索を呼んでいる");
        assert!(report.reserved_http_attempts <= 7);
    }

    /// 1 work 分にも足りなければ、検索は 1 回も呼ばれない
    #[tokio::test]
    async fn an_insufficient_budget_calls_the_provider_zero_times() {
        let (db, _) = seed(10, 2);
        let provider = FakeProvider::ok();
        let report = generate_for_sample(
            &db,
            &provider,
            "sample-a",
            GenerationBudget { max_works: 10, max_outbound_http_attempts: 5 },
        )
        .await
        .unwrap();
        assert_eq!(provider.calls(), 0, "送信前に止まっていない");
        assert_eq!(report.ready, 0);
        assert_eq!(report.skipped_budget, 2);
        assert_eq!(report.reserved_http_attempts, 0);

        let runs: i64 = c(&db)
            .query_row("SELECT COUNT(*) FROM metadata_match_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(runs, 0);
    }

    // ─── 失敗 ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_search_failure_does_not_substitute_another_work() {
        let (db, manifest) = seed(10, 2);
        let selected: Vec<i64> = manifest.entries.iter().map(|e| e.work_id).collect();

        let provider = FakeProvider::failing();
        let report =
            generate_for_sample(&db, &provider, "sample-a", GenerationBudget::default())
                .await
                .unwrap();
        assert_eq!(report.failed, 2);
        assert_eq!(report.ready, 0);

        let runs: i64 = c(&db)
            .query_row("SELECT COUNT(*) FROM metadata_match_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(runs, 0);

        for entry in &manifest.entries {
            assert_eq!(task_state(&c(&db), entry.task_id), STATE_GENERATION_ERROR);
        }

        // sample の中身は変わらない（別 work に差し替えない・補充しない）
        let now: Vec<i64> = {
            let guard = c(&db);
            let mut stmt = guard
                .prepare(
                    "SELECT work_id FROM metadata_review_tasks WHERE reason='audit_sample'
                      ORDER BY id",
                )
                .unwrap();
            stmt.query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect()
        };
        assert_eq!(now, selected);

        // やり直せる
        let provider = FakeProvider::ok();
        let report =
            generate_for_sample(&db, &provider, "sample-a", GenerationBudget::default())
                .await
                .unwrap();
        assert_eq!(report.ready, 2);
    }

    // ─── stale の保護 ───────────────────────────────────────────────────

    /// stale と判定された課題を、遅れて戻ってきた generator が ready に戻せない
    #[tokio::test]
    async fn a_stale_task_cannot_be_turned_back_into_ready() {
        let (db, manifest) = seed(6, 1);
        let entry = &manifest.entries[0];
        gt_sampling::set_task_state(
            &c(&db),
            entry.task_id,
            &[STATE_SELECTED],
            STATE_STALE,
            Some("evidence_class_changed"),
        )
        .unwrap();

        let snapshot = PreMatchSnapshot::capture(&c(&db), entry.work_id).unwrap();
        let ranked = vec![candidate(1001, "候補 A", 70)];
        let parsed = parse_title("audit-1.mkv");
        let outcome =
            crate::services::metadata_matcher::rules_safe_best_candidate(&ranked, &parsed);

        let error = match_history::record_audit_run(
            &c(&db),
            &RunInput {
                work_id: entry.work_id,
                batch_id: Some("sample-a"),
                trigger_kind: TriggerKind::Batch,
                snapshot: &snapshot,
                parsed: &parsed,
                queries: &[],
                ranked: &ranked,
                all: &ranked,
                rules_safe: &outcome,
                rules_one: RulesOneVerdict::default(),
                shadow: None,
                candidate_sources: &[],
                tmdb_calls: 1,
                latency_ms: 1,
                error_text: None,
            },
            entry.task_id,
        )
        .unwrap_err();
        assert!(error.contains("繋げません"), "{error}");

        let (run_id, state) = gt_sampling::task_progress(&c(&db), entry.task_id).unwrap().unwrap();
        assert_eq!(run_id, None, "stale な課題に run が付いている");
        assert_eq!(state, STATE_STALE, "stale が ready に戻されている");
        let runs: i64 = c(&db)
            .query_row("SELECT COUNT(*) FROM metadata_match_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(runs, 0, "孤児 run が残っている");
    }

    // ─── DB ロックと通信の分離 ──────────────────────────────────────────

    /// 検索の最中は DB の lock を握っていない。
    ///
    /// provider が呼ばれた瞬間に別スレッドから lock を取り、待たずに取れることを見る。
    /// generator が Connection の guard を await 越しに持っていれば、ここで固まる。
    #[tokio::test]
    async fn the_db_lock_is_free_while_searching() {
        let (db, _) = seed(6, 1);

        struct LockProbe {
            db: Arc<Mutex<Connection>>,
            free: std::sync::atomic::AtomicBool,
        }

        impl CandidateProvider for LockProbe {
            fn search<'a>(
                &'a self,
                _evidence: &'a LocalEvidence,
            ) -> Pin<Box<dyn Future<Output = Result<CandidateSearch, String>> + 'a>> {
                Box::pin(async move {
                    // 検索中に、別スレッドが DB を使えること
                    let db = Arc::clone(&self.db);
                    let taken = std::thread::spawn(move || db.try_lock().is_ok())
                        .join()
                        .unwrap();
                    self.free.store(taken, std::sync::atomic::Ordering::SeqCst);
                    Err("この検索は失敗でよい".to_string())
                })
            }
        }

        let probe = LockProbe {
            db: Arc::clone(&db),
            free: std::sync::atomic::AtomicBool::new(false),
        };
        let report =
            generate_for_sample(&db, &probe, "sample-a", GenerationBudget::default())
                .await
                .unwrap();
        assert_eq!(report.failed, 1);
        assert!(
            probe.free.load(std::sync::atomic::Ordering::SeqCst),
            "検索の間 DB の lock を握っている"
        );
    }

    // ─── production と同じ検索経路 ──────────────────────────────────────

    /// タイトルが無い作品は production も audit も TMDB を呼ばない。
    /// 予算の見積もりも 0 call になる
    #[test]
    fn a_work_without_a_title_plans_no_tmdb_calls() {
        let conn = open_migrated();
        // 題名の手がかりが何も無い作品を、最初からその形で入れる
        // （original_* は捕捉後に書き換えられないので、後から空にはできない）
        let source_id = insert_source(&conn, r"D:\NoTitle");
        let work_id = insert_scanned_work(&conn, "movie", "unknown", "", "unknown").unwrap();
        conn.execute(
            "INSERT INTO files (source_id, file_path, file_name, extension, container)
             VALUES (?1, ?2, '', 'mkv', 'mkv')",
            rusqlite::params![source_id, r"D:\NoTitle\x"],
        )
        .unwrap();
        let file_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO work_parts (work_id, file_id) VALUES (?1, ?2)",
            rusqlite::params![work_id, file_id],
        )
        .unwrap();
        crate::db::backfill_prematch_inputs(&conn).unwrap();
        conn.execute(
            "UPDATE works SET title_guess = NULL WHERE id = ?1",
            rusqlite::params![work_id],
        )
        .unwrap();

        let snapshot = PreMatchSnapshot::capture(&conn, work_id).unwrap();
        let evidence = snapshot.local_evidence();
        assert!(
            crate::commands::tmdb::skips_tmdb(&evidence),
            "production が短絡する条件を満たしていない"
        );
        assert_eq!(
            crate::commands::tmdb::planned_logical_calls(&evidence),
            0,
            "呼ばない検索の分まで予算を取っている"
        );
    }

    // ─── generator 同士の競合 ───────────────────────────────────────────

    /// 2 本目の generator が負けても、勝った側の ready を壊さない。
    ///
    /// 勝った側が run を繋いで ready にしたあと、負けた側は同じ課題で
    /// record_audit_run に失敗する。そこで generation_error を書いてしまうと、
    /// run を持つ課題が「失敗」に見える。最終状態は ready、run は 1 件。
    #[tokio::test]
    async fn a_losing_generator_does_not_overwrite_a_ready_task() {
        let (db, manifest) = seed(6, 1);
        let task_id = manifest.entries[0].task_id;

        // 勝った側
        let winner = FakeProvider::ok();
        let first = generate_for_sample(
            &db,
            &winner,
            "sample-a",
            GenerationBudget { max_works: 10, max_outbound_http_attempts: 600 },
        )
        .await
        .unwrap();
        assert_eq!(first.ready, 1);

        let (run_id, state) = gt_sampling::task_progress(&c(&db), task_id).unwrap().unwrap();
        assert!(run_id.is_some());
        assert_eq!(state, STATE_READY);

        // 負けた側が同じ sample をもう一度たどる。
        // すでに run があるので検索も走らない
        let loser = FakeProvider::failing();
        let second = generate_for_sample(
            &db,
            &loser,
            "sample-a",
            GenerationBudget { max_works: 10, max_outbound_http_attempts: 600 },
        )
        .await
        .unwrap();
        assert_eq!(second.already_ready, 1);
        assert_eq!(second.failed, 0);
        assert_eq!(loser.calls(), 0);

        // 負けた側が error path を通っても ready は壊れない
        assert_eq!(
            gt_sampling::set_task_state(
                &c(&db),
                task_id,
                &[STATE_SELECTED, STATE_GENERATION_ERROR],
                STATE_GENERATION_ERROR,
                None,
            )
            .unwrap(),
            gt_sampling::TaskStateChange::Skipped
        );

        let (final_run, final_state) = gt_sampling::task_progress(&c(&db), task_id).unwrap().unwrap();
        assert_eq!(final_run, run_id, "run が差し替わっている");
        assert_eq!(final_state, STATE_READY, "ready が塗り潰されている");

        let runs: i64 = c(&db)
            .query_row("SELECT COUNT(*) FROM metadata_match_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(runs, 1, "run が二重にできている");
    }

    // ─── 課題と run の取り違え ───────────────────────────────────────────

    /// 別の work の run を課題に繋げない
    #[tokio::test]
    async fn a_run_for_another_work_cannot_be_linked() {
        let (db, manifest) = seed(6, 1);
        let other_work = insert_reconstructed(&c(&db), 99);
        let snapshot = PreMatchSnapshot::capture(&c(&db), other_work).unwrap();
        let ranked = vec![candidate(1001, "候補 A", 70)];
        let parsed = parse_title("audit-99.mkv");
        let outcome =
            crate::services::metadata_matcher::rules_safe_best_candidate(&ranked, &parsed);

        let error = match_history::record_audit_run(
            &c(&db),
            &RunInput {
                work_id: other_work, // 課題の work とは違う
                batch_id: Some("sample-a"),
                trigger_kind: TriggerKind::Batch,
                snapshot: &snapshot,
                parsed: &parsed,
                queries: &[],
                ranked: &ranked,
                all: &ranked,
                rules_safe: &outcome,
                rules_one: RulesOneVerdict::default(),
                shadow: None,
                candidate_sources: &[],
                tmdb_calls: 1,
                latency_ms: 1,
                error_text: None,
            },
            manifest.entries[0].task_id,
        )
        .unwrap_err();
        assert!(error.contains("繋げません"), "{error}");

        let runs: i64 = c(&db)
            .query_row("SELECT COUNT(*) FROM metadata_match_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(runs, 0, "孤児 run が残っている");
    }

    /// batch_id（sample_id）が課題と違えば繋がらない
    #[tokio::test]
    async fn a_run_from_another_sample_cannot_be_linked() {
        let (db, manifest) = seed(6, 1);
        let work_id = manifest.entries[0].work_id;
        let snapshot = PreMatchSnapshot::capture(&c(&db), work_id).unwrap();
        let ranked = vec![candidate(1001, "候補 A", 70)];
        let parsed = parse_title("x.mkv");
        let outcome =
            crate::services::metadata_matcher::rules_safe_best_candidate(&ranked, &parsed);

        let mut input = RunInput {
            work_id,
            batch_id: Some("sample-zzz"), // 別の sample
            trigger_kind: TriggerKind::Batch,
            snapshot: &snapshot,
            parsed: &parsed,
            queries: &[],
            ranked: &ranked,
            all: &ranked,
            rules_safe: &outcome,
            rules_one: RulesOneVerdict::default(),
            shadow: None,
            candidate_sources: &[],
            tmdb_calls: 1,
            latency_ms: 1,
            error_text: None,
        };
        assert!(match_history::record_audit_run(&c(&db), &input, manifest.entries[0].task_id)
            .is_err());

        // trigger_kind が batch でなければ、そもそも受け付けない
        input.batch_id = Some("sample-a");
        input.trigger_kind = TriggerKind::Single;
        assert!(match_history::record_audit_run(&c(&db), &input, manifest.entries[0].task_id)
            .is_err());
        // batch_id なしも拒否
        input.trigger_kind = TriggerKind::Batch;
        input.batch_id = None;
        assert!(match_history::record_audit_run(&c(&db), &input, manifest.entries[0].task_id)
            .is_err());

        let runs: i64 = c(&db)
            .query_row("SELECT COUNT(*) FROM metadata_match_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(runs, 0);
    }

    /// 同じ課題へ 2 回記録しても safe run は 1 件だけ
    #[tokio::test]
    async fn recording_twice_cannot_create_two_runs() {
        let (db, manifest) = seed(6, 1);
        let work_id = manifest.entries[0].work_id;
        let task_id = manifest.entries[0].task_id;
        let snapshot = PreMatchSnapshot::capture(&c(&db), work_id).unwrap();
        let ranked = vec![candidate(1001, "候補 A", 70)];
        let parsed = parse_title("x.mkv");
        let outcome =
            crate::services::metadata_matcher::rules_safe_best_candidate(&ranked, &parsed);
        let input = RunInput {
            work_id,
            batch_id: Some("sample-a"),
            trigger_kind: TriggerKind::Batch,
            snapshot: &snapshot,
            parsed: &parsed,
            queries: &[],
            ranked: &ranked,
            all: &ranked,
            rules_safe: &outcome,
            rules_one: RulesOneVerdict::default(),
            shadow: None,
            candidate_sources: &[],
            tmdb_calls: 1,
            latency_ms: 1,
            error_text: None,
        };

        assert!(match_history::record_audit_run(&c(&db), &input, task_id).is_ok());
        // 2 回目は run_id が埋まっているので繋がらない（run ごと巻き戻る）
        assert!(match_history::record_audit_run(&c(&db), &input, task_id).is_err());

        let runs: i64 = c(&db)
            .query_row("SELECT COUNT(*) FROM metadata_match_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(runs, 1, "二重に run ができている");
        let candidates: i64 = c(&db)
            .query_row("SELECT COUNT(*) FROM metadata_match_candidates", [], |row| row.get(0))
            .unwrap();
        assert_eq!(candidates, 1, "巻き戻っていない候補が残っている");
    }

    #[tokio::test]
    async fn an_unknown_sample_is_an_error() {
        let (db, _) = seed(4, 1);
        let provider = FakeProvider::ok();
        assert_eq!(
            generate_for_sample(&db, &provider, "sample-zzz", GenerationBudget::default())
                .await
                .unwrap_err(),
            AuditRunError::SampleNotFound("sample-zzz".to_string())
        );
        assert_eq!(provider.calls(), 0);
    }
}
