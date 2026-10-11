//! PR4-PF0G2（2/2）: reviewer 用の型付き API・TMDB 検証 transport・loopback HTTP サーバ。
//!
//! 守ること:
//! - 返す JSON は `Serialize` だけの DTO（`serde_json::Value` の素通し禁止。ソース走査テストで固定）。入力は `deny_unknown_fields`。
//! - machine 型・production DB（rusqlite）・`work_id`・`display_title` を参照しない。
//! - 全 API 応答の (type tag, key path 集合) を allowlist と照合し、外れたら応答を出さず violation として数える（A8 の実行時 scan）。
//! - seal / unblind / errata に到達する route を持たない。
//! - TMDB の API key はここの transport だけが持ち、DTO・ledger・log・Debug に出ない。

#![allow(dead_code)] // reviewer の bin は PF0G3 で追加する。それまで公開 API の一部は未使用
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::pr4_blind_gt::{
    canon, fmt_epoch, hex_sha, overlap_digest, Artifact, BlindError, Decision, MainAppProbe, Next, Store, Submit, SubmitOutcome,
    TmdbVerification,
};
use super::pr4_pf0g2::{pins_from_ledger, ui_package_sha256, UI_FILES, UI_PROTOCOL_VERSION};

type R<T> = Result<T, BlindError>;

pub const VERIFIER_IMPL: &str = "pr4-pf0g2-detail-1";
pub const MAX_BODY: usize = 64 * 1024;
const MAX_QUERY_CHARS: usize = 200;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_SEARCH_RESULTS: usize = 20;
pub const TMDB_PRODUCTION_BASE: &str = "https://api.themoviedb.org/3";

// ─── TMDB transport（reviewer 側の検索・存在確認）─────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetErr {
    Timeout,
    Connect,
    Other,
}

#[derive(Debug, Clone)]
pub enum TmdbReq {
    Search { kind: String, query: String },
    Detail { kind: String, id: i64 },
}

pub struct HttpResp {
    pub status: u16,
    pub body: Vec<u8>,
}

pub trait TmdbTransport {
    fn get(&mut self, req: &TmdbReq) -> Result<HttpResp, NetErr>;
}

/// production `tmdb_client::urlencoding` と同じ規則（ASCII の unreserved 以外は UTF-8 バイトを `%XX`、空白は `+`）
pub fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            ' ' => out.push('+'),
            c => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    out
}

/// 送信する request-target（path + query）。production の URL template（`tmdb_client.rs`）と同じ並び・値
pub fn wire_target(req: &TmdbReq, api_key: &str) -> R<String> {
    let kind = match req {
        TmdbReq::Search { kind, .. } | TmdbReq::Detail { kind, .. } => kind.as_str(),
    };
    if !matches!(kind, "movie" | "tv") {
        return Err(BlindError::Invalid("media_type".into()));
    }
    Ok(match req {
        TmdbReq::Search { query, .. } => {
            format!("/3/search/{kind}?api_key={api_key}&query={}&language=ja-JP&include_adult=false", urlencode(query))
        }
        TmdbReq::Detail { id, .. } => format!("/3/{kind}/{id}?api_key={api_key}&language=ja-JP"),
    })
}

/// 実 HTTP の transport。`base` は origin だけ（例 `http://127.0.0.1:PORT`）。production は `https://api.themoviedb.org`
pub struct HttpTmdb {
    rt: tokio::runtime::Runtime,
    client: reqwest::Client,
    origin: String,
    key: String,
}

impl std::fmt::Debug for HttpTmdb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HttpTmdb(origin={}, key=<redacted>)", self.origin)
    }
}

impl HttpTmdb {
    /// production の TMDB。**テストでは呼ばない**（live 通信になる）
    pub fn production(api_key: &str) -> R<HttpTmdb> {
        Self::with_origin("https://api.themoviedb.org", api_key, Duration::from_secs(30))
    }
    /// loopback 専用（テスト）
    pub fn loopback(addr: &str, api_key: &str, timeout: Duration) -> R<HttpTmdb> {
        if !addr.starts_with("127.0.0.1:") {
            return Err(BlindError::Stop("loopback 以外は production() を使う".into()));
        }
        Self::with_origin(&format!("http://{addr}"), api_key, timeout)
    }
    fn with_origin(origin: &str, api_key: &str, timeout: Duration) -> R<HttpTmdb> {
        if api_key.trim().is_empty() {
            return Err(BlindError::Invalid("TMDB API key がありません".into()));
        }
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| BlindError::Stop(e.to_string()))?;
        let mut b = reqwest::Client::builder().timeout(timeout);
        if origin.starts_with("http://") {
            b = b.no_proxy();
        }
        let client = b.build().map_err(|e| BlindError::Stop(e.to_string()))?;
        Ok(HttpTmdb { rt, client, origin: origin.to_string(), key: api_key.to_string() })
    }
}

