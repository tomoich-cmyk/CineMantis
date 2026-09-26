//! Jev の shadow 呼び出しと監査記録（PR3 C4b、packed のみ）。
//!
//! C4a が用意した session・予算・contract の上で、**1 run につき 1 回**の `packed`
//! 呼び出しを組み立てて実行し、結果を監査テーブルへ残すところまでを担う。
//!
//! ```text
//! C2  materialize            … 送る state（rules-safe を適用する前の証拠）
//! C3A build_questions        … 質問の生成と応答の検証
//! C3B ask_systemone          … HTTP
//! C4a reserve_call_budget    … 送信前の予約
//! C4b ここ                   … 上をつなぎ、metadata_match_jev_calls などへ記録する
//! ```
//!
//! # shadow であること
//!
//! 書いてよいのは監査テーブル（`metadata_match_jev_calls` /
//! `metadata_match_candidate_scores` / `metadata_match_verdicts` の `jev-policy` 行）と、
//! C4a の helper 経由の `jev_eval_sessions` だけ。`works` / `files` / `work_parts` /
//! `persons` 系 / `metadata_match_runs` 本体 / `metadata_review_tasks` / labels /
//! rejections / poster / ファイルシステムには触れない。`match_once` とも繋がない。
//! Jev の結論は **REVIEW か UNRESOLVED でだけ**残し、`AUTO` は書かない。
//!
//! # 使い方（所有権で 1 予約 = 1 送信を縛る）
//!
//! ```text
//! match prepare_call(&mut conn, session_id, &input).await? {
//!     PrepareCallResult::Ready(prepared) => {
//!         let executed = execute_call(&client, prepared).await;   // prepared を消費
//!         persist_call(&mut conn, executed)                       // executed を消費
//!     }
//!     PrepareCallResult::Skipped(skipped) => {
//!         persist_skipped_call(&mut conn, skipped)                // skipped を消費
//!     }
//! }
//! ```
//!
//! `execute_call` は [`PreparedCall`] を、`persist_call` は [`ExecutedCall`] を、
//! `persist_skipped_call` は [`PreparedSkippedCall`] を **値で受け取る**ので、同じ
//! 予約から 2 回送ることも、任意の run に skip 行を足すこともコンパイル時にできない。
//! 予算切れも同じ state machine の一部で、`prepare_call` だけが入口。
//!
//! # 同一 session では必ず逐次実行する
//!
//! `input_tokens_used` は応答が返るまで分からない。複数の呼び出しを同時に
//! in-flight にすると、消費量が判明する前に次の予約が通り、トークン上限を
//! 呼び出し数分だけ超過し得る。そこで process 全体で 1 本の非同期ロックを持ち、
//! **予約 → HTTP → トークン加算 → 監査保存**が終わるまで離さない。guard は
//! `PreparedCall` → `ExecutedCall` と持ち回り、`persist_call` を抜けるときに落ちる。
//!
//! SQLite の transaction や DB の `std::sync::Mutex` は HTTP の待ち時間に握らない。
//! 同期の DB 作業は各区間の中で開いて閉じる。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::services::jev_contract::{
    build_questions, canonical_json, parse_answers, questions_to_json, validate_response,
    JevAnswers, QuestionSpec, JEV_CONTRACT_VERSION, NONE_OPTION,
};
use crate::services::jev_session::{
    load_session, record_token_usage, reserve_call_budget, JevSession, JevSessionError,
};
use crate::services::jev_state::{
    materialize, CandidateIdentity, CandidateInput, JevStateError, LocalEvidenceInput,
    JEV_STATE_SCHEMA_VERSION,
};
use crate::services::typesafe_client::{
    TypeSafeClient, TypeSafeClientError, TypeSafeErrorKind, TypeSafeTransportResult,
};

/// C4b v1 が実行する唯一の呼び出し種別。
pub const CALL_KIND: &str = "packed";

/// 候補スコアの出どころ。
pub const SCORE_MATCHER: &str = "jev-call";

/// Jev の結論を残す matcher。
pub const VERDICT_MATCHER: &str = "jev-policy";

/// shadow verdict の版。contract の版とは別に持つ。
pub const VERDICT_MATCHER_VERSION: &str = "jev-policy-1";

/// 予算切れで送らなかったときに `error_text` へ入れる印。
pub const BUDGET_EXHAUSTED_TEXT: &str = "budget_exhausted";

/// 応答が contract に合わなかったときに `error_text` へ入れる固定値。
///
/// C3A のエラー文にはモデルが返した任意の文字列（未知の question id や choice label）が
/// 混ざり得る。そのまま保存すると監査列に外部由来のテキストが入るので、理由は固定値にする。
pub const VALIDATION_FAILED_TEXT: &str = "jev_response_validation_failed";

/// request を保存しないときに入れる空の JSON。
const EMPTY_JSON: &str = "{}";

/// 同じ process で Jev の呼び出しを同時に走らせないためのロック。
fn call_lock() -> Arc<Mutex<()>> {
    static LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
    LOCK.get_or_init(|| Arc::new(Mutex::new(()))).clone()
}

// ─── 入力 ────────────────────────────────────────────────────────────────────

/// 1 候補分の入力。`candidate_id` は `metadata_match_candidates.id`。
///
/// `cand_key` は呼び出し側から受け取らない。並び順が `c1`, `c2`, `c3` に対応し、
/// その対応が DB の候補行と一致することを送信前に照合する。
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowCandidate {
    pub candidate_id: i64,
    pub input: CandidateInput,
}

/// 1 回の packed 呼び出しの入力。
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowCallInput {
    pub run_id: i64,
    pub local: LocalEvidenceInput,
    pub candidates: Vec<ShadowCandidate>,
}

// ─── 中間状態 ────────────────────────────────────────────────────────────────

/// 監査に残す「送る内容」の凍結。
#[derive(Debug, Clone)]
struct FrozenRequest {
    state_value: Value,
    state_json: String,
    questions_value: Value,
    questions_json: String,
    candidate_order_json: String,
    candidate_order: Vec<String>,
}

/// 予約済みで、まだ送っていない呼び出し。
///
/// 逐次ロックの guard を持つ。`execute_call` が値で受け取るので、同じ予約から
/// 2 回 HTTP を出すことはできない。
pub struct PreparedCall {
    session_id: i64,
    run_id: i64,
    requested_model: String,
    contract_version: String,
    state_schema_version: String,
    request: FrozenRequest,
    questions: Vec<QuestionSpec>,
    identity: Vec<CandidateIdentity>,
    /// cand_key → metadata_match_candidates.id
    candidate_ids: BTreeMap<String, i64>,
    guard: OwnedMutexGuard<()>,
}

impl std::fmt::Debug for PreparedCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedCall")
            .field("session_id", &self.session_id)
            .field("run_id", &self.run_id)
            .field("requested_model", &self.requested_model)
            .field("candidate_order", &self.request.candidate_order)
            .finish()
    }
}

impl PreparedCall {
    pub fn run_id(&self) -> i64 {
        self.run_id
    }

    pub fn session_id(&self) -> i64 {
        self.session_id
    }

    pub fn requested_model(&self) -> &str {
        &self.requested_model
    }

    /// 送信する state（C2 の検証を通った JSON）。
    pub fn state_json(&self) -> &str {
        &self.request.state_json
    }

    /// 送信する questions。
    pub fn questions_value(&self) -> &Value {
        &self.request.questions_value
    }

    /// 送る順の cand_key。
    pub fn candidate_order(&self) -> &[String] {
        &self.request.candidate_order
    }
}

/// 予算切れで送らないと決まった呼び出し。
///
/// このモジュールの外からは作れない（フィールドが private で、構築するのは
/// [`prepare_call`] だけ）。したがって「任意の run に skip 行を足す」ことはできない。
/// ローカルの検証はすべて通った後の状態なので、候補の並びは凍結済みのものを残す。
pub struct PreparedSkippedCall {
    session_id: i64,
    run_id: i64,
    requested_model: String,
    contract_version: String,
    state_schema_version: String,
    candidate_order_json: String,
    guard: OwnedMutexGuard<()>,
}

impl std::fmt::Debug for PreparedSkippedCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedSkippedCall")
            .field("session_id", &self.session_id)
            .field("run_id", &self.run_id)
            .field("candidate_order_json", &self.candidate_order_json)
            .finish()
    }
}

impl PreparedSkippedCall {
    pub fn run_id(&self) -> i64 {
        self.run_id
    }

    pub fn session_id(&self) -> i64 {
        self.session_id
    }

    pub fn candidate_order_json(&self) -> &str {
        &self.candidate_order_json
    }
}

/// [`prepare_call`] の結果。予算が残っていれば送り、切れていれば skip として記録する。
#[derive(Debug)]
pub enum PrepareCallResult {
    Ready(PreparedCall),
    Skipped(PreparedSkippedCall),
}

/// 送信が終わった呼び出し。保存に必要なものだけを持つ。
pub struct ExecutedCall {
    session_id: i64,
    run_id: i64,
    requested_model: String,
    contract_version: String,
    state_schema_version: String,
    request: FrozenRequest,
    candidate_ids: BTreeMap<String, i64>,
    outcome: CallOutcome,
    guard: OwnedMutexGuard<()>,
}

impl std::fmt::Debug for ExecutedCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutedCall")
            .field("session_id", &self.session_id)
            .field("run_id", &self.run_id)
            .field("status", &self.outcome.status())
            .finish()
    }
}

impl ExecutedCall {
    pub fn status(&self) -> &'static str {
        self.outcome.status()
    }

    pub fn run_id(&self) -> i64 {
        self.run_id
    }
}

/// 1 回の呼び出しの結果。DB へ書く前の形。
#[derive(Debug)]
pub enum CallOutcome {
    /// HTTP も検証も通った
    Validated { transport: TypeSafeTransportResult, answers: JevAnswers },
    /// HTTP は通ったが、応答が contract に合わない（理由は保存しない）
    Invalid { transport: TypeSafeTransportResult },
    /// そもそも呼び出せなかった
    Transport(TypeSafeClientError),
}

impl CallOutcome {
    /// `metadata_match_jev_calls.status` に入れる値。
    pub fn status(&self) -> &'static str {
        match self {
            CallOutcome::Validated { .. } => "ok",
            CallOutcome::Invalid { .. } => "invalid",
            CallOutcome::Transport(error) => transport_status(error.kind),
        }
    }
}

/// transport のエラー種別を call の status へ写す。
///
/// `unavailable` は「時間をおけば呼べるかもしれない」もの。`model_mismatch` は
/// 送ったモデルと違うものが返ってきた状態で、再送しても同じなので `error`。
pub fn transport_status(kind: TypeSafeErrorKind) -> &'static str {
    match kind {
        TypeSafeErrorKind::Auth
        | TypeSafeErrorKind::RateLimit
        | TypeSafeErrorKind::Overloaded
        | TypeSafeErrorKind::Network
        | TypeSafeErrorKind::Timeout => "unavailable",
        TypeSafeErrorKind::Unprocessable
        | TypeSafeErrorKind::Parse
        | TypeSafeErrorKind::ResponseTooLarge
        | TypeSafeErrorKind::ModelMismatch
        | TypeSafeErrorKind::Other => "error",
    }
}

/// 保存した結果。
#[derive(Debug, Clone, PartialEq)]
pub struct CallRecord {
    pub call_id: i64,
    pub run_id: i64,
    pub call_seq: i64,
    pub status: String,
    pub candidate_scores: usize,
    pub verdict_decision: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JevShadowError {
    /// 予算・session 側の事情で送れない
    Session(JevSessionError),
    /// state を作れない（候補数・サイズ・禁止フィールドなど）
    State(String),
    /// 質問を作れない
    Contract(String),
    /// 入力・前提が満たされていない（HTTP も予約もしていない）
    Precondition(String),
    /// この run では既に packed を実行済み
    DuplicatePackedCall { run_id: i64 },
    Db(String),
}

impl std::fmt::Display for JevShadowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JevShadowError::Session(error) => write!(f, "{error}"),
            JevShadowError::State(reason) => write!(f, "state を作れません: {reason}"),
            JevShadowError::Contract(reason) => write!(f, "質問を作れません: {reason}"),
            JevShadowError::Precondition(reason) => write!(f, "前提を満たしません: {reason}"),
            JevShadowError::DuplicatePackedCall { run_id } => {
                write!(f, "run {run_id} には既に packed 呼び出しがあります")
            }
            JevShadowError::Db(reason) => write!(f, "DB エラー: {reason}"),
        }
    }
}

