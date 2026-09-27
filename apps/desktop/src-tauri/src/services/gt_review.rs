//! ground truth のレビュー（PR3 C5c.3A）。
//!
//! C5c.2 が作った `audit_sample` 課題を人が確かめ、**強いラベルだけ**を作る。
//!
//! ```text
//! ready な audit_sample 課題
//!   → 人が中身を見る（候補も証拠も task.run_id に凍結されたものだけ）
//!   → confirm / pick_other / none
//!   → label（strong）+ 課題の resolve を 1 トランザクションで
//!   → STOP
//! ```
//!
//! # 触らないもの
//!
//! `works` / `files` / `match_status` / poster / TMDB detail の保存 / credits /
//! persons / series / production の review 課題 / run / candidate / verdict /
//! `sampling_json` / Jev。**production の割り当ては 1 文字も変えない。**
//! 否定の記録（`metadata_match_rejections`）も C5c.3 では書かない。
//!
//! # run の扱い
//!
//! ラベルが指す run は **`metadata_review_tasks.run_id` だけ**。
//! `latest_safe_run_id()` は「その work の最新の safe run」を返すので、
//! 抽出後に別の run が増えていると、人が見た候補と違う run を指してしまう。
//! GT では「人が見たその run」以外に意味が無いので、ここでは使わない。
//! そのうえで、繋がっている run が本当に **その課題の audit run か**を
//! [`validate_bound_run`] で毎回確かめる。
//!
//! # 壊れた記録の扱い
//!
//! sampling / details / snapshot / candidate の JSON は、すべて strict に読む。
//! 既定値で救うと「何を見て決めたか」が言えない GT ができてしまうので、
//! 欠けていても型が違っても [`GtReviewError::CorruptReviewInput`] で止める。

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::services::match_history::{LabelMethod, REVIEW_PROTOCOL_VERSION};

/// 生成が終わって、人が見られる状態
pub const STATE_READY: &str = "ready";
/// 人が「後で」にした状態。ready と行き来する
pub const STATE_DEFERRED: &str = "deferred";
/// ラベルが付いて閉じた状態
pub const STATE_RESOLVED: &str = "resolved";

/// audit run が満たしているべき形（C5c.2 が書いたときの値）
const EXPECTED_RUN_MODE: &str = "safe";
const EXPECTED_RUN_TRIGGER: &str = "batch";

#[derive(Debug, Clone, PartialEq)]
pub enum GtReviewError {
    /// 課題が無い、または audit_sample ではない
    TaskNotFound(i64),
    /// 課題の状態が期待と違う（既に解決済み・deferred・stale など）
    TaskNotReviewable { task_id: i64, reason: String },
    /// 記録が壊れている（既定値で救わない）
    CorruptReviewInput { task_id: i64, reason: String },
    /// 繋がっている run が、その課題の audit run として成立していない
    BoundRunInvalid { task_id: i64, run_id: i64, reason: String },
    /// 抽出後に強いラベルが付いてしまった work
    AlreadyLabelled { work_id: i64 },
    /// 候補がその run のものではない
    CandidateNotInRun { candidate_id: i64, run_id: i64 },
    /// 候補にある TMDB 作品を pick_other で選ぼうとした
    TargetIsCandidate { tmdb_id: i64, media_type: String },
    /// TMDB に存在しない、確認できなかった、または別の作品が返ってきた
    TargetNotVerified { tmdb_id: i64, reason: String },
    InvalidRequest(String),
    Db(String),
}

impl std::fmt::Display for GtReviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GtReviewError::TaskNotFound(id) => write!(f, "audit_sample 課題 {id} がありません"),
            GtReviewError::TaskNotReviewable { task_id, reason } => {
                write!(f, "課題 {task_id} はいまレビューできません: {reason}")
            }
            GtReviewError::CorruptReviewInput { task_id, reason } => {
                write!(f, "課題 {task_id} の記録が壊れています: {reason}")
            }
            GtReviewError::BoundRunInvalid { task_id, run_id, reason } => write!(
                f,
                "課題 {task_id} に繋がった run {run_id} は audit run として使えません: {reason}"
            ),
            GtReviewError::AlreadyLabelled { work_id } => write!(
                f,
                "work {work_id} には抽出後に強いラベルが付いています。GT には使えません"
            ),
            GtReviewError::CandidateNotInRun { candidate_id, run_id } => {
                write!(f, "候補 {candidate_id} は run {run_id} のものではありません")
            }
            GtReviewError::TargetIsCandidate { tmdb_id, media_type } => write!(
                f,
                "{media_type} {tmdb_id} は候補にあります。confirm を使ってください"
            ),
            GtReviewError::TargetNotVerified { tmdb_id, reason } => {
                write!(f, "TMDB {tmdb_id} を確認できません: {reason}")
            }
            GtReviewError::InvalidRequest(reason) => write!(f, "{reason}"),
            GtReviewError::Db(reason) => write!(f, "DB エラー: {reason}"),
        }
    }
}

impl std::error::Error for GtReviewError {}

impl From<rusqlite::Error> for GtReviewError {
    fn from(error: rusqlite::Error) -> Self {
        GtReviewError::Db(error.to_string())
    }
}

// ─── DTO ─────────────────────────────────────────────────────────────────────
//
// **判定器の出力は出さない。** rules-1 / rules-safe / rules-tags-shadow / Jev の
// score も decision も順位も DTO に載せない。人が機械の答えを見てから決めると、
// その GT で測った精度が機械寄りに歪む（anchoring）。
//
// **いまの production の値も出さない。** 見出しもファイル数も run の
// `input_snapshot_json`（抽出・生成時に凍結した証拠）から作る。あとから
// `works.title` が変わっても、人が見る材料は動かない。

/// 一覧の 1 行
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewTaskSummary {
    pub task_id: i64,
    pub work_id: i64,
    pub run_id: i64,
    pub sample_id: String,
    pub sample_rank: i64,
    pub state: String,
    /// run が照合に使った題名（`works.title` ではない）
    pub display_title: String,
    /// run の時点でのファイル数
    pub part_count: i64,
    pub candidate_count: i64,
    pub created_at: String,
}

/// 候補 1 件。順位も score も出さない
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewCandidate {
    pub candidate_id: i64,
    pub tmdb_id: i64,
    pub media_type: String,
    /// run の時点で TMDB から見えていた内容（題名・年・あらすじなど）
    pub tmdb_snapshot: Map<String, Value>,
}

/// 1 件の中身
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewTaskDetail {
    pub task_id: i64,
    pub work_id: i64,
    pub run_id: i64,
    pub sample_id: String,
    pub sample_rank: i64,
    pub state: String,
    pub cohort: String,
    pub display_title: String,
    pub part_count: i64,
    /// run が見ていた手元の証拠（凍結済み）
    pub evidence: Map<String, Value>,
    /// **task.run_id の候補だけ。** 並びは tmdb_id 順（機械の順位を持ち込まない）
    pub candidates: Vec<ReviewCandidate>,
}

/// pick_other で選ぼうとしている TMDB 作品（確認結果）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VerifiedTarget {
    pub tmdb_id: i64,
    pub media_type: String,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
}

/// 状態遷移の結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewStateChange {
    Updated,
    /// 既に別の誰かが動かしていた
    Skipped,
}

// ─── strict な読み取り ───────────────────────────────────────────────────────

fn corrupt(task_id: i64, reason: impl Into<String>) -> GtReviewError {
    GtReviewError::CorruptReviewInput { task_id, reason: reason.into() }
}

fn parse_object(text: &str, task_id: i64, what: &str) -> Result<Map<String, Value>, GtReviewError> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| corrupt(task_id, format!("{what} を読めません: {error}")))?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(corrupt(task_id, format!("{what} が object ではありません"))),
    }
}

fn need_str(map: &Map<String, Value>, key: &str, task_id: i64, what: &str) -> Result<String, GtReviewError> {
    match map.get(key) {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(corrupt(task_id, format!("{what}.{key} が文字列ではありません"))),
        None => Err(corrupt(task_id, format!("{what}.{key} がありません"))),
    }
}

fn need_i64(map: &Map<String, Value>, key: &str, task_id: i64, what: &str) -> Result<i64, GtReviewError> {
    match map.get(key) {
        Some(Value::Number(number)) => number
            .as_i64()
            .ok_or_else(|| corrupt(task_id, format!("{what}.{key} が整数ではありません"))),
        Some(_) => Err(corrupt(task_id, format!("{what}.{key} が数値ではありません"))),
        None => Err(corrupt(task_id, format!("{what}.{key} がありません"))),
    }
}

/// 抽出時に凍結された前提（読むだけ。書き換えない）
#[derive(Debug, Clone, PartialEq)]
struct Sampling {
    sample_id: String,
    sample_rank: i64,
    /// 抽出したときの work。課題行だけ差し替えられていないかを見る
    work_id: i64,
    cohort: String,
    state_schema_version: String,
    rules_policy_version: String,
}

fn parse_sampling(text: &str, task_id: i64) -> Result<Sampling, GtReviewError> {
    let map = parse_object(text, task_id, "sampling_json")?;
    Ok(Sampling {
        sample_id: need_str(&map, "sample_id", task_id, "sampling_json")?,
        sample_rank: need_i64(&map, "sample_rank", task_id, "sampling_json")?,
        work_id: need_i64(&map, "work_id", task_id, "sampling_json")?,
        cohort: need_str(&map, "cohort", task_id, "sampling_json")?,
        state_schema_version: need_str(&map, "state_schema_version", task_id, "sampling_json")?,
        rules_policy_version: need_str(&map, "rules_policy_version", task_id, "sampling_json")?,
    })
}

/// 課題の骨格（内部用）
#[derive(Debug, Clone)]
struct BoundTask {
    task_id: i64,
    work_id: i64,
    run_id: i64,
    state: String,
    sampling: Sampling,
    created_at: String,
}

/// run に凍結された証拠のうち、画面に出す部分
#[derive(Debug, Clone)]
struct FrozenEvidence {
    display_title: String,
    part_count: i64,
    snapshot: Map<String, Value>,
}

// ─── 課題と run の突き合わせ ─────────────────────────────────────────────────

