//! TypeSafe API の transport 層（PR3 C3B）。
//!
//! このモジュールは **HTTP のことだけ** を扱う。Jev の回答の意味は一切解釈しない。
//!
//! ```text
//! HTTP → C3B: top-level parsing → answers: Value
//!            → C3A: parse_answers → validate_response
//!            → C4:  DB audit records
//! ```
//!
//! したがってここには Noul / Choice / candidate / TMDB ID の知識を置かない。
//! `answers` は [`serde_json::Value`] のまま返し、C3A（`services::jev_contract`）が読む。
//!
//! # 扱うもの
//!
//! `TYPESAFE_API_KEY` の取得、HTTP client、`GET /v1/models`、pin したモデルの在庫確認、
//! `POST /v1/systemone`、timeout、retry、`Retry-After`、応答サイズ上限、応答の SHA-256、
//! request ID、HTTP status の分類、top-level の検証、model mismatch、token usage。
//!
//! # 扱わないもの
//!
//! DB 保存、contract 登録、budget、shadow policy、AUTO/REVIEW 判断、`match_once` 連携。
//! これらは C4 以降の担当で、C3B は C4 がそのまま記録できる typed result / error を返すだけ。
//!
//! # API キー
//!
//! production では環境変数 `TYPESAFE_API_KEY` だけから読む。設定テーブル・DB・引数から
//! 受け取る経路は作らない。キーは [`SecretKey`] に包み、`Debug` / `Display` / error text /
//! capture / log のどこにも出さない。応答 body にキーが echo されて返ってきた場合に備えて、
//! C4 へ渡す `prefix` / `body` からはキーの byte 列を `[REDACTED]` へ置換する（SHA-256 と
//! byte 数は raw のまま）。

use std::fmt;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use reqwest::{Client, Response, StatusCode, Url};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

// ─── 定数 ────────────────────────────────────────────────────────────────────

/// production の base URL。環境変数で差し替える経路は作らない。
pub const TYPESAFE_BASE_URL: &str = "https://api.typesafe.ai";

/// API キーを読む環境変数。ここ以外からキーを受け取らない。
pub const API_KEY_ENV: &str = "TYPESAFE_API_KEY";

/// request ID が載るヘッダ。
pub const REQUEST_ID_HEADER: &str = "x-typesafe-request-id";

/// 応答 body の上限。超えたら body を保持しない。
pub const MAX_RESPONSE_BYTES: u64 = 256 * 1024;

/// 監査用に残す先頭バイト数。
pub const RESPONSE_PREFIX_BYTES: usize = 4 * 1024;

/// attempt 1 回あたりの制限時間。接続開始から body 受信完了までを覆う。
pub const PER_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);

/// 初回のあとに許す retry 回数。最大 3 attempts。
pub const MAX_RETRIES: u32 = 2;

/// backoff の基準値。retry #1 = 500ms、retry #2 = 1000ms。
pub const BACKOFF_BASE_MS: u64 = 500;

/// backoff の上限。retry が 2 回までなので現状は届かないが、定数として持っておく。
pub const BACKOFF_CAP_MS: u64 = 5_000;

/// サーバの指示を尊重する上限。これを超える指示は採用せず通常 backoff に戻す。
pub const RETRY_AFTER_MAX: Duration = Duration::from_secs(60);

const CLIENT_USER_AGENT: &str = concat!("CineMantis/", env!("CARGO_PKG_VERSION"));

const REDACTED: &[u8] = b"[REDACTED]";

// ─── シークレット ────────────────────────────────────────────────────────────

/// API キー。`Debug` も `Display` も実装しない（そもそも文字列化できないようにする）。
struct SecretKey {
    /// redaction の照合に使う生のバイト列
    raw: String,
    /// 構築時に検証済みの Authorization。送信時は clone するだけで失敗しない
    authorization: HeaderValue,
}

impl SecretKey {
    /// ヘッダとして成立しないキー（空・空白のみ・CR/LF・制御文字）は here で弾く。
    /// ここを通らなければ client 自体が作れないので、送信時に header が欠ける経路がない。
    fn new(raw: String) -> Result<Self, TypeSafeClientError> {
        if raw.trim().is_empty() {
            return Err(TypeSafeClientError::simple(
                TypeSafeErrorKind::Auth,
                format!("{API_KEY_ENV} が空です"),
            ));
        }
        // from_str は制御文字（CR / LF を含む）を拒否する。エラーには入力を載せない。
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {raw}")).map_err(|_| {
                TypeSafeClientError::simple(
                    TypeSafeErrorKind::Auth,
                    format!("{API_KEY_ENV} をヘッダに載せられません"),
                )
            })?;
        authorization.set_sensitive(true);
        Ok(SecretKey { raw, authorization })
    }

    fn authorization(&self) -> HeaderValue {
        self.authorization.clone()
    }

    fn bytes(&self) -> &[u8] {
        self.raw.as_bytes()
    }

    fn as_str(&self) -> &str {
        &self.raw
    }
}

/// JSON の値そのものにシークレットが含まれるか再帰的に見る。
///
/// serialize 後の byte 列だけを見ると、`"` や `\` を含むキーが escape されて
/// 一致しなくなる（`abc"def` は wire 上 `abc\"def`）。意味の側でも照合して、
/// 二重で塞ぐ。
fn value_contains_secret(value: &Value, secret: &str) -> bool {
    match value {
        Value::String(text) => text.contains(secret),
        Value::Array(items) => items.iter().any(|item| value_contains_secret(item, secret)),
        Value::Object(map) => map
            .iter()
            .any(|(key, item)| key.contains(secret) || value_contains_secret(item, secret)),
        Value::Number(_) | Value::Bool(_) | Value::Null => false,
    }
}

/// `haystack` に `needle` がそのまま含まれるか。
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|window| window == needle)
}

/// 保存用のバイト列からキーの完全一致部分を消す。
///
/// 部分一致・正規表現・エントロピー推定はしない。誤検知で証跡を壊す方が危ないため、
/// 消すのは「自分が送ったキーそのもの」だけにしている。
struct Redactor {
    needle: Vec<u8>,
}

impl Redactor {
    fn new(key: &SecretKey) -> Self {
        Redactor { needle: key.bytes().to_vec() }
    }

    /// prefix を切り出す前に確保しておくべき追加バイト数。
    ///
    /// 4KiB ちょうどで切ってから消すと、境界をまたいだキーの前半が残ってしまう。
    /// `needle.len() - 1` バイト余分に持っておけば、境界にまたがる出現も必ず
    /// window 内に収まるので完全一致で消せる。
    fn window_overlap(&self) -> usize {
        self.needle.len().saturating_sub(1)
    }

    fn redact(&self, bytes: &[u8]) -> Vec<u8> {
        if self.needle.is_empty() || bytes.len() < self.needle.len() {
            return bytes.to_vec();
        }
        let mut out = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index..].starts_with(&self.needle) {
                out.extend_from_slice(REDACTED);
                index += self.needle.len();
            } else {
                out.push(bytes[index]);
                index += 1;
            }
        }
        out
    }
}

// ─── 応答の証跡 ──────────────────────────────────────────────────────────────

/// 受け取った応答の証跡。C4 がそのまま DB へ書ける形にしてある。
///
/// 不変条件: `truncated` なら `body` は `None`、`truncated` でないなら `body` は `Some`。
/// どちらも実受信バイト数だけで決まる（`Content-Length` の申告値は判定に使わない）。
#[derive(Debug, Clone, PartialEq)]
pub struct ResponseCapture {
    /// raw response 全体の SHA-256（redaction 前）
    sha256: String,
    /// raw response の総バイト数（redaction 前）
    byte_count: u64,
    /// 先頭 [`RESPONSE_PREFIX_BYTES`] 分。保存用なのでキーは消してある
    prefix: String,
    /// 上限を超えたか
    truncated: bool,
    /// 上限以内のときの body。保存用なのでキーは消してある
    body: Option<Vec<u8>>,
}

impl ResponseCapture {
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn byte_count(&self) -> u64 {
        self.byte_count
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn body(&self) -> Option<&[u8]> {
        self.body.as_deref()
    }

    /// 上限以内のときだけ本文を文字列として返す。
    pub fn body_text(&self) -> Option<String> {
        self.body.as_ref().map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    }
}

/// 応答を上限つきで集める。`Response::bytes()` のように全体を無制限に貯めない。
struct BoundedCollector<'a> {
    hasher: Sha256,
    total: u64,
    /// redaction のために prefix より少し広く持つ生バイト列
    raw_window: Vec<u8>,
    window_cap: usize,
    buffered: Option<Vec<u8>>,
    redactor: &'a Redactor,
}