impl std::error::Error for JevShadowError {}

impl From<rusqlite::Error> for JevShadowError {
    fn from(error: rusqlite::Error) -> Self {
        JevShadowError::Db(error.to_string())
    }
}

impl From<JevSessionError> for JevShadowError {
    fn from(error: JevSessionError) -> Self {
        JevShadowError::Session(error)
    }
}

impl From<JevStateError> for JevShadowError {
    fn from(error: JevStateError) -> Self {
        JevShadowError::State(error.to_string())
    }
}

// ─── 手順 1: 前提を確かめ、送る内容を凍結し、最後に予約する ─────────────────

/// 送る内容を確定して予算を 1 件予約する。
///
/// **予算を消費しない失敗を先に済ませる**のが要点。run / session の前提、重複 packed、
/// 候補の突き合わせ、C2 の state 生成と検証、C3A の質問生成まで通ってから
/// `reserve_call_budget` を呼ぶ。こうしないと、ローカルの不整合で予算だけが減る。
///
/// 逐次ロックはこの関数の入口で取り、`persist_call` が終わるまで保持する。
pub async fn prepare_call(
    conn: &mut Connection,
    session_id: i64,
    input: &ShadowCallInput,
) -> Result<PrepareCallResult, JevShadowError> {
    let permit = acquire_call_permit().await;
    prepare_call_with_permit(conn, session_id, input, permit)
}

/// 逐次実行の権利。これを持っている間だけ Jev の呼び出しを進められる。
///
/// アプリ側は DB の同期ロックを取る**前**にこれを取り、監査の保存が終わるまで
/// 持ち回る。こうすると「permit を待つ間 DB を止める」ことがなくなる。
pub struct JevCallPermit {
    guard: OwnedMutexGuard<()>,
}

impl std::fmt::Debug for JevCallPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JevCallPermit")
    }
}

/// 逐次実行の権利を取る。**DB のロックを持たずに**呼ぶこと。
pub async fn acquire_call_permit() -> JevCallPermit {
    JevCallPermit { guard: call_lock().lock_owned().await }
}

/// [`prepare_call`] の同期版。permit は呼び出し側が先に取っておく。
///
/// `.await` を含まないので、DB の `std::sync::MutexGuard` を持ったまま安全に呼べる。
pub fn prepare_call_with_permit(
    conn: &mut Connection,
    session_id: i64,
    input: &ShadowCallInput,
    permit: JevCallPermit,
) -> Result<PrepareCallResult, JevShadowError> {
    let guard = permit.guard;

    if input.candidates.is_empty() {
        return Err(JevShadowError::Precondition("候補がありません".into()));
    }

    // ① session の前提（モデルは session のものだけを使う）。
    // 予算切れだけはここで落とさず、ローカル検証を通してから skip として扱う
    let session = load_session(conn, session_id)?;
    let already_exhausted = check_session(&session)?;

    // ② run の前提
    let run_evidence_class = check_run(conn, input.run_id)?;

    // ③ この run で packed を既に実行していないか
    check_no_packed_audit(conn, input.run_id)?;

    // ④ evidence_class は run が正
    let local = resolve_evidence_class(input.local.clone(), &run_evidence_class)?;

    // ⑤ state を作って検証する
    let candidate_inputs: Vec<CandidateInput> =
        input.candidates.iter().map(|c| c.input.clone()).collect();
    let state = materialize(local, candidate_inputs)?;
    let state_json = state.to_validated_json().map_err(JevShadowError::from)?;
    let state_value: Value = serde_json::from_str(&state_json)
        .map_err(|error| JevShadowError::State(format!("state を読み直せません: {error}")))?;

    // ⑥ 候補と DB 行を 5 点で突き合わせる
    let candidate_ids = bind_candidates(conn, input, &state.identity)?;

    // ⑦ 質問を作る
    let cand_keys: Vec<String> = state.identity.iter().map(|c| c.cand_key.clone()).collect();
    let questions =
        build_questions(&cand_keys).map_err(|error| JevShadowError::Contract(error.to_string()))?;
    let questions_value = questions_to_json(&questions);

    let request = FrozenRequest {
        state_json: state_json.clone(),
        state_value,
        questions_json: canonical_json(&questions_value),
        questions_value,
        candidate_order_json: canonical_json(&json!(cand_keys)),
        candidate_order: cand_keys,
    };

    // ⑧ ここまで通ってから予算を見る。ローカルの不整合を skip として記録しない
    let skipped = |guard| {
        PrepareCallResult::Skipped(PreparedSkippedCall {
            session_id,
            run_id: input.run_id,
            requested_model: session.requested_model.clone(),
            contract_version: session.contract_version.clone(),
            state_schema_version: session.state_schema_version.clone(),
            candidate_order_json: request.candidate_order_json.clone(),
            guard,
        })
    };

    if already_exhausted {
        return Ok(skipped(guard));
    }

    match reserve_call_budget(conn, session_id) {
        Ok(_) => {}
        Err(JevSessionError::BudgetExhausted) => return Ok(skipped(guard)),
        Err(error) => return Err(JevShadowError::Session(error)),
    }

    Ok(PrepareCallResult::Ready(PreparedCall {
        session_id,
        run_id: input.run_id,
        requested_model: session.requested_model,
        contract_version: session.contract_version,
        state_schema_version: session.state_schema_version,
        request,
        questions,
        identity: state.identity,
        candidate_ids,
        guard,
    }))
}

/// session が使える状態かを見る。戻り値は「もう予算を使い切っているか」。
///
/// `exhausted` はエラーにしない。ローカルの検証を全部通してから skip として
/// 記録したいので、ここでは印だけ返す。`closed` / `blocked` は送る余地がないので
/// その場で落とす。
fn check_session(session: &JevSession) -> Result<bool, JevShadowError> {
    let already_exhausted = session.status == "exhausted";
    if session.status != "open" && !already_exhausted {
        return Err(JevShadowError::Session(match session.status.as_str() {
            "closed" => JevSessionError::SessionClosed,
            _ => JevSessionError::Blocked {
                reason: session
                    .blocked_reason
                    .clone()
                    .unwrap_or_else(|| "blocked".to_string()),
            },
        }));
    }
    if session.contract_version != JEV_CONTRACT_VERSION {
        return Err(JevShadowError::Precondition(format!(
            "session の contract が {JEV_CONTRACT_VERSION} ではありません"
        )));
    }
    if session.state_schema_version != JEV_STATE_SCHEMA_VERSION {
        return Err(JevShadowError::Precondition(format!(
            "session の state schema が {JEV_STATE_SCHEMA_VERSION} ではありません"
        )));
    }
    Ok(already_exhausted)
}

