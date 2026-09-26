//! Jev shadow 評価の session・contract 登録・予算（PR3 C4a）。
//!
//! ここまでが C4a の範囲で、実際の呼び出し（C2 materialize → C3A 質問生成 →
//! C3B SystemOne → C3A 検証 → call 行の保存）は C4b が行う。
//! このモジュールは `metadata_match_jev_calls` にも `works` にも書かない。
//!
//! # 使い方（3 段構え）
//!
//! HTTP の preflight 中に DB transaction や DB の Mutex を握らないよう、意図的に
//! 同期の DB 処理と非同期の HTTP を分けてある。呼び出し側はこの順で使う。
//!
//! ```text
//! 1. plan_session(&conn, &config)      // 同期。contract 登録 + resume 判定まで
//! 2. preflight_model(&client, &plan)   // 非同期。DB に触らない
//! 3. apply_preflight(&conn, plan, outcome)  // 同期。session 行を作る / 状態を戻す
//! ```
//!
//! # 決めていること
//!
//! - 予算は **session 単位**。run 単位ではないし、アプリを再起動しても同じ
//!   `session_key` なら累積する。
//! - model の pin は `jev_eval_sessions.requested_model` が正。`jev_contracts.default_model`
//!   は legacy の情報列であって、振る舞いの根拠にしない。
//! - alias 解決・`-latest` の自動採用・semver での最大値選択はしない。別モデルを
//!   評価したければ別 session を作る。

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};

use crate::services::jev_contract::{canonical_json, contract_v2, JevContract};
use crate::services::typesafe_client::{ModelCard, TypeSafeClient, TypeSafeClientError};

/// 既定の呼び出し上限（session あたり）
pub const DEFAULT_MAX_CALLS: i64 = 5_000;

/// 既定の入力トークン上限（session あたり）
pub const DEFAULT_MAX_INPUT_TOKENS: i64 = 10_000_000;

/// `session_key` の長さ上限（trim 後）
pub const MAX_SESSION_KEY_LEN: usize = 128;

// ─── 設定 ────────────────────────────────────────────────────────────────────

/// session を開始（または resume）するための設定。
#[derive(Debug, Clone, PartialEq)]
pub struct JevSessionConfig {
    /// 呼び出し側が決める識別子。**自動生成しない**
    pub session_key: String,
    /// 具体的なモデル名。alias を渡さない
    pub requested_model: String,
    pub max_calls: i64,
    pub max_input_tokens: i64,
}

impl JevSessionConfig {
    /// 既定の予算で設定を作る。
    pub fn new(session_key: impl Into<String>, requested_model: impl Into<String>) -> Self {
        JevSessionConfig {
            session_key: session_key.into(),
            requested_model: requested_model.into(),
            max_calls: DEFAULT_MAX_CALLS,
            max_input_tokens: DEFAULT_MAX_INPUT_TOKENS,
        }
    }

    /// trim と範囲を確認して、以後 authoritative に使う形へ正規化する。
    fn normalized(&self) -> Result<JevSessionConfig, JevSessionError> {
        let session_key = self.session_key.trim().to_string();
        if session_key.is_empty() {
            return Err(JevSessionError::InvalidConfig("session_key が空です".into()));
        }
        if session_key.chars().count() > MAX_SESSION_KEY_LEN {
            return Err(JevSessionError::InvalidConfig(format!(
                "session_key が長すぎます（{MAX_SESSION_KEY_LEN} 文字まで）"
            )));
        }
        if session_key.chars().any(|c| c.is_control()) {
            return Err(JevSessionError::InvalidConfig(
                "session_key に制御文字が含まれています".into(),
            ));
        }

        let requested_model = self.requested_model.trim().to_string();
        if requested_model.is_empty() {
            return Err(JevSessionError::InvalidConfig("requested_model が空です".into()));
        }

        if self.max_calls <= 0 {
            return Err(JevSessionError::InvalidConfig("max_calls は 1 以上です".into()));
        }
        if self.max_input_tokens <= 0 {
            return Err(JevSessionError::InvalidConfig(
                "max_input_tokens は 1 以上です".into(),
            ));
        }

        Ok(JevSessionConfig {
            session_key,
            requested_model,
            max_calls: self.max_calls,
            max_input_tokens: self.max_input_tokens,
        })
    }
}