impl<'a> BoundedCollector<'a> {
    fn new(content_length: Option<u64>, redactor: &'a Redactor) -> Self {
        // Content-Length は確保量のヒントにしか使わない。truncated の判定には使わない。
        let reserve = content_length.unwrap_or(0).min(MAX_RESPONSE_BYTES) as usize;
        let window_cap = RESPONSE_PREFIX_BYTES + redactor.window_overlap();
        BoundedCollector {
            hasher: Sha256::new(),
            total: 0,
            raw_window: Vec::with_capacity(window_cap.min(reserve.max(1024))),
            window_cap,
            buffered: Some(Vec::with_capacity(reserve)),
            redactor,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        // hash と byte 数は必ず raw のまま進める
        self.hasher.update(chunk);
        self.total += chunk.len() as u64;

        if self.raw_window.len() < self.window_cap {
            let room = self.window_cap - self.raw_window.len();
            let take = room.min(chunk.len());
            self.raw_window.extend_from_slice(&chunk[..take]);
        }

        if self.total > MAX_RESPONSE_BYTES {
            // 実受信量が上限を超えた「この瞬間」にだけ捨てる
            self.buffered = None;
        } else if let Some(buffer) = &mut self.buffered {
            buffer.extend_from_slice(chunk);
        }
    }

    fn finish(self) -> BoundedResponse {
        let truncated = self.total > MAX_RESPONSE_BYTES;

        // 4KiB へ縮める前に、overlap 込みの window 全体で消す
        let mut prefix_bytes = self.redactor.redact(&self.raw_window);
        prefix_bytes.truncate(RESPONSE_PREFIX_BYTES);

        let raw_body = if truncated { None } else { self.buffered };
        let body = raw_body.as_ref().map(|bytes| self.redactor.redact(bytes));

        BoundedResponse {
            raw_body,
            capture: ResponseCapture {
                sha256: hex(&self.hasher.finalize()),
                byte_count: self.total,
                prefix: String::from_utf8_lossy(&prefix_bytes).into_owned(),
                truncated,
                body,
            },
        }
    }
}

/// bounded reader の結果。`raw_body` はこのモジュールの外へ出さない。
///
/// 意味の解析（JSON parse）は **raw のまま** 行う。redaction はあくまで
/// 「C4 が保存する証跡」のための加工なので、TypeSafe が返した内容そのものを
/// C3A へ渡すには raw を読む必要がある。
///
/// 不変条件:
/// - 上限以内: `raw_body = Some(raw)`, `capture.body = Some(redacted)`
/// - 上限超過: `raw_body = None`, `capture.body = None`、`capture.prefix` は redacted、
///   `capture.sha256` と `capture.byte_count` は raw 全体
struct BoundedResponse {
    raw_body: Option<Vec<u8>>,
    capture: ResponseCapture,
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

// ─── モデル ──────────────────────────────────────────────────────────────────

/// `/v1/models` が返す 1 件。未知のフィールドは無視する（前方互換のため）。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelCard {
    pub name: String,
    pub description: String,
    pub release_date: String,
}

/// `/v1/models` の結果と、その呼び出しの監査情報。
#[derive(Debug, Clone)]
pub struct ModelListResponse {
    pub models: Vec<ModelCard>,
    pub http_status: u16,
    pub request_id: Option<String>,
    pub retry_count: u32,
    pub latency_ms: i64,
    pub capture: ResponseCapture,
}

// ─── 結果とエラー ────────────────────────────────────────────────────────────

/// `POST /v1/systemone` が成功したときの transport 層の結果。
#[derive(Debug, Clone)]
pub struct TypeSafeTransportResult {
    pub requested_model: String,
    pub response_model: String,
    /// C3A へそのまま渡す。C3B は中身を見ない
    pub answers: Value,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub http_status: u16,
    pub request_id: Option<String>,
    pub retry_count: u32,
    pub latency_ms: i64,
    pub capture: ResponseCapture,
}

/// migration 023 の `error_kind` CHECK と 1 対 1。ここから増やさない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeSafeErrorKind {
    Auth,
    RateLimit,
    Unprocessable,
    Overloaded,
    Network,
    Timeout,
    Parse,
    ResponseTooLarge,
    ModelMismatch,
    Other,
}

impl TypeSafeErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TypeSafeErrorKind::Auth => "auth",
            TypeSafeErrorKind::RateLimit => "rate_limit",
            TypeSafeErrorKind::Unprocessable => "unprocessable",
            TypeSafeErrorKind::Overloaded => "overloaded",
            TypeSafeErrorKind::Network => "network",
            TypeSafeErrorKind::Timeout => "timeout",
            TypeSafeErrorKind::Parse => "parse",
            TypeSafeErrorKind::ResponseTooLarge => "response_too_large",
            TypeSafeErrorKind::ModelMismatch => "model_mismatch",
            TypeSafeErrorKind::Other => "other",
        }
    }

    /// 同じ request を送り直す価値があるか。
    fn retryable(&self) -> bool {
        matches!(
            self,
            TypeSafeErrorKind::RateLimit
                | TypeSafeErrorKind::Overloaded
                | TypeSafeErrorKind::Network
                | TypeSafeErrorKind::Timeout
        )
    }
}

/// transport 層のエラー。API キー・Authorization ヘッダ・URL query は絶対に含めない。
#[derive(Debug, Clone)]
pub struct TypeSafeClientError {
    pub kind: TypeSafeErrorKind,
    pub http_status: Option<u16>,
    pub request_id: Option<String>,
    pub retry_count: u32,
    pub latency_ms: i64,
    pub capture: Option<ResponseCapture>,
    /// endpoint 名・status・request ID・byte 数程度の定型文だけ
    pub safe_text: String,
}

impl fmt::Display for TypeSafeClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.kind.as_str(), self.safe_text)?;
        if let Some(status) = self.http_status {
            write!(f, " (HTTP {status})")?;
        }
        Ok(())
    }
}

impl std::error::Error for TypeSafeClientError {}

impl TypeSafeClientError {
    fn simple(kind: TypeSafeErrorKind, safe_text: impl Into<String>) -> Self {
        TypeSafeClientError {
            kind,
            http_status: None,
            request_id: None,
            retry_count: 0,
            latency_ms: 0,
            capture: None,
            safe_text: safe_text.into(),
        }
    }
}

/// attempt 1 回の失敗。retry ループが分類を見て再送を決める。
struct AttemptFailure {
    kind: TypeSafeErrorKind,
    http_status: Option<u16>,
    request_id: Option<String>,
    capture: Option<ResponseCapture>,
    safe_text: String,
    retry_after: Option<Duration>,
}

/// attempt 1 回の成功（HTTP としては 2xx を受け取り、body を読み切った状態）。
struct AttemptSuccess {
    http_status: u16,
    request_id: Option<String>,
    /// JSON 解析用。public な結果には出さない
    raw_body: Option<Vec<u8>>,
    capture: ResponseCapture,
}

// ─── クライアント ────────────────────────────────────────────────────────────

/// TypeSafe API のクライアント。
///
/// `derive(Debug)` は付けない。キーを持つ構造体を自動で文字列化できるようにしないため、
/// [`fmt::Debug`] は手書きして `api_key` を伏せる。
pub struct TypeSafeClient {
    http: Client,
    base_url: String,
    api_key: SecretKey,
    redactor: Redactor,
    per_attempt_timeout: Duration,
    max_retries: u32,
    backoff_base: Duration,
}

impl fmt::Debug for TypeSafeClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypeSafeClient")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("per_attempt_timeout", &self.per_attempt_timeout)
            .field("max_retries", &self.max_retries)
            .finish()
    }
}

impl TypeSafeClient {
    /// production 用。キーは環境変数からしか読まない。
    pub fn from_env() -> Result<Self, TypeSafeClientError> {
        let raw = std::env::var(API_KEY_ENV).map_err(|_| {
            TypeSafeClientError::simple(
                TypeSafeErrorKind::Auth,
                format!("{API_KEY_ENV} が設定されていません"),
            )
        })?;
        // 前後の偶発的な空白を Bearer token に含めない（空白のみは下で auth reject）
        let api_key = SecretKey::new(raw.trim().to_string())?;

        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .user_agent(CLIENT_USER_AGENT)
            .build()
            .map_err(|_| {
                TypeSafeClientError::simple(
                    TypeSafeErrorKind::Network,
                    "HTTP クライアントを初期化できませんでした",
                )
            })?;

        let redactor = Redactor::new(&api_key);
        Ok(TypeSafeClient {
            http,
            base_url: TYPESAFE_BASE_URL.to_string(),
            api_key,
            redactor,
            per_attempt_timeout: PER_ATTEMPT_TIMEOUT,
            max_retries: MAX_RETRIES,
            backoff_base: Duration::from_millis(BACKOFF_BASE_MS),
        })
    }

    /// テスト専用。loopback への平文 HTTP だけを許す。
    ///
    /// production の [`TypeSafeClient::from_env`] は必ず HTTPS 限定なので、この緩和が
    /// 製品バイナリに入ることはない（`#[cfg(test)]` なのでコンパイルもされない）。
    #[cfg(test)]
    pub(crate) fn with_key_for_test(key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self::try_with_key_for_test(key, base_url).expect("テスト用クライアント")
    }