/// run が shadow 用で、まだ適用されていないこと。`evidence_class` を返す。
fn check_run(conn: &Connection, run_id: i64) -> Result<String, JevShadowError> {
    let row: Option<(String, i64, String)> = conn
        .query_row(
            "SELECT mode, applied, evidence_class FROM metadata_match_runs WHERE id = ?1",
            rusqlite::params![run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    let Some((mode, applied, evidence_class)) = row else {
        return Err(JevShadowError::Precondition(format!("run {run_id} がありません")));
    };
    if mode != "shadow" {
        return Err(JevShadowError::Precondition(format!(
            "run {run_id} の mode が shadow ではありません"
        )));
    }
    if applied != 0 {
        return Err(JevShadowError::Precondition(format!(
            "run {run_id} は既に適用済みです"
        )));
    }
    Ok(evidence_class)
}

/// この run に packed の記録（call か jev-policy verdict）が既に無いこと。
///
/// `Transaction` は `Connection` へ deref するので、事前チェックにも
/// 保存 transaction 内の再チェックにも同じものを使う。
fn check_no_packed_audit(conn: &Connection, run_id: i64) -> Result<(), JevShadowError> {
    let calls: i64 = conn.query_row(
        "SELECT COUNT(*) FROM metadata_match_jev_calls
          WHERE run_id = ?1 AND call_kind = ?2",
        rusqlite::params![run_id, CALL_KIND],
        |row| row.get(0),
    )?;
    let verdicts: i64 = conn.query_row(
        "SELECT COUNT(*) FROM metadata_match_verdicts WHERE run_id = ?1 AND matcher = ?2",
        rusqlite::params![run_id, VERDICT_MATCHER],
        |row| row.get(0),
    )?;
    if calls > 0 || verdicts > 0 {
        return Err(JevShadowError::DuplicatePackedCall { run_id });
    }
    Ok(())
}

/// run の `evidence_class` を state に反映する。食い違いは通さない。
fn resolve_evidence_class(
    mut local: LocalEvidenceInput,
    run_evidence_class: &str,
) -> Result<LocalEvidenceInput, JevShadowError> {
    match local.evidence_class.as_deref() {
        None => local.evidence_class = Some(run_evidence_class.to_string()),
        Some(given) if given == run_evidence_class => {}
        Some(given) => {
            return Err(JevShadowError::Precondition(format!(
                "evidence_class が run と違います（run={run_evidence_class} / 入力={given}）"
            )))
        }
    }
    Ok(local)
}

/// 候補を DB 行と 5 点（run_id / candidate_id / cand_key / tmdb_id / media_type）で照合する。
///
/// `cand_key` は C2 が付けた並び（0 番目が `c1`）を使い、呼び出し側の自由入力は受けない。
fn bind_candidates(
    conn: &Connection,
    input: &ShadowCallInput,
    identity: &[CandidateIdentity],
) -> Result<BTreeMap<String, i64>, JevShadowError> {
    if identity.len() != input.candidates.len() {
        return Err(JevShadowError::Precondition(
            "候補数と identity の数が合いません".into(),
        ));
    }

    let mut candidate_ids = BTreeMap::new();
    for (index, (identity, candidate)) in identity.iter().zip(input.candidates.iter()).enumerate() {
        let expected_key = format!("c{}", index + 1);
        if identity.cand_key != expected_key {
            return Err(JevShadowError::Precondition(format!(
                "{} 番目の cand_key が {expected_key} ではありません",
                index + 1
            )));
        }
        if identity.tmdb_id != candidate.input.tmdb_id
            || identity.media_type != candidate.input.media_type
        {
            return Err(JevShadowError::Precondition(
                "identity と入力候補が食い違っています".into(),
            ));
        }

        let row: Option<(String, i64, String)> = conn
            .query_row(
                "SELECT cand_key, tmdb_id, media_type FROM metadata_match_candidates
                  WHERE run_id = ?1 AND id = ?2",
                rusqlite::params![input.run_id, candidate.candidate_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;

        let Some((cand_key, tmdb_id, media_type)) = row else {
            return Err(JevShadowError::Precondition(format!(
                "候補 {} は run {} にありません",
                candidate.candidate_id, input.run_id
            )));
        };
        if cand_key != expected_key {
            return Err(JevShadowError::Precondition(format!(
                "候補 {} の cand_key が {expected_key} と一致しません",
                candidate.candidate_id
            )));
        }
        if tmdb_id != candidate.input.tmdb_id {
            return Err(JevShadowError::Precondition(format!(
                "候補 {} の TMDB ID が一致しません",
                candidate.candidate_id
            )));
        }
        if media_type != candidate.input.media_type {
            return Err(JevShadowError::Precondition(format!(
                "候補 {} の media_type が一致しません",
                candidate.candidate_id
            )));
        }

        candidate_ids.insert(expected_key, candidate.candidate_id);
    }
    Ok(candidate_ids)
}

// ─── 手順 2: 送って検証する ────────────────────────────────────────────────

/// SystemOne を 1 回呼び、応答を C3A で検証する。**DB には触らない。**
///
/// `prepared` を値で受け取るので、同じ予約で再送はできない。`answers` の意味づけは
/// C3A に任せ、ここでは Noul / Choice を解釈しない。
pub async fn execute_call(client: &TypeSafeClient, prepared: PreparedCall) -> ExecutedCall {
    let PreparedCall {
        session_id,
        run_id,
        requested_model,
        contract_version,
        state_schema_version,
        request,
        questions,
        identity,
        candidate_ids,
        guard,
    } = prepared;

    let outcome = match client
        .ask_systemone(&requested_model, &request.state_value, &request.questions_value)
        .await
    {
        Err(error) => CallOutcome::Transport(error),
        Ok(transport) => match parse_answers(&transport.answers) {
            Err(_) => CallOutcome::Invalid { transport },
            Ok(answers) => match validate_response(&questions, &identity, &answers) {
                Ok(answers) => CallOutcome::Validated { transport, answers },
                Err(_) => CallOutcome::Invalid { transport },
            },
        },
    };

    ExecutedCall {
        session_id,
        run_id,
        requested_model,
        contract_version,
        state_schema_version,
        request,
        candidate_ids,
        outcome,
        guard,
    }
}

// ─── 手順 3: 監査テーブルへ残す ────────────────────────────────────────────

/// 呼び出しの結果を記録する。
///
/// HTTP が成立していれば **先に** トークンを加算し、そのあと 1 つの transaction で
/// call 行と（成功時のみ）候補スコア・verdict を書く。逆順だと、監査だけ残って
/// 予算が減らない状態を作れてしまう。監査側が失敗して予算だけ減るのは安全側。
pub fn persist_call(
    conn: &mut Connection,
    executed: ExecutedCall,
) -> Result<CallRecord, JevShadowError> {
    let ExecutedCall {
        session_id,
        run_id,
        requested_model,
        contract_version,
        state_schema_version,
        request,
        candidate_ids,
        outcome,
        guard,
    } = executed;

    let status = outcome.status().to_string();

    // ① トークンは HTTP が成立したときだけ分かる
    if let Some(transport) = transport_of(&outcome) {
        record_token_usage(conn, session_id, transport.input_tokens, transport.output_tokens)?;
    }

    // ② 監査を 1 transaction で
    let tx = conn.transaction()?;

    // 同じ run に packed が 2 件入らないよう、書く直前にもう一度見る
    // （call だけでなく jev-policy verdict も。UNIQUE 違反任せにしない）
    if let Err(error) = check_no_packed_audit(&tx, run_id) {
        tx.rollback()?;
        return Err(error);
    }

    let call_seq: i64 = tx.query_row(
        "SELECT COALESCE(MAX(call_seq), 0) + 1 FROM metadata_match_jev_calls WHERE run_id = ?1",
        rusqlite::params![run_id],
        |row| row.get(0),
    )?;

    let call_id = insert_call_row(
        &tx,
        &CallRowInput {
            run_id,
            call_seq,
            session_id,
            requested_model: &requested_model,
            contract_version: &contract_version,
            state_schema_version: &state_schema_version,
            request: &request,
            candidate_ids: &candidate_ids,
            outcome: &outcome,
            status: &status,
        },
    )?;

    let mut candidate_scores = 0usize;
    let mut verdict_decision = None;
    if let CallOutcome::Validated { answers, .. } = &outcome {
        candidate_scores = insert_candidate_scores(
            &tx,
            run_id,
            &contract_version,
            &candidate_ids,
            call_id,
            answers,
        )?;
        verdict_decision = Some(insert_verdict(&tx, run_id, answers)?);
    }

    tx.commit()?;
    drop(guard);

    Ok(CallRecord { call_id, run_id, call_seq, status, candidate_scores, verdict_decision })
}

fn transport_of(outcome: &CallOutcome) -> Option<&TypeSafeTransportResult> {
    match outcome {
        CallOutcome::Validated { transport, .. } | CallOutcome::Invalid { transport } => {
            Some(transport)
        }
        CallOutcome::Transport(_) => None,
    }
}

struct CallRowInput<'a> {
    run_id: i64,
    call_seq: i64,
    session_id: i64,
    requested_model: &'a str,
    contract_version: &'a str,
    state_schema_version: &'a str,
    request: &'a FrozenRequest,
    candidate_ids: &'a BTreeMap<String, i64>,
    outcome: &'a CallOutcome,
    status: &'a str,
}

/// HTTP 処理まで届いたと言えるか。
///
/// C3B が送信前に弾いた場合（送る body にキーが混ざっていた等）は `http_status` も
/// `capture` も無い。その request は **保存しない**。
fn reached_http(outcome: &CallOutcome) -> bool {
    match outcome {
        CallOutcome::Validated { .. } | CallOutcome::Invalid { .. } => true,
        CallOutcome::Transport(error) => error.http_status.is_some() || error.capture.is_some(),
    }
}

fn insert_call_row(
    tx: &rusqlite::Transaction<'_>,
    input: &CallRowInput<'_>,
) -> Result<i64, JevShadowError> {
    let transport = transport_of(input.outcome);
    let capture = match input.outcome {
        CallOutcome::Validated { transport, .. } | CallOutcome::Invalid { transport } => {
            Some(&transport.capture)
        }
        CallOutcome::Transport(error) => error.capture.as_ref(),
    };

    // 送信前に止められた request は残さない
    let (state_json, questions_json, state_bytes) = if reached_http(input.outcome) {
        (
            input.request.state_json.clone(),
            input.request.questions_json.clone(),
            input.request.state_json.len() as i64,
        )
    } else {
        (EMPTY_JSON.to_string(), EMPTY_JSON.to_string(), 0)
    };

    let (selected_id, selected_key, answered_none, parsed_answer_json) = match input.outcome {
        CallOutcome::Validated { answers, .. } => {
            let key = answers.best_match.as_ref().map(|c| c.cand_key.clone());
            let id = key.as_ref().and_then(|key| input.candidate_ids.get(key).copied());
            (
                id,
                key,
                i64::from(answers.best_match.is_none()),
                Some(safe_parsed_answer_json(answers)),
            )
        }
        _ => (None, None, 0, None),
    };

    let (error_kind, error_text) = match input.outcome {
        CallOutcome::Validated { .. } => (None, None),
        // モデルが返した文字列を保存しないよう、理由は固定値
        CallOutcome::Invalid { .. } => (None, Some(VALIDATION_FAILED_TEXT.to_string())),
        CallOutcome::Transport(error) => {
            (Some(error.kind.as_str().to_string()), Some(error.safe_text.clone()))
        }
    };

    let http_status = transport.map(|t| t.http_status as i64).or_else(|| match input.outcome {
        CallOutcome::Transport(error) => error.http_status.map(i64::from),
        _ => None,
    });
    let latency_ms = transport.map(|t| t.latency_ms).or_else(|| match input.outcome {
        CallOutcome::Transport(error) => Some(error.latency_ms),
        _ => None,
    });
    let retry_count = transport
        .map(|t| t.retry_count as i64)
        .or(match input.outcome {
            CallOutcome::Transport(error) => Some(error.retry_count as i64),
            _ => None,
        })
        .unwrap_or(0);
    let request_id = transport.and_then(|t| t.request_id.clone()).or_else(|| match input.outcome {
        CallOutcome::Transport(error) => error.request_id.clone(),
        _ => None,
    });

    tx.execute(
        "INSERT INTO metadata_match_jev_calls
           (run_id, call_seq, call_kind, contract_version, state_schema_version,
            requested_model, response_model, enrichment_profile,
            state_json, questions_json, candidate_order_json, state_bytes,
            status, http_status, response_json, response_sha256, response_prefix,
            response_truncated, parsed_answer_json,
            selected_candidate_id, selected_cand_key, answered_none,
            input_tokens, output_tokens, latency_ms, retry_count,
            error_kind, error_text, request_id, eval_session_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'none',
                 ?8, ?9, ?10, ?11,
                 ?12, ?13, ?14, ?15, ?16,
                 ?17, ?18,
                 ?19, ?20, ?21,
                 ?22, ?23, ?24, ?25,
                 ?26, ?27, ?28, ?29)",
        rusqlite::params![
            input.run_id,
            input.call_seq,
            CALL_KIND,
            input.contract_version,
            input.state_schema_version,
            input.requested_model,
            transport.map(|t| t.response_model.clone()),
            state_json,
            questions_json,
            input.request.candidate_order_json,
            state_bytes,
            input.status,
            http_status,
            // 保存するのは C3B が redact 済みの capture だけ
            capture.and_then(|c| c.body_text()),
            capture.map(|c| c.sha256().to_string()),
            capture.map(|c| c.prefix().to_string()),
            i64::from(capture.map(|c| c.truncated()).unwrap_or(false)),
            parsed_answer_json,
            selected_id,
            selected_key,
            answered_none,
            transport.map(|t| t.input_tokens),
            transport.map(|t| t.output_tokens),
            latency_ms,
            retry_count,
            error_kind,
            error_text,
            request_id,
            input.session_id,
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

/// 検証済みの回答から、保存してよい値だけで JSON を組み直す。
///
/// 応答の生の JSON は使わない。入るのは cand_key（こちらが作った）、ローカルの
/// identity 由来の TMDB ID と media_type、数値、既知の選択肢キーだけ。自由文は入れない。
fn safe_parsed_answer_json(answers: &JevAnswers) -> String {
    let known_keys: BTreeSet<&str> = answers
        .same_work
        .iter()
        .map(|candidate| candidate.cand_key.as_str())
        .chain(std::iter::once(NONE_OPTION))
        .collect();

    let same_work: Vec<Value> = answers
        .same_work
        .iter()
        .map(|candidate| {
            json!({
                "cand_key": candidate.cand_key,
                "tmdb_id": candidate.tmdb_id,
                "media_type": candidate.media_type,
                "same_work": candidate.same_work,
            })
        })
        .collect();

    // 既知の選択肢キーだけを残す
    let mut probabilities = Map::new();
    for (key, value) in &answers.best_match_probabilities {
        if known_keys.contains(key.as_str()) {
            probabilities.insert(key.clone(), json!(value));
        }
    }

    canonical_json(&json!({
        "same_work": same_work,
        "best_match": answers.best_match.as_ref().map(|candidate| json!({
            "cand_key": candidate.cand_key,
            "tmdb_id": candidate.tmdb_id,
            "media_type": candidate.media_type,
        })),
        "best_match_probabilities": Value::Object(probabilities),
        "best_match_confidence": answers.best_match_confidence,
    }))
}

/// 候補スコアの意味づけ。**Noul は正解確率ではない**ことを機械可読に残す。
fn score_reasons_json() -> String {
    canonical_json(&json!({
        "source": "noul_same_work",
        "diagnostic_only": true,
        "calibrated": false,
    }))
}

/// 候補ごとの Noul 値を残す。Choice の確率とは混ぜない。
fn insert_candidate_scores(
    tx: &rusqlite::Transaction<'_>,
    run_id: i64,
    contract_version: &str,
    candidate_ids: &BTreeMap<String, i64>,
    call_id: i64,
    answers: &JevAnswers,
) -> Result<usize, JevShadowError> {
    // rank は Noul の降順。同値は cand_key の昇順で安定させる
    let mut ordered: Vec<&crate::services::jev_contract::CandidateProbability> =
        answers.same_work.iter().collect();
    ordered.sort_by(|a, b| {
        b.same_work
            .partial_cmp(&a.same_work)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.cand_key.cmp(&b.cand_key))
    });

    for (index, candidate) in ordered.iter().enumerate() {
        let candidate_id = candidate_ids.get(&candidate.cand_key).copied().ok_or_else(|| {
            JevShadowError::Precondition(format!(
                "{} に対応する候補行がありません",
                candidate.cand_key
            ))
        })?;
        tx.execute(
            "INSERT INTO metadata_match_candidate_scores
               (run_id, candidate_id, cand_key, matcher, matcher_version,
                score, rank, in_candidate_set, reasons_json, jev_call_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9)",
            rusqlite::params![
                run_id,
                candidate_id,
                candidate.cand_key,
                SCORE_MATCHER,
                contract_version,
                candidate.same_work,
                (index + 1) as i64,
                score_reasons_json(),
                call_id,
            ],
        )?;
    }
    Ok(ordered.len())
}

/// Jev の結論を shadow verdict として残す。
///
/// **`AUTO` は書かない。** 候補を選んだ場合でも「人が見るべき候補がある」という
/// `REVIEW` までで、本番の適用には使わない。選ばなければ `UNRESOLVED`。
/// `score` は入れない（Noul も Choice confidence も正解確率ではないため）。
/// upsert もしない。1 run に 1 行だけで、既存行は書き換えない。
fn insert_verdict(
    tx: &rusqlite::Transaction<'_>,
    run_id: i64,
    answers: &JevAnswers,
) -> Result<String, JevShadowError> {
    let (decision, tmdb_id, media_type, reason_codes) = match &answers.best_match {
        Some(candidate) => (
            "REVIEW",
            Some(candidate.tmdb_id),
            Some(candidate.media_type.clone()),
            ["jev_selected_candidate", "shadow_only", "uncalibrated"],
        ),
        None => (
            "UNRESOLVED",
            None,
            None,
            ["jev_answered_none", "shadow_only", "uncalibrated"],
        ),
    };

    // 自由文ではなく固定のコードだけを残す
    let reasons = canonical_json(&json!(reason_codes));

    tx.execute(
        "INSERT INTO metadata_match_verdicts
           (run_id, matcher, matcher_version, tmdb_id, media_type, decision, score, reasons_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
        rusqlite::params![
            run_id,
            VERDICT_MATCHER,
            VERDICT_MATCHER_VERSION,
            tmdb_id,
            media_type,
            decision,
            reasons,
        ],
    )?;
    Ok(decision.to_string())
}

/// 予算切れで送らなかったことを記録する。
///
/// [`PreparedSkippedCall`] を値で受け取るので、任意の run に skip 行を足すことは
/// できない。ローカルの検証を通った呼び出しだけがここへ来る。
///
/// `error_kind` は付けない（migration 023 の 10 値は transport の分類で、予算は
/// そこに含まれない）。代わりに `error_text` に [`BUDGET_EXHAUSTED_TEXT`] を入れる。
pub fn persist_skipped_call(
    conn: &mut Connection,
    skipped: PreparedSkippedCall,
) -> Result<CallRecord, JevShadowError> {
    let PreparedSkippedCall {
        session_id,
        run_id,
        requested_model,
        contract_version,
        state_schema_version,
        candidate_order_json,
        guard,
    } = skipped;

    let tx = conn.transaction()?;

    // 成功済みの run に後から skip を足さない
    if let Err(error) = check_no_packed_audit(&tx, run_id) {
        tx.rollback()?;
        return Err(error);
    }

    let call_seq: i64 = tx.query_row(
        "SELECT COALESCE(MAX(call_seq), 0) + 1 FROM metadata_match_jev_calls WHERE run_id = ?1",
        rusqlite::params![run_id],
        |row| row.get(0),
    )?;

    tx.execute(
        "INSERT INTO metadata_match_jev_calls
           (run_id, call_seq, call_kind, contract_version, state_schema_version,
            requested_model, enrichment_profile, state_json, questions_json,
            candidate_order_json, state_bytes, status, error_text, eval_session_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'none', ?7, ?8, ?9, 0, 'skipped', ?10, ?11)",
        rusqlite::params![
            run_id,
            call_seq,
            CALL_KIND,
            contract_version,
            state_schema_version,
            requested_model,
            EMPTY_JSON,
            EMPTY_JSON,
            candidate_order_json,
            BUDGET_EXHAUSTED_TEXT,
            session_id,
        ],
    )?;
    let call_id = tx.last_insert_rowid();
    tx.commit()?;
    drop(guard);

    Ok(CallRecord {
        call_id,
        run_id,
        call_seq,
        status: "skipped".to_string(),
        candidate_scores: 0,
        verdict_decision: None,
    })
}

/// 記録済みの call を読む（テストと監査用）。
pub fn load_call(conn: &Connection, call_id: i64) -> Result<Option<Value>, JevShadowError> {
    let row = conn
        .query_row(
            "SELECT status, call_kind, response_model, http_status, request_id,
                    retry_count, latency_ms, input_tokens, output_tokens,
                    error_kind, error_text, answered_none, selected_cand_key,
                    eval_session_id, response_sha256, response_truncated,
                    state_json, questions_json, state_bytes, parsed_answer_json,
                    response_json, response_prefix, candidate_order_json
               FROM metadata_match_jev_calls WHERE id = ?1",
            rusqlite::params![call_id],
            |row| {
                Ok(json!({
                    "status": row.get::<_, String>(0)?,
                    "call_kind": row.get::<_, String>(1)?,
                    "response_model": row.get::<_, Option<String>>(2)?,
                    "http_status": row.get::<_, Option<i64>>(3)?,
                    "request_id": row.get::<_, Option<String>>(4)?,
                    "retry_count": row.get::<_, i64>(5)?,
                    "latency_ms": row.get::<_, Option<i64>>(6)?,
                    "input_tokens": row.get::<_, Option<i64>>(7)?,
                    "output_tokens": row.get::<_, Option<i64>>(8)?,
                    "error_kind": row.get::<_, Option<String>>(9)?,
                    "error_text": row.get::<_, Option<String>>(10)?,
                    "answered_none": row.get::<_, i64>(11)?,
                    "selected_cand_key": row.get::<_, Option<String>>(12)?,
                    "eval_session_id": row.get::<_, Option<i64>>(13)?,
                    "response_sha256": row.get::<_, Option<String>>(14)?,
                    "response_truncated": row.get::<_, i64>(15)?,
                    "state_json": row.get::<_, String>(16)?,
                    "questions_json": row.get::<_, String>(17)?,
                    "state_bytes": row.get::<_, Option<i64>>(18)?,
                    "parsed_answer_json": row.get::<_, Option<String>>(19)?,
                    "response_json": row.get::<_, Option<String>>(20)?,
                    "response_prefix": row.get::<_, Option<String>>(21)?,
                    "candidate_order_json": row.get::<_, String>(22)?,
                }))
            },
        )
        .optional()?;
    Ok(row)
}

/// 監査テーブル全体を 1 本の文字列にする（漏洩チェック用）。
pub fn audit_dump(conn: &Connection) -> Result<String, JevShadowError> {
    let mut dump = String::new();
    for sql in [
        "SELECT * FROM metadata_match_jev_calls",
        "SELECT * FROM metadata_match_candidate_scores",
        "SELECT * FROM metadata_match_verdicts",
        "SELECT * FROM jev_eval_sessions",
    ] {
        let mut stmt = conn.prepare(sql)?;
        let count = stmt.column_count();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            for index in 0..count {
                let value = row.get_ref(index)?;
                match value {
                    rusqlite::types::ValueRef::Text(text) => {
                        dump.push_str(&String::from_utf8_lossy(text));
                    }
                    rusqlite::types::ValueRef::Integer(number) => {
                        dump.push_str(&number.to_string());
                    }
                    rusqlite::types::ValueRef::Real(number) => {
                        dump.push_str(&number.to_string());
                    }
                    rusqlite::types::ValueRef::Blob(bytes) => {
                        dump.push_str(&String::from_utf8_lossy(bytes));
                    }
                    rusqlite::types::ValueRef::Null => {}
                }
                dump.push('\u{1f}');
            }
        }
    }
    Ok(dump)
}

// ─── テスト ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::services::jev_session::{
        apply_preflight, close_session, plan_session, preflight_model, JevSessionConfig,
    };
    use crate::services::typesafe_client::TypeSafeClient;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const MODEL: &str = "jev-test-model-1";
    const KEY: &str = "sk-test-DO-NOT-LEAK-0123456789";

    // ─── stub サーバ（外部ネットワークへは出ない）────────────────────────

    #[derive(Clone)]
    enum Reply {
        Json(u16, String),
        /// 応答前に待つ（逐次性の確認用）
        Slow(Duration, String),
        Drop,
    }

    struct Stub {
        base: String,
        hits: Arc<AtomicUsize>,
    }

    impl Stub {
        /// SystemOne の回数（最初の 1 件は /v1/models）
        fn systemone_hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst).saturating_sub(1)
        }
    }

    async fn start_stub(replies: Vec<Reply>) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_task = hits.clone();

        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let index = hits_task.fetch_add(1, Ordering::SeqCst);
                let reply = replies
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| replies.last().cloned().unwrap_or(Reply::Drop));
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 65536];
                    let _ = socket.read(&mut buffer).await;
                    let body = match reply {
                        Reply::Drop => None,
                        Reply::Json(status, body) => Some((status, body)),
                        Reply::Slow(delay, body) => {
                            tokio::time::sleep(delay).await;
                            Some((200, body))
                        }
                    };
                    if let Some((status, body)) = body {
                        let response = format!(
                            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                             x-typesafe-request-id: req-{index}\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                    }
                    let _ = socket.flush().await;
                    let _ = socket.shutdown().await;
                });
            }
        });

        Stub { base: format!("http://127.0.0.1:{port}"), hits }
    }

    fn models_body() -> String {
        json!({"models": [
            {"name": MODEL, "description": "test", "release_date": "2026-01-01"}
        ]})
        .to_string()
    }

    /// 候補 K 件の packed 応答
    fn answers_body(count: usize, choice: &str) -> String {
        answers_body_with(count, choice, None, 120, 9)
    }

    fn answers_body_with(
        count: usize,
        choice: &str,
        echo: Option<&str>,
        input_tokens: i64,
        output_tokens: i64,
    ) -> String {
        let mut answers = Map::new();
        for index in 1..=count {
            answers.insert(
                format!("c{index}_same_work"),
                json!({"type": "noul", "noul": 0.9 - 0.1 * index as f64}),
            );
        }
        let mut probabilities = Map::new();
        for index in 1..=count {
            probabilities.insert(format!("c{index}"), json!(0.6 / count as f64));
        }
        probabilities.insert("NONE".to_string(), json!(0.4));
        answers.insert(
            "best_match".to_string(),
            json!({
                "type": "choice",
                "choice": choice,
                "confidence": 0.72,
                "probabilities": Value::Object(probabilities)
            }),
        );

        let mut root = Map::new();
        root.insert("model".to_string(), json!(MODEL));
        root.insert("answers".to_string(), Value::Object(answers));
        root.insert(
            "usage".to_string(),
            json!({"input_tokens": input_tokens, "output_tokens": output_tokens}),
        );
        if let Some(echo) = echo {
            // 応答の無害な別フィールドにキーが echo された状況
            root.insert("debug_note".to_string(), json!(format!("received {echo}")));
        }
        Value::Object(root).to_string()
    }

    // ─── fixtures ────────────────────────────────────────────────────────

    fn open_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        db::apply_migrations(&conn).unwrap();
        conn
    }

    async fn client_with(replies: Vec<Reply>) -> (TypeSafeClient, Stub) {
        let mut all = vec![Reply::Json(200, models_body())];
        all.extend(replies);
        let stub = start_stub(all).await;
        let client = TypeSafeClient::with_key_for_test(KEY, &stub.base);
        (client, stub)
    }

    async fn open_session(conn: &Connection, client: &TypeSafeClient, key: &str) -> JevSession {
        let plan = plan_session(conn, &JevSessionConfig::new(key, MODEL)).unwrap();
        let outcome = preflight_model(client, &plan).await;
        apply_preflight(conn, &plan, outcome).unwrap()
    }

    fn insert_run_with(conn: &Connection, mode: &str, applied: i64) -> i64 {
        conn.execute(
            "INSERT INTO works (title, work_type, media_category)
             VALUES ('テスト作品', 'movie', 'movie')",
            [],
        )
        .unwrap();
        let work_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO metadata_match_runs
               (work_id, trigger_kind, mode, evidence_class, state_schema_version,
                input_snapshot_json, search_queries_json, policy_version,
                policy_snapshot_json, governing_matcher, decision, decision_reasons_json,
                applied, applied_tmdb_id, applied_media_type)
             VALUES (?1, 'single', ?2, 'live', 'cm-prematch-2',
                     '{}', '[]', 'rules-safe-2', '{}', 'rules-safe', 'UNRESOLVED', '[]',
                     ?3, ?4, ?5)",
            rusqlite::params![
                work_id,
                mode,
                applied,
                if applied == 1 { Some(99i64) } else { None },
                if applied == 1 { Some("movie") } else { None },
            ],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn insert_run(conn: &Connection) -> i64 {
        insert_run_with(conn, "shadow", 0)
    }

    fn insert_candidates(conn: &Connection, run_id: i64, count: usize) -> Vec<ShadowCandidate> {
        let mut candidates = Vec::new();
        for index in 1..=count {
            let tmdb_id = 1000 + index as i64;
            conn.execute(
                "INSERT INTO metadata_match_candidates
                   (run_id, cand_key, tmdb_id, media_type, rules_score,
                    rules_reasons_json, tmdb_snapshot_json)
                 VALUES (?1, ?2, ?3, 'movie', 0, '[]', '{}')",
                rusqlite::params![run_id, format!("c{index}"), tmdb_id],
            )
            .unwrap();
            candidates.push(ShadowCandidate {
                candidate_id: conn.last_insert_rowid(),
                input: CandidateInput {
                    tmdb_id,
                    media_type: "movie".to_string(),
                    title: format!("候補 {index}"),
                    original_title: Some(format!("Candidate {index}")),
                    year: Some(2020 + index as i32),
                    original_language: Some("ja".to_string()),
                },
            });
        }
        candidates
    }

    fn local_evidence() -> LocalEvidenceInput {
        LocalEvidenceInput {
            derived_title: Some("テスト作品".to_string()),
            embedded_title: Some("Test Work".to_string()),
            filename_year: Some(2021),
            embedded_year: Some(2021),
            media_kind: Some("movie".to_string()),
            country_type: Some("domestic".to_string()),
            season_no: None,
            episode_no: None,
            part_hint: None,
            edition_markers: vec![],
            audio_languages: vec!["ja".to_string()],
            subtitle_languages: vec!["ja".to_string()],
            cast: vec!["山田 太郎".to_string()],
            evidence_class: None,
            tag_provenance: Some("pipeline_known".to_string()),
        }
    }

    fn call_input(conn: &Connection, run_id: i64, count: usize) -> ShadowCallInput {
        ShadowCallInput {
            run_id,
            local: local_evidence(),
            candidates: insert_candidates(conn, run_id, count),
        }
    }

    /// 1 回分を最後まで通す
    async fn run_one(
        conn: &mut Connection,
        client: &TypeSafeClient,
        session_id: i64,
        input: &ShadowCallInput,
    ) -> Result<CallRecord, JevShadowError> {
        match prepare_call(conn, session_id, input).await? {
            PrepareCallResult::Ready(prepared) => {
                let executed = execute_call(client, prepared).await;
                persist_call(conn, executed)
            }
            PrepareCallResult::Skipped(skipped) => persist_skipped_call(conn, skipped),
        }
    }

    /// Ready であることを前提に取り出す
    async fn prepare_ready(
        conn: &mut Connection,
        session_id: i64,
        input: &ShadowCallInput,
    ) -> PreparedCall {
        match prepare_call(conn, session_id, input).await.unwrap() {
            PrepareCallResult::Ready(prepared) => prepared,
            PrepareCallResult::Skipped(skipped) => {
                panic!("Ready のはずが skip された: {skipped:?}")
            }
        }
    }

    // ─── 送る内容 ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_packed_call_sends_state_and_questions_only() {
        let mut conn = open_db();
        let (client, stub) = client_with(vec![Reply::Json(200, answers_body(3, "c2"))]).await;
        let session = open_session(&conn, &client, "shadow-send").await;
        let run_id = insert_run(&conn);
        let input = call_input(&conn, run_id, 3);

        let prepared = prepare_ready(&mut conn, session.id, &input).await;
        assert_eq!(prepared.candidate_order(), ["c1", "c2", "c3"]);
        assert_eq!(prepared.requested_model(), MODEL);

        // 送る state に禁止項目が無いこと
        let raw = prepared.state_json().to_lowercase();
        for forbidden in ["file_path", "original_file_name", "provider_hint", "overview", KEY] {
            assert!(!raw.contains(&forbidden.to_lowercase()), "{forbidden} が state にある");
        }
        // run の evidence_class が入る
        let state: Value = serde_json::from_str(prepared.state_json()).unwrap();
        assert_eq!(state["local"]["evidence_class"], "live");

        let questions = prepared.questions_value().as_object().unwrap();
        assert_eq!(questions.len(), 4);

        let executed = execute_call(&client, prepared).await;
        assert_eq!(executed.status(), "ok");
        let record = persist_call(&mut conn, executed).unwrap();
        assert_eq!(record.status, "ok");
        assert_eq!(stub.systemone_hits(), 1);
    }

    // ─── 保存される内容 ──────────────────────────────────────────────────

    #[tokio::test]
    async fn a_validated_call_is_recorded_with_scores_and_a_review_verdict() {
        let mut conn = open_db();
        let (client, _stub) = client_with(vec![Reply::Json(200, answers_body(3, "c2"))]).await;
        let session = open_session(&conn, &client, "shadow-ok").await;
        let run_id = insert_run(&conn);
        let input = call_input(&conn, run_id, 3);

        let record = run_one(&mut conn, &client, session.id, &input).await.unwrap();
        assert_eq!(record.status, "ok");
        assert_eq!(record.call_seq, 1);
        assert_eq!(record.candidate_scores, 3);
        assert_eq!(record.verdict_decision.as_deref(), Some("REVIEW"));

        let call = load_call(&conn, record.call_id).unwrap().unwrap();
        assert_eq!(call["status"], "ok");
        assert_eq!(call["call_kind"], "packed");
        assert_eq!(call["response_model"], MODEL);
        assert_eq!(call["http_status"], 200);
        assert_eq!(call["input_tokens"], 120);
        assert_eq!(call["output_tokens"], 9);
        assert_eq!(call["answered_none"], 0);
        assert_eq!(call["selected_cand_key"], "c2");
        assert_eq!(call["eval_session_id"], session.id);
        assert_eq!(call["error_kind"], Value::Null);
        assert_eq!(call["candidate_order_json"], "[\"c1\",\"c2\",\"c3\"]");
        assert!(call["request_id"].as_str().unwrap().starts_with("req-"));
        assert!(call["response_sha256"].as_str().is_some());
        assert!(call["state_bytes"].as_i64().unwrap() > 0);

        // parsed_answer_json はこちらで組み直した安全な形
        let parsed: Value =
            serde_json::from_str(call["parsed_answer_json"].as_str().unwrap()).unwrap();
        assert_eq!(parsed["best_match"]["cand_key"], "c2");
        assert_eq!(parsed["best_match"]["tmdb_id"], 1002);
        assert_eq!(parsed["same_work"].as_array().unwrap().len(), 3);
        assert!(parsed["best_match_confidence"].is_number());

        let mut stmt = conn
            .prepare(
                "SELECT cand_key, matcher, matcher_version, score, rank, in_candidate_set,
                        jev_call_id
                   FROM metadata_match_candidate_scores WHERE run_id = ?1 ORDER BY rank",
            )
            .unwrap();
        let rows: Vec<(String, String, String, f64, i64, i64, i64)> = stmt
            .query_map(rusqlite::params![run_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].0, "c1");
        assert_eq!(rows[2].0, "c3");
        for row in &rows {
            assert_eq!(row.1, "jev-call");
            assert_eq!(row.2, "jev-contract-2");
            assert_eq!(row.5, 1);
            assert_eq!(row.6, record.call_id);
        }

        let (matcher, version, decision, tmdb_id, score): (
            String,
            String,
            String,
            Option<i64>,
            Option<f64>,
        ) = conn
            .query_row(
                "SELECT matcher, matcher_version, decision, tmdb_id, score
                   FROM metadata_match_verdicts WHERE run_id = ?1",
                rusqlite::params![run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(matcher, "jev-policy");
        assert_eq!(version, "jev-policy-1");
        assert_eq!(decision, "REVIEW");
        assert_eq!(tmdb_id, Some(1002));
        assert_eq!(score, None, "score は入れない");
    }

    #[tokio::test]
    async fn answering_none_is_recorded_as_unresolved() {
        let mut conn = open_db();
        let (client, _stub) = client_with(vec![Reply::Json(200, answers_body(2, "NONE"))]).await;
        let session = open_session(&conn, &client, "shadow-none").await;
        let run_id = insert_run(&conn);
        let input = call_input(&conn, run_id, 2);

        let record = run_one(&mut conn, &client, session.id, &input).await.unwrap();
        assert_eq!(record.verdict_decision.as_deref(), Some("UNRESOLVED"));

        let call = load_call(&conn, record.call_id).unwrap().unwrap();
        assert_eq!(call["answered_none"], 1);
        assert_eq!(call["selected_cand_key"], Value::Null);

        let (decision, tmdb_id): (String, Option<i64>) = conn
            .query_row(
                "SELECT decision, tmdb_id FROM metadata_match_verdicts WHERE run_id = ?1",
                rusqlite::params![run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(decision, "UNRESOLVED");
        assert_eq!(tmdb_id, None);
    }

    #[tokio::test]
    async fn the_shadow_verdict_is_never_auto() {
        let mut conn = open_db();
        let (client, _stub) = client_with(vec![
            Reply::Json(200, answers_body(1, "c1")),
            Reply::Json(200, answers_body(1, "NONE")),
        ])
        .await;
        let session = open_session(&conn, &client, "shadow-no-auto").await;

        for _ in 0..2 {
            let run_id = insert_run(&conn);
            let input = call_input(&conn, run_id, 1);
            run_one(&mut conn, &client, session.id, &input).await.unwrap();
        }

        let autos: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata_match_verdicts WHERE decision = 'AUTO'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(autos, 0);
    }

    // ─── 1 run 1 packed ──────────────────────────────────────────────────

    #[tokio::test]
    async fn a_run_accepts_only_one_packed_call() {
        let mut conn = open_db();
        let (client, stub) = client_with(vec![Reply::Json(200, answers_body(2, "c1"))]).await;
        let session = open_session(&conn, &client, "shadow-once").await;
        let run_id = insert_run(&conn);
        let input = call_input(&conn, run_id, 2);

        run_one(&mut conn, &client, session.id, &input).await.unwrap();
        let reserved_after_first = load_session(&conn, session.id).unwrap().calls_reserved;

        // 2 回目は送らずに落ちる
        let error = run_one(&mut conn, &client, session.id, &input).await.unwrap_err();
        assert_eq!(error, JevShadowError::DuplicatePackedCall { run_id });
        assert_eq!(stub.systemone_hits(), 1, "2 回目で HTTP を出している");
        assert_eq!(
            load_session(&conn, session.id).unwrap().calls_reserved,
            reserved_after_first,
            "2 回目で予算を使っている"
        );

        let (calls, verdicts, scores): (i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM metadata_match_jev_calls WHERE run_id = ?1),
                        (SELECT COUNT(*) FROM metadata_match_verdicts WHERE run_id = ?1),
                        (SELECT COUNT(*) FROM metadata_match_candidate_scores WHERE run_id = ?1)",
                rusqlite::params![run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(verdicts, 1);
        assert_eq!(scores, 2);
    }

    /// verdict だけ先にある run も弾く（upsert しない）
    #[tokio::test]
    async fn an_existing_jev_verdict_blocks_a_new_call() {
        let mut conn = open_db();
        let (client, stub) = client_with(vec![Reply::Json(200, answers_body(1, "c1"))]).await;
        let session = open_session(&conn, &client, "shadow-verdict-exists").await;
        let run_id = insert_run(&conn);
        conn.execute(
            "INSERT INTO metadata_match_verdicts
               (run_id, matcher, matcher_version, decision, reasons_json)
             VALUES (?1, 'jev-policy', 'jev-policy-1', 'UNRESOLVED', '{}')",
            rusqlite::params![run_id],
        )
        .unwrap();
        let input = call_input(&conn, run_id, 1);

        assert_eq!(
            run_one(&mut conn, &client, session.id, &input).await.unwrap_err(),
            JevShadowError::DuplicatePackedCall { run_id }
        );
        assert_eq!(stub.systemone_hits(), 0);
        assert_eq!(load_session(&conn, session.id).unwrap().calls_reserved, 0);
    }

    // ─── 逐次実行 ────────────────────────────────────────────────────────

    /// 同じ session で 2 本同時に始めても、逐次に直列化される。
    /// 先に走った A がトークン上限を超え、B は送信せず予算切れになる。
    #[tokio::test]
    async fn concurrent_calls_in_one_session_are_serialised() {
        let path = std::env::temp_dir().join(format!(
            "cinemantis-jev-shadow-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);

        // A はゆっくり応答し、その 1 回でトークン上限を超える
        let (client, stub) = client_with(vec![Reply::Slow(
            Duration::from_millis(300),
            answers_body_with(1, "c1", None, 500, 5),
        )])
        .await;
        let client = Arc::new(client);

        let (session_id, run_a, run_b) = {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
                .unwrap();
            db::apply_migrations(&conn).unwrap();

            let mut config = JevSessionConfig::new("shadow-serial", MODEL);
            config.max_input_tokens = 100;
            let plan = plan_session(&conn, &config).unwrap();
            let outcome = preflight_model(&client, &plan).await;
            let session = apply_preflight(&conn, &plan, outcome).unwrap();
            (session.id, insert_run(&conn), insert_run(&conn))
        };

        let spawn_call = |run_id: i64, client: Arc<TypeSafeClient>, path: std::path::PathBuf| {
            tokio::spawn(async move {
                let mut conn = Connection::open(&path).unwrap();
                conn.busy_timeout(Duration::from_secs(5)).unwrap();
                conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
                let candidates = insert_candidates(&conn, run_id, 1);
                let input = ShadowCallInput { run_id, local: local_evidence(), candidates };
                run_one(&mut conn, &client, session_id, &input).await
            })
        };

        let task_a = spawn_call(run_a, client.clone(), path.clone());
        // A が先にロックを取るよう、わずかにずらす
        tokio::time::sleep(Duration::from_millis(50)).await;
        let task_b = spawn_call(run_b, client.clone(), path.clone());

        let result_a = task_a.await.unwrap();
        let result_b = task_b.await.unwrap();

        assert_eq!(result_a.as_ref().map(|r| r.status.as_str()), Ok("ok"), "{result_a:?}");
        assert_eq!(
            result_b.as_ref().map(|r| r.status.as_str()),
            Ok("skipped"),
            "B が送れてしまった: {result_b:?}"
        );
        assert_eq!(stub.systemone_hits(), 1, "同時に 2 本 in-flight になっている");

        let conn = Connection::open(&path).unwrap();
        let session = load_session(&conn, session_id).unwrap();
        assert_eq!(session.input_tokens_used, 500, "A の分だけ");
        assert_eq!(session.calls_reserved, 1, "B で予約が増えている");
        assert_eq!(session.status, "exhausted");

        // B は送らずに skip として既に記録されている
        let call = load_call(&conn, result_b.unwrap().call_id).unwrap().unwrap();
        assert_eq!(call["status"], "skipped");
        assert_eq!(call["state_json"], "{}");
        assert_eq!(call["questions_json"], "{}");
        assert_eq!(call["state_bytes"], 0);
        assert_eq!(call["error_text"], BUDGET_EXHAUSTED_TEXT);
        assert_eq!(call["candidate_order_json"], "[\"c1\"]", "凍結した並びを残す");

        // run ごとに packed は 1 件
        for run_id in [run_a, run_b] {
            let calls: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM metadata_match_jev_calls WHERE run_id = ?1",
                    rusqlite::params![run_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(calls, 1, "run {run_id}");
        }

        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    // ─── 失敗の記録 ──────────────────────────────────────────────────────

    /// transport の 10 種すべてを status へ写す
    #[test]
    fn transport_kinds_map_to_call_status() {
        let cases = [
            (TypeSafeErrorKind::Auth, "unavailable"),
            (TypeSafeErrorKind::RateLimit, "unavailable"),
            (TypeSafeErrorKind::Overloaded, "unavailable"),
            (TypeSafeErrorKind::Network, "unavailable"),
            (TypeSafeErrorKind::Timeout, "unavailable"),
            (TypeSafeErrorKind::Unprocessable, "error"),
            (TypeSafeErrorKind::Parse, "error"),
            (TypeSafeErrorKind::ResponseTooLarge, "error"),
            (TypeSafeErrorKind::ModelMismatch, "error"),
            (TypeSafeErrorKind::Other, "error"),
        ];
        for (kind, expected) in cases {
            assert_eq!(transport_status(kind), expected, "{}", kind.as_str());
        }
    }

    #[tokio::test]
    async fn transport_failures_are_recorded() {
        let cases: Vec<(&str, Reply, &str, &str)> = vec![
            ("401", Reply::Json(401, "{}".to_string()), "unavailable", "auth"),
            ("429", Reply::Json(429, "{}".to_string()), "unavailable", "rate_limit"),
            ("503", Reply::Json(503, "{}".to_string()), "unavailable", "overloaded"),
            ("422", Reply::Json(422, "{}".to_string()), "error", "unprocessable"),
            ("404", Reply::Json(404, "{}".to_string()), "error", "other"),
            ("drop", Reply::Drop, "unavailable", "network"),
        ];

        for (label, reply, expected_status, expected_kind) in cases {
            let mut conn = open_db();
            let (client, _stub) = client_with(vec![reply]).await;
            let session = open_session(&conn, &client, &format!("shadow-{label}")).await;
            let run_id = insert_run(&conn);
            let input = call_input(&conn, run_id, 2);

            let record = run_one(&mut conn, &client, session.id, &input).await.unwrap();
            assert_eq!(record.status, expected_status, "{label}");
            assert_eq!(record.candidate_scores, 0, "{label}");
            assert_eq!(record.verdict_decision, None, "{label}");

            let call = load_call(&conn, record.call_id).unwrap().unwrap();
            assert_eq!(call["error_kind"], expected_kind, "{label}");
            assert_eq!(call["input_tokens"], Value::Null, "{label}");
            let session_after = load_session(&conn, session.id).unwrap();
            assert_eq!(session_after.calls_reserved, 1, "{label}: 予約は戻さない");
            assert_eq!(session_after.input_tokens_used, 0, "{label}: token は不明");
        }
    }

    /// contract に合わない応答は invalid。理由は固定値で、モデルの文字列は残さない
    #[tokio::test]
    async fn an_invalid_response_stores_a_fixed_reason() {
        // choice label にキー相当の文字列を入れてくる応答
        let body = json!({
            "model": MODEL,
            "answers": {
                "c1_same_work": {"type": "noul", "noul": 0.9},
                "best_match": {
                    "type": "choice",
                    "choice": KEY,
                    "confidence": 0.5,
                    "probabilities": {"c1": 0.5, "NONE": 0.5}
                }
            },
            "usage": {"input_tokens": 30, "output_tokens": 2}
        })
        .to_string();

        let mut conn = open_db();
        let (client, _stub) = client_with(vec![Reply::Json(200, body)]).await;
        let session = open_session(&conn, &client, "shadow-invalid").await;
        let run_id = insert_run(&conn);
        let input = call_input(&conn, run_id, 1);

        let record = run_one(&mut conn, &client, session.id, &input).await.unwrap();
        assert_eq!(record.status, "invalid");
        assert_eq!(record.candidate_scores, 0);
        assert_eq!(record.verdict_decision, None);

        let call = load_call(&conn, record.call_id).unwrap().unwrap();
        assert_eq!(call["error_kind"], Value::Null);
        assert_eq!(call["error_text"], VALIDATION_FAILED_TEXT);
        assert_eq!(call["parsed_answer_json"], Value::Null);
        // HTTP は成立しているので token は記録する
        assert_eq!(call["input_tokens"], 30);
        assert_eq!(load_session(&conn, session.id).unwrap().input_tokens_used, 30);

        // 応答由来の文字列は capture 以外に残っていない
        assert!(!call["error_text"].as_str().unwrap().contains(KEY));
        assert!(!call["state_json"].as_str().unwrap().contains(KEY));
    }

    // ─── シークレット ────────────────────────────────────────────────────

    /// 送信前に C3B が止めた場合、その request を保存しない
    #[tokio::test]
    async fn a_request_stopped_before_sending_is_not_stored() {
        let mut conn = open_db();
        let (client, stub) = client_with(vec![Reply::Json(200, answers_body(1, "c1"))]).await;
        let session = open_session(&conn, &client, "shadow-secret-request").await;
        let run_id = insert_run(&conn);

        // state にキーそのものを混ぜる
        let mut local = local_evidence();
        local.derived_title = Some(format!("作品 {KEY}"));
        let input = ShadowCallInput {
            run_id,
            local,
            candidates: insert_candidates(&conn, run_id, 1),
        };

        let record = run_one(&mut conn, &client, session.id, &input).await.unwrap();
        assert_eq!(record.status, "unavailable");
        assert_eq!(stub.systemone_hits(), 0, "送ってしまっている");

        let call = load_call(&conn, record.call_id).unwrap().unwrap();
        assert_eq!(call["error_kind"], "auth");
        assert_eq!(call["state_json"], "{}");
        assert_eq!(call["questions_json"], "{}");
        assert_eq!(call["state_bytes"], 0);

        // 監査テーブルのどこにもキーが無い
        let dump = audit_dump(&conn).unwrap();
        assert!(!dump.contains(KEY), "監査行にキーが残っている");
    }

    /// 応答がキーを echo しても、保存側には残らない
    #[tokio::test]
    async fn an_echoed_secret_is_redacted_in_the_capture() {
        let body = answers_body_with(2, "c1", Some(KEY), 40, 3);
        let mut conn = open_db();
        let (client, _stub) = client_with(vec![Reply::Json(200, body.clone())]).await;
        let session = open_session(&conn, &client, "shadow-echo").await;
        let run_id = insert_run(&conn);
        let input = call_input(&conn, run_id, 2);

        let record = run_one(&mut conn, &client, session.id, &input).await.unwrap();
        assert_eq!(record.status, "ok", "意味の処理は通る");

        let call = load_call(&conn, record.call_id).unwrap().unwrap();
        assert!(!call["response_json"].as_str().unwrap().contains(KEY));
        assert!(!call["response_prefix"].as_str().unwrap().contains(KEY));
        assert!(call["response_json"].as_str().unwrap().contains("[REDACTED]"));
        assert!(call["response_sha256"].as_str().is_some());

        let dump = audit_dump(&conn).unwrap();
        assert!(!dump.contains(KEY), "監査行にキーが残っている");
    }

    // ─── 前提条件 ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn runs_that_are_not_shadow_are_rejected() {
        let mut conn = open_db();
        let (client, stub) = client_with(vec![Reply::Json(200, answers_body(1, "c1"))]).await;
        let session = open_session(&conn, &client, "shadow-run-precondition").await;

        for (label, mode, applied) in [
            ("safe", "safe", 0),
            ("gated", "gated", 0),
            ("applied", "shadow", 1),
        ] {
            let run_id = insert_run_with(&conn, mode, applied);
            let input = call_input(&conn, run_id, 1);
            let error = prepare_call(&mut conn, session.id, &input).await.unwrap_err();
            assert!(
                matches!(error, JevShadowError::Precondition(_)),
                "{label}: {error}"
            );
        }

        // 存在しない run
        let missing = ShadowCallInput {
            run_id: 999_999,
            local: local_evidence(),
            candidates: vec![ShadowCandidate {
                candidate_id: 1,
                input: CandidateInput {
                    tmdb_id: 1,
                    media_type: "movie".to_string(),
                    title: "x".to_string(),
                    original_title: None,
                    year: None,
                    original_language: None,
                },
            }],
        };
        assert!(matches!(
            prepare_call(&mut conn, session.id, &missing).await.unwrap_err(),
            JevShadowError::Precondition(_)
        ));

        assert_eq!(stub.systemone_hits(), 0);
        assert_eq!(load_session(&conn, session.id).unwrap().calls_reserved, 0);
    }

    #[tokio::test]
    async fn sessions_that_are_not_open_are_rejected() {
        let mut conn = open_db();
        // この test は session を何度も開くので、stub は常に models を返す
        let stub = start_stub(vec![Reply::Json(200, models_body())]).await;
        let client = TypeSafeClient::with_key_for_test(KEY, &stub.base);

        // closed
        let closed = open_session(&conn, &client, "shadow-closed").await;
        close_session(&conn, closed.id).unwrap();
        let run_id = insert_run(&conn);
        let input = call_input(&conn, run_id, 1);
        assert_eq!(
            prepare_call(&mut conn, closed.id, &input).await.unwrap_err(),
            JevShadowError::Session(JevSessionError::SessionClosed)
        );

        // exhausted
        let exhausted = open_session(&conn, &client, "shadow-exhausted").await;
        conn.execute(
            "UPDATE jev_eval_sessions SET status = 'exhausted' WHERE id = ?1",
            rusqlite::params![exhausted.id],
        )
        .unwrap();
        let run_b = insert_run(&conn);
        let input_b = call_input(&conn, run_b, 1);
        // 予算切れはエラーではなく skip 候補になる（送らないことは同じ）
        assert!(matches!(
            prepare_call(&mut conn, exhausted.id, &input_b).await.unwrap(),
            PrepareCallResult::Skipped(_)
        ));

        // blocked
        let blocked = open_session(&conn, &client, "shadow-blocked").await;
        conn.execute(
            "UPDATE jev_eval_sessions
                SET status = 'blocked', blocked_reason = 'model preflight failed: overloaded'
              WHERE id = ?1",
            rusqlite::params![blocked.id],
        )
        .unwrap();
        let run_c = insert_run(&conn);
        let input_c = call_input(&conn, run_c, 1);
        assert!(matches!(
            prepare_call(&mut conn, blocked.id, &input_c).await.unwrap_err(),
            JevShadowError::Session(JevSessionError::Blocked { .. })
        ));

        // contract / state schema が違う
        let wrong = open_session(&conn, &client, "shadow-wrong-contract").await;
        conn.execute(
            "INSERT OR IGNORE INTO jev_contracts
               (contract_version, state_schema_version, instructions_text,
                questions_template_json, criteria_json, validation_policy_json,
                canonical_sha256, default_model)
             VALUES ('jev-contract-1','cm-jev-state-1','x','{}','{}','{}','y',?1)",
            rusqlite::params![MODEL],
        )
        .unwrap();
        conn.execute(
            "UPDATE jev_eval_sessions SET contract_version = 'jev-contract-1' WHERE id = ?1",
            rusqlite::params![wrong.id],
        )
        .unwrap();
        let run_d = insert_run(&conn);
        let input_d = call_input(&conn, run_d, 1);
        assert!(matches!(
            prepare_call(&mut conn, wrong.id, &input_d).await.unwrap_err(),
            JevShadowError::Precondition(_)
        ));

        conn.execute(
            "UPDATE jev_eval_sessions
                SET contract_version = 'jev-contract-2', state_schema_version = 'cm-jev-state-9'
              WHERE id = ?1",
            rusqlite::params![wrong.id],
        )
        .unwrap();
        let run_e = insert_run(&conn);
        let input_e = call_input(&conn, run_e, 1);
        assert!(matches!(
            prepare_call(&mut conn, wrong.id, &input_e).await.unwrap_err(),
            JevShadowError::Precondition(_)
        ));

        // systemone は 1 度も呼ばれていない（call 行が 1 件も無い）
        let calls: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_match_jev_calls", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(calls, 0);
    }

    #[tokio::test]
    async fn the_evidence_class_must_match_the_run() {
        let mut conn = open_db();
        let (client, stub) = client_with(vec![Reply::Json(200, answers_body(1, "c1"))]).await;
        let session = open_session(&conn, &client, "shadow-evidence").await;
        let run_id = insert_run(&conn);

        let mut local = local_evidence();
        local.evidence_class = Some("historical_audit_only".to_string());
        let input = ShadowCallInput {
            run_id,
            local,
            candidates: insert_candidates(&conn, run_id, 1),
        };
        assert!(matches!(
            prepare_call(&mut conn, session.id, &input).await.unwrap_err(),
            JevShadowError::Precondition(_)
        ));
        assert_eq!(stub.systemone_hits(), 0);
        assert_eq!(load_session(&conn, session.id).unwrap().calls_reserved, 0);

        // 同じ値なら通る
        let mut same = local_evidence();
        same.evidence_class = Some("live".to_string());
        let ok_input = ShadowCallInput {
            run_id,
            local: same,
            candidates: input.candidates.clone(),
        };
        let prepared = prepare_ready(&mut conn, session.id, &ok_input).await;
        assert!(prepared.state_json().contains("live"));
    }

    // ─── 候補の突き合わせ ────────────────────────────────────────────────

    #[tokio::test]
    async fn tampered_candidates_are_rejected_before_any_cost() {
        let mut conn = open_db();
        let (client, stub) = client_with(vec![Reply::Json(200, answers_body(2, "c1"))]).await;
        let session = open_session(&conn, &client, "shadow-binding").await;
        let run_id = insert_run(&conn);
        let base = insert_candidates(&conn, run_id, 2);

        // 別 run の候補
        let other_run = insert_run(&conn);
        let other = insert_candidates(&conn, other_run, 1);

        let mut cases: Vec<(&str, Vec<ShadowCandidate>)> = Vec::new();

        let mut wrong_id = base.clone();
        wrong_id[0].candidate_id = 999_999;
        cases.push(("candidate_id", wrong_id));

        let mut swapped = base.clone();
        swapped.swap(0, 1);
        cases.push(("cand_key/order", swapped));

        let mut wrong_tmdb = base.clone();
        wrong_tmdb[1].input.tmdb_id = 4242;
        cases.push(("tmdb_id", wrong_tmdb));

        let mut wrong_media = base.clone();
        wrong_media[1].input.media_type = "tv".to_string();
        cases.push(("media_type", wrong_media));

        let mut foreign = base.clone();
        foreign[1] = other[0].clone();
        cases.push(("別 run の candidate_id", foreign));

        for (label, candidates) in cases {
            let input = ShadowCallInput { run_id, local: local_evidence(), candidates };
            let error = prepare_call(&mut conn, session.id, &input).await.unwrap_err();
            assert!(
                matches!(error, JevShadowError::Precondition(_) | JevShadowError::State(_)),
                "{label}: {error}"
            );
            assert_eq!(stub.systemone_hits(), 0, "{label}: 送っている");
            assert_eq!(
                load_session(&conn, session.id).unwrap().calls_reserved,
                0,
                "{label}: 予算を使っている"
            );
        }

        // 正しい組み合わせなら通る
        let ok = ShadowCallInput { run_id, local: local_evidence(), candidates: base };
        let _ = prepare_ready(&mut conn, session.id, &ok).await;
    }

    #[tokio::test]
    async fn local_failures_do_not_spend_budget() {
        let mut conn = open_db();
        let (client, stub) = client_with(vec![Reply::Json(200, answers_body(1, "c1"))]).await;
        let session = open_session(&conn, &client, "shadow-local-fail").await;
        let run_id = insert_run(&conn);

        // 候補ゼロ
        let empty = ShadowCallInput { run_id, local: local_evidence(), candidates: vec![] };
        assert!(matches!(
            prepare_call(&mut conn, session.id, &empty).await.unwrap_err(),
            JevShadowError::Precondition(_)
        ));

        // 候補が多すぎる（C2 の上限は 3）
        let many = ShadowCallInput {
            run_id,
            local: local_evidence(),
            candidates: insert_candidates(&conn, run_id, 4),
        };
        assert!(matches!(
            prepare_call(&mut conn, session.id, &many).await.unwrap_err(),
            JevShadowError::State(_)
        ));

        assert_eq!(stub.systemone_hits(), 0);
        assert_eq!(load_session(&conn, session.id).unwrap().calls_reserved, 0);
    }

    // ─── 予算 ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn budget_exhaustion_is_recorded_as_skipped() {
        let mut conn = open_db();
        let (client, stub) = client_with(vec![Reply::Json(200, answers_body(1, "c1"))]).await;
        let plan = plan_session(&conn, &{
            let mut config = JevSessionConfig::new("shadow-budget", MODEL);
            config.max_calls = 1;
            config
        })
        .unwrap();
        let outcome = preflight_model(&client, &plan).await;
        let session = apply_preflight(&conn, &plan, outcome).unwrap();

        let run_id = insert_run(&conn);
        let input = call_input(&conn, run_id, 1);
        run_one(&mut conn, &client, session.id, &input).await.unwrap();

        let second_run = insert_run(&conn);
        let second = ShadowCallInput {
            run_id: second_run,
            local: local_evidence(),
            candidates: insert_candidates(&conn, second_run, 1),
        };
        // 予算切れは prepare の結果として skip になる（raw API は無い）
        let record = match prepare_call(&mut conn, session.id, &second).await.unwrap() {
            PrepareCallResult::Skipped(skipped) => {
                assert_eq!(skipped.candidate_order_json(), "[\"c1\"]");
                assert_eq!(skipped.run_id(), second_run);
                persist_skipped_call(&mut conn, skipped).unwrap()
            }
            PrepareCallResult::Ready(_) => panic!("予算が残っていないのに Ready"),
        };
        assert_eq!(stub.systemone_hits(), 1);

        let session = load_session(&conn, session.id).unwrap();
        let call = load_call(&conn, record.call_id).unwrap().unwrap();
        assert_eq!(call["status"], "skipped");
        assert_eq!(call["call_kind"], "packed");
        assert_eq!(call["error_kind"], Value::Null);
        assert_eq!(call["error_text"], BUDGET_EXHAUSTED_TEXT);
        assert_eq!(call["eval_session_id"], session.id);
        assert_eq!(call["state_json"], "{}");
        assert_eq!(call["questions_json"], "{}");
        assert_eq!(call["state_bytes"], 0);
        assert_eq!(call["candidate_order_json"], "[\"c1\"]", "候補の並びは残す");

        let (scores, verdicts): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM metadata_match_candidate_scores WHERE run_id = ?1),
                        (SELECT COUNT(*) FROM metadata_match_verdicts WHERE run_id = ?1)",
                rusqlite::params![second_run],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(scores, 0);
        assert_eq!(verdicts, 0);
    }

    // ─── 予算切れの skip が state machine の一部であること ───────────────

    /// 予算を使い切った session を作る
    async fn exhausted_session(
        conn: &mut Connection,
        client: &TypeSafeClient,
        key: &str,
    ) -> (JevSession, i64) {
        let plan = plan_session(conn, &{
            let mut config = JevSessionConfig::new(key, MODEL);
            config.max_calls = 1;
            config
        })
        .unwrap();
        let outcome = preflight_model(client, &plan).await;
        let session = apply_preflight(conn, &plan, outcome).unwrap();

        let run_id = insert_run(conn);
        let input = call_input(conn, run_id, 1);
        let record = run_one(conn, client, session.id, &input).await.unwrap();
        assert_eq!(record.status, "ok");
        (load_session(conn, session.id).unwrap(), run_id)
    }

    /// 成功した run に、あとから skip 行を足せない
    #[tokio::test]
    async fn a_successful_run_cannot_gain_a_skip_row() {
        let mut conn = open_db();
        let (client, _stub) = client_with(vec![Reply::Json(200, answers_body(1, "c1"))]).await;
        let (session, run_id) = exhausted_session(&mut conn, &client, "shadow-skip-dup").await;
        // 上限まで予約済み。次の予約で exhausted になる
        assert_eq!(session.calls_reserved, session.max_calls);

        let before: (i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM metadata_match_jev_calls WHERE run_id = ?1),
                        (SELECT COUNT(*) FROM metadata_match_verdicts WHERE run_id = ?1),
                        (SELECT COUNT(*) FROM metadata_match_candidate_scores WHERE run_id = ?1)",
                rusqlite::params![run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        // 同じ run で skip を作ろうとしても、prepare の段階で弾かれる
        let same_run = ShadowCallInput {
            run_id,
            local: local_evidence(),
            candidates: load_candidates(&conn, run_id),
        };
        assert_eq!(
            prepare_call(&mut conn, session.id, &same_run).await.unwrap_err(),
            JevShadowError::DuplicatePackedCall { run_id }
        );

        let after: (i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM metadata_match_jev_calls WHERE run_id = ?1),
                        (SELECT COUNT(*) FROM metadata_match_verdicts WHERE run_id = ?1),
                        (SELECT COUNT(*) FROM metadata_match_candidate_scores WHERE run_id = ?1)",
                rusqlite::params![run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(after, before, "成功済みの run が変わっている");
        assert_eq!(after.0, 1, "packed call は 1 件のまま");
        assert_eq!(after.1, 1, "verdict も 1 件のまま");
    }

    /// 既存の run の候補行から入力を組み直す
    fn load_candidates(conn: &Connection, run_id: i64) -> Vec<ShadowCandidate> {
        let mut stmt = conn
            .prepare(
                "SELECT id, cand_key, tmdb_id, media_type FROM metadata_match_candidates
                  WHERE run_id = ?1 ORDER BY cand_key",
            )
            .unwrap();
        stmt.query_map(rusqlite::params![run_id], |row| {
            let index: i64 = row.get(0)?;
            let cand_key: String = row.get(1)?;
            let tmdb_id: i64 = row.get(2)?;
            let media_type: String = row.get(3)?;
            Ok(ShadowCandidate {
                candidate_id: index,
                input: CandidateInput {
                    tmdb_id,
                    media_type,
                    title: format!("候補 {cand_key}"),
                    original_title: None,
                    year: Some(2021),
                    original_language: Some("ja".to_string()),
                },
            })
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }

    /// 予算切れでも、shadow でない run には skip 行を作らない
    #[tokio::test]
    async fn non_shadow_runs_never_get_a_skip_row() {
        let mut conn = open_db();
        let (client, _stub) = client_with(vec![Reply::Json(200, answers_body(1, "c1"))]).await;
        let (session, _) = exhausted_session(&mut conn, &client, "shadow-skip-mode").await;

        for (label, mode, applied) in [
            ("safe", "safe", 0),
            ("gated", "gated", 0),
            ("applied", "shadow", 1),
        ] {
            let run_id = insert_run_with(&conn, mode, applied);
            let input = call_input(&conn, run_id, 1);
            let error = prepare_call(&mut conn, session.id, &input).await.unwrap_err();
            assert!(
                matches!(error, JevShadowError::Precondition(_)),
                "{label}: {error}"
            );
            let calls: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM metadata_match_jev_calls WHERE run_id = ?1",
                    rusqlite::params![run_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(calls, 0, "{label}: skip 行を作っている");
        }
    }

    /// 予算切れでも、ローカルの不整合は skip ではなくエラー
    #[tokio::test]
    async fn local_problems_are_errors_even_when_exhausted() {
        let mut conn = open_db();
        let (client, _stub) = client_with(vec![Reply::Json(200, answers_body(1, "c1"))]).await;
        let (session, _) = exhausted_session(&mut conn, &client, "shadow-skip-local").await;

        // 候補の取り違え
        let run_a = insert_run(&conn);
        let mut tampered = insert_candidates(&conn, run_a, 2);
        tampered[1].input.tmdb_id = 4242;
        let input_a = ShadowCallInput { run_id: run_a, local: local_evidence(), candidates: tampered };
        assert!(matches!(
            prepare_call(&mut conn, session.id, &input_a).await.unwrap_err(),
            JevShadowError::Precondition(_)
        ));

        // evidence_class の食い違い
        let run_b = insert_run(&conn);
        let mut local = local_evidence();
        local.evidence_class = Some("historical_audit_only".to_string());
        let input_b = ShadowCallInput {
            run_id: run_b,
            local,
            candidates: insert_candidates(&conn, run_b, 1),
        };
        assert!(matches!(
            prepare_call(&mut conn, session.id, &input_b).await.unwrap_err(),
            JevShadowError::Precondition(_)
        ));

        let skips: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata_match_jev_calls WHERE status = 'skipped'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(skips, 0, "ローカル失敗を skip として記録している");
    }

    // ─── 未較正であることの記録 ──────────────────────────────────────────

    #[tokio::test]
    async fn scores_and_verdicts_record_that_they_are_uncalibrated() {
        let mut conn = open_db();
        let (client, _stub) = client_with(vec![
            Reply::Json(200, answers_body(2, "c1")),
            Reply::Json(200, answers_body(2, "NONE")),
        ])
        .await;
        let session = open_session(&conn, &client, "shadow-uncalibrated").await;

        // 候補を選んだ場合
        let run_a = insert_run(&conn);
        let input_a = call_input(&conn, run_a, 2);
        run_one(&mut conn, &client, session.id, &input_a).await.unwrap();

        let reasons: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT reasons_json FROM metadata_match_candidate_scores WHERE run_id = ?1",
                )
                .unwrap();
            let rows = stmt
                .query_map(rusqlite::params![run_a], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            rows
        };
        assert_eq!(reasons.len(), 2);
        for reason in &reasons {
            assert_eq!(
                reason,
                "{\"calibrated\":false,\"diagnostic_only\":true,\"source\":\"noul_same_work\"}"
            );
        }

        let verdict: String = conn
            .query_row(
                "SELECT reasons_json FROM metadata_match_verdicts WHERE run_id = ?1",
                rusqlite::params![run_a],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            verdict,
            "[\"jev_selected_candidate\",\"shadow_only\",\"uncalibrated\"]"
        );

        // NONE の場合
        let run_b = insert_run(&conn);
        let input_b = call_input(&conn, run_b, 2);
        run_one(&mut conn, &client, session.id, &input_b).await.unwrap();
        let verdict: String = conn
            .query_row(
                "SELECT reasons_json FROM metadata_match_verdicts WHERE run_id = ?1",
                rusqlite::params![run_b],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            verdict,
            "[\"jev_answered_none\",\"shadow_only\",\"uncalibrated\"]"
        );
    }

    // ─── shadow であることの担保 ─────────────────────────────────────────

    fn install_write_guards(conn: &Connection) {
        for table in ["works", "files", "metadata_match_runs"] {
            for event in ["INSERT", "UPDATE", "DELETE"] {
                conn.execute_batch(&format!(
                    "CREATE TEMP TRIGGER guard_{table}_{event} BEFORE {event} ON {table}
                     BEGIN SELECT RAISE(ABORT, 'shadow must not write {table}'); END;"
                ))
                .unwrap();
            }
        }
    }

    fn fingerprint(conn: &Connection) -> (String, String, String) {
        let one = |sql: &str| -> String {
            let mut stmt = conn.prepare(sql).unwrap();
            let rows: Vec<String> = stmt
                .query_map([], |row| row.get::<_, Option<String>>(0))
                .unwrap()
                .map(|value| value.unwrap().unwrap_or_default())
                .collect();
            rows.join("|")
        };

        (
            one("SELECT title || ':' || COALESCE(original_title,'') || ':' || COALESCE(year,'')
                     || ':' || COALESCE(tmdb_id,'') || ':' || COALESCE(imdb_id,'')
                     || ':' || COALESCE(match_status,'') || ':' || COALESCE(match_confidence,'')
                     || ':' || COALESCE(poster_path,'') || ':' || COALESCE(thumb_path,'')
                     || ':' || COALESCE(match_source,'') || ':' || COALESCE(last_match_run_id,'')
                   FROM works ORDER BY id"),
            one("SELECT file_name || ':' || file_path || ':' || COALESCE(renamed_by_app,'')
                   FROM files ORDER BY id"),
            one("SELECT governing_matcher || ':' || decision || ':' || decision_reasons_json
                     || ':' || applied || ':' || COALESCE(applied_tmdb_id,'')
                     || ':' || COALESCE(applied_media_type,'') || ':' || COALESCE(status_after,'')
                   FROM metadata_match_runs ORDER BY id"),
        )
    }

    fn table_counts(conn: &Connection, tables: &[&str]) -> Vec<i64> {
        tables
            .iter()
            .map(|table| {
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
                    .unwrap()
            })
            .collect()
    }

    #[tokio::test]
    async fn the_shadow_path_never_touches_production_tables() {
        let mut conn = open_db();
        let (client, _stub) = client_with(vec![
            Reply::Json(200, answers_body(3, "c2")),
            Reply::Json(200, answers_body(3, "NONE")),
            Reply::Json(503, "{}".to_string()),
        ])
        .await;
        let session = open_session(&conn, &client, "shadow-guard").await;

        // 先に本番データを作ってから、書き込みを禁止する
        let runs: Vec<i64> = (0..3).map(|_| insert_run(&conn)).collect();
        let inputs: Vec<ShadowCallInput> = runs
            .iter()
            .map(|run_id| ShadowCallInput {
                run_id: *run_id,
                local: local_evidence(),
                candidates: insert_candidates(&conn, *run_id, 3),
            })
            .collect();
        conn.execute(
            "INSERT INTO sources (name, root_path, source_type)
             VALUES ('test', '/tmp/shadow-guard', 'local')",
            [],
        )
        .unwrap();
        let source_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO files (source_id, file_name, file_path)
             VALUES (?1, 'a.mkv', '/tmp/shadow-guard/a.mkv')",
            rusqlite::params![source_id],
        )
        .unwrap();
        install_write_guards(&conn);

        let watched = [
            "metadata_review_tasks",
            "metadata_match_labels",
            "metadata_match_rejections",
        ];
        let before = fingerprint(&conn);
        let watched_before = table_counts(&conn, &watched);
        let audits_before = table_counts(
            &conn,
            &[
                "metadata_match_jev_calls",
                "metadata_match_candidate_scores",
                "metadata_match_verdicts",
            ],
        );

        for input in &inputs {
            let record = run_one(&mut conn, &client, session.id, input).await.unwrap();
            assert!(matches!(record.status.as_str(), "ok" | "unavailable"));
        }

        // 本番側は 1 バイトも動いていない
        assert_eq!(fingerprint(&conn), before, "works / files / runs が変わっている");
        assert_eq!(
            table_counts(&conn, &watched),
            watched_before,
            "review tasks / labels / rejections が増えている"
        );

        // 監査側だけが増えている
        let audits_after = table_counts(
            &conn,
            &[
                "metadata_match_jev_calls",
                "metadata_match_candidate_scores",
                "metadata_match_verdicts",
            ],
        );
        assert_eq!(audits_after[0], audits_before[0] + 3);
        assert!(audits_after[1] > audits_before[1]);
        assert_eq!(audits_after[2], audits_before[2] + 2, "成功した 2 件だけ verdict");
    }

    #[test]
    fn the_write_guards_actually_abort() {
        let conn = open_db();
        insert_run(&conn);
        install_write_guards(&conn);

        for (sql, expected) in [
            ("UPDATE works SET title = 'x'", "works"),
            (
                "INSERT INTO files (source_id, file_name, file_path) VALUES (NULL,'b','/b')",
                "files",
            ),
            (
                "UPDATE metadata_match_runs SET applied = 1",
                "metadata_match_runs",
            ),
        ] {
            let error = conn.execute(sql, []).unwrap_err();
            assert!(
                error.to_string().contains(&format!("shadow must not write {expected}")),
                "{sql}: {error}"
            );
        }
    }
}