impl TmdbTransport for HttpTmdb {
    fn get(&mut self, req: &TmdbReq) -> Result<HttpResp, NetErr> {
        let target = wire_target(req, &self.key).map_err(|_| NetErr::Other)?;
        let url = format!("{}{target}", self.origin);
        let client = self.client.clone();
        self.rt.block_on(async move {
            let classify = |e: &reqwest::Error| {
                if e.is_timeout() {
                    NetErr::Timeout
                } else if e.is_connect() {
                    NetErr::Connect
                } else {
                    NetErr::Other
                }
            };
            match client.get(&url).send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    match resp.bytes().await {
                        Ok(b) => Ok(HttpResp { status, body: b.to_vec() }),
                        Err(e) => Err(classify(&e)),
                    }
                }
                Err(e) => Err(classify(&e)),
            }
        })
    }
}

// ─── DTO（入力は deny_unknown_fields、出力は Serialize のみ）─────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartReq {
    reviewer_id: String,
    attestation: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionReq {
    session_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchReq {
    unit: String,
    lease_id: String,
    query: String,
    media_type: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewReq {
    unit: String,
    lease_id: String,
    tmdb_id: i64,
    media_type: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseReq {
    unit: String,
    lease_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitReq {
    unit: String,
    lease_id: String,
    submission_id: String,
    prev_event_sha: String,
    decision: String,
    candidate_token: Option<String>,
    tmdb_id: Option<i64>,
    media_type: Option<String>,
    defer_reason: Option<String>,
    note: Option<String>,
    correction_reason: Option<String>,
}

#[derive(Serialize)]
struct ProgressDto {
    done: usize,
    remaining: usize,
    deferred: usize,
    /// session の経過秒（schema の `progress{done,remaining,deferred,session_elapsed}`）
    session_elapsed: i64,
}
#[derive(Serialize)]
struct SessionDto {
    session_id: String,
    ui_protocol_version: String,
    max_units: u32,
    max_minutes: i64,
}
#[derive(Serialize)]
struct OkDto {
    ok: bool,
}
#[derive(Serialize)]
struct FileDto {
    part_no: i64,
    original_file_name: String,
    original_rel_path: String,
    container_tags_json: String,
    source_media_kind: String,
}
#[derive(Serialize)]
struct CandDto {
    candidate_token: String,
    title: String,
    original_title: Option<String>,
    year: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    original_language: Option<String>,
}
#[derive(Serialize)]
struct UnitDto {
    review_unit_id: String,
    lease_id: String,
    prev_event_sha: String,
    expires_at: String,
    files: Vec<FileDto>,
    candidates: Vec<CandDto>,
}
#[derive(Serialize)]
struct NextDto {
    done: bool,
    unit: Option<UnitDto>,
    progress: ProgressDto,
}
#[derive(Serialize)]
struct ResultDto {
    tmdb_id: i64,
    media_type: String,
    title: String,
    original_title: Option<String>,
    year: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    already_shown: bool,
}
#[derive(Serialize)]
struct SearchDto {
    prev_event_sha: String,
    results: Vec<ResultDto>,
}
#[derive(Serialize)]
struct PreviewDto {
    tmdb_id: i64,
    media_type: String,
    title: String,
    original_title: Option<String>,
    year: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
}
#[derive(Serialize)]
struct SubmitDto {
    accepted: bool,
    duplicate: bool,
    progress: ProgressDto,
}
#[derive(Serialize)]
struct ErrorDto {
    reason_code: String,
    message: String,
}

/// (type tag, 許される key path)。実応答の key path 集合がこの部分集合でなければ応答を出さない
pub(crate) fn allowed_paths() -> BTreeMap<&'static str, BTreeSet<String>> {
    let p = |tag: &'static str, v: Value| (tag, key_paths(&v));
    let progress = serde_json::json!({"done": 0, "remaining": 0, "deferred": 0, "session_elapsed": 0});
    let cand = serde_json::json!({"candidate_token": "", "title": "", "original_title": null, "year": null, "overview": null, "poster_path": null, "original_language": null});
    let file = serde_json::json!({"part_no": 1, "original_file_name": "", "original_rel_path": "", "container_tags_json": "", "source_media_kind": ""});
    let result = serde_json::json!({"tmdb_id": 1, "media_type": "", "title": "", "original_title": null, "year": null, "overview": null, "poster_path": null, "already_shown": false});
    BTreeMap::from([
        p("progress", progress.clone()),
        p("session", serde_json::json!({"session_id": "", "ui_protocol_version": "", "max_units": 0, "max_minutes": 0})),
        p("ok", serde_json::json!({"ok": true})),
        p(
            "next",
            serde_json::json!({"done": false, "progress": progress,
                "unit": {"review_unit_id": "", "lease_id": "", "prev_event_sha": "", "expires_at": "", "files": [file], "candidates": [cand]}}),
        ),
        p("search", serde_json::json!({"prev_event_sha": "", "results": [result]})),
        p("preview", serde_json::json!({"tmdb_id": 1, "media_type": "", "title": "", "original_title": null, "year": null, "overview": null, "poster_path": null})),
        p("submit", serde_json::json!({"accepted": true, "duplicate": false, "progress": progress})),
        p("error", serde_json::json!({"reason_code": "", "message": ""})),
    ])
}

/// key path の集合（値は見ない。配列は `[]`）
pub fn key_paths(v: &Value) -> BTreeSet<String> {
    fn walk(v: &Value, p: &str, out: &mut BTreeSet<String>) {
        match v {
            Value::Object(o) => {
                for (k, x) in o {
                    let np = if p.is_empty() { k.clone() } else { format!("{p}.{k}") };
                    out.insert(np.clone());
                    walk(x, &np, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, &format!("{p}[]"), out)),
            _ => {}
        }
    }
    let mut s = BTreeSet::new();
    walk(v, "", &mut s);
    s
}

fn error_of(e: &BlindError) -> (u16, &'static str, &'static str) {
    match e {
        BlindError::Conflict => (409, "conflict", "他の更新と競合しました。画面を更新してください。"),
        BlindError::LeaseInvalid(_) => (409, "lease_invalid", "この unit の貸出が無効または期限切れです。"),
        BlindError::UnitAlreadyResolved(_) => (409, "unit_already_resolved", "この unit は既に確定しています。"),
        BlindError::UnitNotAvailable(_) => (409, "unit_not_available", "この unit は今は使えません。"),
        BlindError::TargetIsCandidate => (422, "target_is_candidate", "表示された候補です。候補は「確定」で選んでください。"),
        BlindError::TargetNotVerified(_) => (422, "target_not_verified", "TMDB での存在確認ができていません。"),
        BlindError::NoneNeedsSearch => (422, "none_needs_search", "「該当なし」には TMDB 検索が最低 1 回必要です。"),
        BlindError::Invalid(_) => (400, "invalid_request", "入力が正しくありません。"),
        BlindError::MainAppRunning => (423, "main_app_running", "CineMantis 本体が起動中です。終了してから再開してください。"),
        BlindError::SessionLimit(_) => (429, "session_limit", "この session の上限に達しました。休憩してください。"),
        BlindError::Sealed => (410, "sealed", "レビューは封印済みです。"),
        _ => (500, "stopped", "内部の整合性検査で停止しました。operator に連絡してください。"),
    }
}

// ─── Api ───────────────────────────────────────────────────────────────────────────────────

struct Active {
    session_id: String,
    lease_id: String,
    expires_at: String,
    searches: u32,
}

pub struct Api {
    store: Store,
    tmdb: Box<dyn TmdbTransport>,
    cache_dir: PathBuf,
    clock: Box<dyn FnMut() -> String>,
    active: HashMap<String, Active>,
    /// (unit, lease_id, tmdb_id, media_type) → server が取った検証結果（client の値は信じない）
    verified: HashMap<(String, String, i64, String), TmdbVerification>,
    allowed: BTreeMap<&'static str, BTreeSet<String>>,
    seen: BTreeMap<String, (BTreeSet<String>, u64)>,
    violations: u64,
    session_started: Option<i64>,
}

pub fn system_clock() -> Box<dyn FnMut() -> String> {
    Box::new(|| fmt_epoch(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)))
}

impl Api {
    /// 起動条件を全て確かめてから作る:
    /// - 本体 CineMantis が動いていれば拒否（`MainAppRunning`）
    /// - いま配るのと同じ UI package の SHA が ledger の init pin と一致すること
    /// - artifact は `Artifact::load`（package の hard check 込み）で読んだものだけ
    pub fn attach(store: Store, ledger_dir: &Path, cache_dir: &Path, tmdb: Box<dyn TmdbTransport>, clock: Box<dyn FnMut() -> String>, startup_probe: &dyn MainAppProbe) -> R<Api> {
        if startup_probe.main_app_running() {
            return Err(BlindError::MainAppRunning);
        }
        let pins = pins_from_ledger(ledger_dir)?;
        if pins.ui_package_sha256 != ui_package_sha256() {
            return Err(BlindError::PinMismatch(vec!["ui_package_sha256".into()]));
        }
        std::fs::create_dir_all(cache_dir).map_err(|e| BlindError::Stop(e.to_string()))?;
        Ok(Api {
            store,
            tmdb,
            cache_dir: cache_dir.to_path_buf(),
            clock,
            active: HashMap::new(),
            verified: HashMap::new(),
            allowed: allowed_paths(),
            seen: BTreeMap::new(),
            violations: 0,
            session_started: None,
        })
    }

    pub fn load_artifact(reviewer_dir: &Path) -> R<Artifact> {
        Artifact::load(reviewer_dir)
    }

    pub fn store(&self) -> &Store {
        &self.store
    }
    pub fn into_store(self) -> Store {
        self.store
    }

    fn progress(&mut self) -> ProgressDto {
        let (done, remaining, deferred) = self.store.progress();
        let now = self.now();
        let session_elapsed = self.session_started.and_then(|s| super::pr4_blind_gt::epoch(&now).ok().map(|n| (n - s).max(0))).unwrap_or(0);
        ProgressDto { done, remaining, deferred, session_elapsed }
    }

    /// 実行時の A8 report（型ごとの key path 集合の SHA と件数・violation 数）
    pub fn runtime_a8_report(&self) -> Value {
        let types: BTreeMap<&String, Value> = self
            .seen
            .iter()
            .map(|(tag, (paths, n))| (tag, serde_json::json!({"key_paths_sha256": hex_sha(paths.iter().cloned().collect::<Vec<_>>().join("\n").as_bytes()), "count": n})))
            .collect();
        serde_json::json!({"schema_version": "pr4-pf0g-a8-runtime-1", "violations": self.violations, "types": types})
    }
    pub fn violations(&self) -> u64 {
        self.violations
    }

    pub(crate) fn emit<T: Serialize>(&mut self, status: u16, tag: &'static str, dto: &T) -> (u16, String) {
        let v = match serde_json::to_value(dto) {
            Ok(v) => v,
            Err(_) => return (500, r#"{"reason_code":"stopped","message":"内部の整合性検査で停止しました。operator に連絡してください。"}"#.into()),
        };
        let paths = key_paths(&v);
        let ok = self.allowed.get(tag).map(|a| paths.is_subset(a)).unwrap_or(false);
        if !ok {
            self.violations += 1;
            return (500, r#"{"reason_code":"stopped","message":"内部の整合性検査で停止しました。operator に連絡してください。"}"#.into());
        }
        let e = self.seen.entry(tag.to_string()).or_insert_with(|| (BTreeSet::new(), 0));
        e.0.extend(paths);
        e.1 += 1;
        (status, canon(&v).unwrap_or_default())
    }

    fn fail(&mut self, e: &BlindError) -> (u16, String) {
        let (status, code, msg) = error_of(e);
        self.emit(status, "error", &ErrorDto { reason_code: code.into(), message: msg.into() })
    }

    fn bad_request(&mut self) -> (u16, String) {
        self.fail(&BlindError::Invalid(String::new()))
    }

    fn now(&mut self) -> String {
        (self.clock)()
    }

    /// 1 request を処理する。`method` / `path` / `body` だけを入力にする（HTTP の層とは独立にテストできる）
    pub fn handle(&mut self, method: &str, path: &str, body: &[u8]) -> (u16, String) {
        if body.len() > MAX_BODY {
            return self.bad_request();
        }
        match (method, path) {
            ("GET", "/api/progress") => {
                let p = self.progress();
                self.emit(200, "progress", &p)
            }
            ("POST", "/api/session/start") => self.with(body, |a, r: StartReq| {
                let now = a.now();
                let session_id = a.store.start_session(&r.reviewer_id, r.attestation, UI_PROTOCOL_VERSION, &now)?;
                a.session_started = super::pr4_blind_gt::epoch(&now).ok();
                Ok(a.emit(200, "session", &SessionDto { session_id, ui_protocol_version: UI_PROTOCOL_VERSION.into(), max_units: super::pr4_blind_gt::SESSION_MAX_UNITS, max_minutes: super::pr4_blind_gt::SESSION_MAX_SECONDS / 60 }))
            }),
            ("POST", "/api/session/end") => self.with(body, |a, r: SessionReq| {
                let now = a.now();
                a.store.end_session(&r.session_id, &now)?;
                a.session_started = None;
                a.active.retain(|_, v| v.session_id != r.session_id);
                a.verified.clear();
                Ok(a.emit(200, "ok", &OkDto { ok: true }))
            }),
            ("POST", "/api/next") => self.with(body, |a, r: SessionReq| a.next(&r.session_id)),
            ("POST", "/api/search") => self.with(body, |a, r: SearchReq| a.search(r)),
            ("POST", "/api/preview") => self.with(body, |a, r: PreviewReq| a.preview(r)),
            ("POST", "/api/submit") => self.with(body, |a, r: SubmitReq| a.submit(r)),
            ("POST", "/api/release") => self.with(body, |a, r: ReleaseReq| {
                let now = a.now();
                a.store.release_lease(&r.unit, &r.lease_id, &now)?;
                a.active.remove(&r.unit);
                a.verified.retain(|k, _| k.0 != r.unit);
                Ok(a.emit(200, "ok", &OkDto { ok: true }))
            }),
            _ => self.emit(404, "error", &ErrorDto { reason_code: "not_found".into(), message: "見つかりません。".into() }),
        }
    }

    fn with<T: for<'de> Deserialize<'de>>(&mut self, body: &[u8], f: impl FnOnce(&mut Api, T) -> R<(u16, String)>) -> (u16, String) {
        let req: T = match serde_json::from_slice(body) {
            Ok(r) => r,
            Err(_) => return self.bad_request(),
        };
        match f(self, req) {
            Ok(r) => r,
            Err(e) => self.fail(&e),
        }
    }

    fn unit_dto(&self, unit: &str, a: &Active, prev: String) -> R<UnitDto> {
        let u = self.store.unit(unit).ok_or_else(|| BlindError::Invalid(String::new()))?;
        Ok(UnitDto {
            review_unit_id: u.review_unit_id.clone(),
            lease_id: a.lease_id.clone(),
            prev_event_sha: prev,
            expires_at: a.expires_at.clone(),
            files: u
                .evidence
                .files
                .iter()
                .map(|f| FileDto {
                    part_no: f.part_no,
                    original_file_name: f.original_file_name.clone(),
                    original_rel_path: f.original_rel_path.clone(),
                    container_tags_json: f.container_tags_json.clone(),
                    source_media_kind: f.source_media_kind.clone(),
                })
                .collect(),
            candidates: u
                .candidates
                .iter()
                .map(|c| CandDto {
                    candidate_token: c.candidate_token.clone(),
                    title: c.title.clone(),
                    original_title: c.original_title.clone(),
                    year: c.year.clone(),
                    overview: c.overview.clone(),
                    poster_path: c.poster_path.clone(),
                    original_language: c.original_language.clone(),
                })
                .collect(),
        })
    }

    fn next(&mut self, session_id: &str) -> R<(u16, String)> {
        let now = self.now();
        // 既にこの session が持っている有効な貸出があればそれを返す（画面の再読込で別 unit を取らない）
        let live = self.active.iter().find(|(_, a)| a.session_id == session_id && super::pr4_blind_gt::epoch(&now).ok() < super::pr4_blind_gt::epoch(&a.expires_at).ok()).map(|(u, _)| u.clone());
        let unit = match live {
            Some(unit) => unit,
            None => match self.store.next() {
                Next::Done => {
                    let progress = self.progress();
                    return Ok(self.emit(200, "next", &NextDto { done: true, unit: None, progress }));
                }
                Next::Unit { unit, purpose } => {
                    let g = self.store.acquire_lease(session_id, &unit, purpose, &now)?;
                    self.active.insert(unit.clone(), Active { session_id: session_id.to_string(), lease_id: g.lease_id, expires_at: g.expires_at, searches: 0 });
                    unit
                }
            },
        };
        let prev = self.store.unit_last(&unit).unwrap_or_default();
        let a = &self.active[&unit];
        let dto = self.unit_dto(&unit, a, prev)?;
        let progress = self.progress();
        Ok(self.emit(200, "next", &NextDto { done: false, unit: Some(dto), progress }))
    }

    fn check_active(&mut self, unit: &str, lease_id: &str) -> R<()> {
        let now = self.now();
        match self.active.get(unit) {
            Some(a) if a.lease_id == lease_id && super::pr4_blind_gt::epoch(&now)? < super::pr4_blind_gt::epoch(&a.expires_at)? => Ok(()),
            _ => Err(BlindError::LeaseInvalid("lease".into())),
        }
    }

    fn is_shown(&self, unit: &str, tmdb_id: i64, media_type: &str) -> bool {
        self.store
            .unit(unit)
            .map(|u| overlap_digest(&u.unit_salt, media_type, tmdb_id).map(|d| u.overlap_digests.contains(&d)).unwrap_or(false))
            .unwrap_or(false)
    }

    fn search(&mut self, r: SearchReq) -> R<(u16, String)> {
        self.check_active(&r.unit, &r.lease_id)?;
        let q = r.query.trim().to_string();
        if q.is_empty() || q.chars().count() > MAX_QUERY_CHARS || q.chars().any(char::is_control) || !matches!(r.media_type.as_str(), "movie" | "tv") {
            return Err(BlindError::Invalid(String::new()));
        }
        let resp = self.tmdb.get(&TmdbReq::Search { kind: r.media_type.clone(), query: q.clone() }).map_err(|_| BlindError::TargetNotVerified("transport".into()))?;
        if resp.status != 200 || resp.body.len() > MAX_RESPONSE_BYTES {
            return Err(BlindError::TargetNotVerified("status".into()));
        }
        let v: Value = serde_json::from_slice(&resp.body).map_err(|_| BlindError::TargetNotVerified("body".into()))?;
        let list = v["results"].as_array().ok_or_else(|| BlindError::TargetNotVerified("results".into()))?;
        let mut results = Vec::new();
        for it in list.iter().take(MAX_SEARCH_RESULTS) {
            let Some(id) = it["id"].as_i64().filter(|n| *n >= 1) else { continue };
            let (title, orig, date) = if r.media_type == "movie" { ("title", "original_title", "release_date") } else { ("name", "original_name", "first_air_date") };
            let s = |k: &str| it[k].as_str().map(str::to_string);
            results.push(ResultDto {
                tmdb_id: id,
                media_type: r.media_type.clone(),
                title: s(title).unwrap_or_default(),
                original_title: s(orig),
                year: s(date).and_then(|d| d.get(..4).map(str::to_string)).filter(|y| y.chars().all(|c| c.is_ascii_digit())),
                overview: s("overview"),
                poster_path: s("poster_path"),
                already_shown: self.is_shown(&r.unit, id, &r.media_type),
            });
        }
        let now = self.now();
        let prev = self.store.record_search(&r.unit, &r.lease_id, &q, &r.media_type, list.len() as i64, &now)?;
        if let Some(a) = self.active.get_mut(&r.unit) {
            a.searches += 1;
        }
        Ok(self.emit(200, "search", &SearchDto { prev_event_sha: prev, results }))
    }

    /// pick_other の存在確認（3 段: ①前提 ②通信 ③前提の再確認）。結果は server が保持し、submit はそれだけを使う
    fn preview(&mut self, r: PreviewReq) -> R<(u16, String)> {
        // ① lease と形式と「表示候補ではない」こと
        self.check_active(&r.unit, &r.lease_id)?;
        if !matches!(r.media_type.as_str(), "movie" | "tv") || !(1..=9_007_199_254_740_992i64).contains(&r.tmdb_id) {
            return Err(BlindError::Invalid(String::new()));
        }
        if self.is_shown(&r.unit, r.tmdb_id, &r.media_type) {
            return Err(BlindError::TargetIsCandidate);
        }
        // ② 通信（この間 store には触れない）
        let resp = self.tmdb.get(&TmdbReq::Detail { kind: r.media_type.clone(), id: r.tmdb_id }).map_err(|_| BlindError::TargetNotVerified("transport".into()))?;
        if resp.status != 200 || resp.body.len() > MAX_RESPONSE_BYTES {
            return Err(BlindError::TargetNotVerified(format!("http {}", resp.status)));
        }
        let v: Value = serde_json::from_slice(&resp.body).map_err(|_| BlindError::TargetNotVerified("body".into()))?;
        if v["id"].as_i64() != Some(r.tmdb_id) {
            return Err(BlindError::TargetNotVerified("別の作品が返りました".into()));
        }
        if v["adult"] == serde_json::json!(true) {
            return Err(BlindError::TargetNotVerified("adult".into()));
        }
        let (tk, ok, dk) = if r.media_type == "movie" { ("title", "original_title", "release_date") } else { ("name", "original_name", "first_air_date") };
        let s = |k: &str| v[k].as_str().map(str::to_string);
        let title = s(tk).filter(|t| !t.is_empty()).ok_or_else(|| BlindError::TargetNotVerified("title".into()))?;
        let year = s(dk).and_then(|d| d.get(..4).map(str::to_string)).filter(|y| y.chars().all(|c| c.is_ascii_digit()));
        // ③ 前提の再確認
        self.check_active(&r.unit, &r.lease_id)?;
        let body_sha = hex_sha(&resp.body);
        std::fs::write(self.cache_dir.join(format!("{body_sha}.json")), &resp.body).map_err(|e| BlindError::Stop(e.to_string()))?;
        let now = self.now();
        let ver = TmdbVerification {
            requested_id: r.tmdb_id,
            requested_media_type: r.media_type.clone(),
            http_status: 200,
            response_id: r.tmdb_id,
            response_media_type: r.media_type.clone(),
            response_body_sha256: body_sha,
            title: title.clone(),
            year: year.clone(),
            fetched_at: now,
            verifier_impl: VERIFIER_IMPL.into(),
        };
        self.verified.insert((r.unit.clone(), r.lease_id.clone(), r.tmdb_id, r.media_type.clone()), ver);
        Ok(self.emit(200, "preview", &PreviewDto { tmdb_id: r.tmdb_id, media_type: r.media_type, title, original_title: s(ok), year, overview: s("overview"), poster_path: s("poster_path") }))
    }

    fn submit(&mut self, r: SubmitReq) -> R<(u16, String)> {
        let stray = |fields: &[bool]| fields.iter().any(|b| *b);
        let decision = match r.decision.as_str() {
            "confirm" => {
                if stray(&[r.tmdb_id.is_some(), r.media_type.is_some(), r.defer_reason.is_some()]) {
                    return Err(BlindError::Invalid(String::new()));
                }
                Decision::Confirm { candidate_token: r.candidate_token.clone().ok_or_else(|| BlindError::Invalid(String::new()))? }
            }
            "pick_other" => {
                if stray(&[r.candidate_token.is_some(), r.defer_reason.is_some()]) {
                    return Err(BlindError::Invalid(String::new()));
                }
                let (Some(id), Some(mt)) = (r.tmdb_id, r.media_type.clone()) else { return Err(BlindError::Invalid(String::new())) };
                let verification = self.verified.get(&(r.unit.clone(), r.lease_id.clone(), id, mt.clone())).cloned();
                Decision::PickOther { tmdb_id: id, media_type: mt, verification }
            }
            "none" => {
                if stray(&[r.candidate_token.is_some(), r.tmdb_id.is_some(), r.media_type.is_some(), r.defer_reason.is_some()]) {
                    return Err(BlindError::Invalid(String::new()));
                }
                Decision::None
            }
            "defer" => {
                if stray(&[r.candidate_token.is_some(), r.tmdb_id.is_some(), r.media_type.is_some()]) {
                    return Err(BlindError::Invalid(String::new()));
                }
                Decision::Defer { reason: r.defer_reason.clone().ok_or_else(|| BlindError::Invalid(String::new()))? }
            }
            _ => return Err(BlindError::Invalid(String::new())),
        };
        let now = self.now();
        let out = self.store.submit(
            Submit { unit: r.unit.clone(), submission_id: r.submission_id, lease_id: r.lease_id.clone(), prev_event_sha: r.prev_event_sha, decision, note: r.note, correction_reason: r.correction_reason },
            &now,
        )?;
        self.active.remove(&r.unit);
        self.verified.retain(|k, _| k.0 != r.unit);
        let progress = self.progress();
        Ok(self.emit(200, "submit", &SubmitDto { accepted: matches!(out, SubmitOutcome::Recorded { .. }), duplicate: matches!(out, SubmitOutcome::Duplicate { .. }), progress }))
    }
}

// ─── loopback HTTP ─────────────────────────────────────────────────────────────────────────

/// 起動ごとのランダム token（OS 乱数で seed された hasher を混ぜた SHA-256）
pub fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut buf = String::new();
    for i in 0..4u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i ^ std::process::id() as u64);
        buf.push_str(&format!("{:016x}", h.finish()));
    }
    hex_sha(buf.as_bytes())
}

pub struct Server {
    pub addr: String,
    pub token: String,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    /// `127.0.0.1` 固定。port 0 なら OS が選ぶ。
    /// `Store` は `Send` ではない（PF0G1 の probe が `Send` 境界を持たない）ので、`Api` は **サーバのスレッドの中で** `factory` から作る。
    /// factory が失敗すれば（本体アプリ稼働・UI pin 不一致など）起動せずにその Err を返す
    pub fn start<F>(factory: F, port: u16) -> R<Server>
    where
        F: FnOnce() -> R<Api> + Send + 'static,
    {
        let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| BlindError::Stop(e.to_string()))?;
        let addr = listener.local_addr().map_err(|e| BlindError::Stop(e.to_string()))?.to_string();
        listener.set_nonblocking(true).map_err(|e| BlindError::Stop(e.to_string()))?;
        let token = random_token();
        let stop = Arc::new(AtomicBool::new(false));
        let (s2, t2, a2) = (stop.clone(), token.clone(), addr.clone());
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), BlindError>>();
        let handle = std::thread::spawn(move || {
            let mut api = match factory() {
                Ok(a) => {
                    let _ = tx.send(Ok(()));
                    a
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            while !s2.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => serve_one(&mut api, stream, &a2, &t2),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(5)),
                    Err(_) => break,
                }
            }
        });
        match rx.recv() {
            Ok(Ok(())) => Ok(Server { addr, token, stop, handle: Some(handle) }),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => Err(BlindError::Stop("server thread が起動できません".into())),
        }
    }

    /// 止める。`Api`（= `Store`）はスレッド内で drop され、lock が外れる
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