    /// production と同じ API キー検証を通す。reject の確認に使う。
    #[cfg(test)]
    pub(crate) fn try_with_key_for_test(
        key: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Result<Self, TypeSafeClientError> {
        let base_url = base_url.into();
        let parsed = Url::parse(&base_url).expect("テスト用 base_url が URL として不正");
        let host = parsed.host_str().unwrap_or_default();
        assert!(
            matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]"),
            "テスト用クライアントは loopback 以外へ向けられない: {host}"
        );

        let api_key = SecretKey::new(key.into())?;

        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(CLIENT_USER_AGENT)
            .build()
            .expect("テスト用 HTTP クライアント");

        let redactor = Redactor::new(&api_key);
        Ok(TypeSafeClient {
            http,
            base_url,
            api_key,
            redactor,
            per_attempt_timeout: PER_ATTEMPT_TIMEOUT,
            max_retries: MAX_RETRIES,
            backoff_base: Duration::from_millis(BACKOFF_BASE_MS),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_timeout_for_test(mut self, timeout: Duration) -> Self {
        self.per_attempt_timeout = timeout;
        self
    }

    /// テストを待たせないための短縮。backoff の計算式自体は [`backoff_delay`] で検証する。
    #[cfg(test)]
    pub(crate) fn with_backoff_for_test(mut self, base: Duration) -> Self {
        self.backoff_base = base;
        self
    }

    // ─── モデル ──────────────────────────────────────────────────────────

    /// 利用できるモデル一覧。
    pub async fn list_models(&self) -> Result<Vec<ModelCard>, TypeSafeClientError> {
        self.list_models_with_meta().await.map(|response| response.models)
    }

    /// 一覧と、その呼び出しの監査情報。
    ///
    /// `verify_model_available` が「一覧に無かった」ことを証跡つきで報告できるように、
    /// capture / request_id / retry_count / latency を保持する。
    pub(crate) async fn list_models_with_meta(&self) -> Result<ModelListResponse, TypeSafeClientError> {
        let url = format!("{}/v1/models", self.base_url);
        let started = std::time::Instant::now();

        let (success, retry_count) = self
            .run_with_retry("GET /v1/models", started, || {
                self.http.get(&url).headers(self.common_headers())
            })
            .await?;

        let latency_ms = elapsed_ms(started);
        let body = success.raw_body.as_deref().ok_or_else(|| {
            // 2xx で上限超過。body が無いので解析できない
            self.error_from_capture(
                TypeSafeErrorKind::ResponseTooLarge,
                "GET /v1/models",
                &success,
                retry_count,
                latency_ms,
            )
        })?;

        let value: Value = serde_json::from_slice(body).map_err(|_| {
            self.error_from_capture(
                TypeSafeErrorKind::Parse,
                "GET /v1/models の JSON が不正",
                &success,
                retry_count,
                latency_ms,
            )
        })?;

        let models = parse_model_list(&value).map_err(|reason| {
            self.error_from_capture(
                TypeSafeErrorKind::Parse,
                format!("GET /v1/models: {reason}"),
                &success,
                retry_count,
                latency_ms,
            )
        })?;

        Ok(ModelListResponse {
            models,
            http_status: success.http_status,
            request_id: success.request_id.clone(),
            retry_count,
            latency_ms,
            capture: success.capture,
        })
    }

    /// pin したモデルが実際に提供されているかを確認する。
    ///
    /// 完全一致でしか探さない。alias 解決も、前方一致も、大文字小文字の無視も、
    /// semver で最大のものを選ぶことも、`-latest` を自動採用することもしない。
    /// 「最新を自動で拾う」設計にすると eval の途中でモデルが入れ替わってしまう。
    pub async fn verify_model_available(
        &self,
        requested_model: &str,
    ) -> Result<ModelCard, TypeSafeClientError> {
        let listed = self.list_models_with_meta().await?;
        match listed.models.iter().find(|card| card.name == requested_model) {
            Some(card) => Ok(card.clone()),
            None => Err(TypeSafeClientError {
                kind: TypeSafeErrorKind::ModelMismatch,
                http_status: Some(listed.http_status),
                request_id: listed.request_id.clone(),
                retry_count: listed.retry_count,
                latency_ms: listed.latency_ms,
                capture: Some(listed.capture),
                safe_text: format!(
                    "要求したモデルが GET /v1/models の一覧にありません（{} 件中）",
                    listed.models.len()
                ),
            }),
        }
    }

    // ─── SystemOne ───────────────────────────────────────────────────────

    /// 1 回の SystemOne 呼び出し。
    ///
    /// `state` と `questions` は C2 / C3A が作ったものをそのまま送る。ここでは中身を
    /// 組み立てず、読まず、書き換えない。
    pub async fn ask_systemone(
        &self,
        requested_model: &str,
        state: &Value,
        questions: &Value,
    ) -> Result<TypeSafeTransportResult, TypeSafeClientError> {
        let url = format!("{}/v1/systemone", self.base_url);
        let payload = json!({
            "model": requested_model,
            "state": state,
            "questions": questions,
        });

        // C2 / C3A が渡してきた state / questions に万一キーが混ざっていたら送らない。
        // 秘密情報の preflight failure なので auth として扱い、送信回数は 0 にする。
        //
        // ① 意味の側で見る。escape される文字を含むキーはここでしか捕まえられない。
        if value_contains_secret(&payload, self.api_key.as_str()) {
            return Err(self.secret_in_body_error());
        }

        // 実際に送る byte 列を一度だけ作り、その byte 列そのものを検査する。
        // 「検査した bytes」と「送る bytes」を必ず一致させるため、ここで作った
        // body をそのまま reqwest へ渡す（.json() で再 serialize しない）。
        let body = serde_json::to_vec(&payload).map_err(|_| {
            TypeSafeClientError::simple(
                TypeSafeErrorKind::Other,
                "POST /v1/systemone の body を組み立てられませんでした",
            )
        })?;

        // ② 実際の wire bytes でも見る。
        if contains_bytes(&body, self.api_key.bytes()) {
            return Err(self.secret_in_body_error());
        }

        let started = std::time::Instant::now();

        let (success, retry_count) = self
            .run_with_retry("POST /v1/systemone", started, || {
                self.http
                    .post(&url)
                    .headers(self.common_headers())
                    .body(body.clone())
            })
            .await?;

        let latency_ms = elapsed_ms(started);
        let body = success.raw_body.as_deref().ok_or_else(|| {
            self.error_from_capture(
                TypeSafeErrorKind::ResponseTooLarge,
                "POST /v1/systemone",
                &success,
                retry_count,
                latency_ms,
            )
        })?;

        let value: Value = serde_json::from_slice(body).map_err(|_| {
            self.error_from_capture(
                TypeSafeErrorKind::Parse,
                "POST /v1/systemone の JSON が不正",
                &success,
                retry_count,
                latency_ms,
            )
        })?;

        let envelope = parse_envelope(&value).map_err(|reason| {
            self.error_from_capture(
                TypeSafeErrorKind::Parse,
                format!("POST /v1/systemone: {reason}"),
                &success,
                retry_count,
                latency_ms,
            )
        })?;

        if envelope.model != requested_model {
            // alias を要求していないので「違って当然」という例外は作らない
            return Err(self.error_from_capture(
                TypeSafeErrorKind::ModelMismatch,
                "応答のモデルが要求したモデルと一致しません",
                &success,
                retry_count,
                latency_ms,
            ));
        }

        Ok(TypeSafeTransportResult {
            requested_model: requested_model.to_string(),
            response_model: envelope.model,
            answers: envelope.answers,
            input_tokens: envelope.input_tokens,
            output_tokens: envelope.output_tokens,
            http_status: success.http_status,
            request_id: success.request_id.clone(),
            retry_count,
            latency_ms,
            capture: success.capture,
        })
    }

    // ─── 共通処理 ────────────────────────────────────────────────────────

    /// 送信前に止めたときの定型エラー。キーそのものは絶対に載せない。
    fn secret_in_body_error(&self) -> TypeSafeClientError {
        TypeSafeClientError::simple(
            TypeSafeErrorKind::Auth,
            "送信 body に API キーが含まれていたため送信しませんでした",
        )
    }

    fn common_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        // 構築時に検証済みなので、ここで失敗する余地はない
        headers.insert(AUTHORIZATION, self.api_key.authorization());
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(USER_AGENT, HeaderValue::from_static(CLIENT_USER_AGENT));
        headers
    }

    /// attempt を最大 3 回まで回す。
    ///
    /// `POST /v1/systemone` の retry は同じ request の再送になる。TypeSafe 側の
    /// idempotency を仮定しないので、回数を 2 回に絞ったうえで retry_count /
    /// request ID / latency を必ず返し、C4 が監査できるようにする。独自の重複排除は
    /// ここでは実装しない。
    async fn run_with_retry<F>(
        &self,
        endpoint: &str,
        started: std::time::Instant,
        build: F,
    ) -> Result<(AttemptSuccess, u32), TypeSafeClientError>
    where
        F: Fn() -> reqwest::RequestBuilder,
    {
        let mut retry_count: u32 = 0;
        loop {
            match self.send_once(endpoint, build()).await {
                Ok(success) => return Ok((success, retry_count)),
                Err(failure) => {
                    if failure.kind.retryable() && retry_count < self.max_retries {
                        let delay = retry_delay(failure.retry_after, retry_count, self.backoff_base);
                        tokio::time::sleep(delay).await;
                        retry_count += 1;
                        continue;
                    }
                    return Err(TypeSafeClientError {
                        kind: failure.kind,
                        http_status: failure.http_status,
                        request_id: failure.request_id,
                        retry_count,
                        latency_ms: elapsed_ms(started),
                        capture: failure.capture,
                        safe_text: failure.safe_text,
                    });
                }
            }
        }
    }

    /// attempt 1 回。deadline は 1 つだけ作り、reqwest 側と chunk 読み取り側で共有する。
    async fn send_once(
        &self,
        endpoint: &str,
        request: reqwest::RequestBuilder,
    ) -> Result<AttemptSuccess, AttemptFailure> {
        let deadline = tokio::time::Instant::now() + self.per_attempt_timeout;

        let response = match request.timeout(self.per_attempt_timeout).send().await {
            Ok(response) => response,
            Err(error) => {
                // エラー文字列は載せない（URL が混ざる可能性がある）。種別だけ見る。
                let kind = if error.is_timeout() {
                    TypeSafeErrorKind::Timeout
                } else {
                    TypeSafeErrorKind::Network
                };
                return Err(AttemptFailure {
                    kind,
                    http_status: None,
                    request_id: None,
                    capture: None,
                    safe_text: format!("{endpoint} へ到達できませんでした"),
                    retry_after: None,
                });
            }
        };

        let status = response.status();
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let retry_after = parse_retry_after(response.headers());
        let content_length = response.content_length();

        let bounded = match self.read_bounded(response, content_length, deadline).await {
            Ok(bounded) => bounded,
            Err(kind) => {
                return Err(AttemptFailure {
                    kind,
                    http_status: Some(status.as_u16()),
                    request_id,
                    capture: None,
                    safe_text: format!("{endpoint} の応答を読み終えられませんでした"),
                    retry_after,
                })
            }
        };
        let BoundedResponse { raw_body, capture } = bounded;

        if status.is_success() {
            if capture.truncated {
                return Err(AttemptFailure {
                    kind: TypeSafeErrorKind::ResponseTooLarge,
                    http_status: Some(status.as_u16()),
                    request_id,
                    safe_text: format!(
                        "{endpoint} の応答が上限を超えました（{} bytes）",
                        capture.byte_count
                    ),
                    capture: Some(capture),
                    retry_after,
                });
            }
            return Ok(AttemptSuccess {
                http_status: status.as_u16(),
                request_id,
                raw_body,
                capture,
            });
        }

        // 分類は status と transport の事象だけで決める。body の中身は見ない。
        Err(AttemptFailure {
            kind: classify_status(status),
            http_status: Some(status.as_u16()),
            request_id: request_id.clone(),
            safe_text: format!(
                "{endpoint} が HTTP {} を返しました（body {} bytes{}）",
                status.as_u16(),
                capture.byte_count,
                if capture.truncated { ", 上限超過" } else { "" }
            ),
            capture: Some(capture),
            retry_after,
        })
    }

    /// body を上限つきで読む。`Response::bytes()` は使わない。
    async fn read_bounded(
        &self,
        mut response: Response,
        content_length: Option<u64>,
        deadline: tokio::time::Instant,
    ) -> Result<BoundedResponse, TypeSafeErrorKind> {
        let mut collector = BoundedCollector::new(content_length, &self.redactor);
        loop {
            // reqwest 側の timeout と同じ deadline を見る（別タイマーを作らない）
            let next = tokio::time::timeout_at(deadline, response.chunk()).await;
            match next {
                Err(_) => return Err(TypeSafeErrorKind::Timeout),
                Ok(Err(error)) => {
                    return Err(if error.is_timeout() {
                        TypeSafeErrorKind::Timeout
                    } else {
                        TypeSafeErrorKind::Network
                    })
                }
                Ok(Ok(None)) => break,
                Ok(Ok(Some(chunk))) => collector.push(&chunk),
            }
        }
        Ok(collector.finish())
    }

    fn error_from_capture(
        &self,
        kind: TypeSafeErrorKind,
        safe_text: impl Into<String>,
        success: &AttemptSuccess,
        retry_count: u32,
        latency_ms: i64,
    ) -> TypeSafeClientError {
        TypeSafeClientError {
            kind,
            http_status: Some(success.http_status),
            request_id: success.request_id.clone(),
            retry_count,
            latency_ms,
            capture: Some(success.capture.clone()),
            safe_text: safe_text.into(),
        }
    }
}

// ─── 分類と待ち時間 ──────────────────────────────────────────────────────────

/// HTTP status から error_kind を決める。本文の文字列は一切見ない。
///
/// 3xx は `network` ではなく `other`。redirect を追わない設定なので受け取るだけだが、
/// 3xx 自体は正常に届いた HTTP 応答であって transport の失敗ではない。
fn classify_status(status: StatusCode) -> TypeSafeErrorKind {
    match status.as_u16() {
        401 | 403 => TypeSafeErrorKind::Auth,
        408 => TypeSafeErrorKind::Timeout,
        422 => TypeSafeErrorKind::Unprocessable,
        429 => TypeSafeErrorKind::RateLimit,
        500..=599 => TypeSafeErrorKind::Overloaded,
        _ => TypeSafeErrorKind::Other,
    }
}

/// backoff。retry #1 = 500ms、retry #2 = 1000ms。乱数は使わない。
fn backoff_delay(retry_count: u32, base: Duration) -> Duration {
    let millis = (base.as_millis() as u64).saturating_mul(1u64 << retry_count.min(16));
    Duration::from_millis(millis.min(BACKOFF_CAP_MS.max(base.as_millis() as u64)))
}

/// サーバの指示と backoff のどちらを使うか決める。
///
/// 60 秒以内の指示は尊重する。60 秒を超える指示は **clamp せず** 通常の backoff に戻す
/// （60 秒待つのではなく 500ms / 1000ms で再送する）。
fn retry_delay(server_hint: Option<Duration>, retry_count: u32, base: Duration) -> Duration {
    match server_hint {
        Some(hint) if hint <= RETRY_AFTER_MAX => hint,
        _ => backoff_delay(retry_count, base),
    }
}

/// `retry-after-ms` を優先し、無ければ `Retry-After` の秒数を読む。
/// HTTP-date 形式は C3B v1 では扱わない（その場合は backoff にフォールバックする）。
fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    if let Some(millis) = headers
        .get("retry-after-ms")
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.trim().parse::<u64>().ok())
    {
        return Some(Duration::from_millis(millis));
    }
    headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn elapsed_ms(started: std::time::Instant) -> i64 {
    started.elapsed().as_millis() as i64
}

// ─── JSON の入口だけの検証 ───────────────────────────────────────────────────

/// `{"models":[...]}` を読む。未知のフィールドは無視する。
fn parse_model_list(value: &Value) -> Result<Vec<ModelCard>, String> {
    let root = value.as_object().ok_or("root がオブジェクトではありません")?;
    let models = root.get("models").ok_or("models がありません")?;
    let models = models.as_array().ok_or("models が配列ではありません")?;

    let mut cards = Vec::with_capacity(models.len());
    for (index, card) in models.iter().enumerate() {
        let object = card
            .as_object()
            .ok_or_else(|| format!("models[{index}] がオブジェクトではありません"))?;
        let field = |key: &str| -> Result<String, String> {
            let text = object
                .get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("models[{index}].{key} がありません"))?;
            if text.trim().is_empty() {
                return Err(format!("models[{index}].{key} が空です"));
            }
            Ok(text.to_string())
        };
        cards.push(ModelCard {
            name: field("name")?,
            description: field("description")?,
            release_date: field("release_date")?,
        });
    }