/// `jev-<major>.<minor>.<patch>` の形か。major/minor/patch は 10 進整数。
///
/// alias（`jev-latest` / `jev-preview` / `latest`）や桁の欠けた `jev-1.13`、
/// 接尾辞つきの `jev-1.13.0-preview` は canonical ではない。**評価に使うモデルは
/// 版を明示したものだけ**にしたいので、ここを通らない名前は pin として扱わない。
pub fn is_canonical_model_name(name: &str) -> bool {
    let Some(version) = name.strip_prefix("jev-") else {
        return false;
    };
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    // TypeSafe 側に無い桁数制限をこちらで足さない。数値として解釈もしない
    parts
        .iter()
        .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

// ─── 結果とエラー ────────────────────────────────────────────────────────────

/// DB 上の session 行。
#[derive(Debug, Clone, PartialEq)]
pub struct JevSession {
    pub id: i64,
    pub session_key: String,
    pub contract_version: String,
    pub state_schema_version: String,
    pub requested_model: String,
    pub model_card_json: String,
    pub max_calls: i64,
    pub max_input_tokens: i64,
    pub calls_reserved: i64,
    pub input_tokens_used: i64,
    pub output_tokens_used: i64,
    pub status: String,
    /// status を動かすたびに +1 する世代番号。`open → blocked → open` のように
    /// 文字列が戻っても世代は戻らないので、CAS で「別の世代」を検出できる
    pub lifecycle_revision: i64,
    pub blocked_reason: Option<String>,
}

/// 予約 1 件。C4b はこれを持ってから HTTP を送る。
#[derive(Debug, Clone, PartialEq)]
pub struct CallReservation {
    pub session_id: i64,
    /// 予約後の累計。監査用
    pub calls_reserved: i64,
}

/// token を加算した結果。
#[derive(Debug, Clone, PartialEq)]
pub struct TokenUsage {
    pub input_tokens_used: i64,
    pub output_tokens_used: i64,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JevSessionError {
    /// 設定が不正（送信前に弾く）
    InvalidConfig(String),
    /// 登録済み contract と手元の contract が違う。**直しに行かない**
    ContractMismatch { field: &'static str },
    /// 同じ session_key なのに設定が違う。**既存値を書き換えない**
    SessionConfigMismatch { field: &'static str },
    /// closed / exhausted は resume できない
    NotResumable { status: String },
    /// preflight に失敗して blocked になった
    Blocked { reason: String },
    /// 予算を使い切った（blocked / closed とは区別する）
    BudgetExhausted,
    /// session が閉じている
    SessionClosed,
    /// 期待した session 行が見つからない（別プロセスが消した等）
    SessionNotFound,
    /// plan を立てた後に別の経路が状態を変えた
    ConcurrentSessionChange { expected: String, found: String },
    /// preflight の対象モデルが plan と食い違っている
    PreflightModelMismatch { expected: String, found: String },
    /// トークン数が不正、または加算で溢れた
    InvalidTokenCount(String),
    Db(String),
}

impl std::fmt::Display for JevSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JevSessionError::InvalidConfig(reason) => write!(f, "設定が不正です: {reason}"),
            JevSessionError::ContractMismatch { field } => {
                write!(f, "登録済み contract と一致しません（{field}）")
            }
            JevSessionError::SessionConfigMismatch { field } => {
                write!(f, "既存 session と設定が一致しません（{field}）")
            }
            JevSessionError::NotResumable { status } => {
                write!(f, "status={status} の session は再開できません")
            }
            JevSessionError::Blocked { reason } => write!(f, "session を停止しました: {reason}"),
            JevSessionError::BudgetExhausted => write!(f, "session の予算を使い切りました"),
            JevSessionError::SessionClosed => write!(f, "session は閉じています"),
            JevSessionError::SessionNotFound => write!(f, "session が見つかりません"),
            JevSessionError::ConcurrentSessionChange { expected, found } => write!(
                f,
                "session の状態が変わりました（{expected} を期待しましたが {found} でした）"
            ),
            JevSessionError::PreflightModelMismatch { expected, found } => write!(
                f,
                "preflight のモデルが一致しません（{expected} を期待しましたが {found} でした）"
            ),
            JevSessionError::InvalidTokenCount(reason) => {
                write!(f, "トークン数が不正です: {reason}")
            }
            JevSessionError::Db(reason) => write!(f, "DB エラー: {reason}"),
        }
    }
}

impl std::error::Error for JevSessionError {}

impl From<rusqlite::Error> for JevSessionError {
    fn from(error: rusqlite::Error) -> Self {
        JevSessionError::Db(error.to_string())
    }
}

// ─── contract 登録 ───────────────────────────────────────────────────────────

/// contract を `jev_contracts` へ登録する。既にあれば完全照合する。
///
/// 一致しない場合は **直さずに失敗させる**。UPDATE / REPLACE / INSERT OR REPLACE は
/// 使わない。凍結した文面が後から書き換わると、過去の呼び出しの意味が追えなくなる。
///
/// `default_model` は legacy の情報列なので比較しない。同じ contract を model A で
/// 初回登録し、あとで model B の別 session が使うのは許す（pin は session 側）。
pub fn ensure_contract_registered(
    conn: &Connection,
    contract: &JevContract,
    default_model: &str,
) -> Result<(), JevSessionError> {
    let questions_template = canonical_json(&contract.questions_template);
    let criteria = canonical_json(&contract.criteria);
    let validation_policy = canonical_json(&contract.validation_policy);

    let existing = read_contract_row(conn, &contract.contract_version)?;

    let row = match existing {
        Some(row) => row,
        None => match insert_contract_or_reconcile(
            conn,
            contract,
            default_model,
            &questions_template,
            &criteria,
            &validation_policy,
        )? {
            // INSERT できた
            None => return Ok(()),
            // 競合したので、先に入った行を照合する
            Some(row) => row,
        },
    };

    let (
        stored_schema,
        stored_instructions,
        stored_template,
        stored_criteria,
        stored_validation,
        stored_hash,
    ) = row;

    // 保存時の整形差で誤検出しないよう、JSON は canonical 化してから比べる
    let checks: [(&'static str, String, String); 6] = [
        (
            "state_schema_version",
            stored_schema,
            contract.state_schema_version.clone(),
        ),
        ("instructions_text", stored_instructions, contract.instructions.clone()),
        (
            "questions_template_json",
            canonicalize_stored(&stored_template),
            questions_template,
        ),
        ("criteria_json", canonicalize_stored(&stored_criteria), criteria),
        (
            "validation_policy_json",
            canonicalize_stored(&stored_validation),
            validation_policy,
        ),
        ("canonical_sha256", stored_hash, contract.canonical_sha256.clone()),
    ];

    for (field, stored, local) in checks {
        if stored != local {
            return Err(JevSessionError::ContractMismatch { field });
        }
    }

    Ok(())
}

type StoredContract = (String, String, String, String, String, String);

/// contract を INSERT する。UNIQUE で負けたら既存行を読んで返す。
///
/// `Ok(None)` が「自分が入れた」、`Ok(Some(row))` が「先に入っていた行」。
/// 競合しても UPDATE も REPLACE もしない。
fn insert_contract_or_reconcile(
    conn: &Connection,
    contract: &JevContract,
    default_model: &str,
    questions_template: &str,
    criteria: &str,
    validation_policy: &str,
) -> Result<Option<StoredContract>, JevSessionError> {
    let inserted = conn.execute(
        "INSERT INTO jev_contracts
           (contract_version, state_schema_version, instructions_text,
            questions_template_json, criteria_json, validation_policy_json,
            canonical_sha256, default_model)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            contract.contract_version,
            contract.state_schema_version,
            contract.instructions,
            questions_template,
            criteria,
            validation_policy,
            contract.canonical_sha256,
            default_model,
        ],
    );
    match inserted {
        Ok(_) => Ok(None),
        // 同時に別の接続が同じ版を入れた。先に入った行をそのまま尊重し、
        // 凍結フィールドが一致するかだけを呼び出し側が確かめる（default_model は見ない）。
        Err(error) if is_contract_version_conflict(&error) => {
            let row = read_contract_row(conn, &contract.contract_version)?
                .ok_or(JevSessionError::SessionNotFound)?;
            Ok(Some(row))
        }
        Err(error) => Err(error.into()),
    }
}

fn read_contract_row(
    conn: &Connection,
    contract_version: &str,
) -> Result<Option<StoredContract>, JevSessionError> {
    let row = conn
        .query_row(
            "SELECT state_schema_version, instructions_text, questions_template_json,
                    criteria_json, validation_policy_json, canonical_sha256
               FROM jev_contracts WHERE contract_version = ?1",
            rusqlite::params![contract_version],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()?;
    Ok(row)
}

fn is_contract_version_conflict(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::ConstraintViolation
    ) && error.to_string().contains("jev_contracts.contract_version")
}

/// 保存済み JSON 文字列を canonical 形へ揃える。壊れていればそのまま返して不一致にする。
fn canonicalize_stored(stored: &str) -> String {
    match serde_json::from_str::<Value>(stored) {
        Ok(value) => canonical_json(&value),
        Err(_) => stored.to_string(),
    }
}

// ─── session の開始と再開 ────────────────────────────────────────────────────

/// `plan_session` の結果。HTTP preflight のあと `apply_preflight` へ渡す。
#[derive(Debug, Clone, PartialEq)]
pub struct SessionPlan {
    pub config: JevSessionConfig,
    pub contract_version: String,
    pub state_schema_version: String,
    /// 既存 session の id。新規なら None
    pub existing_id: Option<i64>,
    /// plan を立てた時点の status。新規なら None、resume なら `open` か `blocked`。
    /// apply はこの値に対する compare-and-set で書き込む
    pub expected_status: Option<String>,
    /// plan を立てた時点の lifecycle 世代。status と合わせて CAS の条件にする
    pub expected_lifecycle_revision: Option<i64>,
}

/// preflight の結果。失敗理由は **安全な定型文だけ**にする。
///
/// どのモデルについての結果かを必ず持たせる。こうしないと、モデル B の失敗結果で
/// モデル A の session を blocked にする、といった取り違えが起こり得る。
#[derive(Debug, Clone, PartialEq)]
pub enum PreflightOutcome {
    Available { requested_model: String, identity: ModelIdentity },
    Unavailable { requested_model: String, reason: String },
}

/// そのモデルをどうやって「使える」と判断したか。
///
/// canonical な版は一覧を照会せずに受理する（TypeSafe は現状 alias しか一覧に出さないが、
/// 版を直接指定した request は通る）。記録するのは **確認できた事実だけ**で、
/// 照会していないことを「載っていない」と書き換えない。
#[derive(Debug, Clone, PartialEq)]
pub enum ModelIdentity {
    /// `GET /v1/models` に完全一致で載っていた
    Listed(ModelCard),
    /// 一覧を照会せず、canonical な版を明示指定した
    ExplicitVersionPin,
}

impl PreflightOutcome {
    /// この結果がどのモデルについてのものか。
    pub fn requested_model(&self) -> &str {
        match self {
            PreflightOutcome::Available { requested_model, .. } => requested_model,
            PreflightOutcome::Unavailable { requested_model, .. } => requested_model,
        }
    }
}

/// 手順 1（同期）。設定を検証し、contract を登録し、resume 可能かまで決める。
///
/// ここでは HTTP を呼ばない。DB の作業を先に終わらせてから preflight に入る。
pub fn plan_session(
    conn: &Connection,
    config: &JevSessionConfig,
) -> Result<SessionPlan, JevSessionError> {
    let config = config.normalized()?;
    let contract = contract_v2();
    ensure_contract_registered(conn, &contract, &config.requested_model)?;

    let existing: Option<(i64, String, i64, i64, String, String, String, i64)> = conn
        .query_row(
            "SELECT id, requested_model, max_calls, max_input_tokens,
                    contract_version, state_schema_version, status, lifecycle_revision
               FROM jev_eval_sessions WHERE session_key = ?1",
            rusqlite::params![config.session_key],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .optional()?;

    let (existing_id, expected_status, expected_lifecycle_revision) = match existing {
        None => (None, None, None),
        Some((id, model, max_calls, max_tokens, contract_version, schema, status, revision)) => {
            // 既存値に合わせて UPDATE はしない。違えばそこで止める
            if model != config.requested_model {
                return Err(JevSessionError::SessionConfigMismatch {
                    field: "requested_model",
                });
            }
            if max_calls != config.max_calls {
                return Err(JevSessionError::SessionConfigMismatch { field: "max_calls" });
            }
            if max_tokens != config.max_input_tokens {
                return Err(JevSessionError::SessionConfigMismatch {
                    field: "max_input_tokens",
                });
            }
            if contract_version != contract.contract_version {
                return Err(JevSessionError::SessionConfigMismatch {
                    field: "contract_version",
                });
            }
            if schema != contract.state_schema_version {
                return Err(JevSessionError::SessionConfigMismatch {
                    field: "state_schema_version",
                });
            }
            if status == "closed" || status == "exhausted" {
                return Err(JevSessionError::NotResumable { status });
            }
            (Some(id), Some(status), Some(revision))
        }
    };

    Ok(SessionPlan {
        config,
        contract_version: contract.contract_version,
        state_schema_version: contract.state_schema_version,
        existing_id,
        expected_status,
        expected_lifecycle_revision,
    })
}

/// 手順 2（非同期）。この session で使うモデルの素性を決める。**DB に触らない。**
///
/// - canonical な版名（`jev-1.13.0` など）は **一覧を照会せずそのまま受理**する
/// - それ以外（alias など）だけ `GET /v1/models` で完全一致を確認する
///
/// canonical 版が実際に使えるかどうかと、その identity の最終的な確認は、ここではなく
/// SystemOne の応答で行う（`response.model` が要求と完全一致するか。C3B の exact match）。
/// alias や最新版へ勝手に乗り換えることはしない。
pub async fn preflight_model(client: &TypeSafeClient, plan: &SessionPlan) -> PreflightOutcome {
    // 見るモデルは plan のものだけ。呼び出し側が別の文字列を差し込めないようにする
    let requested_model = plan.config.requested_model.clone();

    // 版を明示した名前は一覧を照会しない。
    // TypeSafe の `GET /v1/models` は現状 alias しか載せないが、版を直接指定した
    // SystemOne は通る。したがって canonical pin の identity を決めるのは一覧ではなく、
    // **実際の応答の `model` が要求と完全一致するか**（C3B の exact match）である。
    // 存在しない版（例: jev-9.9.9）でも session は作れるが、その場合は最初の
    // SystemOne が失敗し、transport error として監査に残る。
    if is_canonical_model_name(&requested_model) {
        return PreflightOutcome::Available {
            requested_model,
            identity: ModelIdentity::ExplicitVersionPin,
        };
    }

    // canonical でない名前（alias など）は従来どおり一覧で確認する
    match client.verify_model_available(&requested_model).await {
        Ok(card) => PreflightOutcome::Available {
            requested_model,
            identity: ModelIdentity::Listed(card),
        },
        Err(error) => PreflightOutcome::Unavailable {
            reason: safe_preflight_reason(&error),
            requested_model,
        },
    }
}

/// blocked_reason に入れる文字列。API キーも応答本文も入れない。
fn safe_preflight_reason(error: &TypeSafeClientError) -> String {
    match error.http_status {
        Some(status) => format!("model preflight failed: {} (HTTP {status})", error.kind.as_str()),
        None => format!("model preflight failed: {}", error.kind.as_str()),
    }
}

/// モデルの素性を canonical JSON にする。
///
/// `typesafe_client.rs` に `Serialize` を足すために触りたくないので、ここで組み立てる。
///
/// **確認できた事実しか書かない。** 版を明示指定した場合は一覧を照会していないので、
/// description も release_date も手元に無い。一覧に「載っていない」ことすら確かめて
/// いないので、そう書くこともしない。作り物の値を入れると、後から「どの世代のモデルで
/// 測ったのか」が辿れなくなる。
fn model_card_json(requested_model: &str, identity: &ModelIdentity) -> String {
    match identity {
        ModelIdentity::Listed(card) => canonical_json(&json!({
            "name": card.name,
            "description": card.description,
            "release_date": card.release_date,
            "identity_source": "models_listing",
            "listed_in_models": true,
        })),
        ModelIdentity::ExplicitVersionPin => canonical_json(&json!({
            "name": requested_model,
            "identity_source": "explicit_version_pin",
        })),
    }
}

/// 手順 3（同期）。preflight の結果を session 行に反映する。
///
/// - 新規 + 成功 → `open` で INSERT
/// - 新規 + 失敗 → **行を作らない**（別モデルへ切り替えない）
/// - 再開 + 成功 → `open` に戻す。`model_card_json` は開始時の snapshot のまま
/// - 再開 + 失敗 → `blocked` にして理由を残す。`requested_model` は変えない
pub fn apply_preflight(
    conn: &Connection,
    plan: &SessionPlan,
    outcome: PreflightOutcome,
) -> Result<JevSession, JevSessionError> {
    // plan / preflight / ModelCard の三者が同じモデルを指していること。
    // 1 つでも違えば DB へ一切書かずに止める
    if outcome.requested_model() != plan.config.requested_model {
        return Err(JevSessionError::PreflightModelMismatch {
            expected: plan.config.requested_model.clone(),
            found: outcome.requested_model().to_string(),
        });
    }
    if let PreflightOutcome::Available { identity: ModelIdentity::Listed(card), .. } = &outcome {
        // C3B の verify_model_available が通常は保証するが、session 層でも確かめる。
        // 版の明示指定には card が無いので、比較できるのは一覧由来のときだけ
        if card.name != plan.config.requested_model {
            return Err(JevSessionError::PreflightModelMismatch {
                expected: plan.config.requested_model.clone(),
                found: card.name.clone(),
            });
        }
    }

    match (plan.existing_id, outcome) {
        (None, PreflightOutcome::Available { identity, requested_model }) => {
            let inserted = conn.execute(
                "INSERT INTO jev_eval_sessions
                   (session_key, contract_version, state_schema_version, requested_model,
                    model_card_json, max_calls, max_input_tokens, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'open')",
                rusqlite::params![
                    plan.config.session_key,
                    plan.contract_version,
                    plan.state_schema_version,
                    plan.config.requested_model,
                    model_card_json(&requested_model, &identity),
                    plan.config.max_calls,
                    plan.config.max_input_tokens,
                ],
            );
            match inserted {
                Ok(_) => load_session(conn, conn.last_insert_rowid()),
                // 同じ session_key を別の呼び出しが先に作った。裸の UNIQUE エラーを
                // 外へ出さず、その行が本当に同じ設定かを見てから決める。
                Err(error) if is_session_key_conflict(&error) => {
                    // 相手が先に open な session を作り終えているならその結果を採用する。
                    // blocked だった場合は開け直さず、resume として plan し直させる。
                    reconcile_existing_session(conn, plan)
                }
                Err(error) => Err(error.into()),
            }
        }
        (None, PreflightOutcome::Unavailable { reason, .. }) => {
            // session 行は作らない。contract 行だけ先に入っていても問題ない
            Err(JevSessionError::Blocked { reason })
        }
        (Some(id), PreflightOutcome::Available { .. }) => {
            // model_card_json は上書きしない（開始時の snapshot を残す）
            let (expected, revision) = expected_generation(plan)?;
            reopen_after_successful_preflight(conn, id, &expected, revision)
        }
        (Some(id), PreflightOutcome::Unavailable { reason, .. }) => {
            // plan 時点の status からしか動かさない。別の preflight が先に
            // open へ戻していたら、この古い失敗で blocked に落とさない
            let (expected, revision) = expected_generation(plan)?;
            let changed = conn.execute(
                "UPDATE jev_eval_sessions
                    SET status = 'blocked',
                        blocked_reason = ?4,
                        lifecycle_revision = lifecycle_revision + 1
                  WHERE id = ?1 AND status = ?2 AND lifecycle_revision = ?3",
                rusqlite::params![id, expected, revision, reason],
            )?;
            if changed == 0 {
                return Err(conflict_error(conn, id, &expected));
            }
            Err(JevSessionError::Blocked { reason })
        }
    }
}

/// plan 時点の (status, lifecycle_revision)。resume の plan なら必ず入っている。
fn expected_generation(plan: &SessionPlan) -> Result<(String, i64), JevSessionError> {
    match (plan.expected_status.clone(), plan.expected_lifecycle_revision) {
        (Some(status), Some(revision)) => Ok((status, revision)),
        _ => Err(JevSessionError::SessionNotFound),
    }
}

/// preflight が通った session を `open` へ戻す。
///
/// plan を立ててから apply するまでの間に状態が動いているかもしれないので、
/// **plan 時点の status に対する compare-and-set** で書き込む。`status IN
/// ('open','blocked')` だけだと、別の preflight による open ↔ blocked の入れ替わりを
/// 見逃してしまう。0 行なら現在値を読み直して fail closed する。
fn reopen_after_successful_preflight(
    conn: &Connection,
    id: i64,
    expected: &str,
    revision: i64,
) -> Result<JevSession, JevSessionError> {
    // open → open でも世代は進める。文字列が同じでも「新しい preflight 結果を
    // 適用した」という別の世代だから
    let changed = conn.execute(
        "UPDATE jev_eval_sessions
            SET status = 'open',
                blocked_reason = NULL,
                lifecycle_revision = lifecycle_revision + 1
          WHERE id = ?1 AND status = ?2 AND lifecycle_revision = ?3",
        rusqlite::params![id, expected, revision],
    )?;
    if changed == 0 {
        return Err(conflict_error(conn, id, expected));
    }
    load_session(conn, id)
}

/// CAS が 0 行だったときの理由を、現在の行から決める。**再試行はしない。**
fn conflict_error(conn: &Connection, id: i64, expected: &str) -> JevSessionError {
    match load_session(conn, id) {
        Ok(session) => match session.status.as_str() {
            "closed" | "exhausted" => JevSessionError::NotResumable { status: session.status },
            found => JevSessionError::ConcurrentSessionChange {
                expected: expected.to_string(),
                found: found.to_string(),
            },
        },
        Err(JevSessionError::Db(_)) => JevSessionError::SessionNotFound,
        Err(other) => other,
    }
}

/// 同時作成でぶつかった相手の行を読み、そのまま使ってよいかを判断する。
///
/// 既存行を UPDATE で合わせにいったり、INSERT OR REPLACE で壊したりはしない。
fn reconcile_existing_session(
    conn: &Connection,
    plan: &SessionPlan,
) -> Result<JevSession, JevSessionError> {
    let id: Option<i64> = conn
        .query_row(
            "SELECT id FROM jev_eval_sessions WHERE session_key = ?1",
            rusqlite::params![plan.config.session_key],
            |row| row.get(0),
        )
        .optional()?;
    let Some(id) = id else {
        // UNIQUE で弾かれたのに行が無い＝さらに別の変更が挟まっている
        return Err(JevSessionError::SessionNotFound);
    };

    let existing = load_session(conn, id)?;
    if existing.requested_model != plan.config.requested_model {
        return Err(JevSessionError::SessionConfigMismatch { field: "requested_model" });
    }
    if existing.max_calls != plan.config.max_calls {
        return Err(JevSessionError::SessionConfigMismatch { field: "max_calls" });
    }
    if existing.max_input_tokens != plan.config.max_input_tokens {
        return Err(JevSessionError::SessionConfigMismatch { field: "max_input_tokens" });
    }
    if existing.contract_version != plan.contract_version {
        return Err(JevSessionError::SessionConfigMismatch { field: "contract_version" });
    }
    if existing.state_schema_version != plan.state_schema_version {
        return Err(JevSessionError::SessionConfigMismatch {
            field: "state_schema_version",
        });
    }
    match existing.status.as_str() {
        // 相手が先に preflight を終えて open な session を作った。その結果を使う
        "open" => Ok(existing),
        // 行が無い前提で取った preflight 結果なので、blocked を開け直す根拠にならない。
        // resume として plan し直せば、そのとき改めて CAS できる
        "blocked" => Err(JevSessionError::ConcurrentSessionChange {
            expected: "(new session)".to_string(),
            found: "blocked".to_string(),
        }),
        _ => Err(JevSessionError::NotResumable { status: existing.status }),
    }
}

fn is_session_key_conflict(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::ConstraintViolation
    ) && error.to_string().contains("jev_eval_sessions.session_key")
}

/// session を閉じる。以後 resume できない。
///
/// status を動かす操作なので世代を進める。今後 `closed` へ遷移させる API を
/// 増やすときも、同じ規則（status を変えたら `lifecycle_revision += 1`）に従うこと。
pub fn close_session(conn: &Connection, session_id: i64) -> Result<(), JevSessionError> {
    conn.execute(
        "UPDATE jev_eval_sessions
            SET status = 'closed',
                lifecycle_revision = lifecycle_revision + 1,
                closed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
          WHERE id = ?1",
        rusqlite::params![session_id],
    )?;
    Ok(())
}

pub fn load_session(conn: &Connection, session_id: i64) -> Result<JevSession, JevSessionError> {
    let session = conn.query_row(
        "SELECT id, session_key, contract_version, state_schema_version, requested_model,
                model_card_json, max_calls, max_input_tokens, calls_reserved,
                input_tokens_used, output_tokens_used, status, lifecycle_revision,
                blocked_reason
           FROM jev_eval_sessions WHERE id = ?1",
        rusqlite::params![session_id],
        |row| {
            Ok(JevSession {
                id: row.get(0)?,
                session_key: row.get(1)?,
                contract_version: row.get(2)?,
                state_schema_version: row.get(3)?,
                requested_model: row.get(4)?,
                model_card_json: row.get(5)?,
                max_calls: row.get(6)?,
                max_input_tokens: row.get(7)?,
                calls_reserved: row.get(8)?,
                input_tokens_used: row.get(9)?,
                output_tokens_used: row.get(10)?,
                status: row.get(11)?,
                lifecycle_revision: row.get(12)?,
                blocked_reason: row.get(13)?,
            })
        },
    )?;
    Ok(session)
}

// ─── 予算 ────────────────────────────────────────────────────────────────────

/// 論理 call を 1 件予約する。**HTTP を送る前に呼ぶ。**
///
/// 予約は commit する。送信が失敗しても、途中で落ちても戻さない。戻す設計にすると
/// crash のたびに上限を超えて呼べてしまうので、安全側（使い切り側）に倒す。
/// C3B の retry は同じ論理 call の中で完結するので、追加の予約はしない。
pub fn reserve_call_budget(
    conn: &mut Connection,
    session_id: i64,
) -> Result<CallReservation, JevSessionError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    let (status, calls_reserved, max_calls, input_tokens_used, max_input_tokens): (
        String,
        i64,
        i64,
        i64,
        i64,
    ) = tx.query_row(
        "SELECT status, calls_reserved, max_calls, input_tokens_used, max_input_tokens
           FROM jev_eval_sessions WHERE id = ?1",
        rusqlite::params![session_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    )?;

    // 予算切れと、停止・終了は別の話として扱う
    match status.as_str() {
        "open" => {}
        "exhausted" => {
            tx.commit()?;
            return Err(JevSessionError::BudgetExhausted);
        }
        "blocked" => {
            let reason = load_blocked_reason(&tx, session_id)?;
            tx.commit()?;
            return Err(JevSessionError::Blocked { reason });
        }
        _ => {
            tx.commit()?;
            return Err(JevSessionError::SessionClosed);
        }
    }

    if calls_reserved >= max_calls || input_tokens_used >= max_input_tokens {
        // status を動かすので世代を進める（予約そのものでは進めない）
        tx.execute(
            "UPDATE jev_eval_sessions
                SET status = 'exhausted', lifecycle_revision = lifecycle_revision + 1
              WHERE id = ?1",
            rusqlite::params![session_id],
        )?;
        tx.commit()?;
        return Err(JevSessionError::BudgetExhausted);
    }

    let reserved = calls_reserved + 1;
    tx.execute(
        "UPDATE jev_eval_sessions SET calls_reserved = ?2 WHERE id = ?1",
        rusqlite::params![session_id, reserved],
    )?;
    tx.commit()?;

    Ok(CallReservation { session_id, calls_reserved: reserved })
}

fn load_blocked_reason(
    tx: &rusqlite::Transaction<'_>,
    session_id: i64,
) -> Result<String, JevSessionError> {
    let reason: Option<String> = tx.query_row(
        "SELECT blocked_reason FROM jev_eval_sessions WHERE id = ?1",
        rusqlite::params![session_id],
        |row| row.get(0),
    )?;
    Ok(reason.unwrap_or_else(|| "blocked".to_string()))
}

/// 成功した呼び出しの token を加算する。
///
/// 実際の消費量は応答が返るまで分からないので、入力トークンの上限は
/// **論理 call 1 回分だけ超過し得る**。超過したらそこで `exhausted` にする。
/// 出力トークンは記録するだけで、C4 v1 では上限に使わない。
///
/// この「1 回分まで」は、**C4b v1 が同じ session の logical call を逐次実行する**
/// という前提の上に成り立つ:
///
/// ```text
/// reserve N → HTTP N → token accounting N → 初めて reserve N+1
/// ```
///
/// 同一 session で複数の call を同時に in-flight にすると、消費量が判明する前に
/// 予約が通ってしまい、上限を call 数分だけ超過し得る。`reserve_call_budget` が
/// 並行安全であることと、C4b が並行実行してよいことは別の話。逐次性は C4b 側で
/// テストする。
///
/// 既に `exhausted` になっていても、予約済み call の記帳は通す。
pub fn record_token_usage(
    conn: &mut Connection,
    session_id: i64,
    input_tokens: i64,
    output_tokens: i64,
) -> Result<TokenUsage, JevSessionError> {
    if input_tokens < 0 || output_tokens < 0 {
        return Err(JevSessionError::InvalidTokenCount("負の値です".into()));
    }

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    let (used_input, used_output, max_input, status): (i64, i64, i64, String) = tx.query_row(
        "SELECT input_tokens_used, output_tokens_used, max_input_tokens, status
           FROM jev_eval_sessions WHERE id = ?1",
        rusqlite::params![session_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;

    // 溢れる場合は何も書かずに失敗させる
    let next_input = used_input
        .checked_add(input_tokens)
        .ok_or_else(|| JevSessionError::InvalidTokenCount("input_tokens が桁あふれします".into()))?;
    let next_output = used_output.checked_add(output_tokens).ok_or_else(|| {
        JevSessionError::InvalidTokenCount("output_tokens が桁あふれします".into())
    })?;

    // closed / blocked は記帳だけ通して status を動かさない
    let next_status = if (status == "open" || status == "exhausted") && next_input >= max_input {
        "exhausted".to_string()
    } else {
        status.clone()
    };

    // status が変わるときだけ世代を進める。単なる加算では進めない
    let bump = if next_status == status { 0 } else { 1 };
    tx.execute(
        "UPDATE jev_eval_sessions
            SET input_tokens_used = ?2,
                output_tokens_used = ?3,
                status = ?4,
                lifecycle_revision = lifecycle_revision + ?5
          WHERE id = ?1",
        rusqlite::params![session_id, next_input, next_output, next_status, bump],
    )?;
    tx.commit()?;

    Ok(TokenUsage {
        input_tokens_used: next_input,
        output_tokens_used: next_output,
        status: next_status,
    })
}

// ─── テスト ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const MODEL: &str = "jev-test-model-1";
    const KEY: &str = "sk-test-DO-NOT-LEAK-0123456789";

    /// 接続を分けたいテスト用に、使い捨ての DB パスを作る
    fn temp_db_path(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "cinemantis-jev-{label}-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn open_db() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        db::apply_migrations(&conn).expect("migrations");
        conn
    }

    fn config(key: &str) -> JevSessionConfig {
        JevSessionConfig::new(key, MODEL)
    }

    // ─── /v1/models の stub（外部ネットワークへは出ない）──────────────────

    struct Stub {
        base: String,
        hits: Arc<AtomicUsize>,
    }

    impl Stub {
        /// stub が受けた HTTP 接続数（このモジュールの stub は /v1/models しか返さない）
        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    async fn start_stub(body: Option<String>) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_task = hits.clone();

        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                hits_task.fetch_add(1, Ordering::SeqCst);
                let body = body.clone();
                tokio::spawn(async move {
                    let mut buffer = [0u8; 4096];
                    let _ = socket.read(&mut buffer).await;
                    match body {
                        Some(body) => {
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = socket.write_all(response.as_bytes()).await;
                        }
                        None => {
                            let response = "HTTP/1.1 503 X\r\nContent-Length: 2\r\n\
                                            Connection: close\r\n\r\n{}";
                            let _ = socket.write_all(response.as_bytes()).await;
                        }
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

    async fn available_client() -> (TypeSafeClient, Stub) {
        let stub = start_stub(Some(models_body())).await;
        let client = TypeSafeClient::with_key_for_test(KEY, &stub.base);
        (client, stub)
    }

    async fn unavailable_client() -> (TypeSafeClient, Stub) {
        let stub = start_stub(None).await;
        let client = TypeSafeClient::with_key_for_test(KEY, &stub.base);
        (client, stub)
    }

    /// 新規 session を open まで通す
    async fn start_open_session(conn: &Connection, key: &str) -> JevSession {
        let plan = plan_session(conn, &config(key)).unwrap();
        let (client, _stub) = available_client().await;
        let outcome = preflight_model(&client, &plan).await;
        apply_preflight(conn, &plan, outcome).unwrap()
    }

    // ─── migration ───────────────────────────────────────────────────────

    fn column_names(conn: &Connection, table: &str) -> Vec<String> {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})")).unwrap();
        let names = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        names
    }

    #[test]
    fn migration_024_adds_the_session_table_and_columns() {
        let conn = open_db();

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='jev_eval_sessions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 1, "jev_eval_sessions が無い");

        let columns = column_names(&conn, "metadata_match_jev_calls");
        assert!(columns.contains(&"request_id".to_string()));
        assert!(columns.contains(&"eval_session_id".to_string()));

        let index: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type='index' AND name='idx_jev_calls_session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index, 1, "index が無い");

        let session_columns = column_names(&conn, "jev_eval_sessions");
        assert!(
            session_columns.contains(&"lifecycle_revision".to_string()),
            "lifecycle_revision が無い"
        );

        // 既定値 0 と、負値の拒否
        conn.execute(
            "INSERT INTO jev_contracts
               (contract_version, state_schema_version, instructions_text,
                questions_template_json, criteria_json, validation_policy_json,
                canonical_sha256, default_model)
             VALUES ('c','s','i','{}','{}','{}','h','m')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO jev_eval_sessions
               (session_key, contract_version, state_schema_version, requested_model,
                model_card_json, max_calls, max_input_tokens, status)
             VALUES ('k','c','s','m','{}',1,1,'open')",
            [],
        )
        .unwrap();
        let revision: i64 = conn
            .query_row(
                "SELECT lifecycle_revision FROM jev_eval_sessions WHERE session_key = 'k'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(revision, 0, "既定値が 0 でない");

        let negative = conn.execute(
            "UPDATE jev_eval_sessions SET lifecycle_revision = -1 WHERE session_key = 'k'",
            [],
        );
        assert!(negative.is_err(), "負の世代が通ってしまう");
        conn.execute("DELETE FROM jev_eval_sessions WHERE session_key = 'k'", [])
            .unwrap();
        conn.execute("DELETE FROM jev_contracts WHERE contract_version = 'c'", [])
            .unwrap();

        let foreign_keys: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0)).unwrap();
        assert_eq!(foreign_keys, 1, "外部キーが有効でない");

        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        assert_eq!(db::foreign_key_check_rows(&conn).unwrap().len(), 0);
    }

    #[test]
    fn migrations_are_idempotent() {
        let conn = open_db();
        // 起動のたびに再実行される前提なので、2 回目も通ること
        db::apply_migrations(&conn).expect("2 回目");
        db::apply_migrations(&conn).expect("3 回目");

        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM jev_eval_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sessions, 0);
        let columns = column_names(&conn, "metadata_match_jev_calls");
        assert_eq!(columns.iter().filter(|name| *name == "request_id").count(), 1);
        assert_eq!(
            columns.iter().filter(|name| *name == "eval_session_id").count(),
            1
        );
    }

    /// 023 までの行を持つ DB に 024 をかけても既存行を壊さない
    #[test]
    fn existing_jev_call_rows_survive_the_upgrade() {
        let conn = open_db();
        let contract = contract_v2();
        ensure_contract_registered(&conn, &contract, MODEL).unwrap();

        let run_id = insert_run(&conn);
        conn.execute(
            "INSERT INTO metadata_match_jev_calls
               (run_id, call_seq, call_kind, contract_version, state_schema_version,
                requested_model, state_json, questions_json, candidate_order_json, status)
             VALUES (?1, 1, 'packed', ?2, ?3, ?4, '{}', '{}', '[]', 'error')",
            rusqlite::params![
                run_id,
                contract.contract_version,
                contract.state_schema_version,
                MODEL
            ],
        )
        .unwrap();

        // もう一度 024 を流す（起動のたびに再実行される）
        db::apply_migrations(&conn).unwrap();

        let (count, session_id, request_id): (i64, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT COUNT(*), MAX(eval_session_id), MAX(request_id)
                   FROM metadata_match_jev_calls",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(count, 1, "既存行が消えている");
        assert_eq!(session_id, None, "legacy 行は session を持たなくてよい");
        assert_eq!(request_id, None);
        assert_eq!(db::foreign_key_check_rows(&conn).unwrap().len(), 0);
    }

    /// 023 までの DB に 024 を当てる、本物の upgrade
    #[test]
    fn upgrading_from_023_adds_the_new_schema_without_touching_old_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        db::apply_migrations_through_023(&conn).unwrap();

        // 024 前の姿であることを確かめる
        let columns = column_names(&conn, "metadata_match_jev_calls");
        assert!(!columns.contains(&"request_id".to_string()), "024 が先に入っている");
        assert!(!columns.contains(&"eval_session_id".to_string()));
        let sessions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type='table' AND name='jev_eval_sessions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sessions, 0, "jev_eval_sessions が既にある");

        // 023 時点の legacy 行を入れる
        let contract = contract_v2();
        ensure_contract_registered(&conn, &contract, MODEL).unwrap();
        let run_id = insert_run(&conn);
        conn.execute(
            "INSERT INTO metadata_match_jev_calls
               (run_id, call_seq, call_kind, contract_version, state_schema_version,
                requested_model, state_json, questions_json, candidate_order_json, status)
             VALUES (?1, 1, 'packed', ?2, ?3, ?4, '{\"s\":1}', '{\"q\":1}', '[\"c1\"]', 'error')",
            rusqlite::params![
                run_id,
                contract.contract_version,
                contract.state_schema_version,
                MODEL
            ],
        )
        .unwrap();

        // ここで 024 を当てる
        db::apply_migration_024(&conn).unwrap();

        let columns = column_names(&conn, "metadata_match_jev_calls");
        assert!(columns.contains(&"request_id".to_string()));
        assert!(columns.contains(&"eval_session_id".to_string()));
        let session_columns = column_names(&conn, "jev_eval_sessions");
        assert!(session_columns.contains(&"lifecycle_revision".to_string()));
        let index: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type='index' AND name='idx_jev_calls_session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index, 1);

        // legacy 行は残り、新しい列は NULL
        let (count, state, request_id, session_id): (i64, String, Option<String>, Option<i64>) =
            conn.query_row(
                "SELECT COUNT(*), MAX(state_json), MAX(request_id), MAX(eval_session_id)
                   FROM metadata_match_jev_calls",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(count, 1, "legacy 行が消えている");
        assert_eq!(state, "{\"s\":1}", "既存の値が書き換わっている");
        assert_eq!(request_id, None);
        assert_eq!(session_id, None);

        // もう一度当てても壊れない
        db::apply_migration_024(&conn).unwrap();
        db::apply_migrations(&conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_match_jev_calls", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);

        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        assert_eq!(db::foreign_key_check_rows(&conn).unwrap().len(), 0);
    }

    #[tokio::test]
    async fn eval_session_id_must_reference_a_real_session() {
        let conn = open_db();
        let session = start_open_session(&conn, "fk-test").await;
        let run_id = insert_run(&conn);

        // 存在しない session を指すと拒否される
        let error = conn
            .execute(
                "INSERT INTO metadata_match_jev_calls
                   (run_id, call_seq, call_kind, contract_version, state_schema_version,
                    requested_model, state_json, questions_json, candidate_order_json,
                    status, eval_session_id)
                 VALUES (?1, 1, 'packed', ?2, ?3, ?4, '{}', '{}', '[]', 'error', 999999)",
                rusqlite::params![run_id, "jev-contract-2", "cm-jev-state-1", MODEL],
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("FOREIGN KEY constraint failed"),
            "{error}"
        );

        // 実在する session なら通る
        conn.execute(
            "INSERT INTO metadata_match_jev_calls
               (run_id, call_seq, call_kind, contract_version, state_schema_version,
                requested_model, state_json, questions_json, candidate_order_json,
                status, eval_session_id)
             VALUES (?1, 2, 'packed', ?2, ?3, ?4, '{}', '{}', '[]', 'error', ?5)",
            rusqlite::params![
                run_id,
                "jev-contract-2",
                "cm-jev-state-1",
                MODEL,
                session.id
            ],
        )
        .unwrap();
    }

    fn insert_run(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO works (title, work_type, media_category) VALUES ('テスト作品', 'movie', 'movie')",
            [],
        )
        .unwrap();
        let work_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO metadata_match_runs
               (work_id, trigger_kind, mode, evidence_class, state_schema_version,
                input_snapshot_json, search_queries_json, policy_version,
                policy_snapshot_json, governing_matcher, decision, decision_reasons_json)
             VALUES (?1, 'single', 'safe', 'live', 'cm-prematch-2',
                     '{}', '[]', 'rules-safe-2', '{}', 'rules-safe', 'UNRESOLVED', '[]')",
            rusqlite::params![work_id],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    // ─── contract 登録 ───────────────────────────────────────────────────

    fn stored_contract(conn: &Connection) -> (String, String, String, String, String, String) {
        conn.query_row(
            "SELECT instructions_text, questions_template_json, criteria_json,
                    validation_policy_json, canonical_sha256, default_model
               FROM jev_contracts WHERE contract_version = 'jev-contract-2'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap()
    }

    #[test]
    fn contract_is_registered_once_and_then_verified() {
        let conn = open_db();
        let contract = contract_v2();

        // 未登録 → INSERT
        ensure_contract_registered(&conn, &contract, MODEL).unwrap();
        let first = stored_contract(&conn);
        assert_eq!(first.4, contract.canonical_sha256);
        assert_eq!(first.5, MODEL, "default_model は初回登録時のモデル");

        // 同じ contract → 何度でも成功し、行は増えない
        ensure_contract_registered(&conn, &contract, MODEL).unwrap();
        ensure_contract_registered(&conn, &contract, "jev-another-model").unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM jev_contracts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // default_model だけ違っても成功し、既存行を書き換えない
        assert_eq!(stored_contract(&conn), first, "既存行が書き換わっている");
    }

    #[test]
    fn stored_json_formatting_does_not_matter() {
        let conn = open_db();
        let contract = contract_v2();
        ensure_contract_registered(&conn, &contract, MODEL).unwrap();

        // 整形とキー順だけ違う形で保存し直す
        let pretty = serde_json::to_string_pretty(&contract.questions_template).unwrap();
        conn.execute(
            "UPDATE jev_contracts SET questions_template_json = ?1
              WHERE contract_version = 'jev-contract-2'",
            rusqlite::params![pretty],
        )
        .unwrap();

        ensure_contract_registered(&conn, &contract, MODEL)
            .expect("canonical 比較なので整形差は許す");
    }

    #[test]
    fn contract_mismatch_fails_closed_without_repairing() {
        let contract = contract_v2();
        let cases: [(&str, &str, &str); 5] = [
            ("instructions_text", "instructions_text", "違う指示"),
            ("questions_template_json", "questions_template_json", "{\"x\":1}"),
            ("criteria_json", "criteria_json", "{\"x\":1}"),
            ("validation_policy_json", "validation_policy_json", "{\"x\":1}"),
            ("canonical_sha256", "canonical_sha256", "0000"),
        ];

        for (field, column, value) in cases {
            let conn = open_db();
            ensure_contract_registered(&conn, &contract, MODEL).unwrap();
            conn.execute(
                &format!(
                    "UPDATE jev_contracts SET {column} = ?1 WHERE contract_version = 'jev-contract-2'"
                ),
                rusqlite::params![value],
            )
            .unwrap();
            let before = stored_contract(&conn);

            let error = ensure_contract_registered(&conn, &contract, MODEL).unwrap_err();
            assert_eq!(error, JevSessionError::ContractMismatch { field }, "{field}");
            assert_eq!(stored_contract(&conn), before, "{field}: 行を直しに行っている");
        }

        // state_schema_version の不一致も同じ
        let conn = open_db();
        ensure_contract_registered(&conn, &contract, MODEL).unwrap();
        conn.execute(
            "UPDATE jev_contracts SET state_schema_version = 'cm-jev-state-9'
              WHERE contract_version = 'jev-contract-2'",
            [],
        )
        .unwrap();
        assert_eq!(
            ensure_contract_registered(&conn, &contract, MODEL).unwrap_err(),
            JevSessionError::ContractMismatch { field: "state_schema_version" }
        );
    }

    /// 2 つの接続が同時に初回登録したとき
    #[test]
    fn concurrent_first_registration_is_idempotent() {
        let path = temp_db_path("contract-race");
        let contract = contract_v2();

        let conn_a = Connection::open(&path).unwrap();
        conn_a.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        db::apply_migrations(&conn_a).unwrap();
        let conn_b = Connection::open(&path).unwrap();
        conn_b.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
        conn_b.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        // 両方が「未登録」を見ている状態から、A が先に入れる
        assert!(read_contract_row(&conn_a, &contract.contract_version)
            .unwrap()
            .is_none());
        assert!(read_contract_row(&conn_b, &contract.contract_version)
            .unwrap()
            .is_none());
        ensure_contract_registered(&conn_a, &contract, "model-a").unwrap();

        // 負けた側は UNIQUE を外へ出さず、凍結フィールドの一致だけを見る。
        // B の関数内 SELECT では既に A の行が見えてしまうので、INSERT 競合の分岐も
        // 直接叩いて、その経路が既存行を返すことまで確かめる。
        ensure_contract_registered(&conn_b, &contract, "model-b")
            .expect("同じ contract なら成功する");

        let reconciled = insert_contract_or_reconcile(
            &conn_b,
            &contract,
            "model-b",
            &canonical_json(&contract.questions_template),
            &canonical_json(&contract.criteria),
            &canonical_json(&contract.validation_policy),
        )
        .expect("競合しても Err にしない")
        .expect("既存行が返る");
        assert_eq!(reconciled.1, contract.instructions);
        assert_eq!(reconciled.5, contract.canonical_sha256);

        let (count, default_model): (i64, String) = conn_a
            .query_row(
                "SELECT COUNT(*), MAX(default_model) FROM jev_contracts",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1, "行が重複している");
        assert_eq!(default_model, "model-a", "後から来た側が default_model を上書きした");

        drop(conn_a);
        drop(conn_b);
        let _ = std::fs::remove_file(&path);
    }

    /// INSERT 競合の分岐が、内容の違う既存行を黙って受け入れないこと
    #[test]
    fn the_insert_conflict_branch_surfaces_a_mismatch() {
        let conn = open_db();
        let contract = contract_v2();

        // 別内容の同じ版が先に入っている
        conn.execute(
            "INSERT INTO jev_contracts
               (contract_version, state_schema_version, instructions_text,
                questions_template_json, criteria_json, validation_policy_json,
                canonical_sha256, default_model)
             VALUES (?1, ?2, '違う指示', '{}', '{}', '{}', 'deadbeef', 'model-a')",
            rusqlite::params![contract.contract_version, contract.state_schema_version],
        )
        .unwrap();

        // 競合分岐は既存行をそのまま返し、判断は呼び出し側の照合に任せる
        let row = insert_contract_or_reconcile(
            &conn,
            &contract,
            "model-b",
            &canonical_json(&contract.questions_template),
            &canonical_json(&contract.criteria),
            &canonical_json(&contract.validation_policy),
        )
        .unwrap()
        .expect("既存行が返る");
        assert_eq!(row.1, "違う指示", "既存行を書き換えている");

        assert_eq!(
            ensure_contract_registered(&conn, &contract, "model-b").unwrap_err(),
            JevSessionError::ContractMismatch { field: "instructions_text" }
        );
    }

    /// 競合相手の contract 内容が違えば、書き換えずに mismatch
    #[test]
    fn a_conflicting_first_registration_is_rejected() {
        let conn = open_db();
        let contract = contract_v2();

        // 先に「別内容の同じ版」が入っている状態を作る
        conn.execute(
            "INSERT INTO jev_contracts
               (contract_version, state_schema_version, instructions_text,
                questions_template_json, criteria_json, validation_policy_json,
                canonical_sha256, default_model)
             VALUES (?1, ?2, '違う指示', '{}', '{}', '{}', 'deadbeef', 'model-a')",
            rusqlite::params![contract.contract_version, contract.state_schema_version],
        )
        .unwrap();
        let before = stored_contract(&conn);

        assert_eq!(
            ensure_contract_registered(&conn, &contract, "model-b").unwrap_err(),
            JevSessionError::ContractMismatch { field: "instructions_text" }
        );
        assert_eq!(stored_contract(&conn), before, "既存行を書き換えている");
    }

    // ─── session lifecycle ───────────────────────────────────────────────

    #[tokio::test]
    async fn a_new_session_opens_with_the_pinned_model() {
        let conn = open_db();
        let session = start_open_session(&conn, "  eval-1  ").await;

        assert_eq!(session.session_key, "eval-1", "session_key は trim 済み");
        assert_eq!(session.status, "open");
        assert_eq!(session.requested_model, MODEL);
        assert_eq!(session.contract_version, "jev-contract-2");
        assert_eq!(session.state_schema_version, "cm-jev-state-1");
        assert_eq!(session.max_calls, DEFAULT_MAX_CALLS);
        assert_eq!(session.max_input_tokens, DEFAULT_MAX_INPUT_TOKENS);
        assert_eq!(session.calls_reserved, 0);
        assert_eq!(session.blocked_reason, None);

        let card: Value = serde_json::from_str(&session.model_card_json).unwrap();
        assert_eq!(card["name"], MODEL);
        assert_eq!(card["release_date"], "2026-01-01");
    }

    #[test]
    fn invalid_configs_are_rejected() {
        let conn = open_db();

        let long_key = "k".repeat(MAX_SESSION_KEY_LEN + 1);
        let cases = [
            ("", MODEL),
            ("   ", MODEL),
            (long_key.as_str(), MODEL),
            ("bad\nkey", MODEL),
            ("bad\tkey", MODEL),
            ("bad\u{0}key", MODEL),
            ("eval-ok", ""),
            ("eval-ok", "   "),
        ];
        for (key, model) in cases {
            let config = JevSessionConfig::new(key, model);
            let error = plan_session(&conn, &config).unwrap_err();
            assert!(
                matches!(error, JevSessionError::InvalidConfig(_)),
                "key={key:?} model={model:?} が通ってしまった"
            );
        }

        // 上限ちょうどは通る
        let ok = JevSessionConfig::new("k".repeat(MAX_SESSION_KEY_LEN), MODEL);
        assert!(plan_session(&conn, &ok).is_ok());

        // 予算は正の数だけ
        let mut zero = config("eval-zero");
        zero.max_calls = 0;
        assert!(matches!(
            plan_session(&conn, &zero).unwrap_err(),
            JevSessionError::InvalidConfig(_)
        ));
        let mut negative = config("eval-neg");
        negative.max_input_tokens = -1;
        assert!(matches!(
            plan_session(&conn, &negative).unwrap_err(),
            JevSessionError::InvalidConfig(_)
        ));
    }

    #[tokio::test]
    async fn the_same_key_resumes_and_a_different_config_is_rejected() {
        let conn = open_db();
        let first = start_open_session(&conn, "eval-resume").await;

        // 同じ設定 → resume（行は増えない）
        let plan = plan_session(&conn, &config("eval-resume")).unwrap();
        assert_eq!(plan.existing_id, Some(first.id));
        let (client, _stub) = available_client().await;
        let outcome = preflight_model(&client, &plan).await;
        let resumed = apply_preflight(&conn, &plan, outcome).unwrap();
        assert_eq!(resumed.id, first.id);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM jev_eval_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // model が違う → 既存値を書き換えず拒否
        let other_model = JevSessionConfig::new("eval-resume", "jev-other-model");
        assert_eq!(
            plan_session(&conn, &other_model).unwrap_err(),
            JevSessionError::SessionConfigMismatch { field: "requested_model" }
        );

        // 予算が違う → 拒否
        let mut other_calls = config("eval-resume");
        other_calls.max_calls = 10;
        assert_eq!(
            plan_session(&conn, &other_calls).unwrap_err(),
            JevSessionError::SessionConfigMismatch { field: "max_calls" }
        );
        let mut other_tokens = config("eval-resume");
        other_tokens.max_input_tokens = 10;
        assert_eq!(
            plan_session(&conn, &other_tokens).unwrap_err(),
            JevSessionError::SessionConfigMismatch { field: "max_input_tokens" }
        );

        // contract / state schema が違う（DB 側を書き換えて再現）
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
            rusqlite::params![first.id],
        )
        .unwrap();
        assert_eq!(
            plan_session(&conn, &config("eval-resume")).unwrap_err(),
            JevSessionError::SessionConfigMismatch { field: "contract_version" }
        );

        // 元に戻して state_schema_version の不一致も確認
        conn.execute(
            "UPDATE jev_eval_sessions
                SET contract_version = 'jev-contract-2',
                    state_schema_version = 'cm-jev-state-9'
              WHERE id = ?1",
            rusqlite::params![first.id],
        )
        .unwrap();
        assert_eq!(
            plan_session(&conn, &config("eval-resume")).unwrap_err(),
            JevSessionError::SessionConfigMismatch { field: "state_schema_version" }
        );
    }

    #[tokio::test]
    async fn closed_and_exhausted_sessions_cannot_resume() {
        let conn = open_db();

        let closed = start_open_session(&conn, "eval-closed").await;
        close_session(&conn, closed.id).unwrap();
        assert_eq!(
            plan_session(&conn, &config("eval-closed")).unwrap_err(),
            JevSessionError::NotResumable { status: "closed".to_string() }
        );

        let exhausted = start_open_session(&conn, "eval-exhausted").await;
        conn.execute(
            "UPDATE jev_eval_sessions SET status = 'exhausted' WHERE id = ?1",
            rusqlite::params![exhausted.id],
        )
        .unwrap();
        assert_eq!(
            plan_session(&conn, &config("eval-exhausted")).unwrap_err(),
            JevSessionError::NotResumable { status: "exhausted".to_string() }
        );
    }

    #[tokio::test]
    async fn preflight_failure_blocks_without_switching_models() {
        let conn = open_db();
        let session = start_open_session(&conn, "eval-block").await;
        let original_card = session.model_card_json.clone();

        // 再開時に preflight が失敗 → blocked
        let plan = plan_session(&conn, &config("eval-block")).unwrap();
        let (client, _stub) = unavailable_client().await;
        let outcome = preflight_model(&client, &plan).await;
        let error = apply_preflight(&conn, &plan, outcome).unwrap_err();
        assert!(matches!(error, JevSessionError::Blocked { .. }), "{error}");

        let blocked = load_session(&conn, session.id).unwrap();
        assert_eq!(blocked.status, "blocked");
        assert_eq!(blocked.requested_model, MODEL, "モデルを差し替えない");
        let reason = blocked.blocked_reason.expect("理由を残す");
        assert!(reason.contains("model preflight failed"));
        assert!(!reason.contains(KEY), "API キーが理由に入っている");
        assert!(!reason.contains("{"), "応答本文が理由に入っている");

        // blocked でも preflight が通れば open に戻る。snapshot は上書きしない
        let plan = plan_session(&conn, &config("eval-block")).unwrap();
        let (client, _stub) = available_client().await;
        let outcome = preflight_model(&client, &plan).await;
        let reopened = apply_preflight(&conn, &plan, outcome).unwrap();
        assert_eq!(reopened.status, "open");
        assert_eq!(reopened.blocked_reason, None);
        assert_eq!(reopened.model_card_json, original_card, "snapshot を上書きしている");
    }

    #[tokio::test]
    async fn a_failed_new_session_preflight_creates_no_row() {
        let conn = open_db();
        let plan = plan_session(&conn, &config("eval-new-fail")).unwrap();
        let (client, _stub) = unavailable_client().await;
        let outcome = preflight_model(&client, &plan).await;

        let error = apply_preflight(&conn, &plan, outcome).unwrap_err();
        assert!(matches!(error, JevSessionError::Blocked { .. }));

        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM jev_eval_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sessions, 0, "session 行を作ってしまっている");

        // contract 行だけ先に入っているのは問題ない
        let contracts: i64 = conn
            .query_row("SELECT COUNT(*) FROM jev_contracts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(contracts, 1);
    }

    /// pin したモデルが一覧に無ければ、似た名前へ寄せずに落とす
    #[tokio::test]
    async fn no_alias_or_latest_fallback() {
        let conn = open_db();
        let stub = start_stub(Some(models_body())).await;
        let client = TypeSafeClient::with_key_for_test(KEY, &stub.base);

        for name in ["jev-latest", "jev-test-model", "JEV-TEST-MODEL-1"] {
            let config = JevSessionConfig::new(format!("eval-{name}"), name);
            let plan = plan_session(&conn, &config).unwrap();
            let outcome = preflight_model(&client, &plan).await;
            assert!(
                matches!(outcome, PreflightOutcome::Unavailable { .. }),
                "{name} が採用されてしまった"
            );
            assert!(apply_preflight(&conn, &plan, outcome).is_err());
        }

        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM jev_eval_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sessions, 0);
    }

    // ─── preflight と plan のモデル一致 ──────────────────────────────────

    const OTHER_MODEL: &str = "jev-test-model-0";

    /// 別モデルの preflight 結果で新規 session を作らない
    #[tokio::test]
    async fn a_new_session_rejects_an_outcome_for_another_model() {
        let conn = open_db();
        let plan = plan_session(&conn, &config("eval-model-bind")).unwrap();

        // 手作りで「モデル B についての成功」を作る
        let outcome = PreflightOutcome::Available {
            requested_model: OTHER_MODEL.to_string(),
            identity: ModelIdentity::Listed(ModelCard {
                name: OTHER_MODEL.to_string(),
                description: "other".to_string(),
                release_date: "2025-01-01".to_string(),
            }),
        };
        assert_eq!(
            apply_preflight(&conn, &plan, outcome).unwrap_err(),
            JevSessionError::PreflightModelMismatch {
                expected: MODEL.to_string(),
                found: OTHER_MODEL.to_string(),
            }
        );

        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM jev_eval_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sessions, 0, "session 行を作ってしまっている");
    }

    /// resume でも、別モデルの成功・失敗のどちらも受け付けない
    #[tokio::test]
    async fn a_resume_rejects_outcomes_for_another_model() {
        let conn = open_db();
        let session = start_open_session(&conn, "eval-model-bind-resume").await;
        let before = load_session(&conn, session.id).unwrap();

        let outcomes = [
            PreflightOutcome::Available {
                requested_model: OTHER_MODEL.to_string(),
                identity: ModelIdentity::Listed(ModelCard {
                    name: OTHER_MODEL.to_string(),
                    description: "other".to_string(),
                    release_date: "2025-01-01".to_string(),
                }),
            },
            PreflightOutcome::Unavailable {
                requested_model: OTHER_MODEL.to_string(),
                reason: "model preflight failed: overloaded".to_string(),
            },
        ];

        for outcome in outcomes {
            let plan = plan_session(&conn, &config("eval-model-bind-resume")).unwrap();
            assert_eq!(
                apply_preflight(&conn, &plan, outcome).unwrap_err(),
                JevSessionError::PreflightModelMismatch {
                    expected: MODEL.to_string(),
                    found: OTHER_MODEL.to_string(),
                }
            );
            assert_eq!(
                load_session(&conn, session.id).unwrap(),
                before,
                "status / revision / snapshot のいずれかが動いている"
            );
        }
    }

    /// requested_model は合っているが ModelCard の名前が違う場合も弾く
    #[tokio::test]
    async fn a_mismatched_model_card_is_rejected() {
        let conn = open_db();
        let plan = plan_session(&conn, &config("eval-card-mismatch")).unwrap();

        let outcome = PreflightOutcome::Available {
            requested_model: MODEL.to_string(),
            identity: ModelIdentity::Listed(ModelCard {
                name: OTHER_MODEL.to_string(),
                description: "swapped".to_string(),
                release_date: "2025-01-01".to_string(),
            }),
        };
        assert_eq!(
            apply_preflight(&conn, &plan, outcome).unwrap_err(),
            JevSessionError::PreflightModelMismatch {
                expected: MODEL.to_string(),
                found: OTHER_MODEL.to_string(),
            }
        );

        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM jev_eval_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sessions, 0);
    }

    /// preflight は plan のモデルしか見に行かない
    #[tokio::test]
    async fn preflight_uses_the_planned_model_only() {
        let conn = open_db();
        let plan = plan_session(&conn, &config("eval-plan-model")).unwrap();
        let (client, _stub) = available_client().await;

        let outcome = preflight_model(&client, &plan).await;
        assert_eq!(outcome.requested_model(), MODEL);
        match outcome {
            PreflightOutcome::Available { identity: ModelIdentity::Listed(card), .. } => {
                assert_eq!(card.name, MODEL)
            }
            other => panic!("利用可のはずが {other:?}"),
        }
    }

    // ─── plan と apply の間に状態が変わる場合 ────────────────────────────

    /// preflight の最中に closed / exhausted になった session を開け直さない
    #[tokio::test]
    async fn a_finished_session_is_not_reopened_by_a_successful_preflight() {
        for terminal in ["closed", "exhausted"] {
            let conn = open_db();
            let session = start_open_session(&conn, &format!("eval-race-{terminal}")).await;
            conn.execute(
                "UPDATE jev_eval_sessions SET blocked_reason = '理由なし' WHERE id = ?1",
                rusqlite::params![session.id],
            )
            .unwrap();

            // plan の時点ではまだ open
            let plan = plan_session(&conn, &config(&format!("eval-race-{terminal}"))).unwrap();
            assert_eq!(plan.existing_id, Some(session.id));

            // preflight を待つ間に別の経路が終了させた
            conn.execute(
                "UPDATE jev_eval_sessions SET status = ?2 WHERE id = ?1",
                rusqlite::params![session.id, terminal],
            )
            .unwrap();

            let (client, _stub) = available_client().await;
            let outcome = preflight_model(&client, &plan).await;
            let error = apply_preflight(&conn, &plan, outcome).unwrap_err();
            assert_eq!(
                error,
                JevSessionError::NotResumable { status: terminal.to_string() },
                "{terminal}"
            );

            let after = load_session(&conn, session.id).unwrap();
            assert_eq!(after.status, terminal, "{terminal}: open へ戻している");
            assert_eq!(
                after.blocked_reason.as_deref(),
                Some("理由なし"),
                "{terminal}: blocked_reason を勝手に消している"
            );
        }
    }

    /// preflight の最中に closed / exhausted になった session を blocked で塗り潰さない
    #[tokio::test]
    async fn a_finished_session_is_not_blocked_by_a_failed_preflight() {
        for terminal in ["closed", "exhausted"] {
            let conn = open_db();
            let session = start_open_session(&conn, &format!("eval-race-fail-{terminal}")).await;
            let plan =
                plan_session(&conn, &config(&format!("eval-race-fail-{terminal}"))).unwrap();

            conn.execute(
                "UPDATE jev_eval_sessions SET status = ?2 WHERE id = ?1",
                rusqlite::params![session.id, terminal],
            )
            .unwrap();

            let (client, _stub) = unavailable_client().await;
            let outcome = preflight_model(&client, &plan).await;
            let error = apply_preflight(&conn, &plan, outcome).unwrap_err();
            assert_eq!(
                error,
                JevSessionError::NotResumable { status: terminal.to_string() },
                "{terminal}"
            );

            let after = load_session(&conn, session.id).unwrap();
            assert_eq!(after.status, terminal, "{terminal}: blocked へ書き換えている");
            assert_eq!(after.blocked_reason, None, "{terminal}: 理由を書き込んでいる");
        }
    }

    /// plan=open のまま別 preflight が blocked にしていたら、古い成功で塗り替えない
    #[tokio::test]
    async fn a_stale_success_does_not_overwrite_a_newer_block() {
        let conn = open_db();
        let session = start_open_session(&conn, "eval-cas-success").await;

        let plan = plan_session(&conn, &config("eval-cas-success")).unwrap();
        assert_eq!(plan.expected_status.as_deref(), Some("open"));

        // 別の preflight が先に blocked にした
        conn.execute(
            "UPDATE jev_eval_sessions
                SET status = 'blocked', blocked_reason = 'model preflight failed: overloaded'
              WHERE id = ?1",
            rusqlite::params![session.id],
        )
        .unwrap();

        let (client, _stub) = available_client().await;
        let outcome = preflight_model(&client, &plan).await;
        assert_eq!(
            apply_preflight(&conn, &plan, outcome).unwrap_err(),
            JevSessionError::ConcurrentSessionChange {
                expected: "open".to_string(),
                found: "blocked".to_string(),
            }
        );

        let after = load_session(&conn, session.id).unwrap();
        assert_eq!(after.status, "blocked", "古い成功で open に戻している");
        assert_eq!(
            after.blocked_reason.as_deref(),
            Some("model preflight failed: overloaded"),
            "新しい理由を消している"
        );
    }

    /// plan=blocked のまま別 preflight が open にしていたら、古い失敗で落とさない
    #[tokio::test]
    async fn a_stale_failure_does_not_overwrite_a_newer_open() {
        let conn = open_db();
        let session = start_open_session(&conn, "eval-cas-failure").await;
        conn.execute(
            "UPDATE jev_eval_sessions
                SET status = 'blocked', blocked_reason = '古い理由' WHERE id = ?1",
            rusqlite::params![session.id],
        )
        .unwrap();

        let plan = plan_session(&conn, &config("eval-cas-failure")).unwrap();
        assert_eq!(plan.expected_status.as_deref(), Some("blocked"));

        // 別の preflight が先に成功して open へ戻した
        conn.execute(
            "UPDATE jev_eval_sessions
                SET status = 'open', blocked_reason = NULL WHERE id = ?1",
            rusqlite::params![session.id],
        )
        .unwrap();

        let (client, _stub) = unavailable_client().await;
        let outcome = preflight_model(&client, &plan).await;
        assert_eq!(
            apply_preflight(&conn, &plan, outcome).unwrap_err(),
            JevSessionError::ConcurrentSessionChange {
                expected: "blocked".to_string(),
                found: "open".to_string(),
            }
        );

        let after = load_session(&conn, session.id).unwrap();
        assert_eq!(after.status, "open", "古い失敗で blocked に落としている");
        assert_eq!(after.blocked_reason, None);
    }

    /// open → blocked → open と往復したあとの古い失敗を弾く（ABA）
    #[tokio::test]
    async fn a_stale_outcome_is_rejected_even_when_the_status_returns() {
        let conn = open_db();
        let session = start_open_session(&conn, "eval-aba-open").await;
        let plan = plan_session(&conn, &config("eval-aba-open")).unwrap();
        assert_eq!(plan.expected_status.as_deref(), Some("open"));
        let revision = plan.expected_lifecycle_revision.unwrap();

        // 別の preflight が blocked にし、さらに別の preflight が open へ戻した
        conn.execute(
            "UPDATE jev_eval_sessions
                SET status = 'blocked', blocked_reason = 'B',
                    lifecycle_revision = lifecycle_revision + 1
              WHERE id = ?1",
            rusqlite::params![session.id],
        )
        .unwrap();
        conn.execute(
            "UPDATE jev_eval_sessions
                SET status = 'open', blocked_reason = NULL,
                    lifecycle_revision = lifecycle_revision + 1
              WHERE id = ?1",
            rusqlite::params![session.id],
        )
        .unwrap();
        let current = load_session(&conn, session.id).unwrap();
        assert_eq!(current.status, "open", "文字列としては plan 時点と同じ");
        assert_eq!(current.lifecycle_revision, revision + 2);

        // 古い失敗を適用しようとしても通らない
        let (client, _stub) = unavailable_client().await;
        let outcome = preflight_model(&client, &plan).await;
        assert_eq!(
            apply_preflight(&conn, &plan, outcome).unwrap_err(),
            JevSessionError::ConcurrentSessionChange {
                expected: "open".to_string(),
                found: "open".to_string(),
            },
            "status が同じでも世代が違えば弾く"
        );

        let after = load_session(&conn, session.id).unwrap();
        assert_eq!(after.status, "open", "stale failure で blocked にしている");
        assert_eq!(after.lifecycle_revision, revision + 2, "世代が動いている");
        assert_eq!(after.blocked_reason, None);
    }

    /// blocked → open → blocked の往復のあとの古い成功を弾く（ABA の逆方向）
    #[tokio::test]
    async fn a_stale_success_is_rejected_after_a_blocked_round_trip() {
        let conn = open_db();
        let session = start_open_session(&conn, "eval-aba-blocked").await;
        conn.execute(
            "UPDATE jev_eval_sessions
                SET status = 'blocked', blocked_reason = '最初の理由',
                    lifecycle_revision = lifecycle_revision + 1
              WHERE id = ?1",
            rusqlite::params![session.id],
        )
        .unwrap();

        let plan = plan_session(&conn, &config("eval-aba-blocked")).unwrap();
        assert_eq!(plan.expected_status.as_deref(), Some("blocked"));
        let revision = plan.expected_lifecycle_revision.unwrap();

        // 別経路が open へ戻し、また別の失敗で blocked になった
        conn.execute(
            "UPDATE jev_eval_sessions
                SET status = 'open', blocked_reason = NULL,
                    lifecycle_revision = lifecycle_revision + 1
              WHERE id = ?1",
            rusqlite::params![session.id],
        )
        .unwrap();
        conn.execute(
            "UPDATE jev_eval_sessions
                SET status = 'blocked', blocked_reason = '新しい理由',
                    lifecycle_revision = lifecycle_revision + 1
              WHERE id = ?1",
            rusqlite::params![session.id],
        )
        .unwrap();

        let (client, _stub) = available_client().await;
        let outcome = preflight_model(&client, &plan).await;
        assert_eq!(
            apply_preflight(&conn, &plan, outcome).unwrap_err(),
            JevSessionError::ConcurrentSessionChange {
                expected: "blocked".to_string(),
                found: "blocked".to_string(),
            }
        );

        let after = load_session(&conn, session.id).unwrap();
        assert_eq!(after.status, "blocked");
        assert_eq!(after.lifecycle_revision, revision + 2);
        assert_eq!(
            after.blocked_reason.as_deref(),
            Some("新しい理由"),
            "古い成功が新しい結果を消している"
        );
    }

    /// 世代が進むのは status を動かす操作のときだけ
    #[tokio::test]
    async fn the_lifecycle_revision_tracks_status_transitions_only() {
        let mut conn = open_db();

        // open → open の成功 preflight でも +1
        let session = start_open_session(&conn, "eval-rev").await;
        assert_eq!(session.lifecycle_revision, 0, "新規は 0 から");
        let plan = plan_session(&conn, &config("eval-rev")).unwrap();
        let (client, _stub) = available_client().await;
        let outcome = preflight_model(&client, &plan).await;
        let reopened = apply_preflight(&conn, &plan, outcome).unwrap();
        assert_eq!(reopened.status, "open");
        assert_eq!(reopened.lifecycle_revision, 1, "open→open でも世代を進める");

        // blocked → blocked の失敗 preflight でも +1
        let plan = plan_session(&conn, &config("eval-rev")).unwrap();
        let (client, _stub) = unavailable_client().await;
        let outcome = preflight_model(&client, &plan).await;
        assert!(apply_preflight(&conn, &plan, outcome).is_err());
        let blocked = load_session(&conn, session.id).unwrap();
        assert_eq!(blocked.status, "blocked");
        assert_eq!(blocked.lifecycle_revision, 2);

        let plan = plan_session(&conn, &config("eval-rev")).unwrap();
        let (client, _stub) = unavailable_client().await;
        let outcome = preflight_model(&client, &plan).await;
        assert!(apply_preflight(&conn, &plan, outcome).is_err());
        let still_blocked = load_session(&conn, session.id).unwrap();
        assert_eq!(still_blocked.status, "blocked");
        assert_eq!(still_blocked.lifecycle_revision, 3, "blocked→blocked でも +1");

        // 通常の予約とトークン加算では世代を動かさない
        let plan = plan_session(&conn, &config("eval-rev")).unwrap();
        let (client, _stub) = available_client().await;
        let outcome = preflight_model(&client, &plan).await;
        let open = apply_preflight(&conn, &plan, outcome).unwrap();
        let base = open.lifecycle_revision;

        reserve_call_budget(&mut conn, session.id).unwrap();
        assert_eq!(
            load_session(&conn, session.id).unwrap().lifecycle_revision,
            base,
            "通常の予約で世代が動いている"
        );
        record_token_usage(&mut conn, session.id, 10, 2).unwrap();
        assert_eq!(
            load_session(&conn, session.id).unwrap().lifecycle_revision,
            base,
            "通常のトークン加算で世代が動いている"
        );
    }

    /// 予算切れとトークン切れで世代が 1 つ進む
    #[tokio::test]
    async fn exhaustion_advances_the_lifecycle_revision() {
        // 呼び出し回数で使い切る
        let mut conn = open_db();
        let plan = plan_session(&conn, &{
            let mut config = config("eval-rev-calls");
            config.max_calls = 1;
            config
        })
        .unwrap();
        let (client, _stub) = available_client().await;
        let outcome = preflight_model(&client, &plan).await;
        let session = apply_preflight(&conn, &plan, outcome).unwrap();
        let base = session.lifecycle_revision;

        reserve_call_budget(&mut conn, session.id).unwrap();
        assert_eq!(
            load_session(&conn, session.id).unwrap().lifecycle_revision,
            base
        );
        assert_eq!(
            reserve_call_budget(&mut conn, session.id).unwrap_err(),
            JevSessionError::BudgetExhausted
        );
        let after = load_session(&conn, session.id).unwrap();
        assert_eq!(after.status, "exhausted");
        assert_eq!(after.lifecycle_revision, base + 1, "予算切れで世代を進める");

        // トークンで使い切る
        let mut conn = open_db();
        let plan = plan_session(&conn, &{
            let mut config = config("eval-rev-tokens");
            config.max_input_tokens = 50;
            config
        })
        .unwrap();
        let (client, _stub) = available_client().await;
        let outcome = preflight_model(&client, &plan).await;
        let session = apply_preflight(&conn, &plan, outcome).unwrap();
        let base = session.lifecycle_revision;

        record_token_usage(&mut conn, session.id, 10, 1).unwrap();
        assert_eq!(
            load_session(&conn, session.id).unwrap().lifecycle_revision,
            base,
            "上限未満では世代を動かさない"
        );
        let usage = record_token_usage(&mut conn, session.id, 100, 1).unwrap();
        assert_eq!(usage.status, "exhausted");
        let after = load_session(&conn, session.id).unwrap();
        assert_eq!(after.lifecycle_revision, base + 1, "トークン切れで世代を進める");

        // exhausted のまま記帳を続けても世代は増えない
        record_token_usage(&mut conn, session.id, 5, 1).unwrap();
        assert_eq!(
            load_session(&conn, session.id).unwrap().lifecycle_revision,
            base + 1
        );
    }

    /// 行そのものが消えていたら、作り直さずに fail closed
    #[tokio::test]
    async fn a_missing_session_row_fails_closed() {
        let conn = open_db();
        let session = start_open_session(&conn, "eval-vanish").await;
        let plan = plan_session(&conn, &config("eval-vanish")).unwrap();

        conn.execute(
            "DELETE FROM jev_eval_sessions WHERE id = ?1",
            rusqlite::params![session.id],
        )
        .unwrap();

        let (client, _stub) = available_client().await;
        let outcome = preflight_model(&client, &plan).await;
        assert_eq!(
            apply_preflight(&conn, &plan, outcome).unwrap_err(),
            JevSessionError::SessionNotFound
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM jev_eval_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "消えた session を作り直している");
    }

    /// 同じ session_key を 2 つの plan が「新規」と判断した場合
    #[tokio::test]
    async fn concurrent_new_sessions_do_not_duplicate_or_overwrite() {
        let conn = open_db();
        let (client, _stub) = available_client().await;

        // 2 つの plan がどちらも「存在しない」と判断する
        let plan_a = plan_session(&conn, &config("eval-concurrent-new")).unwrap();
        let plan_b = plan_session(&conn, &config("eval-concurrent-new")).unwrap();
        assert_eq!(plan_a.existing_id, None);
        assert_eq!(plan_b.existing_id, None);

        let outcome_a = preflight_model(&client, &plan_a).await;
        let created = apply_preflight(&conn, &plan_a, outcome_a).unwrap();

        // 後から来た方は UNIQUE 違反を外へ出さず、既存 session を使う
        let outcome_b = preflight_model(&client, &plan_b).await;
        let second = apply_preflight(&conn, &plan_b, outcome_b).unwrap();
        assert_eq!(second.id, created.id, "別の session を作っている");
        assert_eq!(second.model_card_json, created.model_card_json, "snapshot を上書きしている");
        assert_eq!(second.status, "open");
        assert_eq!(
            second.lifecycle_revision, created.lifecycle_revision,
            "既存 row を UPDATE している"
        );

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM jev_eval_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "重複 session ができている");
    }

    /// 設定が違うまま同時作成された場合は、既存を書き換えずに拒否する
    #[tokio::test]
    async fn a_conflicting_concurrent_creation_is_rejected() {
        let conn = open_db();
        let (client, _stub) = available_client().await;

        let plan_a = plan_session(&conn, &config("eval-concurrent-conflict")).unwrap();
        let mut different = config("eval-concurrent-conflict");
        different.max_calls = 7;
        let plan_b = plan_session(&conn, &different).unwrap();
        assert_eq!(plan_b.existing_id, None);

        let outcome_a = preflight_model(&client, &plan_a).await;
        let created = apply_preflight(&conn, &plan_a, outcome_a).unwrap();

        let outcome_b = preflight_model(&client, &plan_b).await;
        assert_eq!(
            apply_preflight(&conn, &plan_b, outcome_b).unwrap_err(),
            JevSessionError::SessionConfigMismatch { field: "max_calls" }
        );

        assert_eq!(
            load_session(&conn, created.id).unwrap(),
            created,
            "既存 session を書き換えている"
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM jev_eval_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    /// 相手が blocked なら、行が無い前提で取った成功結果で開け直さない
    #[tokio::test]
    async fn a_concurrent_creation_does_not_reopen_a_blocked_session() {
        let conn = open_db();

        let plan_a = plan_session(&conn, &config("eval-concurrent-blocked")).unwrap();
        let plan_b = plan_session(&conn, &config("eval-concurrent-blocked")).unwrap();
        assert_eq!(plan_b.existing_id, None);

        let (ok_client, _ok_stub) = available_client().await;
        let created = apply_preflight(&conn, &plan_a, preflight_model(&ok_client, &plan_a).await)
            .unwrap();

        // 相手が blocked になった
        conn.execute(
            "UPDATE jev_eval_sessions
                SET status = 'blocked', blocked_reason = 'B',
                    lifecycle_revision = lifecycle_revision + 1
              WHERE id = ?1",
            rusqlite::params![created.id],
        )
        .unwrap();
        let before = load_session(&conn, created.id).unwrap();

        let outcome = preflight_model(&ok_client, &plan_b).await;
        let error = apply_preflight(&conn, &plan_b, outcome).unwrap_err();
        assert!(
            matches!(error, JevSessionError::ConcurrentSessionChange { .. }),
            "{error}"
        );
        assert_eq!(
            load_session(&conn, created.id).unwrap(),
            before,
            "blocked を開け直している"
        );
    }

    /// 同時作成の相手が既に終了していたら、開け直さない
    #[tokio::test]
    async fn a_concurrent_creation_against_a_finished_session_is_rejected() {
        let conn = open_db();
        let (client, _stub) = available_client().await;

        let plan_a = plan_session(&conn, &config("eval-concurrent-closed")).unwrap();
        let plan_b = plan_session(&conn, &config("eval-concurrent-closed")).unwrap();

        let outcome_a = preflight_model(&client, &plan_a).await;
        let created = apply_preflight(&conn, &plan_a, outcome_a).unwrap();
        close_session(&conn, created.id).unwrap();

        let outcome_b = preflight_model(&client, &plan_b).await;
        assert_eq!(
            apply_preflight(&conn, &plan_b, outcome_b).unwrap_err(),
            JevSessionError::NotResumable { status: "closed".to_string() }
        );
        assert_eq!(load_session(&conn, created.id).unwrap().status, "closed");
    }

    // ─── canonical な版の pin ────────────────────────────────────────────

    #[test]
    fn canonical_model_names_are_versioned_only() {
        for good in [
            "jev-1.13.0",
            "jev-2.0.0",
            "jev-0.0.1",
            "jev-10.200.3000",
            // 桁数の上限は設けない（TypeSafe に無い制限を足さない）
            "jev-1000000000.0.0",
            "jev-99999999999999999999.1.2",
        ] {
            assert!(is_canonical_model_name(good), "{good} を弾いている");
        }
        for bad in [
            "jev-preview",
            "jev-latest",
            "latest",
            "jev-1.13",
            "jev-1.13.0-preview",
            "jev-a.b.c",
            "jev-1.13.0.1",
            "jev-1..0",
            "jev-",
            "1.13.0",
            "",
            "   ",
        ] {
            assert!(!is_canonical_model_name(bad), "{bad:?} を受け入れている");
        }
    }

    /// canonical な版は `GET /v1/models` を見ずに使う
    ///
    /// TypeSafe の一覧には alias しか出ないが、版を直接指定した SystemOne は通る。
    /// identity の最終的な根拠は応答の `model` の完全一致（C3B）であって一覧ではない。
    #[tokio::test]
    async fn a_canonical_version_pin_never_calls_the_models_endpoint() {
        const PINNED: &str = "jev-1.13.0";
        let conn = open_db();
        // 一覧は alias だけ（実 API の現状と同じ）。呼ばれないことを確かめたいので用意する
        let body = json!({"models": [
            {"name": "jev-latest", "description": "alias", "release_date": "2026-09-10"},
            {"name": "jev-preview", "description": "alias", "release_date": "2026-09-10"}
        ]})
        .to_string();
        let stub = start_stub(Some(body)).await;
        let client = TypeSafeClient::with_key_for_test(KEY, &stub.base);

        let config = JevSessionConfig::new("eval-pin", PINNED);
        let plan = plan_session(&conn, &config).unwrap();
        let outcome = preflight_model(&client, &plan).await;
        assert_eq!(
            outcome,
            PreflightOutcome::Available {
                requested_model: PINNED.to_string(),
                identity: ModelIdentity::ExplicitVersionPin,
            }
        );
        assert_eq!(stub.hits(), 0, "canonical pin で GET /v1/models を呼んでいる");

        let session = apply_preflight(&conn, &plan, outcome).unwrap();
        assert_eq!(session.status, "open");
        assert_eq!(session.requested_model, PINNED);
        assert_eq!(stub.hits(), 0);

        // 確認していないことは書かない
        let card: Value = serde_json::from_str(&session.model_card_json).unwrap();
        assert_eq!(card["name"], PINNED);
        assert_eq!(card["identity_source"], "explicit_version_pin");
        assert_eq!(
            card.get("listed_in_models"),
            None,
            "一覧を見ていないのに掲載状況を書いている"
        );
        assert_eq!(card.get("description"), None, "description を捏造している");
        assert_eq!(card.get("release_date"), None, "release_date を捏造している");
    }

    /// 一覧が落ちていても canonical pin は通る（そもそも見に行かない）
    #[tokio::test]
    async fn a_canonical_pin_does_not_depend_on_the_models_endpoint() {
        let conn = open_db();
        let stub = start_stub(None).await; // 何を返しても関係ない（HTTP 503）
        let client = TypeSafeClient::with_key_for_test(KEY, &stub.base);

        let config = JevSessionConfig::new("eval-pin-down", "jev-1.13.0");
        let plan = plan_session(&conn, &config).unwrap();
        let outcome = preflight_model(&client, &plan).await;
        assert_eq!(
            outcome,
            PreflightOutcome::Available {
                requested_model: "jev-1.13.0".to_string(),
                identity: ModelIdentity::ExplicitVersionPin,
            }
        );
        assert_eq!(stub.hits(), 0, "一覧を見に行っている");
        assert!(apply_preflight(&conn, &plan, outcome).is_ok());
    }

    /// 構文上 canonical なら、実在しない版でも session は作れる。
    /// 実際の可否は最初の SystemOne（応答 model の完全一致）で決まる
    #[tokio::test]
    async fn a_nonexistent_canonical_version_still_opens_a_session() {
        let conn = open_db();
        let stub = start_stub(Some(models_body())).await;
        let client = TypeSafeClient::with_key_for_test(KEY, &stub.base);

        let config = JevSessionConfig::new("eval-pin-ghost", "jev-9.9.9");
        let plan = plan_session(&conn, &config).unwrap();
        let outcome = preflight_model(&client, &plan).await;
        let session = apply_preflight(&conn, &plan, outcome).unwrap();
        assert_eq!(session.requested_model, "jev-9.9.9");
        assert_eq!(session.status, "open");
        assert_eq!(stub.hits(), 0);
    }

    /// 一覧に載っていれば従来どおり card をそのまま残す
    #[tokio::test]
    async fn a_listed_model_keeps_its_card() {
        let conn = open_db();
        let session = start_open_session(&conn, "eval-listed").await;
        let card: Value = serde_json::from_str(&session.model_card_json).unwrap();
        assert_eq!(card["name"], MODEL);
        assert_eq!(card["description"], "test");
        assert_eq!(card["release_date"], "2026-01-01");
        assert_eq!(card["identity_source"], "models_listing");
        assert_eq!(card["listed_in_models"], true);
    }

    /// canonical でない名前（alias）は従来どおり一覧で確認し、無ければ止める
    #[tokio::test]
    async fn an_unlisted_alias_is_still_rejected() {
        let conn = open_db();
        let body = json!({"models": [
            {"name": "jev-1.13.0", "description": "x", "release_date": "2026-09-10"}
        ]})
        .to_string();
        let stub = start_stub(Some(body)).await;
        let client = TypeSafeClient::with_key_for_test(KEY, &stub.base);

        for alias in ["jev-preview", "jev-latest"] {
            let config = JevSessionConfig::new(format!("eval-alias-{alias}"), alias);
            let plan = plan_session(&conn, &config).unwrap();
            let outcome = preflight_model(&client, &plan).await;
            assert!(
                matches!(outcome, PreflightOutcome::Unavailable { .. }),
                "{alias} が通ってしまう"
            );
            assert!(apply_preflight(&conn, &plan, outcome).is_err());
        }
        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM jev_eval_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sessions, 0);
    }

    // ─── 予算 ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn reservations_stop_at_max_calls() {
        let mut conn = open_db();
        let plan = plan_session(&conn, &{
            let mut config = config("eval-budget");
            config.max_calls = 3;
            config
        })
        .unwrap();
        let (client, _stub) = available_client().await;
        let outcome = preflight_model(&client, &plan).await;
        let session = apply_preflight(&conn, &plan, outcome).unwrap();

        for expected in 1..=3 {
            let reservation = reserve_call_budget(&mut conn, session.id).unwrap();
            assert_eq!(reservation.calls_reserved, expected);
        }
        assert_eq!(load_session(&conn, session.id).unwrap().status, "open");

        // 上限に達した次は失敗し、exhausted になる
        assert_eq!(
            reserve_call_budget(&mut conn, session.id).unwrap_err(),
            JevSessionError::BudgetExhausted
        );
        let after = load_session(&conn, session.id).unwrap();
        assert_eq!(after.status, "exhausted");
        assert_eq!(after.calls_reserved, 3, "上限を超えて予約していない");

        // exhausted 後も予約は失敗のまま
        assert_eq!(
            reserve_call_budget(&mut conn, session.id).unwrap_err(),
            JevSessionError::BudgetExhausted
        );
        assert_eq!(load_session(&conn, session.id).unwrap().calls_reserved, 3);
    }

    /// 予約したあとに送信が失敗しても戻さない（crash 時に安全側へ倒すため）
    #[tokio::test]
    async fn a_reservation_is_not_returned_after_failure() {
        let mut conn = open_db();
        let session = start_open_session(&conn, "eval-nogiveback").await;

        let reservation = reserve_call_budget(&mut conn, session.id).unwrap();
        assert_eq!(reservation.calls_reserved, 1);
        // ここで HTTP が失敗した、という想定。戻す API は用意しない
        assert_eq!(load_session(&conn, session.id).unwrap().calls_reserved, 1);
    }

    #[tokio::test]
    async fn blocked_and_closed_are_not_budget_exhaustion() {
        let mut conn = open_db();

        let blocked = start_open_session(&conn, "eval-b").await;
        conn.execute(
            "UPDATE jev_eval_sessions SET status='blocked', blocked_reason='model preflight failed: overloaded'
              WHERE id = ?1",
            rusqlite::params![blocked.id],
        )
        .unwrap();
        match reserve_call_budget(&mut conn, blocked.id).unwrap_err() {
            JevSessionError::Blocked { reason } => {
                assert!(reason.contains("model preflight failed"))
            }
            other => panic!("blocked を budget exhausted と混同している: {other}"),
        }

        let closed = start_open_session(&conn, "eval-c").await;
        close_session(&conn, closed.id).unwrap();
        assert_eq!(
            reserve_call_budget(&mut conn, closed.id).unwrap_err(),
            JevSessionError::SessionClosed
        );
    }

    #[tokio::test]
    async fn token_usage_accumulates_and_exhausts_the_session() {
        let mut conn = open_db();
        let plan = plan_session(&conn, &{
            let mut config = config("eval-tokens");
            config.max_input_tokens = 100;
            config
        })
        .unwrap();
        let (client, _stub) = available_client().await;
        let outcome = preflight_model(&client, &plan).await;
        let session = apply_preflight(&conn, &plan, outcome).unwrap();

        // 上限未満なら open のまま予約できる
        let usage = record_token_usage(&mut conn, session.id, 60, 5).unwrap();
        assert_eq!(usage.input_tokens_used, 60);
        assert_eq!(usage.output_tokens_used, 5);
        assert_eq!(usage.status, "open");
        assert!(reserve_call_budget(&mut conn, session.id).is_ok());

        // 上限を跨ぐ（1 call 分の overshoot は許容）
        let usage = record_token_usage(&mut conn, session.id, 60, 7).unwrap();
        assert_eq!(usage.input_tokens_used, 120, "overshoot を許す");
        assert_eq!(usage.output_tokens_used, 12, "output は積むだけ");
        assert_eq!(usage.status, "exhausted");

        // 以後は予約できない
        assert_eq!(
            reserve_call_budget(&mut conn, session.id).unwrap_err(),
            JevSessionError::BudgetExhausted
        );

        // exhausted でも、予約済み call の記帳は通る
        let usage = record_token_usage(&mut conn, session.id, 10, 1).unwrap();
        assert_eq!(usage.input_tokens_used, 130);
        assert_eq!(usage.status, "exhausted");
    }

    #[tokio::test]
    async fn token_counts_are_validated() {
        let mut conn = open_db();
        let session = start_open_session(&conn, "eval-token-guard").await;

        for (input, output) in [(-1, 0), (0, -1), (-5, -5)] {
            let error = record_token_usage(&mut conn, session.id, input, output).unwrap_err();
            assert!(
                matches!(error, JevSessionError::InvalidTokenCount(_)),
                "({input},{output}) が通ってしまった"
            );
        }

        // 桁あふれは何も書かずに失敗する
        conn.execute(
            "UPDATE jev_eval_sessions SET input_tokens_used = ?2 WHERE id = ?1",
            rusqlite::params![session.id, i64::MAX],
        )
        .unwrap();
        let error = record_token_usage(&mut conn, session.id, 1, 0).unwrap_err();
        assert!(matches!(error, JevSessionError::InvalidTokenCount(_)), "{error}");
        assert_eq!(
            load_session(&conn, session.id).unwrap().input_tokens_used,
            i64::MAX,
            "失敗時に値が動いている"
        );
    }

    /// 2 本の接続から同時に予約しても max_calls を超えない
    #[tokio::test]
    async fn concurrent_reservations_respect_the_limit() {
        let path = temp_db_path("reservation");

        let session_id = {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
                .unwrap();
            db::apply_migrations(&conn).unwrap();

            let mut config = config("eval-concurrent");
            config.max_calls = 5;
            let plan = plan_session(&conn, &config).unwrap();
            let (client, _stub) = available_client().await;
            let outcome = preflight_model(&client, &plan).await;
            apply_preflight(&conn, &plan, outcome).unwrap().id
        };

        let granted = Arc::new(Mutex::new(0usize));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let path = path.clone();
            let granted = granted.clone();
            handles.push(std::thread::spawn(move || {
                let mut conn = Connection::open(&path).unwrap();
                conn.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
                conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
                for _ in 0..5 {
                    if reserve_call_budget(&mut conn, session_id).is_ok() {
                        *granted.lock().unwrap() += 1;
                    }
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let conn = Connection::open(&path).unwrap();
        let session = load_session(&conn, session_id).unwrap();
        assert_eq!(session.calls_reserved, 5, "上限を超えて予約された");
        assert_eq!(*granted.lock().unwrap(), 5, "成功した予約数が上限と一致しない");
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    // ─── scope guard ─────────────────────────────────────────────────────

    /// C4a はまだ call 行も works も書かない
    #[tokio::test]
    async fn c4a_writes_nothing_outside_its_allowlist() {
        let mut conn = open_db();
        let run_id = insert_run(&conn);
        let works_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM works", [], |row| row.get(0))
            .unwrap();

        let session = start_open_session(&conn, "eval-scope").await;
        let _ = reserve_call_budget(&mut conn, session.id).unwrap();
        let _ = record_token_usage(&mut conn, session.id, 10, 2).unwrap();

        for table in [
            "metadata_match_jev_calls",
            "metadata_match_candidate_scores",
            "metadata_match_verdicts",
            "metadata_review_tasks",
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table} に書いている");
        }

        let works_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM works", [], |row| row.get(0))
            .unwrap();
        assert_eq!(works_after, works_before, "works を増やしている");

        let run_applied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata_match_runs WHERE id = ?1 AND applied = 1",
                rusqlite::params![run_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(run_applied, 0, "run を適用済みにしている");
    }
}