struct Req {
    method: String,
    path: String,
    query: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_req(s: &mut TcpStream) -> Option<Req> {
    s.set_nonblocking(false).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        if buf.len() > 16 * 1024 {
            return None;
        }
        let n = s.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8(buf[..head_end].to_vec()).ok()?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, target) = (first.next()?.to_string(), first.next()?.to_string());
    let (path, query) = target.split_once('?').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((target.clone(), String::new()));
    let headers: HashMap<String, String> = lines.filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))).collect();
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return Some(Req { method, path, query, headers, body: vec![0; MAX_BODY + 1] });
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = s.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(len);
    Some(Req { method, path, query, headers, body })
}

/// narrowest CSP: inline script/style なし（UI は同一 origin の app.js / app.css）、画像・font・frame・form・base なし、接続は同一 origin のみ
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'none'; font-src 'none'; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

fn respond(s: &mut TcpStream, status: u16, ctype: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Error",
    };
    let head = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: {CSP}\r\nConnection: close\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body);
}

fn serve_one(api: &mut Api, mut s: TcpStream, addr: &str, token: &str) {
    let Some(req) = read_req(&mut s) else { return };
    // Host / Origin 検査（DNS rebinding・他 origin からの利用を拒否）
    let host_ok = req.headers.get("host").map(|h| h == addr).unwrap_or(false);
    let origin_ok = req.headers.get("origin").map(|o| *o == format!("http://{addr}")).unwrap_or(true);
    if !host_ok || !origin_ok {
        return respond(&mut s, 403, "application/json", r#"{"reason_code":"forbidden","message":"許可されていません。"}"#.as_bytes());
    }
    if req.method == "GET" && req.path == "/" {
        if !req.query.split('&').any(|kv| kv == format!("t={token}")) {
            return respond(&mut s, 403, "application/json", r#"{"reason_code":"forbidden","message":"許可されていません。"}"#.as_bytes());
        }
        let html = UI_FILES.iter().find(|(n, _)| *n == "index.html").map(|(_, b)| *b).unwrap_or("");
        return respond(&mut s, 200, "text/html; charset=utf-8", html.as_bytes());
    }
    // 静的 asset（pin 済み・secret を含まない）。token なしで配る（<script src> は header を付けられない）。Host / Origin 検査は上で済み
    if req.method == "GET" && (req.path == "/app.css" || req.path == "/app.js") {
        let name = &req.path[1..];
        let ctype = if name.ends_with(".css") { "text/css; charset=utf-8" } else { "text/javascript; charset=utf-8" };
        let body = UI_FILES.iter().find(|(n, _)| *n == name).map(|(_, b)| *b).unwrap_or("");
        return respond(&mut s, 200, ctype, body.as_bytes());
    }
    if req.headers.get("x-review-token").map(String::as_str) != Some(token) {
        return respond(&mut s, 403, "application/json", r#"{"reason_code":"forbidden","message":"許可されていません。"}"#.as_bytes());
    }
    let (status, body) = api.handle(&req.method, &req.path, &req.body);
    respond(&mut s, status, "application/json", body.as_bytes());
}