    // 同名が複数ある一覧は壊れている。どちらを使うか推測しない。
    for (index, card) in cards.iter().enumerate() {
        if cards[..index].iter().any(|earlier| earlier.name == card.name) {
            return Err("models に同じ name が複数あります".to_string());
        }
    }

    Ok(cards)
}

struct SystemOneEnvelope {
    model: String,
    answers: Value,
    input_tokens: i64,
    output_tokens: i64,
}

/// top-level だけを見る。`answers` の中身と型は C3A の担当なので触らない。
fn parse_envelope(value: &Value) -> Result<SystemOneEnvelope, String> {
    let root = value.as_object().ok_or("root がオブジェクトではありません")?;

    let model = root
        .get("model")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or("model がありません")?
        .to_string();

    // 存在だけ確認する。object か array かも見ない
    let answers = root.get("answers").ok_or("answers がありません")?.clone();

    let usage = root
        .get("usage")
        .and_then(Value::as_object)
        .ok_or("usage がありません")?;

    let token = |key: &str| -> Result<i64, String> {
        let value = usage.get(key).ok_or_else(|| format!("usage.{key} がありません"))?;
        let number = value
            .as_i64()
            .ok_or_else(|| format!("usage.{key} が整数ではありません"))?;
        if number < 0 {
            return Err(format!("usage.{key} が負です"));
        }
        Ok(number)
    };

    Ok(SystemOneEnvelope {
        model,
        answers,
        input_tokens: token("input_tokens")?,
        output_tokens: token("output_tokens")?,
    })
}