/// 課題を読み、**繋がっている run が本当にその課題の audit run か**まで確かめる。
///
/// GT のラベルは「この run を見てこう決めた」という記録なので、run が
/// 別の work のものだったり、production の適用済み run だったりしたら、
/// ラベルの意味そのものが崩れる。読むたびに確かめて fail closed にする。
fn load_task(conn: &Connection, task_id: i64) -> Result<BoundTask, GtReviewError> {
    let row = conn
        .query_row(
            "SELECT id, work_id, run_id, sampling_json, details_json, resolved_at, created_at
               FROM metadata_review_tasks
              WHERE id = ?1 AND reason = 'audit_sample'",
            rusqlite::params![task_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()?
        .ok_or(GtReviewError::TaskNotFound(task_id))?;
    let (task_id, work_id, run_id, sampling_text, details_text, resolved_at, created_at) = row;

    if resolved_at.is_some() {
        return Err(GtReviewError::TaskNotReviewable {
            task_id,
            reason: "既に解決済みです".to_string(),
        });
    }
    // run が繋がっていない課題（selected / stale / generation_error）は、
    // 人に見せる候補そのものが無い
    let run_id = run_id.ok_or_else(|| GtReviewError::TaskNotReviewable {
        task_id,
        reason: "run がまだ繋がっていません".to_string(),
    })?;

    let sampling_text =
        sampling_text.ok_or_else(|| corrupt(task_id, "sampling_json がありません"))?;
    let sampling = parse_sampling(&sampling_text, task_id)?;

    // 課題の work と、抽出時に記録した work が同じであること。
    // 課題行と run をそろって別の work に付け替えられても、
    // 凍結された sampling_json とは合わなくなる
    if sampling.work_id != work_id {
        return Err(corrupt(
            task_id,
            format!("work が食い違っています（課題 {work_id} / 抽出 {}）", sampling.work_id),
        ));
    }

    let details_text = details_text.ok_or_else(|| corrupt(task_id, "details_json がありません"))?;
    let details = parse_object(&details_text, task_id, "details_json")?;
    let state = need_str(&details, "state", task_id, "details_json")?;

    let task = BoundTask { task_id, work_id, run_id, state, sampling, created_at };
    validate_bound_run(conn, &task)?;
    Ok(task)
}

/// 繋がっている run が audit run の形をしているか。
///
/// C5c.2 の `record_audit_run` が書いた run だけが通る。production の適用済み
/// run や、別の work / 別 sample の run、失敗 run は GT の土台にしない。
fn validate_bound_run(conn: &Connection, task: &BoundTask) -> Result<(), GtReviewError> {
    let row = conn
        .query_row(
            "SELECT work_id, mode, COALESCE(applied,0), status_after, error_text,
                    trigger_kind, batch_id, evidence_class, state_schema_version, policy_version
               FROM metadata_match_runs WHERE id = ?1",
            rusqlite::params![task.run_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            },
        )
        .optional()?;

    let invalid = |reason: String| GtReviewError::BoundRunInvalid {
        task_id: task.task_id,
        run_id: task.run_id,
        reason,
    };
    let (
        run_work_id,
        mode,
        applied,
        status_after,
        error_text,
        trigger_kind,
        batch_id,
        evidence_class,
        state_schema_version,
        policy_version,
    ) = row.ok_or_else(|| invalid("run がありません".to_string()))?;

    if run_work_id != task.work_id {
        return Err(invalid(format!(
            "別の work の run です（課題 {} / run {run_work_id}）",
            task.work_id
        )));
    }
    if mode != EXPECTED_RUN_MODE {
        return Err(invalid(format!("mode = {mode}")));
    }
    if applied != 0 {
        return Err(invalid("production に適用された run です".to_string()));
    }
    if status_after.is_some() {
        return Err(invalid("status_after が入っています".to_string()));
    }
    if error_text.is_some() {
        return Err(invalid("失敗した run です".to_string()));
    }
    if trigger_kind != EXPECTED_RUN_TRIGGER {
        return Err(invalid(format!("trigger_kind = {trigger_kind}")));
    }
    if batch_id.as_deref() != Some(task.sampling.sample_id.as_str()) {
        return Err(invalid(format!(
            "別の sample の run です（{:?}）",
            batch_id.unwrap_or_default()
        )));
    }
    if evidence_class != task.sampling.cohort {
        return Err(invalid(format!(
            "evidence_class = {evidence_class}（抽出時は {}）",
            task.sampling.cohort
        )));
    }
    if state_schema_version != task.sampling.state_schema_version {
        return Err(invalid(format!("state_schema_version = {state_schema_version}")));
    }
    if policy_version != task.sampling.rules_policy_version {
        return Err(invalid(format!("policy_version = {policy_version}")));
    }
    Ok(())
}

/// run に凍結された証拠を strict に読む
fn load_frozen_evidence(conn: &Connection, task: &BoundTask) -> Result<FrozenEvidence, GtReviewError> {
    let text: String = conn
        .query_row(
            "SELECT input_snapshot_json FROM metadata_match_runs WHERE id = ?1",
            rusqlite::params![task.run_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| corrupt(task.task_id, "run の input_snapshot_json がありません"))?;

    let snapshot = parse_object(&text, task.task_id, "input_snapshot_json")?;
    let display_title = need_str(&snapshot, "derived_title", task.task_id, "input_snapshot_json")?;
    let part_count = match snapshot.get("files") {
        Some(Value::Array(files)) => files.len() as i64,
        Some(_) => {
            return Err(corrupt(task.task_id, "input_snapshot_json.files が配列ではありません"))
        }
        None => return Err(corrupt(task.task_id, "input_snapshot_json.files がありません")),
    };
    Ok(FrozenEvidence { display_title, part_count, snapshot })
}

/// その run の候補を strict に読む
fn load_candidates(
    conn: &Connection,
    task: &BoundTask,
) -> Result<Vec<ReviewCandidate>, GtReviewError> {
    let mut stmt = conn.prepare(
        "SELECT id, tmdb_id, media_type, tmdb_snapshot_json
           FROM metadata_match_candidates
          WHERE run_id = ?1
          ORDER BY media_type, tmdb_id",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![task.run_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut candidates = Vec::with_capacity(rows.len());
    for (candidate_id, tmdb_id, media_type, snapshot_text) in rows {
        let snapshot = parse_object(
            &snapshot_text,
            task.task_id,
            &format!("候補 {candidate_id} の tmdb_snapshot_json"),
        )?;
        candidates.push(ReviewCandidate { candidate_id, tmdb_id, media_type, tmdb_snapshot: snapshot });
    }
    Ok(candidates)
}

// ─── 読み出し ────────────────────────────────────────────────────────────────

/// sample の課題を並べる。`states` が空なら全部。
///
/// **1 件でも記録が壊れていたら、その場で止める。** 黙って抜かすと母数が
/// 静かに縮み、あとから「何件のうち何件を測ったのか」が言えなくなる。
///
/// そのため、課題の見つけ方も片側に頼らない。sample の印は 2 か所
/// （課題の `sampling_json.sample_id` と、凍結された run の `batch_id`）に
/// あるので、**どちらかが一致すれば拾う**。片方だけ壊れた課題は、拾った先の
/// [`load_task`] → [`validate_bound_run`] で両者の食い違いとして落ちる。
/// 一方だけを鍵にすると、その一方が壊れた課題は一覧から消えてしまう。
pub fn list_tasks(
    conn: &Connection,
    sample_id: &str,
    states: &[&str],
) -> Result<Vec<ReviewTaskSummary>, GtReviewError> {
    let mut stmt = conn.prepare(
        // LEFT JOIN にするのは、run 行そのものが消えている課題も拾うため。
        // INNER JOIN だと、その課題が一覧から黙って落ちてしまう
        "SELECT t.id
           FROM metadata_review_tasks t
           LEFT JOIN metadata_match_runs r ON r.id = t.run_id
          WHERE t.reason = 'audit_sample'
            AND t.run_id IS NOT NULL
            AND t.resolved_at IS NULL
            AND (
                 json_extract(t.sampling_json,'$.sample_id') = ?1
                 OR r.batch_id = ?1
            )
          ORDER BY json_extract(t.sampling_json,'$.sample_rank'), t.id",
    )?;
    let ids = stmt
        .query_map(rusqlite::params![sample_id], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    let mut summaries = Vec::with_capacity(ids.len());
    for task_id in ids {
        let task = load_task(conn, task_id)?;
        if !states.is_empty() && !states.contains(&task.state.as_str()) {
            continue;
        }
        let evidence = load_frozen_evidence(conn, &task)?;
        // 単純な COUNT では候補の中身を見ない。読める候補だけを数える
        let candidate_count = load_candidates(conn, &task)?.len() as i64;
        summaries.push(ReviewTaskSummary {
            task_id: task.task_id,
            work_id: task.work_id,
            run_id: task.run_id,
            sample_id: task.sampling.sample_id.clone(),
            sample_rank: task.sampling.sample_rank,
            state: task.state.clone(),
            display_title: evidence.display_title,
            part_count: evidence.part_count,
            candidate_count,
            created_at: task.created_at.clone(),
        });
    }
    Ok(summaries)
}

/// 1 件の中身を読む
pub fn get_task(conn: &Connection, task_id: i64) -> Result<ReviewTaskDetail, GtReviewError> {
    let task = load_task(conn, task_id)?;
    let evidence = load_frozen_evidence(conn, &task)?;
    let candidates = load_candidates(conn, &task)?;

    Ok(ReviewTaskDetail {
        task_id: task.task_id,
        work_id: task.work_id,
        run_id: task.run_id,
        sample_id: task.sampling.sample_id,
        sample_rank: task.sampling.sample_rank,
        state: task.state,
        cohort: task.sampling.cohort,
        display_title: evidence.display_title,
        part_count: evidence.part_count,
        evidence: evidence.snapshot,
        candidates,
    })
}

// ─── lifecycle の記録 ────────────────────────────────────────────────────────

/// `details_json` を組み立てる。**何を根拠に閉じたか**まで残す
fn review_details_json(state: &str, resolution_method: Option<&str>, tmdb_verified: bool) -> String {
    let mut object = Map::new();
    object.insert(
        "review_protocol_version".to_string(),
        Value::String(REVIEW_PROTOCOL_VERSION.to_string()),
    );
    object.insert("state".to_string(), Value::String(state.to_string()));
    if let Some(method) = resolution_method {
        object.insert("resolution_method".to_string(), Value::String(method.to_string()));
    }
    if tmdb_verified {
        object.insert("tmdb_verified".to_string(), json!(true));
    }
    Value::Object(object).to_string()
}

// ─── ready ⇄ deferred ────────────────────────────────────────────────────────

/// レビュー用の状態遷移。
///
/// [`crate::services::gt_sampling::set_task_state`] は「まだ run の無い課題」を
/// 動かすためのもので、`run_id IS NULL` を条件にしている。レビューはその逆で、
/// **run が繋がっていて未解決の課題**だけを動かす。条件が正反対なので使い回さない。
pub fn set_review_state(
    conn: &Connection,
    task_id: i64,
    expected: &[&str],
    next: &str,
) -> Result<ReviewStateChange, GtReviewError> {
    if !matches!(next, STATE_READY | STATE_DEFERRED) {
        return Err(GtReviewError::InvalidRequest(format!(
            "{next} はレビューで動かせる状態ではありません"
        )));
    }
    // 課題そのものと、繋がっている run を先に確かめる
    load_task(conn, task_id)?;

    let placeholders =
        (0..expected.len()).map(|index| format!("?{}", index + 3)).collect::<Vec<_>>().join(", ");
    let sql = format!(
        "UPDATE metadata_review_tasks SET details_json = ?2
          WHERE id = ?1
            AND reason = 'audit_sample'
            AND run_id IS NOT NULL
            AND resolved_at IS NULL
            AND resolution_label_id IS NULL
            AND COALESCE(json_extract(details_json,'$.state'),'') IN ({placeholders})"
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> =
        vec![Box::new(task_id), Box::new(review_details_json(next, None, false))];
    for value in expected {
        params.push(Box::new(value.to_string()));
    }
    let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|value| value.as_ref()).collect();
    let changed = conn.execute(&sql, refs.as_slice())?;
    Ok(if changed == 1 { ReviewStateChange::Updated } else { ReviewStateChange::Skipped })
}

/// 「後で」にする
pub fn defer(conn: &Connection, task_id: i64) -> Result<ReviewStateChange, GtReviewError> {
    set_review_state(conn, task_id, &[STATE_READY], STATE_DEFERRED)
}

/// 「後で」から戻す
pub fn resume(conn: &Connection, task_id: i64) -> Result<ReviewStateChange, GtReviewError> {
    set_review_state(conn, task_id, &[STATE_DEFERRED], STATE_READY)
}

// ─── 確定 ────────────────────────────────────────────────────────────────────

/// 人が出した答え
#[derive(Debug, Clone, PartialEq)]
enum Decision {
    /// run の候補から選んだ
    Confirm { candidate_id: i64 },
    /// 候補に無い TMDB 作品を指定した（**存在確認済みのものだけ**）
    PickOther { tmdb_id: i64, media_type: String },
    /// TMDB に該当が無い
    None,
}

/// 候補から選んで確定する。
///
/// `candidate_id` は **その課題の run の候補**でなければならない。
/// tmdb_id / media_type は呼び出し側から受け取らず、DB の候補行から読む
/// （画面が古いまま別の作品を送ってきても、run に無いものは入らない）。
pub fn resolve_confirm(
    conn: &Connection,
    task_id: i64,
    candidate_id: i64,
    note: Option<&str>,
) -> Result<i64, GtReviewError> {
    resolve(conn, task_id, Decision::Confirm { candidate_id }, note)
}

/// 「TMDB に該当なし」で確定する
pub fn resolve_none(
    conn: &Connection,
    task_id: i64,
    note: Option<&str>,
) -> Result<i64, GtReviewError> {
    resolve(conn, task_id, Decision::None, note)
}

/// 候補外の TMDB 作品で確定する（**private**）。
///
/// 公開しているのは [`resolve_pick_other_verified`] だけ。ここを公開すると
/// [`VerifiedTarget`] を手で組み立てて、存在確認を通さずに強いラベルを
/// 書けてしまう。確認は迂回できない道の途中に置く。
fn resolve_pick_other(
    conn: &Connection,
    task_id: i64,
    target: &VerifiedTarget,
    note: Option<&str>,
) -> Result<i64, GtReviewError> {
    resolve(
        conn,
        task_id,
        Decision::PickOther { tmdb_id: target.tmdb_id, media_type: target.media_type.clone() },
        note,
    )
}

/// ラベルの作成と課題の解決を **1 トランザクション**で行う。
///
/// production の [`crate::services::match_history::record_label`] は使わない。
/// あちらは production の review lifecycle のためのもので、GT の前提
/// （exact task、強いラベルがあれば拒否、弱いラベルだけ差し替え）と合わない。
/// production の挙動を変えずに済ませるため、GT は自前の経路を持つ。
fn resolve(
    conn: &Connection,
    task_id: i64,
    decision: Decision,
    note: Option<&str>,
) -> Result<i64, GtReviewError> {
    let tx = conn.unchecked_transaction()?;

    // ① 課題と run（トランザクションの中で読み直す）。
    // 確定できるのは ready だけ。「後で」にした課題をそのまま閉じられると、
    // 保留中のものを取り違えて確定してしまう。deferred は resume してから
    let task = load_task(&tx, task_id)?;
    if task.state != STATE_READY {
        return Err(GtReviewError::TaskNotReviewable {
            task_id,
            reason: format!("state = {}（確定できるのは ready だけです）", task.state),
        });
    }

    // ② 人が見たはずの材料が、いまも strict に読めること。
    // 証拠や候補が壊れた状態で強いラベルを作ると、その GT は
    // 「何を見て決めたか」を後から言えない
    load_frozen_evidence(&tx, &task)?;
    load_candidates(&tx, &task)?;

    // ③ 抽出後に付いた強いラベルがあれば、GT にしない。
    // 抽出の母集団は「強いラベルが 1 件も無い work」だけなので、
    // いま強いラベルがあるなら、それは抽出より後に付いたもの
    if has_strong_label(&tx, task.work_id)? {
        return Err(GtReviewError::AlreadyLabelled { work_id: task.work_id });
    }

    // ④ 何を確定するのか
    let (label, tmdb_id, media_type, method) = match &decision {
        Decision::Confirm { candidate_id } => {
            let found: Option<(i64, String)> = tx
                .query_row(
                    "SELECT tmdb_id, media_type FROM metadata_match_candidates
                      WHERE id = ?1 AND run_id = ?2",
                    rusqlite::params![candidate_id, task.run_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let (tmdb_id, media_type) = found.ok_or(GtReviewError::CandidateNotInRun {
                candidate_id: *candidate_id,
                run_id: task.run_id,
            })?;
            ("tmdb", Some(tmdb_id), Some(media_type), LabelMethod::ReviewConfirm)
        }
        Decision::PickOther { tmdb_id, media_type } => {
            check_media_type(media_type)?;
            // 候補にあるものは confirm で選ぶ。pick_other は「候補の外」専用
            if candidate_in_run(&tx, task.run_id, *tmdb_id, media_type)?.is_some() {
                return Err(GtReviewError::TargetIsCandidate {
                    tmdb_id: *tmdb_id,
                    media_type: media_type.clone(),
                });
            }
            ("tmdb", Some(*tmdb_id), Some(media_type.clone()), LabelMethod::ReviewPickOther)
        }
        Decision::None => ("none", None, None, LabelMethod::ReviewNone),
    };

    // ⑤ 弱いラベル（lock など）だけは、同じトランザクションで取り下げる
    tx.execute(
        "UPDATE metadata_match_labels
            SET superseded_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
          WHERE work_id = ?1 AND superseded_at IS NULL AND strength = 'weak'",
        rusqlite::params![task.work_id],
    )?;

    // ⑥ ラベル。run と課題は **その課題のもの**に固定する
    tx.execute(
        "INSERT INTO metadata_match_labels
           (work_id, run_id, review_task_id, label, tmdb_id, media_type, method, strength, note)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'strong', ?8)",
        rusqlite::params![
            task.work_id,
            task.run_id,
            task.task_id,
            label,
            tmdb_id,
            media_type,
            method.as_str(),
            note,
        ],
    )?;
    let label_id = tx.last_insert_rowid();

    // ⑦ 課題を閉じる。ここも exact task に絞り、1 行でなければ全部戻す
    let details = review_details_json(
        STATE_RESOLVED,
        Some(method.as_str()),
        matches!(decision, Decision::PickOther { .. }),
    );
    let changed = tx.execute(
        "UPDATE metadata_review_tasks
            SET resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                resolution_label_id = ?2,
                details_json = ?3
          WHERE id = ?1
            AND reason = 'audit_sample'
            AND run_id = ?4
            AND resolved_at IS NULL
            AND resolution_label_id IS NULL
            AND COALESCE(json_extract(details_json,'$.state'),'') = 'ready'",
        rusqlite::params![task.task_id, label_id, details, task.run_id],
    )?;
    if changed != 1 {
        tx.rollback()?;
        return Err(GtReviewError::TaskNotReviewable {
            task_id,
            reason: "解決の直前に誰かが状態を変えました".to_string(),
        });
    }

    tx.commit()?;
    Ok(label_id)
}

fn has_strong_label(conn: &Connection, work_id: i64) -> Result<bool, GtReviewError> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT id FROM metadata_match_labels
              WHERE work_id = ?1 AND strength = 'strong' ORDER BY id LIMIT 1",
            rusqlite::params![work_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

fn candidate_in_run(
    conn: &Connection,
    run_id: i64,
    tmdb_id: i64,
    media_type: &str,
) -> Result<Option<i64>, GtReviewError> {
    Ok(conn
        .query_row(
            "SELECT id FROM metadata_match_candidates
              WHERE run_id = ?1 AND tmdb_id = ?2 AND media_type = ?3",
            rusqlite::params![run_id, tmdb_id, media_type],
            |row| row.get(0),
        )
        .optional()?)
}

fn check_media_type(media_type: &str) -> Result<(), GtReviewError> {
    if matches!(media_type, "movie" | "tv") {
        Ok(())
    } else {
        Err(GtReviewError::InvalidRequest(format!("media_type が不正です: {media_type}")))
    }
}

// ─── TMDB の存在確認 ─────────────────────────────────────────────────────────

/// 候補外 TMDB ID の存在確認。テストは通信しない実装を渡す
pub trait TmdbTargetVerifier: Send + Sync {
    fn verify<'a>(
        &'a self,
        tmdb_id: i64,
        media_type: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<VerifiedTarget, String>> + Send + 'a>>;
}

/// 候補外の TMDB 作品を確認するだけ（DB は触らない）。
///
/// 画面のプレビューにも、確定前の確認にも同じものを使う。
/// **返ってきた作品が、頼んだ作品と同じであることまで確かめる**
/// （リダイレクトや実装の取り違えで、別の作品を GT にしないため）。
pub async fn preview_target(
    verifier: &dyn TmdbTargetVerifier,
    tmdb_id: i64,
    media_type: &str,
) -> Result<VerifiedTarget, GtReviewError> {
    check_media_type(media_type)?;
    if tmdb_id <= 0 {
        return Err(GtReviewError::InvalidRequest("tmdb_id が不正です".to_string()));
    }
    let target = verifier
        .verify(tmdb_id, media_type)
        .await
        .map_err(|reason| GtReviewError::TargetNotVerified { tmdb_id, reason })?;

    if target.tmdb_id != tmdb_id {
        return Err(GtReviewError::TargetNotVerified {
            tmdb_id,
            reason: format!("別の作品が返りました（{}）", target.tmdb_id),
        });
    }
    if target.media_type != media_type {
        return Err(GtReviewError::TargetNotVerified {
            tmdb_id,
            reason: format!("media_type が違います（{}）", target.media_type),
        });
    }
    Ok(target)
}

/// 候補外 TMDB ID で確定する（存在確認つき）。
///
/// **TMDB を待っている間、DB の lock は持たない。**
/// 手順は 3 段で、真ん中だけが通信:
///
/// 1. lock: 課題・run・証拠・候補・state・強いラベル・候補外 をすべて確かめる
///    （ここで落ちるなら通信しない）
/// 2. lock なし: TMDB detail で存在を確かめる
/// 3. lock: 同じ前提をトランザクションの中で確かめ直してから確定
pub async fn resolve_pick_other_verified(
    db: &Arc<Mutex<Connection>>,
    verifier: &dyn TmdbTargetVerifier,
    task_id: i64,
    tmdb_id: i64,
    media_type: &str,
    note: Option<&str>,
) -> Result<i64, GtReviewError> {
    check_media_type(media_type)?;
    {
        let conn = lock(db)?;
        // 課題と run の突き合わせは load_task の中で行う
        let task = load_task(&conn, task_id)?;
        if task.state != STATE_READY {
            return Err(GtReviewError::TaskNotReviewable {
                task_id,
                reason: format!("state = {}（確定できるのは ready だけです）", task.state),
            });
        }
        // 証拠と候補が読めない課題では、TMDB を呼ぶ前に止める
        load_frozen_evidence(&conn, &task)?;
        load_candidates(&conn, &task)?;
        if has_strong_label(&conn, task.work_id)? {
            return Err(GtReviewError::AlreadyLabelled { work_id: task.work_id });
        }
        if candidate_in_run(&conn, task.run_id, tmdb_id, media_type)?.is_some() {
            return Err(GtReviewError::TargetIsCandidate {
                tmdb_id,
                media_type: media_type.to_string(),
            });
        }
    }

    let target = preview_target(verifier, tmdb_id, media_type).await?;

    let conn = lock(db)?;
    resolve_pick_other(&conn, task_id, &target, note)
}

fn lock(db: &Arc<Mutex<Connection>>) -> Result<MutexGuard<'_, Connection>, GtReviewError> {
    db.lock().map_err(|error| GtReviewError::Db(error.to_string()))
}

// ─── テスト ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::scan::insert_scanned_work;
    use crate::db::test_support::*;
    use crate::services::gt_sampling::{
        self, freeze_sample, SamplePurpose, SampleRequest, STATE_SELECTED,
    };
    use crate::services::match_history::{self, LabelWrite};

    const SHA: &str = "77e02dafea36c41182af5e787454df47a8491cfa";

    // ─── 通信しない確認役 ────────────────────────────────────────────────

    struct FakeVerifier {
        found: bool,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl FakeVerifier {
        fn ok() -> Self {
            FakeVerifier { found: true, calls: std::sync::atomic::AtomicUsize::new(0) }
        }
        fn missing() -> Self {
            FakeVerifier { found: false, calls: std::sync::atomic::AtomicUsize::new(0) }
        }
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl TmdbTargetVerifier for FakeVerifier {
        fn verify<'a>(
            &'a self,
            tmdb_id: i64,
            media_type: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<VerifiedTarget, String>> + Send + 'a>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let media_type = media_type.to_string();
            let found = self.found;
            Box::pin(async move {
                if found {
                    Ok(VerifiedTarget {
                        tmdb_id,
                        media_type,
                        title: format!("TMDB {tmdb_id}"),
                        original_title: None,
                        year: Some("2001-01-01".to_string()),
                        overview: None,
                        poster_path: None,
                    })
                } else {
                    Err("404 Not Found".to_string())
                }
            })
        }
    }

    // ─── 下ごしらえ ──────────────────────────────────────────────────────

    fn insert_work(conn: &Connection, index: usize) -> i64 {
        let root = r"D:\Import";
        let source_id = conn
            .query_row(
                "SELECT id FROM sources WHERE root_path = ?1",
                rusqlite::params![root],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or_else(|_| insert_source(conn, root));
        let file_name = format!("gt-{index}.mkv");
        let work_id =
            insert_scanned_work(conn, "movie", "unknown", &format!("作品 {index}"), "unknown")
                .unwrap();
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

    /// run の入力として凍結する証拠。production と同じ形（PreMatchSnapshot）
    fn frozen_snapshot(conn: &Connection, work_id: i64) -> String {
        let snapshot =
            crate::services::prematch_snapshot::PreMatchSnapshot::capture(conn, work_id).unwrap();
        serde_json::to_string(&snapshot).unwrap()
    }

    /// run を 1 件作って課題に繋ぎ、候補を 2 件入れる（C5c.2 の結果を模す）
    fn attach_run(conn: &Connection, task_id: i64, work_id: i64, tmdb_ids: &[i64]) -> i64 {
        conn.execute(
            "INSERT INTO metadata_match_runs
                 (work_id, batch_id, trigger_kind, mode, evidence_class, decision,
                  state_schema_version, policy_version, input_snapshot_json,
                  search_queries_json, policy_snapshot_json, governing_matcher,
                  decision_reasons_json, error_text)
             VALUES (?1,?3,'batch','safe','reconstructed_clean','REVIEW',
                     'cm-prematch-2','rules-safe-2',
                     ?2,'[]','{}','rules-safe','[]',NULL)",
            rusqlite::params![
                work_id,
                frozen_snapshot(conn, work_id),
                // batch_id は課題に凍結された sample_id と同じでなければならない
                conn.query_row(
                    "SELECT json_extract(sampling_json,'$.sample_id')
                       FROM metadata_review_tasks WHERE id = ?1",
                    rusqlite::params![task_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let run_id = conn.last_insert_rowid();
        for (index, tmdb_id) in tmdb_ids.iter().enumerate() {
            conn.execute(
                "INSERT INTO metadata_match_candidates
                     (run_id, cand_key, tmdb_id, media_type, search_rank, rules_rank,
                      rules_score, rules_reasons_json, tmdb_snapshot_json)
                 VALUES (?1, ?2, ?3, 'movie', ?4, ?4, 70, '[]', ?5)",
                rusqlite::params![
                    run_id,
                    format!("movie:{tmdb_id}"),
                    tmdb_id,
                    index as i64 + 1,
                    format!("{{\"title\":\"候補 {tmdb_id}\"}}"),
                ],
            )
            .unwrap();
        }
        conn.execute(
            "UPDATE metadata_review_tasks SET run_id = ?2, details_json = ?3 WHERE id = ?1",
            rusqlite::params![task_id, run_id, review_details_json(STATE_READY, None, false)],
        )
        .unwrap();
        run_id
    }

    /// ready な課題を 1 件だけ持つ DB
    fn seed() -> (Connection, i64, i64, i64) {
        let mut conn = open_migrated();
        for index in 1..=6 {
            insert_work(&conn, index);
        }
        let manifest = freeze_sample(&mut conn, &request("sample-a", 1)).unwrap();
        let entry = manifest.entries[0].clone();
        let run_id = attach_run(&conn, entry.task_id, entry.work_id, &[1001, 1002]);
        (conn, entry.task_id, entry.work_id, run_id)
    }

    fn candidate_id(conn: &Connection, run_id: i64, tmdb_id: i64) -> i64 {
        conn.query_row(
            "SELECT id FROM metadata_match_candidates WHERE run_id = ?1 AND tmdb_id = ?2",
            rusqlite::params![run_id, tmdb_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn label_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM metadata_match_labels", [], |row| row.get(0))
            .unwrap()
    }

    fn state_of(conn: &Connection, task_id: i64) -> String {
        conn.query_row(
            "SELECT COALESCE(json_extract(details_json,'$.state'),'')
               FROM metadata_review_tasks WHERE id = ?1",
            rusqlite::params![task_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    /// production 側のテーブルの指紋（1 文字でも変われば書いている）
    fn production_fingerprint(conn: &Connection) -> String {
        crate::services::audit_run::tests_support::production_fingerprint(conn)
    }

    /// run / candidate / verdict / sampling_json の指紋
    fn audit_input_fingerprint(conn: &Connection) -> String {
        let mut parts = Vec::new();
        for sql in [
            "SELECT id || ':' || decision || ':' || COALESCE(applied,'') FROM metadata_match_runs
              ORDER BY id",
            "SELECT id || ':' || tmdb_id || ':' || media_type || ':' || rules_score
               FROM metadata_match_candidates ORDER BY id",
            "SELECT id || ':' || matcher || ':' || decision FROM metadata_match_verdicts
              ORDER BY id",
            "SELECT id || ':' || COALESCE(sampling_json,'') FROM metadata_review_tasks ORDER BY id",
        ] {
            let mut stmt = conn.prepare(sql).unwrap();
            let rows: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            parts.push(rows.join("|"));
        }
        parts.join("#")
    }

    // ─── 1. 一覧と詳細 ───────────────────────────────────────────────────

    #[test]
    fn the_list_shows_only_this_sample() {
        let (mut conn, task_id, _, _) = seed();
        // 別 sample も 1 件作る
        let other = freeze_sample(&mut conn, &request("sample-b", 1)).unwrap();
        attach_run(&conn, other.entries[0].task_id, other.entries[0].work_id, &[2001]);

        let rows = list_tasks(&conn, "sample-a", &[]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].task_id, task_id);
        assert_eq!(rows[0].sample_id, "sample-a");
        assert_eq!(rows[0].candidate_count, 2);
        assert_eq!(rows[0].state, STATE_READY);
    }

    #[test]
    fn the_list_can_be_filtered_by_state() {
        let (conn, task_id, _, _) = seed();
        assert_eq!(list_tasks(&conn, "sample-a", &[STATE_READY]).unwrap().len(), 1);
        assert!(list_tasks(&conn, "sample-a", &[STATE_DEFERRED]).unwrap().is_empty());
        defer(&conn, task_id).unwrap();
        assert!(list_tasks(&conn, "sample-a", &[STATE_READY]).unwrap().is_empty());
        assert_eq!(list_tasks(&conn, "sample-a", &[STATE_DEFERRED]).unwrap().len(), 1);
    }

    /// sample の印が片方だけ壊れても、一覧から黙って消えない
    #[test]
    fn a_half_broken_sample_marker_fails_the_list() {
        // ① 課題側の sample_id だけ改変
        let (conn, task_id, _, _) = seed();
        conn.execute(
            "UPDATE metadata_review_tasks
                SET sampling_json = json_set(sampling_json,'$.sample_id','sample-zzz')
              WHERE id = ?1",
            rusqlite::params![task_id],
        )
        .unwrap();
        let error = list_tasks(&conn, "sample-a", &[]).unwrap_err();
        assert!(
            matches!(error, GtReviewError::BoundRunInvalid { .. }),
            "課題が黙って消えている: {error:?}"
        );

        // ② run 側の batch_id だけ改変
        let (conn, _, _, run_id) = seed();
        conn.execute(
            "UPDATE metadata_match_runs SET batch_id = 'sample-zzz' WHERE id = ?1",
            rusqlite::params![run_id],
        )
        .unwrap();
        let error = list_tasks(&conn, "sample-a", &[]).unwrap_err();
        assert!(
            matches!(error, GtReviewError::BoundRunInvalid { .. }),
            "run が黙って消えている: {error:?}"
        );

        // ③ sampling_json の sample_id ごと欠落
        let (conn, task_id, _, _) = seed();
        conn.execute(
            "UPDATE metadata_review_tasks
                SET sampling_json = json_remove(sampling_json,'$.sample_id') WHERE id = ?1",
            rusqlite::params![task_id],
        )
        .unwrap();
        assert!(matches!(
            list_tasks(&conn, "sample-a", &[]),
            Err(GtReviewError::CorruptReviewInput { .. })
        ));
    }

    /// run 行ごと消えていても、一覧から黙って消えない
    #[test]
    fn a_missing_bound_run_fails_the_list() {
        let (conn, task_id, _, run_id) = seed();
        // FK があるので、通常は run を消すと task.run_id が NULL になる。
        // 「run_id は残ったまま run だけ無い」壊れ方を作るために外す
        conn.pragma_update(None, "foreign_keys", false).unwrap();
        conn.execute("DELETE FROM metadata_match_runs WHERE id = ?1", rusqlite::params![run_id])
            .unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        let still_bound: Option<i64> = conn
            .query_row(
                "SELECT run_id FROM metadata_review_tasks WHERE id = ?1",
                rusqlite::params![task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(still_bound, Some(run_id), "前提が作れていない");

        let error = list_tasks(&conn, "sample-a", &[]).unwrap_err();
        assert!(
            matches!(error, GtReviewError::BoundRunInvalid { .. }),
            "run の無い課題が一覧から消えている: {error:?}"
        );
    }

    /// 候補の中身が壊れていたら、一覧の時点で止まる
    #[test]
    fn a_corrupt_candidate_fails_the_list() {
        let (conn, _, _, run_id) = seed();
        conn.execute(
            "UPDATE metadata_match_candidates SET tmdb_snapshot_json = '[]' WHERE run_id = ?1",
            rusqlite::params![run_id],
        )
        .unwrap();
        assert!(matches!(
            list_tasks(&conn, "sample-a", &[]),
            Err(GtReviewError::CorruptReviewInput { .. })
        ));
    }

    /// run が繋がっていない課題（selected / stale）は一覧に出ない
    #[test]
    fn tasks_without_a_run_are_not_listed() {
        let mut conn = open_migrated();
        for index in 1..=6 {
            insert_work(&conn, index);
        }
        let manifest = freeze_sample(&mut conn, &request("sample-a", 2)).unwrap();
        attach_run(&conn, manifest.entries[0].task_id, manifest.entries[0].work_id, &[1001]);

        let rows = list_tasks(&conn, "sample-a", &[]).unwrap();
        assert_eq!(rows.len(), 1, "run の無い課題を見せている");
        assert_eq!(state_of(&conn, manifest.entries[1].task_id), STATE_SELECTED);
        assert!(matches!(
            get_task(&conn, manifest.entries[1].task_id),
            Err(GtReviewError::TaskNotReviewable { .. })
        ));
    }

    /// 詳細に出す候補は task.run_id のものだけ
    #[test]
    fn the_detail_only_shows_candidates_of_the_bound_run() {
        let (conn, task_id, work_id, run_id) = seed();
        // 同じ work に別の run と候補が増えても、見せるのは元の run のまま
        let newer = attach_run_without_task(&conn, work_id, &[9001, 9002, 9003]);
        assert_ne!(newer, run_id);

        let detail = get_task(&conn, task_id).unwrap();
        assert_eq!(detail.run_id, run_id, "新しい run を見せている");
        let ids: Vec<i64> = detail.candidates.iter().map(|c| c.tmdb_id).collect();
        assert_eq!(ids, vec![1001, 1002]);
    }

    /// 課題に紐づかない run を作るだけのヘルパ
    fn attach_run_without_task(conn: &Connection, work_id: i64, tmdb_ids: &[i64]) -> i64 {
        conn.execute(
            "INSERT INTO metadata_match_runs
                 (work_id, trigger_kind, mode, evidence_class, decision,
                  state_schema_version, policy_version, input_snapshot_json,
                  search_queries_json, policy_snapshot_json, governing_matcher,
                  decision_reasons_json)
             VALUES (?1,'single','safe','reconstructed_clean','REVIEW',
                     'cm-prematch-2','rules-safe-2',?2,'[]','{}','rules-safe','[]')",
            rusqlite::params![work_id, frozen_snapshot(conn, work_id)],
        )
        .unwrap();
        let run_id = conn.last_insert_rowid();
        for tmdb_id in tmdb_ids {
            conn.execute(
                "INSERT INTO metadata_match_candidates
                     (run_id, cand_key, tmdb_id, media_type, rules_score,
                      rules_reasons_json, tmdb_snapshot_json)
                 VALUES (?1, ?2, ?3, 'movie', 10, '[]', '{}')",
                rusqlite::params![run_id, format!("movie:{tmdb_id}"), tmdb_id],
            )
            .unwrap();
        }
        run_id
    }

    /// DTO に判定器の出力を混ぜない
    #[test]
    fn the_dto_does_not_leak_matcher_output() {
        let (conn, task_id, _, run_id) = seed();
        conn.execute(
            "INSERT INTO metadata_match_verdicts
                 (run_id, matcher, matcher_version, tmdb_id, media_type, decision, score,
                  reasons_json)
             VALUES (?1,'rules-safe','rules-safe-2',1001,'movie','AUTO',0.97,'[\"x\"]')",
            rusqlite::params![run_id],
        )
        .unwrap();

        let detail = get_task(&conn, task_id).unwrap();
        let json = serde_json::to_string(&detail).unwrap();
        for leaked in ["rules_score", "rules_rank", "decision", "AUTO", "0.97", "verdict", "jev"] {
            assert!(!json.contains(leaked), "DTO に {leaked} が出ている: {json}");
        }
        // 候補そのものは見える
        assert!(json.contains("1001"));
    }

    // ─── 2. ready ⇄ deferred ─────────────────────────────────────────────

    #[test]
    fn a_task_moves_between_ready_and_deferred() {
        let (conn, task_id, _, _) = seed();
        assert_eq!(defer(&conn, task_id).unwrap(), ReviewStateChange::Updated);
        assert_eq!(state_of(&conn, task_id), STATE_DEFERRED);
        assert_eq!(resume(&conn, task_id).unwrap(), ReviewStateChange::Updated);
        assert_eq!(state_of(&conn, task_id), STATE_READY);
    }

    /// 期待と違う状態からは動かさない
    #[test]
    fn a_state_change_needs_the_expected_state() {
        let (conn, task_id, _, _) = seed();
        assert_eq!(resume(&conn, task_id).unwrap(), ReviewStateChange::Skipped);
        assert_eq!(state_of(&conn, task_id), STATE_READY);
        defer(&conn, task_id).unwrap();
        assert_eq!(defer(&conn, task_id).unwrap(), ReviewStateChange::Skipped);
        assert_eq!(state_of(&conn, task_id), STATE_DEFERRED);
    }

    /// 解決済みの課題は動かせない
    #[test]
    fn a_resolved_task_cannot_be_deferred() {
        let (conn, task_id, _, run_id) = seed();
        resolve_confirm(&conn, task_id, candidate_id(&conn, run_id, 1001), None).unwrap();
        assert!(matches!(defer(&conn, task_id), Err(GtReviewError::TaskNotReviewable { .. })));
        assert_eq!(state_of(&conn, task_id), STATE_RESOLVED);
    }

    /// 無い課題は静かに成功させない
    #[test]
    fn an_unknown_task_is_an_error() {
        let (conn, _, _, _) = seed();
        assert_eq!(defer(&conn, 999_999).unwrap_err(), GtReviewError::TaskNotFound(999_999));
        assert_eq!(get_task(&conn, 999_999).unwrap_err(), GtReviewError::TaskNotFound(999_999));
    }

    /// production の review 課題は GT レビューの対象にしない
    #[test]
    fn a_production_review_task_is_not_a_gt_task() {
        let (conn, _, work_id, run_id) = seed();
        conn.execute(
            "INSERT INTO metadata_review_tasks (work_id, run_id, reason, details_json)
             VALUES (?1, ?2, 'review_decision', '{}')",
            rusqlite::params![work_id, run_id],
        )
        .unwrap();
        let production_task = conn.last_insert_rowid();
        assert_eq!(
            defer(&conn, production_task).unwrap_err(),
            GtReviewError::TaskNotFound(production_task)
        );
        assert!(matches!(
            resolve_none(&conn, production_task, None),
            Err(GtReviewError::TaskNotFound(_))
        ));
    }

    // ─── 3. confirm ──────────────────────────────────────────────────────

    #[test]
    fn confirm_writes_a_strong_label_bound_to_the_task_run() {
        let (conn, task_id, work_id, run_id) = seed();
        let before = production_fingerprint(&conn);
        let inputs = audit_input_fingerprint(&conn);

        let label_id =
            resolve_confirm(&conn, task_id, candidate_id(&conn, run_id, 1002), Some("確認")).unwrap();

        let (label, tmdb_id, media_type, method, strength, bound_run, bound_task, note): (
            String,
            Option<i64>,
            Option<String>,
            String,
            String,
            Option<i64>,
            Option<i64>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT label, tmdb_id, media_type, method, strength, run_id, review_task_id, note
                   FROM metadata_match_labels WHERE id = ?1",
                rusqlite::params![label_id],
                |row| {
                    Ok((
                        row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                        row.get(5)?, row.get(6)?, row.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(label, "tmdb");
        assert_eq!(tmdb_id, Some(1002), "画面の言い値ではなく候補行から取る");
        assert_eq!(media_type.as_deref(), Some("movie"));
        assert_eq!(method, "review_confirm");
        assert_eq!(strength, "strong");
        assert_eq!(bound_run, Some(run_id), "task.run_id 以外を指している");
        assert_eq!(bound_task, Some(task_id));
        assert_eq!(note.as_deref(), Some("確認"));

        // 課題は同じトランザクションで閉じている
        let (resolved_at, resolution, state): (Option<String>, Option<i64>, String) = conn
            .query_row(
                "SELECT resolved_at, resolution_label_id,
                        COALESCE(json_extract(details_json,'$.state'),'')
                   FROM metadata_review_tasks WHERE id = ?1",
                rusqlite::params![task_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(resolved_at.is_some());
        assert_eq!(resolution, Some(label_id));
        assert_eq!(state, STATE_RESOLVED);

        // production も、run / candidate / verdict / sampling_json も動かない
        assert_eq!(production_fingerprint(&conn), before, "production を書いている");
        assert_eq!(audit_input_fingerprint(&conn), inputs, "run 側を書いている");
        let work_status: (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT tmdb_id, match_status FROM works WHERE id = ?1",
                rusqlite::params![work_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(work_status.0, None, "works に TMDB を割り当てている");
    }

    /// 別 run の候補は使えない
    #[test]
    fn a_candidate_from_another_run_is_rejected() {
        let (conn, task_id, work_id, run_id) = seed();
        let other_run = attach_run_without_task(&conn, work_id, &[9001]);
        let foreign = candidate_id(&conn, other_run, 9001);

        assert_eq!(
            resolve_confirm(&conn, task_id, foreign, None).unwrap_err(),
            GtReviewError::CandidateNotInRun { candidate_id: foreign, run_id }
        );
        let labels: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_match_labels", [], |row| row.get(0))
            .unwrap();
        assert_eq!(labels, 0);
        assert_eq!(state_of(&conn, task_id), STATE_READY);
    }

    /// 無い候補 id も同じく拒否
    #[test]
    fn an_unknown_candidate_is_rejected() {
        let (conn, task_id, _, _) = seed();
        assert!(matches!(
            resolve_confirm(&conn, task_id, 999_999, None),
            Err(GtReviewError::CandidateNotInRun { .. })
        ));
    }

    /// 「後で」にした課題はそのままでは確定できない
    #[test]
    fn a_deferred_task_cannot_be_resolved() {
        let (conn, task_id, _, run_id) = seed();
        defer(&conn, task_id).unwrap();

        assert!(matches!(
            resolve_confirm(&conn, task_id, candidate_id(&conn, run_id, 1001), None),
            Err(GtReviewError::TaskNotReviewable { .. })
        ));
        assert!(matches!(
            resolve_none(&conn, task_id, None),
            Err(GtReviewError::TaskNotReviewable { .. })
        ));

        let labels: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_match_labels", [], |row| row.get(0))
            .unwrap();
        assert_eq!(labels, 0);
        assert_eq!(state_of(&conn, task_id), STATE_DEFERRED, "拒否したのに状態が動いている");
    }

    /// deferred の pick_other は TMDB を呼ばずに止まる
    #[tokio::test]
    async fn a_deferred_task_does_not_reach_tmdb() {
        let (conn, task_id, _, _) = seed();
        defer(&conn, task_id).unwrap();
        let db = Arc::new(Mutex::new(conn));
        let verifier = FakeVerifier::ok();

        let error = resolve_pick_other_verified(&db, &verifier, task_id, 7777, "movie", None)
            .await
            .unwrap_err();
        assert!(matches!(error, GtReviewError::TaskNotReviewable { .. }));
        assert_eq!(verifier.calls(), 0, "deferred なのに TMDB を呼んでいる");

        let conn = db.lock().unwrap();
        let labels: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_match_labels", [], |row| row.get(0))
            .unwrap();
        assert_eq!(labels, 0);
    }

    /// resume すれば確定できる
    #[test]
    fn a_resumed_task_can_be_resolved() {
        let (conn, task_id, _, run_id) = seed();
        defer(&conn, task_id).unwrap();
        resume(&conn, task_id).unwrap();
        assert_eq!(state_of(&conn, task_id), STATE_READY);
        assert!(resolve_confirm(&conn, task_id, candidate_id(&conn, run_id, 1001), None).is_ok());
        assert_eq!(state_of(&conn, task_id), STATE_RESOLVED);
    }

    /// 二重確定はできない
    #[test]
    fn a_task_cannot_be_resolved_twice() {
        let (conn, task_id, _, run_id) = seed();
        let first = candidate_id(&conn, run_id, 1001);
        let second = candidate_id(&conn, run_id, 1002);
        resolve_confirm(&conn, task_id, first, None).unwrap();

        assert!(matches!(
            resolve_confirm(&conn, task_id, second, None),
            Err(GtReviewError::TaskNotReviewable { .. })
        ));
        let labels: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_match_labels", [], |row| row.get(0))
            .unwrap();
        assert_eq!(labels, 1, "ラベルが二重にできている");
    }

    // ─── 4. none ─────────────────────────────────────────────────────────

    #[test]
    fn none_writes_a_strong_none_label() {
        let (conn, task_id, _, run_id) = seed();
        let label_id = resolve_none(&conn, task_id, None).unwrap();
        let (label, tmdb_id, media_type, method, strength, bound_run): (
            String,
            Option<i64>,
            Option<String>,
            String,
            String,
            Option<i64>,
        ) = conn
            .query_row(
                "SELECT label, tmdb_id, media_type, method, strength, run_id
                   FROM metadata_match_labels WHERE id = ?1",
                rusqlite::params![label_id],
                |row| {
                    Ok((
                        row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(label, "none");
        assert_eq!(tmdb_id, None);
        assert_eq!(media_type, None);
        assert_eq!(method, "review_none");
        assert_eq!(strength, "strong");
        assert_eq!(bound_run, Some(run_id));
        assert_eq!(state_of(&conn, task_id), STATE_RESOLVED);
    }

    // ─── 5. pick_other ───────────────────────────────────────────────────

    #[tokio::test]
    async fn pick_other_needs_the_target_to_exist() {
        let (conn, task_id, _, _) = seed();
        let db = Arc::new(Mutex::new(conn));
        let verifier = FakeVerifier::missing();

        let error =
            resolve_pick_other_verified(&db, &verifier, task_id, 7777, "movie", None)
                .await
                .unwrap_err();
        assert!(matches!(error, GtReviewError::TargetNotVerified { tmdb_id: 7777, .. }));
        assert_eq!(verifier.calls(), 1);

        let conn = db.lock().unwrap();
        let labels: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_match_labels", [], |row| row.get(0))
            .unwrap();
        assert_eq!(labels, 0, "存在しない作品でラベルを作っている");
        assert_eq!(state_of(&conn, task_id), STATE_READY);
    }

    #[tokio::test]
    async fn pick_other_writes_a_strong_label_for_a_verified_target() {
        let (conn, task_id, _, run_id) = seed();
        let db = Arc::new(Mutex::new(conn));
        let verifier = FakeVerifier::ok();

        let label_id = resolve_pick_other_verified(&db, &verifier, task_id, 7777, "tv", None)
            .await
            .unwrap();

        let conn = db.lock().unwrap();
        let (tmdb_id, media_type, method, strength, bound_run): (
            Option<i64>,
            Option<String>,
            String,
            String,
            Option<i64>,
        ) = conn
            .query_row(
                "SELECT tmdb_id, media_type, method, strength, run_id
                   FROM metadata_match_labels WHERE id = ?1",
                rusqlite::params![label_id],
                |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
                },
            )
            .unwrap();
        assert_eq!(tmdb_id, Some(7777));
        assert_eq!(media_type.as_deref(), Some("tv"));
        assert_eq!(method, "review_pick_other");
        assert_eq!(strength, "strong");
        assert_eq!(bound_run, Some(run_id));
    }

    /// 候補にある作品を pick_other では選ばせない（確認の通信もしない）
    #[tokio::test]
    async fn pick_other_refuses_a_candidate_before_calling_tmdb() {
        let (conn, task_id, _, _) = seed();
        let db = Arc::new(Mutex::new(conn));
        let verifier = FakeVerifier::ok();

        let error = resolve_pick_other_verified(&db, &verifier, task_id, 1001, "movie", None)
            .await
            .unwrap_err();
        assert!(matches!(error, GtReviewError::TargetIsCandidate { tmdb_id: 1001, .. }));
        assert_eq!(verifier.calls(), 0, "拒否する相手に TMDB を呼んでいる");
    }

    /// media_type が不正なら通信しない
    #[tokio::test]
    async fn an_invalid_media_type_never_reaches_tmdb() {
        let verifier = FakeVerifier::ok();
        assert!(matches!(
            preview_target(&verifier, 1, "anime").await,
            Err(GtReviewError::InvalidRequest(_))
        ));
        assert!(matches!(
            preview_target(&verifier, 0, "movie").await,
            Err(GtReviewError::InvalidRequest(_))
        ));
        assert_eq!(verifier.calls(), 0);
    }

    /// TMDB を待っている間、DB の lock を持たない
    #[tokio::test]
    async fn the_db_lock_is_free_while_verifying() {
        let (conn, task_id, _, _) = seed();
        let db = Arc::new(Mutex::new(conn));

        struct LockProbe {
            db: Arc<Mutex<Connection>>,
            free: std::sync::atomic::AtomicBool,
        }

        impl TmdbTargetVerifier for LockProbe {
            fn verify<'a>(
                &'a self,
                tmdb_id: i64,
                media_type: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<VerifiedTarget, String>> + Send + 'a>> {
                let media_type = media_type.to_string();
                Box::pin(async move {
                    let db = Arc::clone(&self.db);
                    let taken =
                        std::thread::spawn(move || db.try_lock().is_ok()).join().unwrap();
                    self.free.store(taken, std::sync::atomic::Ordering::SeqCst);
                    Ok(VerifiedTarget {
                        tmdb_id,
                        media_type,
                        title: "t".to_string(),
                        original_title: None,
                        year: None,
                        overview: None,
                        poster_path: None,
                    })
                })
            }
        }

        let probe = LockProbe {
            db: Arc::clone(&db),
            free: std::sync::atomic::AtomicBool::new(false),
        };
        resolve_pick_other_verified(&db, &probe, task_id, 7777, "movie", None)
            .await
            .unwrap();
        assert!(
            probe.free.load(std::sync::atomic::Ordering::SeqCst),
            "TMDB を待つ間 DB の lock を握っている"
        );
    }

    // ─── 5b. 繋がった run の突き合わせ ───────────────────────────────────

    /// run の各項目を壊すと、読むことも確定することもできない
    #[test]
    fn a_bound_run_must_look_like_its_audit_run() {
        // (列, 壊した値) の組。1 件ずつ別の DB で試す
        let breakages: Vec<(&str, &str)> = vec![
            ("mode", "'shadow'"),
            ("applied = 1, applied_tmdb_id = 1, applied_media_type", "'movie'"),
            ("status_after", "'matched'"),
            ("error_text", "'boom'"),
            ("trigger_kind", "'single'"),
            ("batch_id", "'sample-zzz'"),
            ("evidence_class", "'live'"),
            ("state_schema_version", "'cm-prematch-1'"),
            ("policy_version", "'rules-safe-1'"),
        ];
        for (column, value) in breakages {
            let (conn, task_id, _, run_id) = seed();
            conn.execute(
                &format!("UPDATE metadata_match_runs SET {column} = {value} WHERE id = ?1"),
                rusqlite::params![run_id],
            )
            .unwrap();

            assert!(
                matches!(get_task(&conn, task_id), Err(GtReviewError::BoundRunInvalid { .. })),
                "{column} を壊しても読めてしまう"
            );
            assert!(
                matches!(defer(&conn, task_id), Err(GtReviewError::BoundRunInvalid { .. })),
                "{column} を壊しても defer できてしまう"
            );
            assert!(
                matches!(resolve_none(&conn, task_id, None), Err(GtReviewError::BoundRunInvalid { .. })),
                "{column} を壊しても確定できてしまう"
            );
            assert!(
                matches!(list_tasks(&conn, "sample-a", &[]), Err(GtReviewError::BoundRunInvalid { .. })),
                "{column} を壊しても一覧に出る"
            );
            let labels: i64 = conn
                .query_row("SELECT COUNT(*) FROM metadata_match_labels", [], |row| row.get(0))
                .unwrap();
            assert_eq!(labels, 0, "{column}");
        }
    }

    /// 別 work の run が繋がっていたら使わない
    #[test]
    fn a_run_of_another_work_is_rejected() {
        let (conn, task_id, _, run_id) = seed();
        let other_work = insert_work(&conn, 99);
        conn.execute(
            "UPDATE metadata_match_runs SET work_id = ?2 WHERE id = ?1",
            rusqlite::params![run_id, other_work],
        )
        .unwrap();
        assert!(matches!(
            get_task(&conn, task_id),
            Err(GtReviewError::BoundRunInvalid { .. })
        ));
    }

    /// run ごと消えていた場合も fail closed
    #[test]
    fn a_missing_bound_run_is_rejected() {
        let (conn, task_id, _, run_id) = seed();
        conn.execute(
            "UPDATE metadata_review_tasks SET run_id = NULL WHERE id = ?1",
            rusqlite::params![task_id],
        )
        .unwrap();
        conn.execute("DELETE FROM metadata_match_runs WHERE id = ?1", rusqlite::params![run_id])
            .unwrap();
        conn.execute(
            "UPDATE metadata_review_tasks SET run_id = ?2 WHERE id = ?1",
            rusqlite::params![task_id, 999_999],
        )
        .unwrap_err();
        // FK があるので存在しない run は繋げない。繋がりが切れた形でも読めないこと
        assert!(matches!(
            get_task(&conn, task_id),
            Err(GtReviewError::TaskNotReviewable { .. })
        ));
    }

    // ─── 5c. 壊れた記録 ──────────────────────────────────────────────────

    /// sampling / details / snapshot / candidate の JSON は既定値で救わない
    #[test]
    fn corrupt_records_are_never_rescued() {
        // ① sampling_json が壊れている
        let (conn, task_id, _, _) = seed();
        conn.execute(
            "UPDATE metadata_review_tasks
                SET sampling_json = json_remove(sampling_json,'$.cohort') WHERE id = ?1",
            rusqlite::params![task_id],
        )
        .unwrap();
        assert!(matches!(
            get_task(&conn, task_id),
            Err(GtReviewError::CorruptReviewInput { .. })
        ));
        assert!(matches!(
            resolve_none(&conn, task_id, None),
            Err(GtReviewError::CorruptReviewInput { .. })
        ));

        // ② details_json が壊れている
        let (conn, task_id, _, run_id) = seed();
        conn.execute(
            "UPDATE metadata_review_tasks SET details_json = '{ broken' WHERE id = ?1",
            rusqlite::params![task_id],
        )
        .unwrap();
        assert!(matches!(
            get_task(&conn, task_id),
            Err(GtReviewError::CorruptReviewInput { .. })
        ));
        conn.execute(
            "UPDATE metadata_review_tasks SET details_json = '{\"state\":3}' WHERE id = ?1",
            rusqlite::params![task_id],
        )
        .unwrap();
        assert!(matches!(
            defer(&conn, task_id),
            Err(GtReviewError::CorruptReviewInput { .. })
        ));

        // ③ 証拠（input_snapshot_json）が壊れている
        conn.execute(
            "UPDATE metadata_review_tasks SET details_json = ?2 WHERE id = ?1",
            rusqlite::params![task_id, review_details_json(STATE_READY, None, false)],
        )
        .unwrap();
        conn.execute(
            "UPDATE metadata_match_runs SET input_snapshot_json = '[]' WHERE id = ?1",
            rusqlite::params![run_id],
        )
        .unwrap();
        assert!(matches!(
            get_task(&conn, task_id),
            Err(GtReviewError::CorruptReviewInput { .. })
        ));
        assert!(matches!(
            list_tasks(&conn, "sample-a", &[]),
            Err(GtReviewError::CorruptReviewInput { .. })
        ));

        // ④ 候補の snapshot が壊れている
        let (conn, task_id, _, run_id) = seed();
        conn.execute(
            "UPDATE metadata_match_candidates SET tmdb_snapshot_json = 'null' WHERE run_id = ?1",
            rusqlite::params![run_id],
        )
        .unwrap();
        assert!(matches!(
            get_task(&conn, task_id),
            Err(GtReviewError::CorruptReviewInput { .. })
        ));
        // 確定も通さない（候補が読めない状態で強いラベルを作らない）
        let labels: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_match_labels", [], |row| row.get(0))
            .unwrap();
        assert_eq!(labels, 0);
        let _ = task_id;
    }

    /// 壊れた証拠・候補のままでは確定できない（label を 1 件も作らない）
    #[test]
    fn a_corrupt_record_blocks_every_resolution() {
        // ① 証拠が壊れている → none も confirm も通さない
        let (conn, task_id, _, run_id) = seed();
        let candidate = candidate_id(&conn, run_id, 1001);
        conn.execute(
            "UPDATE metadata_match_runs SET input_snapshot_json = '{\"files\":[]}' WHERE id = ?1",
            rusqlite::params![run_id],
        )
        .unwrap();
        assert!(matches!(
            resolve_none(&conn, task_id, None),
            Err(GtReviewError::CorruptReviewInput { .. })
        ));
        assert!(matches!(
            resolve_confirm(&conn, task_id, candidate, None),
            Err(GtReviewError::CorruptReviewInput { .. })
        ));
        assert_eq!(label_count(&conn), 0);
        assert_eq!(state_of(&conn, task_id), STATE_READY);

        // ② 候補の snapshot が壊れている → confirm も none も通さない
        let (conn, task_id, _, run_id) = seed();
        let candidate = candidate_id(&conn, run_id, 1001);
        conn.execute(
            "UPDATE metadata_match_candidates SET tmdb_snapshot_json = '[1,2]'
              WHERE id = ?1",
            rusqlite::params![candidate],
        )
        .unwrap();
        assert!(matches!(
            resolve_confirm(&conn, task_id, candidate, None),
            Err(GtReviewError::CorruptReviewInput { .. })
        ));
        assert!(matches!(
            resolve_none(&conn, task_id, None),
            Err(GtReviewError::CorruptReviewInput { .. })
        ));
        assert_eq!(label_count(&conn), 0);
        assert_eq!(state_of(&conn, task_id), STATE_READY);
    }

    /// 壊れた証拠・候補なら、pick_other は TMDB を呼ばない
    #[tokio::test]
    async fn a_corrupt_record_stops_pick_other_before_tmdb() {
        for breakage in [
            "UPDATE metadata_match_runs SET input_snapshot_json = '\"x\"'
              WHERE id = (SELECT run_id FROM metadata_review_tasks WHERE reason='audit_sample')",
            "UPDATE metadata_match_candidates SET tmdb_snapshot_json = 'null'
              WHERE run_id = (SELECT run_id FROM metadata_review_tasks
                               WHERE reason='audit_sample')",
        ] {
            let (conn, task_id, _, _) = seed();
            conn.execute(breakage, []).unwrap();
            let db = Arc::new(Mutex::new(conn));
            let verifier = FakeVerifier::ok();

            let error = resolve_pick_other_verified(&db, &verifier, task_id, 7777, "movie", None)
                .await
                .unwrap_err();
            assert!(
                matches!(error, GtReviewError::CorruptReviewInput { .. }),
                "壊れた記録で通信している: {error:?}"
            );
            assert_eq!(verifier.calls(), 0);
            assert_eq!(label_count(&db.lock().unwrap()), 0);
        }
    }

    /// 課題と run をそろって別の work に付け替えても、凍結された sampling とは合わない
    #[test]
    fn moving_a_task_and_its_run_to_another_work_is_detected() {
        let (conn, task_id, _, run_id) = seed();
        let other_work = insert_work(&conn, 99);
        conn.execute(
            "UPDATE metadata_match_runs SET work_id = ?2 WHERE id = ?1",
            rusqlite::params![run_id, other_work],
        )
        .unwrap();
        conn.execute(
            "UPDATE metadata_review_tasks SET work_id = ?2 WHERE id = ?1",
            rusqlite::params![task_id, other_work],
        )
        .unwrap();

        // task.work_id == run.work_id だが、sampling_json.work_id は元のまま
        for result in [
            get_task(&conn, task_id).map(|_| ()),
            defer(&conn, task_id).map(|_| ()),
            resolve_none(&conn, task_id, None).map(|_| ()),
        ] {
            assert!(
                matches!(
                    result,
                    Err(GtReviewError::CorruptReviewInput { .. })
                        | Err(GtReviewError::BoundRunInvalid { .. })
                ),
                "付け替えを見逃している: {result:?}"
            );
        }
        assert_eq!(label_count(&conn), 0);
    }

    // ─── 5d. 凍結された証拠だけを見せる ──────────────────────────────────

    /// 後から works / files を変えても、レビューの見え方は変わらない
    #[test]
    fn the_review_shows_frozen_evidence_only() {
        let (conn, task_id, work_id, _) = seed();
        let before_summary = list_tasks(&conn, "sample-a", &[]).unwrap();
        let before_detail = get_task(&conn, task_id).unwrap();

        // production 側で題名を変え、ファイルを 1 つ増やす
        conn.execute(
            "UPDATE works SET title = '変えた題名', title_guess = '変えた題名' WHERE id = ?1",
            rusqlite::params![work_id],
        )
        .unwrap();
        let source_id = conn
            .query_row(
                "SELECT id FROM sources WHERE root_path = ?1",
                rusqlite::params![r"D:\Import"],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO files (source_id, file_path, file_name, extension, container)
             VALUES (?1, ?2, 'extra.mkv', 'mkv', 'mkv')",
            rusqlite::params![source_id, r"D:\Import\extra.mkv"],
        )
        .unwrap();
        let file_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO work_parts (work_id, file_id, part_no) VALUES (?1, ?2, 2)",
            rusqlite::params![work_id, file_id],
        )
        .unwrap();

        let after_summary = list_tasks(&conn, "sample-a", &[]).unwrap();
        let after_detail = get_task(&conn, task_id).unwrap();
        assert_eq!(after_summary, before_summary, "一覧が現在の works を読んでいる");
        assert_eq!(after_detail, before_detail, "詳細が現在の works を読んでいる");
        assert_ne!(after_summary[0].display_title, "変えた題名");
        assert_eq!(after_summary[0].part_count, 1, "現在の work_parts を数えている");
    }

    // ─── 5e. pick_other の迂回不能性 ─────────────────────────────────────

    /// verifier が別の作品を返したら受け付けない
    #[tokio::test]
    async fn a_mismatched_verification_is_rejected() {
        struct Liar {
            tmdb_id: i64,
            media_type: &'static str,
        }
        impl TmdbTargetVerifier for Liar {
            fn verify<'a>(
                &'a self,
                _tmdb_id: i64,
                _media_type: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<VerifiedTarget, String>> + Send + 'a>> {
                let target = VerifiedTarget {
                    tmdb_id: self.tmdb_id,
                    media_type: self.media_type.to_string(),
                    title: "別の作品".to_string(),
                    original_title: None,
                    year: None,
                    overview: None,
                    poster_path: None,
                };
                Box::pin(async move { Ok(target) })
            }
        }

        // 別の tmdb_id
        let (conn, task_id, _, _) = seed();
        let db = Arc::new(Mutex::new(conn));
        let liar = Liar { tmdb_id: 8888, media_type: "movie" };
        let error = resolve_pick_other_verified(&db, &liar, task_id, 7777, "movie", None)
            .await
            .unwrap_err();
        assert!(matches!(error, GtReviewError::TargetNotVerified { tmdb_id: 7777, .. }));

        // 別の media_type
        let liar = Liar { tmdb_id: 7777, media_type: "tv" };
        let error = resolve_pick_other_verified(&db, &liar, task_id, 7777, "movie", None)
            .await
            .unwrap_err();
        assert!(matches!(error, GtReviewError::TargetNotVerified { tmdb_id: 7777, .. }));

        let conn = db.lock().unwrap();
        let labels: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_match_labels", [], |row| row.get(0))
            .unwrap();
        assert_eq!(labels, 0);
    }

    /// 強いラベルが付いた work では、TMDB を呼ぶ前に止まる
    #[tokio::test]
    async fn a_contaminated_task_never_reaches_tmdb() {
        let (conn, task_id, work_id, run_id) = seed();
        conn.execute(
            "INSERT INTO metadata_match_labels
                 (work_id, run_id, label, tmdb_id, media_type, method, strength)
             VALUES (?1, ?2, 'tmdb', 5555, 'movie', 'manual_apply', 'strong')",
            rusqlite::params![work_id, run_id],
        )
        .unwrap();
        let db = Arc::new(Mutex::new(conn));
        let verifier = FakeVerifier::ok();

        let error = resolve_pick_other_verified(&db, &verifier, task_id, 7777, "movie", None)
            .await
            .unwrap_err();
        assert_eq!(error, GtReviewError::AlreadyLabelled { work_id });
        assert_eq!(verifier.calls(), 0, "汚染された課題で TMDB を呼んでいる");
    }

    /// run が壊れていたら、TMDB を呼ぶ前に止まる
    #[tokio::test]
    async fn an_invalid_bound_run_never_reaches_tmdb() {
        let (conn, task_id, _, run_id) = seed();
        conn.execute(
            "UPDATE metadata_match_runs SET applied = 1, applied_tmdb_id = 1,
                    applied_media_type = 'movie' WHERE id = ?1",
            rusqlite::params![run_id],
        )
        .unwrap();
        let db = Arc::new(Mutex::new(conn));
        let verifier = FakeVerifier::ok();

        let error = resolve_pick_other_verified(&db, &verifier, task_id, 7777, "movie", None)
            .await
            .unwrap_err();
        assert!(matches!(error, GtReviewError::BoundRunInvalid { .. }));
        assert_eq!(verifier.calls(), 0);
    }

    // ─── 5f. lifecycle の記録 ────────────────────────────────────────────

    /// 閉じた課題には「どう決めたか」が残る
    #[test]
    fn a_resolved_task_records_how_it_was_decided() {
        let (conn, task_id, _, run_id) = seed();
        resolve_confirm(&conn, task_id, candidate_id(&conn, run_id, 1001), None).unwrap();
        let details: String = conn
            .query_row(
                "SELECT details_json FROM metadata_review_tasks WHERE id = ?1",
                rusqlite::params![task_id],
                |row| row.get(0),
            )
            .unwrap();
        let details: Value = serde_json::from_str(&details).unwrap();
        assert_eq!(details["review_protocol_version"], json!(REVIEW_PROTOCOL_VERSION));
        assert_eq!(details["state"], json!(STATE_RESOLVED));
        assert_eq!(details["resolution_method"], json!("review_confirm"));
        assert_eq!(details.get("tmdb_verified"), None, "confirm に確認済みの印が付いている");
    }

    #[tokio::test]
    async fn a_pick_other_resolution_records_the_verification() {
        let (conn, task_id, _, _) = seed();
        let db = Arc::new(Mutex::new(conn));
        let verifier = FakeVerifier::ok();
        resolve_pick_other_verified(&db, &verifier, task_id, 7777, "movie", None)
            .await
            .unwrap();

        let conn = db.lock().unwrap();
        let details: String = conn
            .query_row(
                "SELECT details_json FROM metadata_review_tasks WHERE id = ?1",
                rusqlite::params![task_id],
                |row| row.get(0),
            )
            .unwrap();
        let details: Value = serde_json::from_str(&details).unwrap();
        assert_eq!(details["resolution_method"], json!("review_pick_other"));
        assert_eq!(details["tmdb_verified"], json!(true));
    }

    #[test]
    fn a_none_resolution_records_its_method() {
        let (conn, task_id, _, _) = seed();
        resolve_none(&conn, task_id, None).unwrap();
        let details: String = conn
            .query_row(
                "SELECT details_json FROM metadata_review_tasks WHERE id = ?1",
                rusqlite::params![task_id],
                |row| row.get(0),
            )
            .unwrap();
        let details: Value = serde_json::from_str(&details).unwrap();
        assert_eq!(details["resolution_method"], json!("review_none"));
        assert_eq!(details.get("tmdb_verified"), None);
    }

    // ─── 6. 既存ラベルとの関係 ───────────────────────────────────────────

    /// 抽出後に強いラベルが付いた work は GT に使わない。
    ///
    /// production の `record_label` はその work の未解決課題をまとめて閉じるので、
    /// ここではラベルだけを直接入れて「課題は開いたまま強いラベルがある」状態を作る
    /// （guard そのものを見るため）。production 経由の場合は課題自体が閉じるので、
    /// どちらの道でも GT ラベルは作られない。
    #[test]
    fn a_work_labelled_after_sampling_is_refused() {
        let (conn, task_id, work_id, run_id) = seed();
        conn.execute(
            "INSERT INTO metadata_match_labels
                 (work_id, run_id, label, tmdb_id, media_type, method, strength)
             VALUES (?1, ?2, 'tmdb', 5555, 'movie', 'manual_apply', 'strong')",
            rusqlite::params![work_id, run_id],
        )
        .unwrap();

        assert_eq!(
            resolve_confirm(&conn, task_id, candidate_id(&conn, run_id, 1001), None).unwrap_err(),
            GtReviewError::AlreadyLabelled { work_id }
        );
        assert_eq!(
            resolve_none(&conn, task_id, None).unwrap_err(),
            GtReviewError::AlreadyLabelled { work_id }
        );
        let labels: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata_match_labels WHERE method LIKE 'review_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(labels, 0);
    }

    /// production の確定は audit_sample 課題を閉じない。
    ///
    /// production の review lifecycle（review_decision など）と、
    /// GT の抽出課題は別物。閉じられてしまうと「なぜ GT が欠けたのか」が
    /// 記録に残らないので、audit_sample だけ production の解決対象から外す。
    /// 代わりに GT 側が「強いラベルがある」として拒否する。
    #[test]
    fn a_production_label_does_not_close_the_audit_task() {
        let (conn, task_id, work_id, run_id) = seed();
        let production_label = match_history::record_label(
            &conn,
            &LabelWrite {
                work_id,
                run_id: Some(run_id),
                tmdb: Some((5555, "movie")),
                method: match_history::LabelMethod::ManualApply,
                note: None,
            },
        )
        .unwrap();

        // production のラベルは今までどおり作られる
        let (method, strength, bound_task): (String, String, Option<i64>) = conn
            .query_row(
                "SELECT method, strength, review_task_id FROM metadata_match_labels WHERE id = ?1",
                rusqlite::params![production_label],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(method, "manual_apply");
        assert_eq!(strength, "strong");
        assert_eq!(bound_task, None, "audit_sample 課題を production のラベルに繋いでいる");

        // audit_sample 課題はそのまま
        let (resolved_at, resolution, run, state): (
            Option<String>,
            Option<i64>,
            Option<i64>,
            String,
        ) = conn
            .query_row(
                "SELECT resolved_at, resolution_label_id, run_id,
                        COALESCE(json_extract(details_json,'$.state'),'')
                   FROM metadata_review_tasks WHERE id = ?1",
                rusqlite::params![task_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(resolved_at, None);
        assert_eq!(resolution, None);
        assert_eq!(run, Some(run_id));
        assert_eq!(state, STATE_READY);

        // GT 側は「抽出後に強いラベルが付いた」として断る
        assert_eq!(
            resolve_none(&conn, task_id, None).unwrap_err(),
            GtReviewError::AlreadyLabelled { work_id }
        );
        let review_labels: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata_match_labels WHERE method LIKE 'review_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(review_labels, 0);
    }

    /// production の固定（weak）は audit 課題を閉じず、GT がその上に強いラベルを作る
    #[test]
    fn a_production_lock_still_allows_a_gt_label() {
        let (conn, task_id, work_id, run_id) = seed();
        let weak = match_history::record_label(
            &conn,
            &LabelWrite {
                work_id,
                run_id: Some(run_id),
                tmdb: Some((4444, "movie")),
                method: match_history::LabelMethod::Lock,
                note: None,
            },
        )
        .unwrap();
        assert_eq!(state_of(&conn, task_id), STATE_READY, "固定で audit 課題が閉じている");

        let label_id =
            resolve_confirm(&conn, task_id, candidate_id(&conn, run_id, 1001), None).unwrap();

        let weak_superseded: Option<String> = conn
            .query_row(
                "SELECT superseded_at FROM metadata_match_labels WHERE id = ?1",
                rusqlite::params![weak],
                |row| row.get(0),
            )
            .unwrap();
        assert!(weak_superseded.is_some(), "弱いラベルが active のまま");
        let (active, strength, method): (i64, String, String) = conn
            .query_row(
                "SELECT id, strength, method FROM metadata_match_labels
                  WHERE work_id = ?1 AND superseded_at IS NULL",
                rusqlite::params![work_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(active, label_id);
        assert_eq!(strength, "strong");
        assert_eq!(method, "review_confirm");
        assert_eq!(state_of(&conn, task_id), STATE_RESOLVED);
    }

    /// 弱いラベル（固定など）だけは同じトランザクションで取り下げる
    #[test]
    fn an_active_weak_label_is_superseded_in_the_same_transaction() {
        let (conn, task_id, work_id, run_id) = seed();
        // 固定（lock）だけが付いている状態。課題は開いたまま
        conn.execute(
            "INSERT INTO metadata_match_labels
                 (work_id, run_id, label, tmdb_id, media_type, method, strength)
             VALUES (?1, ?2, 'tmdb', 4444, 'movie', 'lock', 'weak')",
            rusqlite::params![work_id, run_id],
        )
        .unwrap();
        let weak = conn.last_insert_rowid();

        let label_id =
            resolve_confirm(&conn, task_id, candidate_id(&conn, run_id, 1001), None).unwrap();

        let weak_superseded: Option<String> = conn
            .query_row(
                "SELECT superseded_at FROM metadata_match_labels WHERE id = ?1",
                rusqlite::params![weak],
                |row| row.get(0),
            )
            .unwrap();
        assert!(weak_superseded.is_some(), "弱いラベルが active のまま");
        let active: i64 = conn
            .query_row(
                "SELECT id FROM metadata_match_labels
                  WHERE work_id = ?1 AND superseded_at IS NULL",
                rusqlite::params![work_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active, label_id, "active なラベルが GT のものになっていない");
    }

    /// 失敗したときは何も残さない（ラベルも状態も）
    #[test]
    fn a_failed_resolution_leaves_nothing_behind() {
        let (conn, task_id, work_id, run_id) = seed();
        // 解決の直前に他所から閉じられた状況を作る
        conn.execute(
            "UPDATE metadata_review_tasks SET details_json = ?2 WHERE id = ?1",
            rusqlite::params![task_id, crate::services::match_history::audit_details_json("stale", Some("x"))],
        )
        .unwrap();

        assert!(matches!(
            resolve_confirm(&conn, task_id, candidate_id(&conn, run_id, 1001), None),
            Err(GtReviewError::TaskNotReviewable { .. })
        ));
        let labels: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_match_labels", [], |row| row.get(0))
            .unwrap();
        assert_eq!(labels, 0);
        let (resolved_at, resolution): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT resolved_at, resolution_label_id FROM metadata_review_tasks WHERE id = ?1",
                rusqlite::params![task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(resolved_at, None);
        assert_eq!(resolution, None);
        assert_eq!(state_of(&conn, task_id), "stale");
        let _ = work_id;
    }

    // ─── 7. 触らないものの確認 ───────────────────────────────────────────

    /// 否定の記録は C5c.3 では書かない
    #[test]
    fn no_rejection_is_written() {
        let (conn, task_id, _, run_id) = seed();
        resolve_confirm(&conn, task_id, candidate_id(&conn, run_id, 1002), None).unwrap();
        let rejections: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_match_rejections", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rejections, 0);
    }

    /// Jev 側のテーブルも空のまま
    #[test]
    fn jev_is_not_touched() {
        let (conn, task_id, _, _) = seed();
        resolve_none(&conn, task_id, None).unwrap();
        let (calls, links, shadow): (i64, i64, i64) = conn
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

    /// 同じ sample の他の課題は動かさない
    #[test]
    fn resolving_one_task_does_not_touch_the_others() {
        let mut conn = open_migrated();
        for index in 1..=8 {
            insert_work(&conn, index);
        }
        let manifest = freeze_sample(&mut conn, &request("sample-a", 3)).unwrap();
        let mut runs = Vec::new();
        for entry in &manifest.entries {
            runs.push(attach_run(&conn, entry.task_id, entry.work_id, &[1001, 1002]));
        }

        resolve_confirm(&conn, manifest.entries[1].task_id, candidate_id(&conn, runs[1], 1001), None)
            .unwrap();

        assert_eq!(state_of(&conn, manifest.entries[0].task_id), STATE_READY);
        assert_eq!(state_of(&conn, manifest.entries[1].task_id), STATE_RESOLVED);
        assert_eq!(state_of(&conn, manifest.entries[2].task_id), STATE_READY);
        let labels: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_match_labels", [], |row| row.get(0))
            .unwrap();
        assert_eq!(labels, 1);
    }

    /// production の record_label は従来どおり（GT は別経路）
    #[test]
    fn production_record_label_is_unchanged() {
        let conn = open_migrated();
        let work_id = insert_work(&conn, 1);
        let run_id = attach_run_without_task(&conn, work_id, &[1001]);
        conn.execute(
            "INSERT INTO metadata_review_tasks (work_id, run_id, reason, details_json)
             VALUES (?1, ?2, 'review_decision', '{}')",
            rusqlite::params![work_id, run_id],
        )
        .unwrap();
        let task_id = conn.last_insert_rowid();

        let label_id = match_history::record_label(
            &conn,
            &LabelWrite {
                work_id,
                run_id: Some(run_id),
                tmdb: Some((1001, "movie")),
                method: match_history::LabelMethod::ManualApply,
                note: None,
            },
        )
        .unwrap();

        let (resolved, resolution, bound_task): (Option<String>, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT t.resolved_at, t.resolution_label_id, l.review_task_id
                   FROM metadata_review_tasks t, metadata_match_labels l
                  WHERE t.id = ?1 AND l.id = ?2",
                rusqlite::params![task_id, label_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(resolved.is_some(), "production の課題解決が変わっている");
        assert_eq!(resolution, Some(label_id));
        assert_eq!(bound_task, Some(task_id));
    }

    /// production の課題と audit_sample が同時にあるとき、閉じるのは production だけ
    #[test]
    fn a_production_label_closes_only_the_production_task() {
        let (conn, audit_task, work_id, run_id) = seed();
        conn.execute(
            "INSERT INTO metadata_review_tasks (work_id, run_id, reason, details_json)
             VALUES (?1, ?2, 'review_decision', '{}')",
            rusqlite::params![work_id, run_id],
        )
        .unwrap();
        let production_task = conn.last_insert_rowid();

        let label_id = match_history::record_label(
            &conn,
            &LabelWrite {
                work_id,
                run_id: Some(run_id),
                tmdb: Some((5555, "movie")),
                method: match_history::LabelMethod::ManualApply,
                note: None,
            },
        )
        .unwrap();

        let (prod_resolved, prod_label): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT resolved_at, resolution_label_id FROM metadata_review_tasks WHERE id = ?1",
                rusqlite::params![production_task],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(prod_resolved.is_some(), "production の課題が閉じていない");
        assert_eq!(prod_label, Some(label_id));

        let (audit_resolved, audit_label, state): (Option<String>, Option<i64>, String) = conn
            .query_row(
                "SELECT resolved_at, resolution_label_id,
                        COALESCE(json_extract(details_json,'$.state'),'')
                   FROM metadata_review_tasks WHERE id = ?1",
                rusqlite::params![audit_task],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(audit_resolved, None, "audit 課題まで閉じている");
        assert_eq!(audit_label, None);
        assert_eq!(state, STATE_READY);

        // ラベルは production の課題に繋がる
        let bound: Option<i64> = conn
            .query_row(
                "SELECT review_task_id FROM metadata_match_labels WHERE id = ?1",
                rusqlite::params![label_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bound, Some(production_task));
    }

    /// GT のラベルは gt_sampling の母集団から外れる（二重に測らない）
    #[test]
    fn a_reviewed_work_leaves_the_sampling_frame() {
        let (conn, task_id, work_id, run_id) = seed();
        resolve_confirm(&conn, task_id, candidate_id(&conn, run_id, 1001), None).unwrap();

        let frame = gt_sampling::build_frame(&conn, "reconstructed_clean", "sample-next").unwrap();
        assert!(
            !frame.iter().any(|row| row.work_id == work_id),
            "確定済みの work がまだ母集団に残っている"
        );
    }

    /// レビューの状態遷移は gt_sampling の CAS を使い回さない（条件が逆）
    #[test]
    fn the_sampling_cas_cannot_move_a_bound_task() {
        let (conn, task_id, _, _) = seed();
        // run が繋がっているので、抽出側の CAS では動かない
        assert_eq!(
            gt_sampling::set_task_state(&conn, task_id, &[STATE_READY], "generation_error", None)
                .unwrap(),
            gt_sampling::TaskStateChange::Skipped
        );
        assert_eq!(state_of(&conn, task_id), STATE_READY);
    }
}