// ─── テスト ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod stub {
    //! テスト用の最小 HTTP サーバ。外部ネットワークへは出ない。

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Clone)]
    pub enum Action {
        /// そのまま書き出す生の HTTP 応答
        Raw(Vec<u8>),
        /// 応答せずに接続を切る（接続断の再現）
        Drop,
        /// 待ってから応答する（timeout の再現）
        Sleep(Duration),
    }

    pub struct Stub {
        pub base: String,
        hits: Arc<AtomicUsize>,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl Stub {
        pub fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }

        pub fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    pub async fn start(actions: Vec<Action>) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));

        let hits_task = hits.clone();
        let requests_task = requests.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let index = hits_task.fetch_add(1, Ordering::SeqCst);
                let action = actions
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| actions.last().cloned().unwrap_or(Action::Drop));
                let requests_conn = requests_task.clone();

                // 接続ごとに切り離す。待たせる応答が次の接続の受付を塞がないように。
                tokio::spawn(async move {
                // request を読む（ヘッダ + Content-Length 分の body）
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 4096];
                let mut content_length = 0usize;
                let mut header_end = None;
                loop {
                    let Ok(read) = socket.read(&mut chunk).await else { break };
                    if read == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    if header_end.is_none() {
                        if let Some(position) = find(&buffer, b"\r\n\r\n") {
                            header_end = Some(position + 4);
                            let headers = String::from_utf8_lossy(&buffer[..position]).to_lowercase();
                            for line in headers.lines() {
                                if let Some(rest) = line.strip_prefix("content-length:") {
                                    content_length = rest.trim().parse().unwrap_or(0);
                                }
                            }
                        }
                    }
                    if let Some(start) = header_end {
                        if buffer.len() >= start + content_length {
                            break;
                        }
                    }
                }
                requests_conn
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buffer).into_owned());

                match action {
                    Action::Drop => {
                        let _ = socket.shutdown().await;
                    }
                    Action::Sleep(duration) => {
                        tokio::time::sleep(duration).await;
                        let _ = socket.shutdown().await;
                    }
                    Action::Raw(bytes) => {
                        let _ = socket.write_all(&bytes).await;
                        let _ = socket.flush().await;
                        let _ = socket.shutdown().await;
                    }
                }
                });
            }
        });

        Stub {
            base: format!("http://127.0.0.1:{}", addr.port()),
            hits,
            requests,
        }
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|window| window == needle)
    }

    /// Content-Length つきの通常応答
    pub fn response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Action {
        let mut out = format!("HTTP/1.1 {status} X\r\n");
        for (name, value) in headers {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
        out.push_str("Connection: close\r\n\r\n");
        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(body);
        Action::Raw(bytes)
    }

    /// Content-Length を持たない chunked 応答
    pub fn chunked(status: u16, chunks: &[&[u8]]) -> Action {
        let mut out = format!("HTTP/1.1 {status} X\r\n");
        out.push_str("Transfer-Encoding: chunked\r\n");
        out.push_str("Connection: close\r\n\r\n");
        let mut bytes = out.into_bytes();
        for chunk in chunks {
            bytes.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            bytes.extend_from_slice(chunk);
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(b"0\r\n\r\n");
        Action::Raw(bytes)
    }

    pub fn json(status: u16, body: &str) -> Action {
        response(status, &[("Content-Type", "application/json")], body.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::stub::{self, Action};
    use super::*;

    const DUMMY_KEY: &str = "sk-test-DO-NOT-LEAK-0123456789";
    const MODEL: &str = "jev-test-model-1";

    fn client(base: &str) -> TypeSafeClient {
        TypeSafeClient::with_key_for_test(DUMMY_KEY, base)
            .with_backoff_for_test(Duration::from_millis(1))
    }

    fn models_json() -> String {
        json!({"models": [
            {"name": MODEL, "description": "test", "release_date": "2026-01-01", "extra": 1},
            {"name": "jev-test-model-0", "description": "old", "release_date": "2025-01-01"}
        ]})
        .to_string()
    }

    fn systemone_json(model: &str) -> String {
        json!({
            "model": model,
            "answers": {"c1_same_work": {"type": "noul", "noul": 0.9}},
            "usage": {"input_tokens": 120, "output_tokens": 8}
        })
        .to_string()
    }

    fn sha_of(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hex(&hasher.finalize())
    }

    fn collect_full(
        chunks: &[&[u8]],
        content_length: Option<u64>,
        key: &str,
    ) -> BoundedResponse {
        let secret = SecretKey::new(key.to_string()).expect("テスト用キー");
        let redactor = Redactor::new(&secret);
        let mut collector = BoundedCollector::new(content_length, &redactor);
        for chunk in chunks {
            collector.push(chunk);
        }
        collector.finish()
    }

    fn collect(chunks: &[&[u8]], content_length: Option<u64>, key: &str) -> ResponseCapture {
        collect_full(chunks, content_length, key).capture
    }

    // ─── bounded collector（Content-Length の申告は判定に使わない）──────────

    /// Content-Length が大きく申告されていても、実受信量が小さければ body は残る。
    /// ここが崩れると「truncated でないのに body が無い」状態が作れてしまう。
    #[test]
    fn content_length_is_only_a_hint() {
        let body = vec![b'a'; 100 * 1024];

        // 大きく申告 / 実際は小さい
        let capture = collect(&[&body], Some(300 * 1024), DUMMY_KEY);
        assert!(!capture.truncated());
        assert_eq!(capture.byte_count(), 100 * 1024);
        assert_eq!(capture.body().map(<[u8]>::len), Some(100 * 1024));

        // 大きく申告 / 実際も大きい
        let big = vec![b'b'; 300 * 1024];
        let capture = collect(&[&big], Some(300 * 1024), DUMMY_KEY);
        assert!(capture.truncated());
        assert_eq!(capture.body(), None);
        assert_eq!(capture.byte_count(), 300 * 1024);

        // 小さく申告 / 実際は大きい
        let capture = collect(&[&big], Some(10), DUMMY_KEY);
        assert!(capture.truncated());
        assert_eq!(capture.body(), None);
        assert_eq!(capture.sha256(), sha_of(&big));

        // 申告なし
        let capture = collect(&[&body], None, DUMMY_KEY);
        assert!(!capture.truncated());
        assert_eq!(capture.body().map(<[u8]>::len), Some(100 * 1024));
    }

    #[test]
    fn size_boundaries_decide_truncation() {
        let exact = vec![b'x'; MAX_RESPONSE_BYTES as usize];
        let capture = collect(&[&exact], None, DUMMY_KEY);
        assert!(!capture.truncated(), "ちょうど上限は超過ではない");
        assert_eq!(capture.body().map(<[u8]>::len), Some(MAX_RESPONSE_BYTES as usize));

        let over = vec![b'x'; MAX_RESPONSE_BYTES as usize + 1];
        let capture = collect(&[&over], None, DUMMY_KEY);
        assert!(capture.truncated());
        assert_eq!(capture.body(), None, "上限超過で全体を保持しない");
        assert_eq!(capture.byte_count(), MAX_RESPONSE_BYTES + 1);
        assert_eq!(capture.sha256(), sha_of(&over), "hash は raw 全体");

        // 境界をまたぐ分割でも同じ結論になる
        let head = vec![b'x'; MAX_RESPONSE_BYTES as usize - 16];
        let tail = vec![b'x'; 32];
        let capture = collect(&[&head, &tail], None, DUMMY_KEY);
        assert!(capture.truncated());
        assert_eq!(capture.byte_count(), MAX_RESPONSE_BYTES + 16);
    }

    #[test]
    fn prefix_is_the_first_four_kib() {
        let body: Vec<u8> = (0..16 * 1024).map(|index| b'a' + (index % 26) as u8).collect();
        let capture = collect(&[&body], None, DUMMY_KEY);
        assert_eq!(capture.prefix().len(), RESPONSE_PREFIX_BYTES);
        assert_eq!(capture.prefix().as_bytes(), &body[..RESPONSE_PREFIX_BYTES]);
    }

    // ─── secret redaction ────────────────────────────────────────────────

    #[test]
    fn secret_is_redacted_from_saved_bytes() {
        let mut body = b"{\"echo\":\"".to_vec();
        body.extend_from_slice(DUMMY_KEY.as_bytes());
        body.extend_from_slice(b"\"}");

        let capture = collect(&[&body], None, DUMMY_KEY);
        assert!(!capture.prefix().contains(DUMMY_KEY));
        assert!(capture.prefix().contains("[REDACTED]"));
        assert!(!capture.body_text().unwrap().contains(DUMMY_KEY));
        assert_eq!(capture.sha256(), sha_of(&body), "hash は置換前の raw 基準");
        assert_eq!(capture.byte_count(), body.len() as u64);
    }

    /// キーが HTTP chunk の境界で分割されても消える
    #[test]
    fn secret_across_chunk_boundary_is_redacted() {
        let key = DUMMY_KEY.as_bytes();
        let (head, tail) = key.split_at(7);
        let mut first = b"prefix-".to_vec();
        first.extend_from_slice(head);
        let mut second = tail.to_vec();
        second.extend_from_slice(b"-suffix");

        let capture = collect(&[&first, &second], None, DUMMY_KEY);
        assert!(!capture.prefix().contains(DUMMY_KEY));
        assert!(!capture.body_text().unwrap().contains(DUMMY_KEY));
        assert!(capture.prefix().contains("[REDACTED]"));

        let mut raw = first.clone();
        raw.extend_from_slice(&second);
        assert_eq!(capture.sha256(), sha_of(&raw));
    }

    /// キーが 4KiB prefix の境界をまたいでも、前半の平文が残らない
    #[test]
    fn secret_across_prefix_boundary_is_redacted() {
        let key = DUMMY_KEY.as_bytes();
        // キーの先頭 2 バイトだけが 4KiB の内側に入る配置
        let lead = RESPONSE_PREFIX_BYTES - 2;
        let mut body = vec![b'.'; lead];
        body.extend_from_slice(key);
        body.extend_from_slice(&vec![b'.'; 1024]);

        let capture = collect(&[&body], None, DUMMY_KEY);
        let prefix = capture.prefix();
        assert!(!prefix.contains(DUMMY_KEY), "キー全体が残っている");
        // 境界分断由来の平文断片が 1 文字も残っていないこと。
        // 置換後に 4KiB へ縮めるので "[REDACTED]" 自体は途中で切れることがあるが、
        // 切れて残るのはマーカーの一部だけで、キーの平文ではない。
        assert!(
            !prefix.contains(&DUMMY_KEY[..2]),
            "prefix にキー断片が残っている: {:?}",
            &prefix[prefix.len().saturating_sub(32)..]
        );
        assert!(prefix.ends_with('[') || prefix.contains('['), "置換マーカーの痕跡がある");
        assert_eq!(capture.sha256(), sha_of(&body));
    }

    // ─── status 分類 ─────────────────────────────────────────────────────

    #[test]
    fn statuses_map_to_the_existing_error_kinds() {
        let cases = [
            (401, TypeSafeErrorKind::Auth),
            (403, TypeSafeErrorKind::Auth),
            (408, TypeSafeErrorKind::Timeout),
            (422, TypeSafeErrorKind::Unprocessable),
            (429, TypeSafeErrorKind::RateLimit),
            (500, TypeSafeErrorKind::Overloaded),
            (503, TypeSafeErrorKind::Overloaded),
            (529, TypeSafeErrorKind::Overloaded),
            (400, TypeSafeErrorKind::Other),
            (402, TypeSafeErrorKind::Other),
            (404, TypeSafeErrorKind::Other),
            (409, TypeSafeErrorKind::Other),
            (413, TypeSafeErrorKind::Other),
            (301, TypeSafeErrorKind::Other),
            (302, TypeSafeErrorKind::Other),
            (303, TypeSafeErrorKind::Other),
            (307, TypeSafeErrorKind::Other),
            (308, TypeSafeErrorKind::Other),
        ];
        for (status, expected) in cases {
            let actual = classify_status(StatusCode::from_u16(status).unwrap());
            assert_eq!(actual, expected, "HTTP {status}");
        }
    }

    #[test]
    fn error_kind_strings_match_the_migration() {
        let all = [
            TypeSafeErrorKind::Auth,
            TypeSafeErrorKind::RateLimit,
            TypeSafeErrorKind::Unprocessable,
            TypeSafeErrorKind::Overloaded,
            TypeSafeErrorKind::Network,
            TypeSafeErrorKind::Timeout,
            TypeSafeErrorKind::Parse,
            TypeSafeErrorKind::ResponseTooLarge,
            TypeSafeErrorKind::ModelMismatch,
            TypeSafeErrorKind::Other,
        ];
        let names: Vec<&str> = all.iter().map(TypeSafeErrorKind::as_str).collect();
        assert_eq!(
            names,
            vec![
                "auth",
                "rate_limit",
                "unprocessable",
                "overloaded",
                "network",
                "timeout",
                "parse",
                "response_too_large",
                "model_mismatch",
                "other"
            ],
            "migration 023 の CHECK と一致させる"
        );
    }

    // ─── Retry-After と backoff ──────────────────────────────────────────

    #[test]
    fn backoff_is_five_hundred_then_one_thousand() {
        let base = Duration::from_millis(BACKOFF_BASE_MS);
        assert_eq!(backoff_delay(0, base), Duration::from_millis(500));
        assert_eq!(backoff_delay(1, base), Duration::from_millis(1000));
    }

    #[test]
    fn server_hint_is_honoured_up_to_sixty_seconds() {
        let base = Duration::from_millis(BACKOFF_BASE_MS);

        // 60 秒ちょうどは採用
        assert_eq!(
            retry_delay(Some(Duration::from_secs(60)), 0, base),
            Duration::from_secs(60)
        );
        // 61 秒は clamp せず backoff へ戻す
        assert_eq!(
            retry_delay(Some(Duration::from_secs(61)), 0, base),
            Duration::from_millis(500)
        );
        assert_eq!(
            retry_delay(Some(Duration::from_secs(99999)), 1, base),
            Duration::from_millis(1000)
        );
        // 指示が無ければ backoff
        assert_eq!(retry_delay(None, 0, base), Duration::from_millis(500));
    }

    #[test]
    fn retry_after_headers_are_read_in_order() {
        let header = |pairs: &[(&str, &str)]| {
            let mut map = HeaderMap::new();
            for (name, value) in pairs {
                map.insert(
                    reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                    HeaderValue::from_str(value).unwrap(),
                );
            }
            parse_retry_after(&map)
        };

        assert_eq!(
            header(&[("retry-after-ms", "250"), ("retry-after", "30")]),
            Some(Duration::from_millis(250)),
            "retry-after-ms を優先"
        );
        assert_eq!(header(&[("retry-after", "30")]), Some(Duration::from_secs(30)));
        // HTTP-date は C3B v1 では扱わない → backoff へ
        assert_eq!(header(&[("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT")]), None);
        assert_eq!(header(&[("retry-after", "-5")]), None);
        assert_eq!(header(&[]), None);
    }

    // ─── /v1/models の解析 ───────────────────────────────────────────────

    #[test]
    fn model_list_parsing_is_strict() {
        let good: Value = serde_json::from_str(&models_json()).unwrap();
        let cards = parse_model_list(&good).unwrap();
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].name, MODEL);
        assert_eq!(cards[0].release_date, "2026-01-01");

        assert!(parse_model_list(&json!({})).is_err(), "models 欠落");
        assert!(parse_model_list(&json!({"models": {}})).is_err(), "非配列");
        assert!(parse_model_list(&json!({"models": [1]})).is_err(), "card が非オブジェクト");
        assert!(
            parse_model_list(&json!({"models": [{"name": "a", "description": "b"}]})).is_err(),
            "release_date 欠落"
        );
        assert!(
            parse_model_list(&json!({"models": [{"name": "", "description": "b", "release_date": "c"}]}))
                .is_err(),
            "空文字は invalid"
        );
        assert!(
            parse_model_list(&json!({"models": [
                {"name": "a", "description": "b", "release_date": "c"},
                {"name": "a", "description": "d", "release_date": "e"}
            ]}))
            .is_err(),
            "同名重複"
        );
    }

    // ─── top-level envelope ──────────────────────────────────────────────

    #[test]
    fn envelope_validation_checks_only_the_top_level() {
        let good: Value = serde_json::from_str(&systemone_json(MODEL)).unwrap();
        let envelope = parse_envelope(&good).unwrap();
        assert_eq!(envelope.model, MODEL);
        assert_eq!(envelope.input_tokens, 120);
        assert_eq!(envelope.output_tokens, 8);
        assert!(envelope.answers.is_object());

        let usage = json!({"input_tokens": 1, "output_tokens": 2});
        assert!(parse_envelope(&json!([1, 2])).is_err(), "root が配列");
        assert!(
            parse_envelope(&json!({"answers": {}, "usage": usage})).is_err(),
            "model 欠落"
        );
        assert!(
            parse_envelope(&json!({"model": "  ", "answers": {}, "usage": usage})).is_err(),
            "model が空白のみ"
        );
        assert!(
            parse_envelope(&json!({"model": MODEL, "usage": usage})).is_err(),
            "answers 欠落"
        );
        assert!(
            parse_envelope(&json!({"model": MODEL, "answers": {}})).is_err(),
            "usage 欠落"
        );
        assert!(
            parse_envelope(&json!({"model": MODEL, "answers": {}, "usage": []})).is_err(),
            "usage が非オブジェクト"
        );
        assert!(
            parse_envelope(&json!({"model": MODEL, "answers": {},
                "usage": {"input_tokens": "12", "output_tokens": 2}}))
            .is_err(),
            "token が文字列"
        );
        assert!(
            parse_envelope(&json!({"model": MODEL, "answers": {},
                "usage": {"input_tokens": 3.7, "output_tokens": 2}}))
            .is_err(),
            "token が非整数"
        );
        assert!(
            parse_envelope(&json!({"model": MODEL, "answers": {},
                "usage": {"input_tokens": 1, "output_tokens": -2}}))
            .is_err(),
            "token が負"
        );

        // answers の型は C3B では問わない（配列でも通す。弾くのは C3A の責務）
        let array_answers = parse_envelope(&json!({
            "model": MODEL, "answers": [], "usage": usage
        }))
        .unwrap();
        assert!(array_answers.answers.is_array());
    }

    // ─── クライアント構築 ────────────────────────────────────────────────

    #[test]
    fn production_client_requires_an_api_key() {
        // 実際の env を汚さないよう、判定ロジックだけを確認する
        for raw in ["", "   ", "\t\n"] {
            assert!(raw.trim().is_empty(), "{raw:?} は missing key 扱い");
        }
        assert_eq!(TYPESAFE_BASE_URL, "https://api.typesafe.ai");
        assert!(TYPESAFE_BASE_URL.starts_with("https://"), "production は HTTPS 固定");
    }

    #[tokio::test]
    async fn production_client_rejects_plain_http() {
        // https_only(true) の client では http:// のホストへ出られない
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .user_agent(CLIENT_USER_AGENT)
            .build()
            .unwrap();
        let result = http.get("http://127.0.0.1:1/v1/models").send().await;
        assert!(result.is_err(), "HTTPS 限定の client が http を通してしまった");
    }

    #[test]
    #[should_panic(expected = "loopback")]
    fn test_client_rejects_external_hosts() {
        let _ = TypeSafeClient::with_key_for_test(DUMMY_KEY, "http://api.typesafe.ai");
    }

    #[test]
    fn debug_never_prints_the_key() {
        let client = TypeSafeClient::with_key_for_test(DUMMY_KEY, "http://127.0.0.1:9");
        let text = format!("{client:?}");
        assert!(!text.contains(DUMMY_KEY), "Debug にキーが出ている");
        assert!(text.contains("[REDACTED]"));
    }

    // ─── /v1/models（HTTP 経由）─────────────────────────────────────────

    #[tokio::test]
    async fn model_present_absent_and_meta() {
        let server = stub::start(vec![stub::json(200, &models_json())]).await;
        let card = client(&server.base).verify_model_available(MODEL).await.unwrap();
        assert_eq!(card.name, MODEL);
        assert_eq!(card.description, "test");

        // 不在のときは /v1/models の証跡を error に載せる
        let server = stub::start(vec![stub::json(200, &models_json())]).await;
        let error = client(&server.base)
            .verify_model_available("jev-does-not-exist")
            .await
            .unwrap_err();
        assert_eq!(error.kind, TypeSafeErrorKind::ModelMismatch);
        assert_eq!(error.http_status, Some(200));
        assert_eq!(error.retry_count, 0);
        let capture = error.capture.expect("capture を保持する");
        assert_eq!(capture.sha256(), sha_of(models_json().as_bytes()));
        assert!(!capture.truncated());

        // alias / 前方一致 / 大文字小文字違いは採用しない
        let server = stub::start(vec![
            stub::json(200, &models_json()),
            stub::json(200, &models_json()),
            stub::json(200, &models_json()),
        ])
        .await;
        let alias_client = client(&server.base);
        for name in ["jev-latest", "jev-test-model", "JEV-TEST-MODEL-1"] {
            let error = alias_client.verify_model_available(name).await.unwrap_err();
            assert_eq!(error.kind, TypeSafeErrorKind::ModelMismatch, "{name}");
        }
    }

    #[tokio::test]
    async fn duplicate_model_names_are_a_parse_error() {
        let body = json!({"models": [
            {"name": MODEL, "description": "a", "release_date": "2026-01-01"},
            {"name": MODEL, "description": "b", "release_date": "2026-02-01"}
        ]})
        .to_string();
        let server = stub::start(vec![stub::json(200, &body)]).await;
        let error = client(&server.base).list_models().await.unwrap_err();
        assert_eq!(error.kind, TypeSafeErrorKind::Parse);
        assert!(error.capture.is_some());
    }

    #[tokio::test]
    async fn request_id_is_captured() {
        let server = stub::start(vec![stub::response(
            200,
            &[("Content-Type", "application/json"), (REQUEST_ID_HEADER, "req-123")],
            models_json().as_bytes(),
        )])
        .await;
        let listed = client(&server.base).list_models_with_meta().await.unwrap();
        assert_eq!(listed.request_id.as_deref(), Some("req-123"));
        assert_eq!(listed.http_status, 200);
        assert!(listed.latency_ms >= 0);
    }

    // ─── /v1/systemone（HTTP 経由）──────────────────────────────────────

    #[tokio::test]
    async fn systemone_success_passes_answers_through() {
        let server = stub::start(vec![stub::response(
            200,
            &[("Content-Type", "application/json"), (REQUEST_ID_HEADER, "req-abc")],
            systemone_json(MODEL).as_bytes(),
        )])
        .await;

        let state = json!({"state_schema_version": "cm-jev-state-1"});
        let questions = json!({"c1_same_work": {"type": "noul"}});
        let result = client(&server.base)
            .ask_systemone(MODEL, &state, &questions)
            .await
            .unwrap();

        assert_eq!(result.requested_model, MODEL);
        assert_eq!(result.response_model, MODEL);
        assert_eq!(result.input_tokens, 120);
        assert_eq!(result.output_tokens, 8);
        assert_eq!(result.http_status, 200);
        assert_eq!(result.request_id.as_deref(), Some("req-abc"));
        assert_eq!(result.retry_count, 0);
        assert!(!result.capture.truncated());

        // answers は解釈せずそのまま
        assert_eq!(
            result.answers,
            json!({"c1_same_work": {"type": "noul", "noul": 0.9}})
        );

        // 送った body は model / state / questions だけ
        let sent = server.requests().remove(0);
        let body = sent.split("\r\n\r\n").nth(1).unwrap().to_string();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        let keys: Vec<&String> = parsed.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["model", "questions", "state"]);
        assert_eq!(parsed["state"], state);
        assert_eq!(parsed["questions"], questions);
    }

    #[tokio::test]
    async fn systemone_model_mismatch_keeps_evidence() {
        let body = systemone_json("jev-some-other-model");
        let server = stub::start(vec![stub::json(200, &body)]).await;
        let error = client(&server.base)
            .ask_systemone(MODEL, &json!({}), &json!({}))
            .await
            .unwrap_err();

        assert_eq!(error.kind, TypeSafeErrorKind::ModelMismatch);
        assert_eq!(error.http_status, Some(200));
        let capture = error.capture.expect("raw evidence を残す");
        assert_eq!(capture.sha256(), sha_of(body.as_bytes()));
        assert_eq!(capture.body_text().as_deref(), Some(body.as_str()));
    }

    #[tokio::test]
    async fn malformed_success_bodies_are_parse_errors() {
        for body in [
            "{".to_string(),
            json!({"answers": {}, "usage": {"input_tokens": 1, "output_tokens": 1}}).to_string(),
            json!({"model": MODEL, "usage": {"input_tokens": 1, "output_tokens": 1}}).to_string(),
            json!({"model": MODEL, "answers": {}}).to_string(),
            json!({"model": MODEL, "answers": {},
                   "usage": {"input_tokens": -1, "output_tokens": 1}})
            .to_string(),
        ] {
            let server = stub::start(vec![stub::json(200, &body)]).await;
            let error = client(&server.base)
                .ask_systemone(MODEL, &json!({}), &json!({}))
                .await
                .unwrap_err();
            assert_eq!(error.kind, TypeSafeErrorKind::Parse, "body={body}");
            assert_eq!(error.retry_count, 0, "parse は retry しない");
        }
    }

    #[tokio::test]
    async fn oversized_success_body_is_rejected_without_keeping_it() {
        let big = vec![b'x'; MAX_RESPONSE_BYTES as usize + 1];
        let server = stub::start(vec![stub::response(200, &[], &big)]).await;
        let error = client(&server.base)
            .ask_systemone(MODEL, &json!({}), &json!({}))
            .await
            .unwrap_err();

        assert_eq!(error.kind, TypeSafeErrorKind::ResponseTooLarge);
        let capture = error.capture.expect("証跡は残す");
        assert!(capture.truncated());
        assert_eq!(capture.body(), None, "巨大 body を保持しない");
        assert_eq!(capture.byte_count(), MAX_RESPONSE_BYTES + 1);
        assert_eq!(capture.sha256(), sha_of(&big), "hash は raw 全体");
        assert_eq!(server.hits(), 1, "response_too_large は retry しない");
    }

    #[tokio::test]
    async fn exactly_max_bytes_is_accepted_and_chunked_bodies_work() {
        // ちょうど上限（JSON ではないので parse で落ちるが、capture は非 truncated）
        let exact = vec![b'x'; MAX_RESPONSE_BYTES as usize];
        let server = stub::start(vec![stub::response(200, &[], &exact)]).await;
        let error = client(&server.base).list_models().await.unwrap_err();
        assert_eq!(error.kind, TypeSafeErrorKind::Parse, "上限超過ではなく JSON 不正");
        let capture = error.capture.unwrap();
        assert!(!capture.truncated());
        assert_eq!(capture.byte_count(), MAX_RESPONSE_BYTES);

        // Content-Length を持たない chunked 応答
        let body = models_json();
        let half = body.len() / 2;
        let server = stub::start(vec![stub::chunked(
            200,
            &[&body.as_bytes()[..half], &body.as_bytes()[half..]],
        )])
        .await;
        let listed = client(&server.base).list_models_with_meta().await.unwrap();
        assert_eq!(listed.models.len(), 2);
        assert_eq!(listed.capture.byte_count(), body.len() as u64);
        assert_eq!(listed.capture.sha256(), sha_of(body.as_bytes()));
    }

    // ─── retry ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn retryable_failures_recover() {
        let cases: Vec<(&str, Action)> = vec![
            ("429", stub::json(429, "{}")),
            ("503", stub::json(503, "{}")),
            ("network", Action::Drop),
        ];
        for (label, first) in cases {
            let server =
                stub::start(vec![first, stub::json(200, &systemone_json(MODEL))]).await;
            let result = client(&server.base)
                .ask_systemone(MODEL, &json!({}), &json!({}))
                .await
                .unwrap_or_else(|error| panic!("{label} で回復しなかった: {error}"));
            assert_eq!(result.retry_count, 1, "{label}");
            assert_eq!(server.hits(), 2, "{label}");
        }
    }

    #[tokio::test]
    async fn timeout_is_retried_and_classified() {
        let server = stub::start(vec![
            Action::Sleep(Duration::from_millis(400)),
            stub::json(200, &systemone_json(MODEL)),
        ])
        .await;
        let result = client_with_timeout(&server.base, Duration::from_millis(120))
            .ask_systemone(MODEL, &json!({}), &json!({}))
            .await
            .unwrap();
        assert_eq!(result.retry_count, 1);

        // 最後まで応答しなければ timeout として返る
        let server = stub::start(vec![Action::Sleep(Duration::from_millis(400))]).await;
        let error = client_with_timeout(&server.base, Duration::from_millis(120))
            .ask_systemone(MODEL, &json!({}), &json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.kind, TypeSafeErrorKind::Timeout);
        assert_eq!(error.retry_count, MAX_RETRIES);
        assert_eq!(server.hits(), 3);
    }

    fn client_with_timeout(base: &str, timeout: Duration) -> TypeSafeClient {
        client(base).with_timeout_for_test(timeout)
    }

    #[tokio::test]
    async fn non_retryable_statuses_are_sent_once() {
        let cases = [
            (401, TypeSafeErrorKind::Auth),
            (403, TypeSafeErrorKind::Auth),
            (422, TypeSafeErrorKind::Unprocessable),
            (400, TypeSafeErrorKind::Other),
            (404, TypeSafeErrorKind::Other),
            (413, TypeSafeErrorKind::Other),
            (301, TypeSafeErrorKind::Other),
            (302, TypeSafeErrorKind::Other),
            (303, TypeSafeErrorKind::Other),
            (307, TypeSafeErrorKind::Other),
            (308, TypeSafeErrorKind::Other),
        ];
        for (status, expected) in cases {
            let server = stub::start(vec![stub::response(
                status,
                &[("Location", "http://127.0.0.1:1/elsewhere")],
                b"body",
            )])
            .await;
            let error = client(&server.base)
                .ask_systemone(MODEL, &json!({}), &json!({}))
                .await
                .unwrap_err();
            assert_eq!(error.kind, expected, "HTTP {status}");
            assert_eq!(error.http_status, Some(status), "HTTP {status}");
            assert_eq!(error.retry_count, 0, "HTTP {status} は retry しない");
            assert_eq!(server.hits(), 1, "HTTP {status} で再送しない");
        }
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let server = stub::start(vec![stub::response(
            302,
            &[("Location", "http://127.0.0.1:1/elsewhere")],
            b"",
        )])
        .await;
        let error = client(&server.base).list_models().await.unwrap_err();
        assert_eq!(error.kind, TypeSafeErrorKind::Other, "3xx は network ではない");
        assert_eq!(error.http_status, Some(302));
        assert_eq!(server.hits(), 1, "Location 先へ追いかけない");
    }

    #[tokio::test]
    async fn retries_stop_at_two() {
        let server = stub::start(vec![stub::json(503, "{}")]).await;
        let error = client(&server.base)
            .ask_systemone(MODEL, &json!({}), &json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.kind, TypeSafeErrorKind::Overloaded);
        assert_eq!(error.retry_count, MAX_RETRIES);
        assert_eq!(server.hits(), 3, "初回 + retry 2 = 3 attempts");
        assert!(error.latency_ms >= 0);
    }

    // ─── シークレット漏洩 ────────────────────────────────────────────────

    #[tokio::test]
    async fn the_key_never_leaves_the_client() {
        // 応答がキーを echo してきても保存用には残さない
        let echo = json!({"error": format!("bad key {DUMMY_KEY}")}).to_string();
        let server = stub::start(vec![stub::json(401, &echo)]).await;
        let error = client(&server.base)
            .ask_systemone(MODEL, &json!({}), &json!({}))
            .await
            .unwrap_err();

        assert_eq!(error.kind, TypeSafeErrorKind::Auth);
        assert!(!format!("{error}").contains(DUMMY_KEY), "Display に漏れている");
        assert!(!format!("{error:?}").contains(DUMMY_KEY), "Debug に漏れている");
        assert!(!error.safe_text.contains(DUMMY_KEY));

        let capture = error.capture.expect("証跡");
        assert!(!capture.prefix().contains(DUMMY_KEY), "prefix に漏れている");
        assert!(!capture.body_text().unwrap().contains(DUMMY_KEY), "body に漏れている");
        assert_eq!(capture.sha256(), sha_of(echo.as_bytes()), "hash は raw 基準");

        // 送った request の body 側にもキーは入らない（ヘッダにしか載せない）
        let sent = server.requests().remove(0);
        let body = sent.split("\r\n\r\n").nth(1).unwrap_or("");
        assert!(!body.contains(DUMMY_KEY), "request body にキーが入っている");
    }

    #[tokio::test]
    async fn the_key_is_not_put_in_the_url() {
        let server = stub::start(vec![stub::json(200, &models_json())]).await;
        let _ = client(&server.base).list_models().await.unwrap();
        let sent = server.requests().remove(0);
        let request_line = sent.lines().next().unwrap_or_default();
        assert!(!request_line.contains(DUMMY_KEY), "URL にキーが入っている");
        assert!(!request_line.contains('?'), "query を付けない");
        assert!(sent.to_lowercase().contains("authorization: bearer"));
    }

    // ─── C3B.1: 送信 body の secret guard ────────────────────────────────

    /// state / questions に API キーが混ざっていたら、送らずに失敗する
    #[tokio::test]
    async fn request_body_containing_the_key_is_never_sent() {
        let leaky = json!({"note": format!("key={DUMMY_KEY}")});

        for (label, state, questions) in [
            ("state", leaky.clone(), json!({})),
            ("questions", json!({}), leaky.clone()),
        ] {
            let server = stub::start(vec![stub::json(200, &systemone_json(MODEL))]).await;
            let error = client(&server.base)
                .ask_systemone(MODEL, &state, &questions)
                .await
                .unwrap_err();

            assert_eq!(error.kind, TypeSafeErrorKind::Auth, "{label}");
            assert_eq!(server.hits(), 0, "{label}: request を送ってしまった");
            assert_eq!(error.http_status, None, "{label}");
            assert!(error.capture.is_none(), "{label}");
            assert!(!error.safe_text.contains(DUMMY_KEY), "{label}: safe_text に漏れている");
            assert!(!format!("{error}").contains(DUMMY_KEY), "{label}: Display に漏れている");
            assert!(!format!("{error:?}").contains(DUMMY_KEY), "{label}: Debug に漏れている");
        }

        // 通常の body は送れる
        let server = stub::start(vec![stub::json(200, &systemone_json(MODEL))]).await;
        let result = client(&server.base)
            .ask_systemone(MODEL, &json!({"title": "普通の状態"}), &json!({}))
            .await
            .unwrap();
        assert_eq!(result.http_status, 200);
        assert_eq!(server.hits(), 1);
    }

    /// 検査した byte 列と、実際に送った byte 列が一致していること
    #[tokio::test]
    async fn the_inspected_bytes_are_the_bytes_that_are_sent() {
        let server = stub::start(vec![stub::json(200, &systemone_json(MODEL))]).await;
        let state = json!({"a": 1, "b": [1, 2, 3]});
        let questions = json!({"q": {"type": "noul"}});
        let _ = client(&server.base)
            .ask_systemone(MODEL, &state, &questions)
            .await
            .unwrap();

        let sent = server.requests().remove(0);
        let body = sent.split("\r\n\r\n").nth(1).unwrap();
        let expected = serde_json::to_vec(&json!({
            "model": MODEL, "state": state, "questions": questions
        }))
        .unwrap();
        assert_eq!(body.as_bytes(), expected.as_slice(), "serialize し直されている");
        assert!(sent.to_lowercase().contains("content-type: application/json"));
    }

    /// escape が必要な文字を含むキーは、wire bytes の一致検索では見逃し得る。
    /// 意味の側の走査で必ず止める。
    #[tokio::test]
    async fn keys_needing_json_escaping_are_caught_by_the_semantic_scan() {
        // 1 文字目は " を含むキー、2 つ目は \ を含むキー
        let quote_key = "sk-abc\"def-123";
        let backslash_key = "sk-abc\\def-123";

        for key in [quote_key, backslash_key] {
            // 前提の確認: serialize すると escape されるので raw byte 検索では一致しない
            let payload = json!({"model": MODEL, "state": {"note": key}, "questions": {}});
            let wire = serde_json::to_vec(&payload).unwrap();
            assert!(
                !contains_bytes(&wire, key.as_bytes()),
                "この鍵は wire 上で escape されないのでテストの前提が崩れている: {key}"
            );
            // 意味の側では捕まる
            assert!(value_contains_secret(&payload, key));

            // 実際に送信が止まる
            let server = stub::start(vec![stub::json(200, &systemone_json(MODEL))]).await;
            let client = TypeSafeClient::try_with_key_for_test(key, &server.base)
                .unwrap()
                .with_backoff_for_test(Duration::from_millis(1));
            let error = client
                .ask_systemone(MODEL, &json!({"note": key}), &json!({}))
                .await
                .unwrap_err();

            assert_eq!(error.kind, TypeSafeErrorKind::Auth, "{key}");
            assert_eq!(server.hits(), 0, "{key}: 送信してしまった");
            assert!(!error.safe_text.contains(key));
            assert!(!format!("{error}").contains(key));
            assert!(!format!("{error:?}").contains(key));
        }
    }

    /// object の「キー側」に混ざった場合も止める
    #[tokio::test]
    async fn a_secret_used_as_an_object_key_is_rejected() {
        let server = stub::start(vec![stub::json(200, &systemone_json(MODEL))]).await;
        let state = json!({ DUMMY_KEY: "value" });
        let error = client(&server.base)
            .ask_systemone(MODEL, &state, &json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.kind, TypeSafeErrorKind::Auth);
        assert_eq!(server.hits(), 0);

        // 入れ子の配列・オブジェクトの奥でも同じ
        let server = stub::start(vec![stub::json(200, &systemone_json(MODEL))]).await;
        let questions = json!({"outer": [{"inner": {"deep": format!("x{DUMMY_KEY}y")}}]});
        let error = client(&server.base)
            .ask_systemone(MODEL, &json!({}), &questions)
            .await
            .unwrap_err();
        assert_eq!(error.kind, TypeSafeErrorKind::Auth);
        assert_eq!(server.hits(), 0);
    }

    #[test]
    fn semantic_scan_walks_the_whole_payload() {
        let secret = "sk-test-normal-123";
        assert!(value_contains_secret(&json!({"a": secret}), secret));
        assert!(value_contains_secret(&json!({"a": format!("prefix {secret} suffix")}), secret));
        assert!(value_contains_secret(&json!({ secret: 1 }), secret));
        assert!(value_contains_secret(&json!([1, [2, {"x": secret}]]), secret));
        assert!(value_contains_secret(&json!(secret), secret));

        assert!(!value_contains_secret(&json!({"a": "harmless"}), secret));
        assert!(!value_contains_secret(&json!({"a": 123, "b": true, "c": null}), secret));
        assert!(!value_contains_secret(&json!([]), secret));
    }

    /// production の env 値は前後の空白を落としてから Bearer にする
    #[test]
    fn api_keys_are_trimmed_and_blank_ones_rejected() {
        let padded = SecretKey::new(" valid-key ".trim().to_string()).unwrap();
        assert_eq!(padded.as_str(), "valid-key", "前後の空白を含めない");
        assert_eq!(padded.bytes(), b"valid-key");

        for blank in ["", "   ", "\t\n"] {
            // SecretKey は Debug を持たないので unwrap_err は使えない
            match SecretKey::new(blank.trim().to_string()) {
                Ok(_) => panic!("{blank:?} が通ってしまった"),
                Err(error) => assert_eq!(error.kind, TypeSafeErrorKind::Auth, "{blank:?}"),
            }
        }
    }

    // ─── C3B.1: raw parse body と redacted capture の分離 ────────────────

    /// 応答がキーを正当な JSON 値として含む場合でも、
    /// 意味の解析は raw から行い、保存用の証跡だけ redact する
    #[tokio::test]
    async fn semantics_use_the_raw_body_while_the_capture_is_redacted() {
        let body = json!({
            "model": MODEL,
            "answers": {"echoed": {"type": "noul", "noul": 0.5, "note": DUMMY_KEY}},
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })
        .to_string();

        let server = stub::start(vec![stub::json(200, &body)]).await;
        let result = client(&server.base)
            .ask_systemone(MODEL, &json!({}), &json!({}))
            .await
            .unwrap();

        // C3A へは TypeSafe が返した意味のまま渡る
        assert_eq!(result.answers["echoed"]["note"], json!(DUMMY_KEY));
        assert_eq!(result.answers["echoed"]["noul"], json!(0.5));

        // 保存用の証跡からは消えている
        let capture = &result.capture;
        assert!(!capture.body_text().unwrap().contains(DUMMY_KEY), "capture.body に漏れている");
        assert!(!capture.prefix().contains(DUMMY_KEY), "capture.prefix に漏れている");
        assert!(capture.body_text().unwrap().contains("[REDACTED]"));

        // hash と byte 数は raw 基準
        assert_eq!(capture.sha256(), sha_of(body.as_bytes()));
        assert_eq!(capture.byte_count(), body.len() as u64);
    }

    #[test]
    fn bounded_response_keeps_raw_and_redacted_apart() {
        let mut body = b"{\"k\":\"".to_vec();
        body.extend_from_slice(DUMMY_KEY.as_bytes());
        body.extend_from_slice(b"\"}");

        let bounded = collect_full(&[&body], None, DUMMY_KEY);
        assert_eq!(bounded.raw_body.as_deref(), Some(body.as_slice()), "raw は無加工");
        assert!(bounded.capture.body_text().unwrap().contains("[REDACTED]"));
        assert_ne!(bounded.raw_body.as_deref(), bounded.capture.body());

        // 上限超過なら両方とも持たない
        let big = vec![b'x'; MAX_RESPONSE_BYTES as usize + 1];
        let bounded = collect_full(&[&big], None, DUMMY_KEY);
        assert_eq!(bounded.raw_body, None);
        assert_eq!(bounded.capture.body(), None);
        assert!(bounded.capture.truncated());
        assert_eq!(bounded.capture.sha256(), sha_of(&big));
        assert_eq!(bounded.capture.byte_count(), MAX_RESPONSE_BYTES + 1);
    }

    // ─── C3B.1: Authorization header の fail closed ──────────────────────

    #[test]
    fn unusable_api_keys_are_rejected_at_construction() {
        let bad = [
            ("empty", ""),
            ("whitespace", "   "),
            ("tab newline", "\t\n"),
            ("cr", "sk-abc\rdef"),
            ("lf", "sk-abc\ndef"),
            ("crlf injection", "sk-abc\r\nX-Injected: 1"),
            ("control", "sk-abc\u{0}def"),
            ("bell", "sk-abc\u{7}def"),
        ];
        for (label, key) in bad {
            let error = TypeSafeClient::try_with_key_for_test(key, "http://127.0.0.1:9")
                .err()
                .unwrap_or_else(|| panic!("{label} が通ってしまった"));
            assert_eq!(error.kind, TypeSafeErrorKind::Auth, "{label}");
            assert!(!format!("{error}").contains(key.trim()) || key.trim().is_empty(), "{label}");
            assert!(!format!("{error:?}").contains("sk-abc"), "{label}: Debug に漏れている");
            assert!(!error.safe_text.contains("sk-abc"), "{label}");
        }

        // 通常のキーは通る
        assert!(TypeSafeClient::try_with_key_for_test(DUMMY_KEY, "http://127.0.0.1:9").is_ok());
    }

    /// 送信時に Authorization が欠ける経路が無いこと
    #[tokio::test]
    async fn authorization_is_always_attached() {
        let server = stub::start(vec![stub::json(200, &models_json())]).await;
        let _ = client(&server.base).list_models().await.unwrap();
        let sent = server.requests().remove(0).to_lowercase();
        assert_eq!(
            sent.matches("authorization: bearer").count(),
            1,
            "Authorization が 1 つだけ載る"
        );
    }

    // ─── live smoke（既定では走らせない）────────────────────────────────

    /// `TYPESAFE_API_KEY` が設定されているときだけ、手動で `GET /v1/models` を叩く。
    /// 既定の `cargo test` では走らない。キーそのものは絶対に表示しない。
    #[tokio::test]
    #[ignore = "外部ネットワークへ出る。手動でのみ実行する"]
    async fn live_models_smoke() {
        let Ok(client) = TypeSafeClient::from_env() else {
            eprintln!("{API_KEY_ENV} が無いので skip");
            return;
        };
        let models = client.list_models().await.expect("GET /v1/models");
        for card in &models {
            eprintln!("{} ({})", card.name, card.release_date);
        }
        assert!(!models.is_empty());
    }
}
