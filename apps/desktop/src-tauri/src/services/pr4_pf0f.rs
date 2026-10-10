//! PR4-PF0F1: FINAL machine-output の generator / replay / PF0B adapter 前段（**synthetic・mock transport のみ・test ビルドだけ・製品には出ない**）。
//!
//! 契約は `docs/PR4_PF0F0_FINAL_MACHINE_ARTIFACT_CONTRACT.md`（r2）。この実装は live TMDB・H2 実データ・production DB・GT・Jev に触れない。
//! TMDB への到達は [`Transport`] trait だけを通す（ここには実 HTTP の実装を持たない）。
//!
//! - 候補の生成は `commands::tmdb::fetch_candidates` と同じ手順を [`derive`] で写し、**generator と replay が同じ関数**を使う
//!   （replay は transport の代わりに保存済みの raw body を読む）。production と違い、year なし再検索の失敗も握りつぶさない。
//! - binding は `rules-tags-shadow-2`（combined-ranked）。`rules-safe-2`（legacy-ranked）は reference/control。rules-1 は対象外。
//! - membership の authority は PF0B の `membership_commitment` だけ。
//! - 成功 response は本文を content-addressed で保存。API key / Authorization は保存しない（書き込み前に走査して中止）。
//! - recovery は primary 後に 1 回だけ。対象集合を先に commitment し、成功済み unit は再実行しない。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::commands::tmdb::{merge_candidate_sets, parsed_for_query, planned_logical_calls, query_plan_for, skips_tmdb};
use crate::models::tmdb::{TmdbCandidate, TmdbSearchMovie, TmdbSearchResponse, TmdbSearchTv};
use crate::services::match_history::CandidateSource;
use crate::services::metadata_matcher::{
    merge_and_rank, rules_safe_with_embedded, rules_tags_shadow_best_candidate, score_movie, score_tv, SafeDecision, SafeMatchOutcome,
    THRESHOLD_CANDIDATE,
};
use crate::services::pr4_pf0a::Stop;
use crate::services::pr4_pf0b::{membership_commitment, FinalInput, Gt, Machine};
use crate::services::pr4_pf0e::{canon, evidence_hashes, snapshot_from_payload};
use crate::services::prematch_snapshot::{EvidenceClass, LocalEvidence, QuerySource};
use crate::services::title_parser::{parse_title, ParsedTitle};

pub const CONTRACT_VERSION: &str = "pr4-pf0f0-contract-2";
pub const BINDING_MATCHER: &str = "rules-tags-shadow-2";
pub const REFERENCE_MATCHER: &str = "rules-safe-2";
const ORDERING_RULE: &str = "STABLE_SORT_CONFIDENCE_DESC_THEN_INSERTION_ORDER";
const RANK_CAP: usize = 10;
const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const F_UNIT: &str = "unit_input.jsonl";
const F_REQ: &str = "tmdb_request.jsonl";
const F_CAND: &str = "candidate_set.jsonl";
const F_OUT: &str = "matcher_output.jsonl";
const F_PROJ: &str = "reviewer_projection.jsonl";
const F_LOG: &str = "run_log.jsonl";
const F_EVAL: &str = "evaluation_unit.jsonl";
const CHAIN_FILES: [&str; 7] = [F_UNIT, F_REQ, F_CAND, F_OUT, F_PROJ, F_LOG, F_EVAL];
const FORBIDDEN_KEYS: [&str; 6] = ["api_key", "apikey", "authorization", "token", "secret", "password"];

fn stop<T>(m: impl Into<String>) -> Result<T, Stop> {
    Err(Stop(m.into()))
}
fn io<E: ToString>(e: E) -> Stop {
    Stop(e.to_string())
}
fn sha(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
fn sha_v(v: &Value) -> Result<String, Stop> {
    Ok(sha(canon(v)?.as_bytes()))
}

// ─── transport ──────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetErr {
    Timeout,
    Connect,
    Interrupted,
    Other,
}
impl NetErr {
    fn as_str(self) -> &'static str {
        match self {
            NetErr::Timeout => "timeout",
            NetErr::Connect => "connect",
            NetErr::Interrupted => "interrupted",
            NetErr::Other => "other",
        }
    }
}
pub struct HttpResp {
    pub status: u16,
    pub body: Vec<u8>,
    /// `Retry-After`（秒）。policy が `honor_capped` のときだけ使う
    pub retry_after_secs: Option<u32>,
}
/// api_key を含まない正規化済みの request。実 transport は秘密をここに足して送る（この module は実装しない）
#[derive(Debug, Clone)]
pub struct CanonReq {
    pub path: &'static str,
    pub params: BTreeMap<String, String>,
}
impl CanonReq {
    fn to_json(&self) -> Value {
        json!({"method": "GET", "path": self.path, "params": self.params})
    }
}
pub trait Transport {
    fn get(&mut self, req: &CanonReq) -> Result<HttpResp, NetErr>;
    /// policy が決めた待ち時間。実 transport は sleep する。mock は記録するだけ（既定は何もしない）
    fn pause(&mut self, _ms: u64) {}
}

/// transport policy（manifest で pin。結果を見て変更しない）。generator はこの値だけで再試行・待ち・並列度を決める
#[derive(Debug, Clone)]
pub struct TransportPolicy {
    pub max_attempts: usize,
    pub backoff_ms: Vec<u64>,
    pub retry_statuses: Vec<u16>,
    pub honor_retry_after: bool,
    pub retry_after_cap_ms: u64,
}
impl TransportPolicy {
    /// `manifest.transport_policy` を検査する。`policy_sha256` は policy 本体（その項目を除く）の canonical sha256 と一致しなければならない
    pub fn from_manifest(m: &Value) -> Result<TransportPolicy, Stop> {
        let p = &m["transport_policy"];
        let mut body = p.clone();
        let want = body.as_object_mut().and_then(|o| o.remove("policy_sha256")).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        if body.as_object().is_none() || sha_v(&body)? != want {
            return stop("transport_policy_sha256 が policy 本体と一致しません");
        }
        if body["concurrency"] != json!(1) {
            return stop("並列度は 1 に固定です（generator は逐次実行）");
        }
        if !matches!(body["retry_after"].as_str(), Some("ignore") | Some("honor_capped")) {
            return stop("retry_after は ignore | honor_capped");
        }
        let max_attempts = body["max_attempts"].as_u64().filter(|n| (1..=5).contains(n)).ok_or_else(|| Stop("max_attempts は 1..=5".into()))? as usize;
        let backoff_ms: Vec<u64> = body["backoff_ms"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).collect()).unwrap_or_default();
        if backoff_ms.len() + 1 < max_attempts {
            return stop("backoff_ms が足りません（max_attempts - 1 個以上）");
        }
        let retry_statuses: Vec<u16> = body["retry_statuses"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64().map(|n| n as u16)).collect()).unwrap_or_default();
        Ok(TransportPolicy {
            max_attempts,
            backoff_ms,
            retry_statuses,
            honor_retry_after: body["retry_after"] == json!("honor_capped"),
            retry_after_cap_ms: body["retry_after_cap_ms"].as_u64().unwrap_or(0),
        })
    }
    fn wait_ms(&self, n: usize, retry_after_secs: Option<u32>) -> u64 {
        let base = self.backoff_ms.get(n - 1).copied().unwrap_or(0);
        match (self.honor_retry_after, retry_after_secs) {
            (true, Some(s)) => base.max((s as u64 * 1000).min(self.retry_after_cap_ms)),
            _ => base,
        }
    }
}

// ─── query / derive（generator と replay が共有） ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CallSpec {
    pub input_index: usize,
    pub source: &'static str,
    pub kind: &'static str,
    pub variant: &'static str,
    pub query: String,
    pub year: Option<i32>,
}
impl CallSpec {
    fn canon_req(&self) -> CanonReq {
        let mut params = BTreeMap::new();
        params.insert("include_adult".to_string(), "false".to_string());
        params.insert("language".to_string(), "ja-JP".to_string());
        params.insert("query".to_string(), self.query.clone());
        let path = if self.kind == "movie" { "/3/search/movie" } else { "/3/search/tv" };
        if let Some(y) = self.year {
            params.insert(if self.kind == "movie" { "year" } else { "first_air_date_year" }.to_string(), y.to_string());
        }
        CanonReq { path, params }
    }
    fn request_id(&self, key: &str) -> String {
        format!("{key}#{}.{}.{}", self.input_index, self.kind, self.variant)
    }
}

pub enum Items {
    Movies(Vec<TmdbSearchMovie>),
    Tv(Vec<TmdbSearchTv>),
}

fn parse_items(kind: &str, body: &[u8]) -> Result<Items, ()> {
    if kind == "movie" {
        serde_json::from_slice::<TmdbSearchResponse<TmdbSearchMovie>>(body).map(|r| Items::Movies(r.results)).map_err(|_| ())
    } else {
        serde_json::from_slice::<TmdbSearchResponse<TmdbSearchTv>>(body).map(|r| Items::Tv(r.results)).map_err(|_| ())
    }
}

/// 実行予定の call の一覧（`fetch_candidates` の枝分かれと同じ。TMDB には触れない）
pub fn planned_specs(evidence: &LocalEvidence) -> Vec<CallSpec> {
    if skips_tmdb(evidence) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, input) in evidence.search_inputs().iter().enumerate() {
        let plan = query_plan_for(input, evidence.media_kind());
        let source = input.source.as_str();
        let mk = |kind: &'static str, variant: &'static str, year: Option<i32>| CallSpec { input_index: i, source, kind, variant, query: plan.query.clone(), year };
        if plan.search_movie {
            out.push(mk("movie", "primary", plan.year));
            if plan.retry_without_year {
                out.push(mk("movie", "retry_without_year", None));
            }
        }
        if plan.search_tv {
            out.push(mk("tv", "primary", plan.year));
        }
    }
    out
}

pub struct Derived {
    pub legacy_ranked: Vec<TmdbCandidate>,
    pub legacy_all: Vec<TmdbCandidate>,
    pub combined_ranked: Vec<TmdbCandidate>,
    pub combined_all: Vec<TmdbCandidate>,
    pub sources: Vec<CandidateSource>,
    pub parsed: ParsedTitle,
    /// `combined_all` と同じ順の、各 candidate の **元になった凍結 response の日付**（verbatim）。reviewer projection の release_date の authority
    pub combined_dates: Vec<Option<String>>,
}

/// `fetch_candidates` を写したもの。違いは、失敗した call を握りつぶさず `Err` で返すこと（year なし再検索を含む）
pub fn derive(evidence: &LocalEvidence, fetch: &mut dyn FnMut(&CallSpec) -> Result<Items, String>) -> Result<Derived, String> {
    let mut legacy_all: Vec<TmdbCandidate> = Vec::new();
    let mut embedded_all: Vec<TmdbCandidate> = Vec::new();
    let mut legacy_parsed: Option<ParsedTitle> = None;
    // bucket と同じ並びで、candidate を作った response の日付を持つ
    let mut legacy_dates: Vec<Option<String>> = Vec::new();
    let mut embedded_dates: Vec<Option<String>> = Vec::new();
    if !skips_tmdb(evidence) {
        let specs = planned_specs(evidence);
        let inputs = evidence.search_inputs();
        for (i, input) in inputs.iter().enumerate() {
            let parsed = parsed_for_query(input);
            let (bucket, bucket_dates): (&mut Vec<TmdbCandidate>, &mut Vec<Option<String>>) = match input.source {
                QuerySource::Legacy => (&mut legacy_all, &mut legacy_dates),
                QuerySource::Embedded => (&mut embedded_all, &mut embedded_dates),
            };
            for spec in specs.iter().filter(|s| s.input_index == i) {
                match fetch(spec)? {
                    Items::Movies(rs) => {
                        for r in &rs {
                            let scored = score_movie(r, &parsed);
                            if spec.variant == "primary" || !bucket.iter().any(|c| c.tmdb_id == scored.tmdb_id) {
                                bucket.push(scored);
                                bucket_dates.push(r.release_date.clone());
                            }
                        }
                    }
                    Items::Tv(rs) => {
                        for r in &rs {
                            bucket.push(score_tv(r, &parsed));
                            bucket_dates.push(r.first_air_date.clone());
                        }
                    }
                }
            }
            if input.source == QuerySource::Legacy {
                legacy_parsed = Some(parsed);
            }
        }
    }
    let parsed = legacy_parsed.unwrap_or_else(|| parse_title(evidence.title()));
    let (combined_all, sources) = merge_candidate_sets(&legacy_all, &embedded_all);
    // merge は「legacy を先に置き、embedded の confidence が **厳密に高いときだけ** 置き換える」。同じ規則で、採用された元の response を特定する
    let find = |bucket: &[TmdbCandidate], dates: &[Option<String>], c: &TmdbCandidate| {
        bucket.iter().position(|b| same(b, c) && b.confidence == c.confidence && b.title == c.title && b.year == c.year).map(|i| dates[i].clone())
    };
    let combined_dates: Vec<Option<String>> = combined_all
        .iter()
        .map(|c| find(&legacy_all, &legacy_dates, c).or_else(|| find(&embedded_all, &embedded_dates, c)).unwrap_or(None))
        .collect();
    Ok(Derived {
        legacy_ranked: merge_and_rank(legacy_all.clone()),
        legacy_all,
        combined_ranked: merge_and_rank(combined_all.clone()),
        combined_all,
        sources,
        parsed,
        combined_dates,
    })
}

// ─── record bodies（generator と replay で同じ関数） ────────────────────────────────────────────

fn ident(c: &TmdbCandidate) -> Value {
    json!({"media_type": c.media_type, "tmdb_id": c.tmdb_id})
}
fn same(a: &TmdbCandidate, b: &TmdbCandidate) -> bool {
    a.tmdb_id == b.tmdb_id && a.media_type == b.media_type
}

fn candidate_list(all: &[TmdbCandidate], ranked: &[TmdbCandidate], sources: Option<&[CandidateSource]>) -> Result<Value, Stop> {
    let all_j: Vec<Value> = all
        .iter()
        .enumerate()
        .map(|(ord, c)| {
            let qs = sources
                .and_then(|s| s.iter().find(|s| s.tmdb_id == c.tmdb_id && s.media_type == c.media_type))
                .map(|s| s.query_source.clone());
            json!({"ord": ord, "media_type": c.media_type, "tmdb_id": c.tmdb_id, "title": c.title, "original_title": c.original_title,
                   "year": c.year, "original_language": c.original_language, "confidence": c.confidence, "reasons": c.reasons, "query_source": qs})
        })
        .collect();
    let ranked_j: Vec<Value> = ranked
        .iter()
        .enumerate()
        .map(|(r, c)| json!({"final_rank": r + 1, "ord": all.iter().position(|a| same(a, c))}))
        .collect();
    let mut v = json!({"all": all_j, "ranked": ranked_j});
    v["candidate_list_sha256"] = json!(sha_v(&v)?);
    Ok(v)
}

fn candidate_set_body(key: &str, d: &Derived) -> Result<Value, Stop> {
    Ok(json!({"evaluation_unit_key": key, "ordering_rule": ORDERING_RULE, "threshold": THRESHOLD_CANDIDATE, "cap": RANK_CAP,
              "legacy": candidate_list(&d.legacy_all, &d.legacy_ranked, None)?,
              "combined": candidate_list(&d.combined_all, &d.combined_ranked, Some(&d.sources))?}))
}

fn matcher_json(o: &SafeMatchOutcome, set: &str, list_sha: &str) -> Value {
    json!({"decision": o.decision.as_str(), "top": o.top.map(ident), "top_confidence": o.top.map(|c| c.confidence).unwrap_or(0),
           "reasons": o.reasons, "input_candidate_set": set, "input_candidate_list_sha256": list_sha})
}

/// reviewer へ渡す候補集合。**rank / score / source / machine decision は含めない**。順序も rank を漏らさないよう (media_type, tmdb_id) 昇順。
/// 内容は binding matcher が判定に使った凍結済み combined-ranked 集合そのもの（PF0G が TMDB を再 fetch して作ることは禁止）
fn projection_body(key: &str, d: &Derived) -> Result<Value, Stop> {
    let mut items: Vec<&TmdbCandidate> = d.combined_ranked.iter().collect();
    items.sort_by(|a, b| (a.media_type.as_str(), a.tmdb_id).cmp(&(b.media_type.as_str(), b.tmdb_id)));
    let cands: Vec<Value> = items
        .iter()
        .map(|c| {
            // release_date は凍結 response の値そのまま、year は既存ロジック（score_*）が同じ response から導いた値。両方保存し、食い違っても除外しない
            let release_date = d.combined_all.iter().position(|a| same(a, c)).and_then(|i| d.combined_dates[i].clone());
            let derived_year = release_date.as_deref().and_then(|s| s.get(..4)).and_then(|y| y.parse::<i32>().ok());
            json!({"tmdb_id": c.tmdb_id, "media_type": c.media_type, "title": c.title, "original_title": c.original_title, "year": c.year,
                   "release_date": release_date, "release_year_from_date": derived_year, "year_consistent_with_release_date": derived_year == c.year,
                   "overview": c.overview, "poster_path": c.poster_path})
        })
        .collect();
    let shown = sha_v(&Value::Array(cands.clone()))?;
    Ok(json!({"evaluation_unit_key": key, "candidates": cands, "shown_set_sha256": shown}))
}

fn embedded_json(e: &crate::services::metadata_matcher::EmbeddedEvidence) -> Value {
    json!({"embedded_year": e.embedded_year, "filename_year": e.filename_year, "audio_languages": e.audio_languages,
           "cut_editions": e.cut_editions, "episode_id_marks_movie": e.episode_id_marks_movie})
}

/// (matcher_output body, projection body, candidate_set body)。成功した derive から作る
fn bodies_ok(key: &str, d: &Derived, e: &crate::services::metadata_matcher::EmbeddedEvidence, skipped: bool) -> Result<(Value, Value, Value), Stop> {
    let cs = candidate_set_body(key, d)?;
    let safe = rules_safe_with_embedded(&d.legacy_ranked, &d.parsed, e);
    let shadow = rules_tags_shadow_best_candidate(&d.combined_ranked, &d.parsed, e);
    let legacy_sha = cs["legacy"]["candidate_list_sha256"].as_str().unwrap_or_default().to_string();
    let comb_sha = cs["combined"]["candidate_list_sha256"].as_str().unwrap_or_default().to_string();
    let (kind, identity) = match (shadow.decision, shadow.top) {
        (SafeDecision::Auto, Some(c)) => ("AUTO", Some(ident(c))),
        (SafeDecision::Auto, None) => ("AUTO_INVALID", None),
        (SafeDecision::Review, _) => ("REVIEW", None),
        (SafeDecision::Unresolved, _) => ("UNRESOLVED", None),
    };
    let status = if kind == "AUTO_INVALID" {
        "invalid_output"
    } else if skipped {
        "skipped_no_title"
    } else {
        "ok"
    };
    let mut machine = json!({"kind": kind});
    if let Some(i) = identity {
        machine["identity"] = i;
    }
    let out = json!({"evaluation_unit_key": key, "technical_status": status, "retryable": false,
        "binding_matcher": BINDING_MATCHER, "reference_matcher": REFERENCE_MATCHER,
        "embedded_evidence": embedded_json(e), "parsed_year_hint": d.parsed.year_hint,
        "rules_safe": matcher_json(&safe, "legacy", &legacy_sha),
        "rules_tags_shadow": matcher_json(&shadow, "combined", &comb_sha),
        "pf0b_machine": machine});
    Ok((out, projection_body(key, d)?, cs))
}

fn body_failed(key: &str, status: &str, retryable: bool, detail: Value) -> Value {
    json!({"evaluation_unit_key": key, "technical_status": status, "retryable": retryable, "binding_matcher": BINDING_MATCHER,
           "reference_matcher": REFERENCE_MATCHER, "failure": detail, "pf0b_machine": {"kind": "MISSING"}})
}

// ─── chain files ────────────────────────────────────────────────────────────────────────────────

fn commitment(rec: &Value, prev: &str) -> Result<String, Stop> {
    Ok(sha(format!("{}\n{}", canon(rec)?, prev).as_bytes()))
}

/// 1 ファイルの hash chain を検証して records を返す。無ければ空
pub fn read_chain(path: &Path) -> Result<(Vec<Value>, String), Stop> {
    let mut recs = Vec::new();
    let mut prev = GENESIS.to_string();
    if !path.exists() {
        return Ok((recs, prev));
    }
    let text = std::fs::read_to_string(path).map_err(io)?;
    for (i, line) in text.lines().enumerate() {
        let mut v: Value = serde_json::from_str(line).map_err(io)?;
        let c = v.get("record_commitment").and_then(|x| x.as_str()).unwrap_or_default().to_string();
        v.as_object_mut().ok_or_else(|| Stop("record が object ではありません".into()))?.remove("record_commitment");
        if v["seq"] != json!(i + 1) || v["prev_commitment"] != json!(prev) || commitment(&v, &prev)? != c {
            return stop(format!("{}: hash chain が壊れています（seq {}）", path.display(), i + 1));
        }
        v["record_commitment"] = json!(c);
        prev = c;
        recs.push(v);
    }
    Ok((recs, prev))
}

fn scan_secret(v: &Value, probe: Option<&str>) -> Result<(), Stop> {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                if FORBIDDEN_KEYS.iter().any(|f| k.to_ascii_lowercase().contains(f)) {
                    return stop("secret_scan_hit: 禁止キー");
                }
                scan_secret(x, probe)?;
            }
        }
        Value::Array(a) => a.iter().try_for_each(|x| scan_secret(x, probe))?,
        Value::String(s) => {
            if probe.map(|p| !p.is_empty() && s.contains(p)).unwrap_or(false) || s.to_ascii_lowercase().contains("api_key=") {
                return stop("secret_scan_hit: 値");
            }
        }
        _ => {}
    }
    Ok(())
}

// ─── unit input ─────────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct UnitIn {
    pub key: String,
    /// ledger の matcher_input_sha256
    pub matcher_input_sha256: String,
    pub payload: Value,
    /// ledger の enrollment_context.decision_evidence_sha256（あれば照合する）
    pub expected_decision_evidence_sha256: Option<String>,
}

fn work_id(key: &str) -> Result<i64, Stop> {
    key.strip_prefix("work:").and_then(|n| n.parse().ok()).ok_or_else(|| Stop(format!("evaluation_unit_key が不正: {key}")))
}

#[derive(Debug, Clone, PartialEq)]
struct UnitState {
    attempt: u32,
    status: String,
    retryable: bool,
}

// ─── generator ──────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Begun,
    PrimaryDone,
    RecoverySetCommitted,
    RecoveryDone,
    Sealed,
}

pub struct Generator {
    dir: PathBuf,
    manifest: Value,
    manifest_sha: String,
    units: Vec<UnitIn>,
    chains: BTreeMap<String, (u64, String)>,
    states: BTreeMap<String, UnitState>,
    clock: Box<dyn FnMut() -> String>,
    probe: Option<String>,
    started: String,
    phase: Phase,
    recovery_enabled: bool,
    policy: TransportPolicy,
    unit_inputs_written: bool,
    resumes: u64,
}

impl std::fmt::Debug for Generator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Generator({:?})", self.phase)
    }
}

fn valid_time(t: &str) -> bool {
    t.len() == 20 && t.ends_with('Z') && t.as_bytes()[10] == b'T'
}

impl Generator {
    /// manifest と unit 集合の検査（begin / resume 共通）。TMDB にも filesystem にも触れない
    fn validate(manifest: &Value, units: &[UnitIn], probe: Option<&str>) -> Result<TransportPolicy, Stop> {
        if manifest["contract_version"] != json!(CONTRACT_VERSION) {
            return stop("contract_version が違います");
        }
        if manifest["binding_matcher"] != json!(BINDING_MATCHER) || manifest["reference_matcher"] != json!(REFERENCE_MATCHER) {
            return stop("binding は rules-tags-shadow-2、reference は rules-safe-2 でなければなりません");
        }
        if manifest["rules_1_in_final"] != json!(false) {
            return stop("rules-1 は FINAL 対象外です（rules_1_in_final=false が必要）");
        }
        if !manifest["code_commit"].as_str().map(|c| c.len() == 40 && c.bytes().all(|b| b.is_ascii_hexdigit())).unwrap_or(false) {
            return stop("code_commit が 40 hex ではありません");
        }
        let m = &manifest["membership"];
        let ver = m["eligibility_rule_version"].as_str().ok_or_else(|| Stop("membership.eligibility_rule_version がありません".into()))?;
        let want = m["commitment"].as_str().ok_or_else(|| Stop("membership.commitment がありません".into()))?;
        if m["pf0h_final_artifact_sha256"].as_str().map(|s| s.len() != 64).unwrap_or(true) {
            return stop("PF0H 最終成果物の sha256 が pin されていません");
        }
        let keys: Vec<String> = units.iter().map(|u| u.key.clone()).collect();
        if keys.iter().collect::<BTreeSet<_>>().len() != keys.len() {
            return stop("unit key が重複しています");
        }
        // authority は PF0B の定義だけ。PF0H の unit_set_sha256 / payload_set_sha256 は補助値で、ここでは見ない
        if membership_commitment(&keys, ver) != want {
            return stop("membership commitment（PF0B 定義）が一致しません");
        }
        for sec in ["pf0h_unit_set_sha256_aux", "payload_set_sha256_aux"] {
            if m.get(sec).is_some() && m[sec].as_str().map(|s| s.len() != 64).unwrap_or(true) {
                return stop("補助 hash の形式が不正です");
            }
        }
        // unit の処理順は事前 commit（resume が「未実行 unit だけ」を証明するための基準）
        if manifest["unit_order_sha256"] != json!(sha_v(&json!(keys))?) {
            return stop("unit_order_sha256 が unit の処理順と一致しません");
        }
        // resume の自由度を残さない: primary は最大 1 回、recovery は resume 禁止（fail-closed）
        if manifest["resume_policy"]["max_primary_resumes"] != json!(1) || manifest["resume_policy"]["recovery_resume"] != json!("forbidden") {
            return stop("resume_policy は max_primary_resumes=1 かつ recovery_resume=forbidden に固定です");
        }
        let policy = TransportPolicy::from_manifest(manifest)?;
        scan_secret(manifest, probe)?;
        Ok(policy)
    }

    /// manifest を検査して固定し、排他 lock を取る。**TMDB はまだ呼ばない**
    pub fn begin(dir: &Path, manifest: Value, units: Vec<UnitIn>, mut clock: Box<dyn FnMut() -> String>, probe: Option<String>) -> Result<Generator, Stop> {
        let policy = Self::validate(&manifest, &units, probe.as_deref())?;
        std::fs::create_dir_all(dir).map_err(io)?;
        if dir.join("ARTIFACT_INDEX.json").exists() {
            return stop("この run は封印済みです");
        }
        std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join("RUN.lock")).map_err(|_| Stop("RUN.lock が既に存在します（1 manifest = 1 session）".into()))?;
        let text = canon(&manifest)?;
        std::fs::write(dir.join("MANIFEST.json"), &text).map_err(io)?;
        let started = clock();
        if !valid_time(&started) {
            return stop("時刻の形式が不正です");
        }
        let recovery_enabled = manifest["recovery"]["mode"] == json!("retryable_failed_units_once");
        let mut g = Generator {
            dir: dir.to_path_buf(),
            manifest_sha: sha(text.as_bytes()),
            manifest,
            units,
            chains: BTreeMap::new(),
            states: BTreeMap::new(),
            clock,
            probe,
            started: started.clone(),
            phase: Phase::Begun,
            recovery_enabled,
            policy,
            unit_inputs_written: false,
            resumes: 0,
        };
        g.log("run_start", None, None)?;
        Ok(g)
    }

    /// primary の途中 crash からの再開。**同じ manifest・同じ unit 順序の同一 logical run** であることを証明できたときだけ、
    /// 完了済み unit を再実行せず、未実行 unit だけを続ける。証明できなければ `CONTINUITY_FAILURE.json` を残して Err（= FINAL は TECHNICAL FAILURE）。
    /// 対象は primary のみ。recovery 対象集合の確定後・封印後は再開不可（GT 開始前・seal 前に限る）。
    pub fn resume(dir: &Path, manifest: Value, units: Vec<UnitIn>, mut clock: Box<dyn FnMut() -> String>, probe: Option<String>) -> Result<Generator, Stop> {
        let policy = Self::validate(&manifest, &units, probe.as_deref())?;
        // 封印済みの artifact には何も書かない（index 外のファイルを増やさない）
        if dir.join("ARTIFACT_INDEX.json").exists() {
            return stop("封印済みの run は再開できません");
        }
        let fail = |why: &str| -> Stop {
            let _ = std::fs::write(dir.join("CONTINUITY_FAILURE.json"), canon(&json!({"reason": why})).unwrap_or_default());
            Stop(format!("continuity を証明できません（TECHNICAL FAILURE）: {why}"))
        };
        if dir.join("ARTIFACT_INDEX.json").exists() || dir.join("recovery_set.json").exists() {
            return Err(fail("recovery フェーズ（対象集合の確定後）は resume 禁止: 中断は CONTINUITY_FAILURE"));
        }
        if !dir.join("RUN.lock").exists() || dir.join("CONTINUITY_FAILURE.json").exists() {
            return Err(fail("RUN.lock が無い、または既に continuity failure"));
        }
        let text = canon(&manifest)?;
        if std::fs::read_to_string(dir.join("MANIFEST.json")).map_err(io)? != text {
            return Err(fail("manifest が元の run と違う"));
        }
        let mut chains = BTreeMap::new();
        let mut recs: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
        for f in CHAIN_FILES {
            let (r, head) = read_chain(&dir.join(f)).map_err(|e| fail(&e.0))?;
            chains.insert(f.to_string(), (r.len() as u64, head));
            recs.insert(f, r);
        }
        if !recs[F_EVAL].is_empty() || !recs[F_PROJ].iter().all(|r| r["attempt"] == json!(1)) {
            return Err(fail("primary 以外の記録がある"));
        }
        let first = recs[F_LOG].first().ok_or_else(|| fail("run_log が空"))?;
        if first["event"] != json!("run_start") {
            return Err(fail("run_start が先頭にない"));
        }
        let started = first["at_utc"].as_str().unwrap_or_default().to_string();
        let keys: Vec<String> = units.iter().map(|u| u.key.clone()).collect();
        // unit_input は全 unit 分が先に書かれる。一部だけなら continuity 不明
        let ui: Vec<&str> = recs[F_UNIT].iter().map(|r| r["evaluation_unit_key"].as_str().unwrap_or_default()).collect();
        let unit_inputs_written = !ui.is_empty();
        if unit_inputs_written && ui != keys.iter().map(String::as_str).collect::<Vec<_>>() {
            return Err(fail("unit_input が manifest の unit 順序と一致しない（途中書き込み）"));
        }
        // 完了済み = matcher_output がある unit。manifest 順序の **先頭からの連続した prefix** でなければならない
        let done: Vec<&str> = recs[F_OUT].iter().map(|r| r["evaluation_unit_key"].as_str().unwrap_or_default()).collect();
        if done.len() > keys.len() || done.iter().zip(keys.iter()).any(|(d, k)| d != k) {
            return Err(fail("完了済み unit が manifest 順序の prefix ではない"));
        }
        // 完了していない unit の痕跡（request / candidate / projection）が残っていれば、再実行すると二重になるので証明不能
        let doneset: BTreeSet<&str> = done.iter().copied().collect();
        for f in [F_REQ, F_CAND, F_PROJ] {
            if recs[f].iter().any(|r| !doneset.contains(r["evaluation_unit_key"].as_str().unwrap_or_default())) {
                return Err(fail("未完了 unit の痕跡が残っている"));
            }
        }
        let mut states = BTreeMap::new();
        for r in &recs[F_OUT] {
            states.insert(
                r["evaluation_unit_key"].as_str().unwrap_or_default().to_string(),
                UnitState { attempt: 1, status: r["technical_status"].as_str().unwrap_or_default().to_string(), retryable: r["retryable"] == json!(true) },
            );
        }
        let max = manifest["resume_policy"]["max_primary_resumes"].as_u64().unwrap_or(0);
        let resumes = recs[F_LOG].iter().filter(|r| r["event"] == json!("resume_primary")).count() as u64 + 1;
        if resumes > max {
            return Err(fail("再開回数が manifest の上限を超える"));
        }
        let now = clock();
        if !valid_time(&now) {
            return Err(fail("時刻の形式が不正"));
        }
        let recovery_enabled = manifest["recovery"]["mode"] == json!("retryable_failed_units_once");
        let mut g = Generator {
            dir: dir.to_path_buf(),
            manifest_sha: sha(text.as_bytes()),
            manifest,
            units,
            chains,
            states,
            clock,
            probe,
            started,
            phase: Phase::Begun,
            recovery_enabled,
            policy,
            unit_inputs_written,
            resumes,
        };
        g.log("resume_primary", None, None)?;
        Ok(g)
    }

    fn append(&mut self, file: &str, mut rec: Value) -> Result<(), Stop> {
        scan_secret(&rec, self.probe.as_deref())?;
        let e = self.chains.entry(file.to_string()).or_insert((0, GENESIS.to_string()));
        rec["seq"] = json!(e.0 + 1);
        rec["prev_commitment"] = json!(e.1);
        let c = commitment(&rec, &e.1)?;
        rec["record_commitment"] = json!(c);
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join(file)).map_err(io)?;
        writeln!(f, "{}", canon(&rec)?).map_err(io)?;
        *e = (e.0 + 1, c);
        Ok(())
    }

    fn log(&mut self, event: &str, key: Option<&str>, kind: Option<&str>) -> Result<(), Stop> {
        let at = (self.clock)();
        let mut r = json!({"at_utc": at, "event": event});
        if let Some(k) = key {
            r["evaluation_unit_key"] = json!(k);
        }
        if let Some(k) = kind {
            r["failure_kind"] = json!(k);
        }
        self.append(F_LOG, r)
    }

    fn store_raw(&self, body: &[u8]) -> Result<String, Stop> {
        let s = sha(body);
        let d = self.dir.join("raw");
        std::fs::create_dir_all(&d).map_err(io)?;
        let p = d.join(format!("{s}.bin"));
        if !p.exists() {
            if let Some(probe) = self.probe.as_deref() {
                if !probe.is_empty() && String::from_utf8_lossy(body).contains(probe) {
                    return stop("secret_scan_hit: raw body");
                }
            }
            std::fs::write(&p, body).map_err(io)?;
        }
        Ok(s)
    }

    pub fn run_primary(&mut self, transport: &mut dyn Transport) -> Result<(), Stop> {
        if self.phase != Phase::Begun {
            return stop("primary は 1 回だけです");
        }
        let units = self.units.clone();
        if !self.unit_inputs_written {
            for u in &units {
                self.write_unit_input(u)?;
            }
            self.unit_inputs_written = true;
        }
        for u in &units {
            if self.states.contains_key(&u.key) {
                continue; // resume: 完了済み unit は再実行しない
            }
            self.process_unit(u, 1, transport)?;
        }
        self.phase = Phase::PrimaryDone;
        Ok(())
    }

    fn write_unit_input(&mut self, u: &UnitIn) -> Result<(), Stop> {
        let rec = match self.snapshot(u) {
            Ok(snap) => {
                let ev = snap.local_evidence();
                let (l, e, d) = evidence_hashes(&snap)?;
                let specs = planned_specs(&ev);
                let plans: Vec<Value> = specs.iter().map(|s| json!({"request_id": s.request_id(&u.key), "source": s.source, "kind": s.kind, "variant": s.variant, "query": s.query, "year": s.year})).collect();
                json!({"evaluation_unit_key": u.key, "matcher_input_sha256": u.matcher_input_sha256, "payload_match": true,
                       "evidence_hashes": {"local": l, "embedded": e, "decision": d},
                       "derived": {"media_kind": ev.media_kind(), "skips_tmdb": skips_tmdb(&ev),
                                   "search_inputs": ev.search_inputs().iter().enumerate().map(|(i, s)| json!({"input_index": i, "source": s.source.as_str(), "title": s.title, "year": s.year})).collect::<Vec<_>>()},
                       "planned_calls": plans, "planned_logical_calls": planned_logical_calls(&ev)})
            }
            Err(why) => json!({"evaluation_unit_key": u.key, "matcher_input_sha256": u.matcher_input_sha256, "payload_match": false, "mismatch": why}),
        };
        self.append(F_UNIT, rec)
    }

    /// payload を検査して snapshot にする。不一致は理由（短い固定文字列）を返す
    fn snapshot(&self, u: &UnitIn) -> Result<crate::services::prematch_snapshot::PreMatchSnapshot, String> {
        let sha_now = sha_v(&u.payload).map_err(|e| e.0)?;
        if sha_now != u.matcher_input_sha256 {
            return Err("matcher_input_sha256".into());
        }
        let wid = work_id(&u.key).map_err(|e| e.0)?;
        let snap = snapshot_from_payload(&u.payload, wid, EvidenceClass::Live).map_err(|e| e.0)?;
        if let Some(want) = &u.expected_decision_evidence_sha256 {
            if &evidence_hashes(&snap).map_err(|e| e.0)?.2 != want {
                return Err("decision_evidence_sha256".into());
            }
        }
        Ok(snap)
    }

    fn process_unit(&mut self, u: &UnitIn, attempt: u32, transport: &mut dyn Transport) -> Result<(), Stop> {
        let key = u.key.clone();
        self.log("unit_start", Some(&key), None)?;
        let snap = match self.snapshot(u) {
            Ok(s) => s,
            Err(_) => {
                return self.finish_failed(&key, attempt, "input_mismatch", false, json!({"kind": "input_mismatch"}));
            }
        };
        let ev = snap.local_evidence();
        let embedded = snap.embedded_evidence();
        let policy = self.policy.clone();
        let mut failure: Option<(&'static str, bool)> = None;
        let mut pending: Vec<Value> = Vec::new();
        let mut raw_to_store: Vec<Vec<u8>> = Vec::new();
        let result = {
            let mut fetch = |spec: &CallSpec| -> Result<Items, String> {
                let req = spec.canon_req();
                let mut attempts = Vec::new();
                let mut got: Result<HttpResp, NetErr> = Err(NetErr::Other);
                for n in 1..=policy.max_attempts {
                    match transport.get(&req) {
                        Ok(r) => {
                            let mut a = json!({"n": n, "outcome": "response", "http_status": r.status});
                            if let Some(ra) = r.retry_after_secs {
                                a["retry_after_secs"] = json!(ra);
                            }
                            let again = policy.retry_statuses.contains(&r.status) && n < policy.max_attempts;
                            if again {
                                let ms = policy.wait_ms(n, r.retry_after_secs);
                                a["backoff_ms"] = json!(ms);
                                transport.pause(ms);
                            }
                            attempts.push(a);
                            got = Ok(r);
                            if !again {
                                break;
                            }
                        }
                        Err(e) => {
                            let mut a = json!({"n": n, "outcome": e.as_str()});
                            if n < policy.max_attempts {
                                let ms = policy.wait_ms(n, None);
                                a["backoff_ms"] = json!(ms);
                                transport.pause(ms);
                            }
                            attempts.push(a);
                            got = Err(e);
                        }
                    }
                }
                let mut rec = json!({"evaluation_unit_key": key, "attempt": attempt, "request_id": spec.request_id(&key), "input_index": spec.input_index,
                    "source": spec.source, "kind": spec.kind, "variant": spec.variant,
                    "canonical_request": req.to_json(), "canonical_request_sha256": sha_v(&req.to_json()).map_err(|e| e.0)?, "attempts": attempts});
                let out = match got {
                    Err(_) => {
                        rec["final_status"] = json!("network_error");
                        failure = Some(("network_error", true));
                        Err("network_error".to_string())
                    }
                    Ok(r) if r.status != 200 => {
                        rec["final_status"] = json!("http_error");
                        rec["evidence"] = json!({"http_status": r.status, "body_bytes": r.body.len(), "body_sha256": sha(&r.body)});
                        failure = Some(("http_error", r.status == 429 || r.status >= 500));
                        Err("http_error".to_string())
                    }
                    Ok(r) => match parse_items(spec.kind, &r.body) {
                        Err(()) => {
                            rec["final_status"] = json!("decode_error");
                            rec["evidence"] = json!({"http_status": r.status, "body_bytes": r.body.len(), "body_sha256": sha(&r.body)});
                            failure = Some(("decode_error", false));
                            Err("decode_error".to_string())
                        }
                        Ok(items) => {
                            rec["final_status"] = json!("ok");
                            rec["raw_body_sha256"] = json!(sha(&r.body));
                            rec["raw_body_bytes"] = json!(r.body.len());
                            raw_to_store.push(r.body);
                            Ok(items)
                        }
                    },
                };
                pending.push(rec);
                out
            };
            derive(&ev, &mut fetch)
        };
        for body in &raw_to_store {
            self.store_raw(body)?;
        }
        for rec in pending {
            self.append(F_REQ, rec)?;
        }
        match result {
            Err(_) => {
                let (kind, retryable) = failure.unwrap_or(("internal_error", false));
                let status = if kind == "decode_error" { "decode_error" } else { "call_failure" };
                self.finish_failed(&key, attempt, status, retryable, json!({"kind": kind}))
            }
            Ok(d) => {
                let (mut out, proj, cs) = bodies_ok(&key, &d, &embedded, skips_tmdb(&ev))?;
                let status = out["technical_status"].as_str().unwrap_or("ok").to_string();
                out["attempt"] = json!(attempt);
                let (mut proj, mut cs) = (proj, cs);
                proj["attempt"] = json!(attempt);
                cs["attempt"] = json!(attempt);
                self.append(F_CAND, cs)?;
                self.append(F_PROJ, proj)?;
                self.append(F_OUT, out)?;
                self.states.insert(key.clone(), UnitState { attempt, status, retryable: false });
                self.log("unit_done", Some(&key), None)
            }
        }
    }

    fn finish_failed(&mut self, key: &str, attempt: u32, status: &str, retryable: bool, detail: Value) -> Result<(), Stop> {
        let mut out = body_failed(key, status, retryable, detail);
        out["attempt"] = json!(attempt);
        self.append(F_OUT, out)?;
        self.states.insert(key.to_string(), UnitState { attempt, status: status.to_string(), retryable });
        self.log("unit_failed", Some(key), Some(status))
    }

    /// primary 終了後、retryable な失敗 unit だけを recovery 対象として **先に** commitment する。GT review 開始前でなければならない
    pub fn commit_recovery_set(&mut self) -> Result<Value, Stop> {
        if self.phase != Phase::PrimaryDone {
            return stop("recovery 対象の確定は primary の直後に 1 回だけです");
        }
        let keys: Vec<String> = if self.recovery_enabled {
            self.states.iter().filter(|(_, s)| s.retryable).map(|(k, _)| k.clone()).collect()
        } else {
            Vec::new()
        };
        let heads: BTreeMap<String, String> = self.chains.iter().map(|(k, v)| (k.clone(), v.1.clone())).collect();
        let body = json!({"manifest_sha256": self.manifest_sha, "primary_chain_heads": heads, "unit_keys": keys, "max_attempts": 2});
        let rec = json!({"set": body, "set_commitment": sha_v(&body)?});
        std::fs::write(self.dir.join("recovery_set.json"), canon(&rec)?).map_err(io)?;
        self.log("recovery_set_committed", None, None)?;
        self.phase = Phase::RecoverySetCommitted;
        Ok(rec)
    }

    /// 対象集合の unit だけを 1 回再実行する。成功済み unit には触れない
    pub fn run_recovery(&mut self, transport: &mut dyn Transport) -> Result<(), Stop> {
        if self.phase != Phase::RecoverySetCommitted {
            return stop("recovery は対象集合の commitment 後に 1 回だけです");
        }
        let text = std::fs::read_to_string(self.dir.join("recovery_set.json")).map_err(io)?;
        let rec: Value = serde_json::from_str(&text).map_err(io)?;
        if sha_v(&rec["set"])? != rec["set_commitment"].as_str().unwrap_or_default() {
            return stop("recovery_set が改ざんされています");
        }
        let keys: Vec<String> = rec["set"]["unit_keys"].as_array().cloned().unwrap_or_default().iter().filter_map(|k| k.as_str().map(str::to_string)).collect();
        self.log("resume_start", None, None)?;
        for k in keys {
            if !self.states.get(&k).map(|s| s.retryable && s.attempt == 1).unwrap_or(false) {
                return stop("recovery 対象が retryable な失敗 unit ではありません");
            }
            let u = self.units.iter().find(|u| u.key == k).cloned().ok_or_else(|| Stop("unit が見つかりません".into()))?;
            self.process_unit(&u, 2, transport)?;
        }
        self.phase = Phase::RecoveryDone;
        Ok(())
    }

    /// 封印。recovery を使わない run でも `commit_recovery_set` を（空集合で）通してから封印する
    pub fn seal(mut self, env: &BTreeMap<String, String>) -> Result<(), Stop> {
        if self.phase != Phase::RecoveryDone && !(self.phase == Phase::RecoverySetCommitted && self.recovery_set_empty()?) {
            return stop("primary → recovery 対象の確定 →（recovery）の後でなければ封印できません");
        }
        if self.dir.join("CONTINUITY_FAILURE.json").exists() {
            return stop("continuity failure のある run は封印できません");
        }
        let keys: Vec<String> = self.units.iter().map(|u| u.key.clone()).collect();
        for k in &keys {
            let st = self.states.get(k).cloned().ok_or_else(|| Stop("状態のない unit があります".into()))?;
            let (recs, _) = read_chain(&self.dir.join(F_OUT))?;
            let last = recs.iter().rev().find(|r| r["evaluation_unit_key"] == json!(k)).ok_or_else(|| Stop("matcher_output がありません".into()))?;
            self.append(F_EVAL, json!({"evaluation_unit_key": k, "machine": last["pf0b_machine"], "technical_status": st.status}))?;
        }
        let ended = (self.clock)();
        self.log("run_end", None, None)?;
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for s in self.states.values() {
            *counts.entry(s.status.clone()).or_default() += 1;
        }
        let env_ok: BTreeMap<&String, &String> = env
            .iter()
            .filter(|(k, _)| ["LANG", "TZ"].contains(&k.as_str()) && !FORBIDDEN_KEYS.iter().any(|f| k.to_ascii_lowercase().contains(f)))
            .collect();
        let prov = json!({"run_id": self.manifest["run_id"], "manifest_sha256": self.manifest_sha, "code_commit": self.manifest["code_commit"],
            "session": {"started_at_utc": self.started, "ended_at_utc": ended}, "env_allowlist": env_ok,
            "primary_resumes": self.resumes,
            "counts": {"units": keys.len(), "units_by_status": counts}});
        scan_secret(&prov, self.probe.as_deref())?;
        std::fs::write(self.dir.join("RUN_PROVENANCE.json"), canon(&prov)?).map_err(io)?;
        let mut files = BTreeMap::new();
        let mut names: Vec<String> = CHAIN_FILES.iter().map(|s| s.to_string()).collect();
        names.extend(["MANIFEST.json", "RUN_PROVENANCE.json", "recovery_set.json"].map(String::from));
        for n in names {
            let p = self.dir.join(&n);
            if p.exists() {
                files.insert(n, sha(&std::fs::read(&p).map_err(io)?));
            }
        }
        let mut raws = BTreeMap::new();
        if let Ok(rd) = std::fs::read_dir(self.dir.join("raw")) {
            for e in rd.flatten() {
                raws.insert(e.file_name().to_string_lossy().to_string(), sha(&std::fs::read(e.path()).map_err(io)?));
            }
        }
        let heads: BTreeMap<String, String> = self.chains.iter().map(|(k, v)| (k.clone(), v.1.clone())).collect();
        let idx = json!({"run_id": self.manifest["run_id"], "files": files, "raw": raws, "chain_heads": heads});
        std::fs::write(self.dir.join("ARTIFACT_INDEX.json"), canon(&idx)?).map_err(io)?;
        self.phase = Phase::Sealed;
        Ok(())
    }

    fn recovery_set_empty(&self) -> Result<bool, Stop> {
        let rec: Value = serde_json::from_str(&std::fs::read_to_string(self.dir.join("recovery_set.json")).map_err(io)?).map_err(io)?;
        Ok(rec["set"]["unit_keys"].as_array().map(|a| a.is_empty()).unwrap_or(false))
    }
}

// ─── replay / verify ────────────────────────────────────────────────────────────────────────────

fn strip(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        for k in ["seq", "prev_commitment", "record_commitment", "attempt"] {
            o.remove(k);
        }
    }
    v
}

#[derive(Debug, Clone, PartialEq)]
pub struct VerifyReport {
    pub units: usize,
    pub replayed_units: usize,
    pub failed_units: usize,
}

fn read_json(p: &Path) -> Result<Value, Stop> {
    serde_json::from_str(&std::fs::read_to_string(p).map_err(io)?).map_err(io)
}

/// 封印済み artifact を **offline** で検証・再導出する（TMDB には触れない）。`units` は凍結した payload 集合（evaluator が持つもの）
pub fn verify_and_replay(dir: &Path, units: &[UnitIn]) -> Result<VerifyReport, Stop> {
    let idx = read_json(&dir.join("ARTIFACT_INDEX.json")).map_err(|_| Stop("ARTIFACT_INDEX がありません（未封印）".into()))?;
    for (n, want) in idx["files"].as_object().ok_or_else(|| Stop("index が不正".into()))? {
        if sha(&std::fs::read(dir.join(n)).map_err(io)?) != want.as_str().unwrap_or_default() {
            return stop(format!("{n} の sha256 が index と違います"));
        }
    }
    for (n, want) in idx["raw"].as_object().ok_or_else(|| Stop("index が不正".into()))? {
        let b = std::fs::read(dir.join("raw").join(n)).map_err(io)?;
        if sha(&b) != want.as_str().unwrap_or_default() || n != &format!("{}.bin", sha(&b)) {
            return stop(format!("raw/{n} が改ざんされています"));
        }
    }
    let mut chains = BTreeMap::new();
    for f in CHAIN_FILES {
        let (recs, head) = read_chain(&dir.join(f))?;
        if idx["chain_heads"].get(f).map(|h| h != &json!(head)).unwrap_or(!recs.is_empty()) {
            return stop(format!("{f} の chain head が index と違います"));
        }
        chains.insert(f, recs);
    }
    let manifest_text = std::fs::read_to_string(dir.join("MANIFEST.json")).map_err(io)?;
    let manifest: Value = serde_json::from_str(&manifest_text).map_err(io)?;
    TransportPolicy::from_manifest(&manifest)?;
    if dir.join("CONTINUITY_FAILURE.json").exists() {
        return stop("CONTINUITY_FAILURE のある run は検証・変換できません");
    }
    let prov = read_json(&dir.join("RUN_PROVENANCE.json"))?;
    if prov["manifest_sha256"] != json!(sha(manifest_text.as_bytes())) || prov["code_commit"] != manifest["code_commit"] {
        return stop("provenance が manifest と一致しません");
    }
    // membership: PF0B 定義の commitment だけが authority
    let keys: Vec<String> = chains[F_UNIT].iter().map(|r| r["evaluation_unit_key"].as_str().unwrap_or_default().to_string()).collect();
    let ver = manifest["membership"]["eligibility_rule_version"].as_str().unwrap_or_default();
    if membership_commitment(&keys, ver) != manifest["membership"]["commitment"].as_str().unwrap_or_default() {
        return stop("membership commitment が一致しません");
    }
    if manifest["unit_order_sha256"] != json!(sha_v(&json!(keys))?) {
        return stop("unit の処理順が manifest の事前 commit と一致しません");
    }
    let by_key: BTreeMap<&str, &UnitIn> = units.iter().map(|u| (u.key.as_str(), u)).collect();
    if by_key.len() != keys.len() || keys.iter().any(|k| !by_key.contains_key(k.as_str())) {
        return stop("payload 集合が membership と一致しません");
    }
    let (mut replayed, mut failed) = (0usize, 0usize);
    let mut eval_expected: BTreeMap<String, Value> = BTreeMap::new();
    for key in &keys {
        let u = by_key[key.as_str()];
        let latest = chains[F_OUT].iter().rev().find(|r| r["evaluation_unit_key"] == json!(key)).ok_or_else(|| Stop(format!("{key}: matcher_output がありません")))?;
        let attempt = latest["attempt"].clone();
        let status = latest["technical_status"].as_str().unwrap_or_default().to_string();
        eval_expected.insert(key.clone(), json!({"evaluation_unit_key": key, "machine": latest["pf0b_machine"], "technical_status": status}));
        let gen = Generator::snapshot_static(u);
        if gen.is_err() {
            if status != "input_mismatch" {
                return stop(format!("{key}: payload が不一致なのに status={status}"));
            }
            failed += 1;
            continue;
        }
        if matches!(status.as_str(), "call_failure" | "decode_error" | "input_mismatch" | "internal_error") {
            if latest["pf0b_machine"]["kind"] != json!("MISSING") {
                return stop(format!("{key}: 失敗なのに machine が MISSING ではありません"));
            }
            failed += 1;
            continue;
        }
        let snap = gen?;
        let ev = snap.local_evidence();
        let mut q: VecDeque<&Value> = chains[F_REQ].iter().filter(|r| r["evaluation_unit_key"] == json!(key) && r["attempt"] == attempt).collect();
        let mut fetch = |spec: &CallSpec| -> Result<Items, String> {
            let r = q.pop_front().ok_or("request が不足しています")?;
            if r["request_id"] != json!(spec.request_id(key)) || r["canonical_request_sha256"] != json!(sha_v(&spec.canon_req().to_json()).map_err(|e| e.0)?) || r["final_status"] != json!("ok") {
                return Err("request が query plan と一致しません".into());
            }
            let h = r["raw_body_sha256"].as_str().unwrap_or_default();
            let body = std::fs::read(dir.join("raw").join(format!("{h}.bin"))).map_err(|e| e.to_string())?;
            if sha(&body) != h {
                return Err("raw body の sha256 が違います".into());
            }
            parse_items(spec.kind, &body).map_err(|_| "raw body を decode できません".to_string())
        };
        let d = derive(&ev, &mut fetch).map_err(|e| Stop(format!("{key}: replay 失敗: {e}")))?;
        if !q.is_empty() {
            return stop(format!("{key}: 使われない request が残っています"));
        }
        let (out, proj, cs) = bodies_ok(key, &d, &snap.embedded_evidence(), skips_tmdb(&ev))?;
        let pick = |f: &str| chains[f].iter().rev().find(|r| r["evaluation_unit_key"] == json!(key) && r["attempt"] == attempt).map(strip);
        for (f, want) in [(F_OUT, &out), (F_PROJ, &proj), (F_CAND, &cs)] {
            if pick(f).as_ref().map(canon).transpose()? != Some(canon(want)?) {
                return stop(format!("{key}: {f} が replay と一致しません"));
            }
        }
        replayed += 1;
    }
    let got_eval: BTreeMap<String, Value> = chains[F_EVAL].iter().map(|r| (r["evaluation_unit_key"].as_str().unwrap_or_default().to_string(), strip(r))).collect();
    if got_eval != eval_expected {
        return stop("evaluation_unit が matcher_output と一致しません");
    }
    Ok(VerifyReport { units: keys.len(), replayed_units: replayed, failed_units: failed })
}

impl Generator {
    fn snapshot_static(u: &UnitIn) -> Result<crate::services::prematch_snapshot::PreMatchSnapshot, Stop> {
        if sha_v(&u.payload)? != u.matcher_input_sha256 {
            return stop("matcher_input_sha256");
        }
        let snap = snapshot_from_payload(&u.payload, work_id(&u.key)?, EvidenceClass::Live)?;
        if let Some(want) = &u.expected_decision_evidence_sha256 {
            if &evidence_hashes(&snap)?.2 != want {
                return stop("decision_evidence_sha256");
            }
        }
        Ok(snap)
    }
}

// ─── PF0B adapter（前段） ───────────────────────────────────────────────────────────────────────

/// 封印・検証済みの artifact から PF0B の `FinalInput` を作る。GT は呼び出し側が **別経路で** 渡す。
/// machine artifact 自体は GT を持たない。検証（`verify_and_replay`）に失敗した artifact は変換しない。
pub fn final_input(dir: &Path, units: &[UnitIn], gts: &BTreeMap<String, Gt>) -> Result<FinalInput, Stop> {
    verify_and_replay(dir, units)?;
    let manifest = read_json(&dir.join("MANIFEST.json"))?;
    let (evals, _) = read_chain(&dir.join(F_EVAL))?;
    if gts.len() != evals.len() || evals.iter().any(|e| !gts.contains_key(e["evaluation_unit_key"].as_str().unwrap_or_default())) {
        return stop("GT の集合が membership と一致しません");
    }
    let mut out = Vec::new();
    let mut failure = None;
    for e in &evals {
        let key = e["evaluation_unit_key"].as_str().unwrap_or_default().to_string();
        let m = &e["machine"];
        let machine = match m["kind"].as_str().unwrap_or_default() {
            "AUTO" => Machine::Auto(Some((m["identity"]["media_type"].as_str().unwrap_or_default().to_string(), m["identity"]["tmdb_id"].as_i64().unwrap_or_default()))),
            "AUTO_INVALID" => Machine::Auto(None),
            "REVIEW" => Machine::Review,
            "UNRESOLVED" => Machine::Unresolved,
            _ => {
                failure = Some(format!("{}: {}", key, e["technical_status"].as_str().unwrap_or("missing")));
                Machine::Missing
            }
        };
        out.push(crate::services::pr4_pf0b::Unit { key: key.clone(), machine, gt: gts[&key].clone() });
    }
    Ok(FinalInput {
        units: out,
        expected_membership_commitment: manifest["membership"]["commitment"].as_str().unwrap_or_default().to_string(),
        eligibility_rule_version: manifest["membership"]["eligibility_rule_version"].as_str().unwrap_or_default().to_string(),
        run_failure: failure,
    })
}

// ─── tests（synthetic / mock のみ） ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::pr4_pf0b::{evaluate, Decision, PolicyConfig, UnevaluableAuto};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);
    const VER: &str = "pr4-test-eligibility-1";
    const FAKE_KEY: &str = "FAKE-TEST-KEY-0123456789abcdef";

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pr4_pf0f_{tag}_{}_{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn payload(title_guess: Option<&str>, file: &str) -> Value {
        payload_full(title_guess, file, "movie", None)
    }
    fn payload_full(title_guess: Option<&str>, file: &str, kind: &str, tags: Option<&str>) -> Value {
        json!({"payload_schema_version": "pr4-h2-input-payload-1", "canonicalization_version": "pr4-canonical-json-1",
               "file_order_basis": "work_parts.part_no,files.id", "work": {"title_guess": title_guess},
               "files": [{"original_file_name": file, "original_rel_path": file, "original_captured": true, "renamed_by_app": null,
                          "container_tags_json": tags, "tags_provenance": null, "tags_provider_hint": null, "source_media_kind": kind, "path_derived": null}]})
    }
    fn unit(i: i64, tg: Option<&str>, file: &str) -> UnitIn {
        let p = payload(tg, file);
        UnitIn { key: format!("work:{i}"), matcher_input_sha256: sha_v(&p).unwrap(), payload: p, expected_decision_evidence_sha256: None }
    }
    fn units() -> Vec<UnitIn> {
        vec![
            unit(1, Some("Alpha Movie 1999"), "Alpha Movie 1999.mkv"), // AUTO になりうる（年あり・一意）
            unit(2, Some("Beta Film"), "Beta Film.mkv"),               // 年なし → REVIEW
            unit(3, None, ""),                                          // タイトル無し → 0 call
            unit(4, Some("Gamma Story 2005"), "Gamma Story 2005.mkv"),
        ]
    }
    fn policy_json(retry_after: &str, cap: u64) -> Value {
        let mut p = json!({"version": "transport-policy-1", "max_attempts": 3, "backoff_ms": [400, 800], "retry_statuses": [429, 500, 502, 503, 504],
                           "retry_after": retry_after, "retry_after_cap_ms": cap, "concurrency": 1, "timeout_secs": 30, "pacing_ms": 0});
        p["policy_sha256"] = json!(sha_v(&p).unwrap());
        p
    }
    fn manifest(us: &[UnitIn]) -> Value {
        manifest_with(us, policy_json("ignore", 0))
    }
    fn manifest_with(us: &[UnitIn], policy: Value) -> Value {
        let keys: Vec<String> = us.iter().map(|u| u.key.clone()).collect();
        json!({"unit_order_sha256": sha_v(&json!(keys)).unwrap(), "transport_policy": policy, "resume_policy": {"max_primary_resumes": 1, "recovery_resume": "forbidden"},"contract_version": CONTRACT_VERSION, "run_id": "00000000-0000-4000-8000-000000000001",
               "binding_matcher": BINDING_MATCHER, "reference_matcher": REFERENCE_MATCHER, "rules_1_in_final": false,
               "code_commit": "0aca746e2d0fee12ecfc660c5fc2e40d619158c6",
               "membership": {"commitment": membership_commitment(&keys, VER), "eligibility_rule_version": VER,
                              "pf0h_final_artifact_sha256": "a".repeat(64), "pf0h_unit_set_sha256_aux": "b".repeat(64), "payload_set_sha256_aux": "c".repeat(64)},
               "recovery": {"mode": "retryable_failed_units_once", "max_attempts": 2},
               "tmdb_request_template": {"language": "ja-JP", "include_adult": "false", "page": 1}})
    }
    fn clock() -> Box<dyn FnMut() -> String> {
        let mut n = 0u32;
        Box::new(move || {
            n += 1;
            format!("2026-11-01T00:{:02}:{:02}Z", (n / 60) % 60, n % 60)
        })
    }

    /// 応答を作る mock。`fail` に入った query は指定の失敗を返す。`calls` は query ごとの回数
    struct Mock {
        fail: BTreeMap<String, Vec<Result<u16, NetErr>>>,
        calls: BTreeMap<String, usize>,
        echo_key: bool,
        pauses: Vec<u64>,
        retry_after: Option<u32>,
        panic_on: Option<String>,
    }
    impl Mock {
        fn new() -> Self {
            Mock { fail: BTreeMap::new(), calls: BTreeMap::new(), echo_key: false, pauses: Vec::new(), retry_after: None, panic_on: None }
        }
        fn body(req: &CanonReq) -> Vec<u8> {
            let q = req.params["query"].to_lowercase();
            let year = req.params.get("year").cloned();
            let movies = if req.path == "/3/search/movie" && q.contains("alpha") {
                vec![json!({"id": 100, "title": "Alpha Movie", "original_title": "Alpha Movie", "overview": "Overview A ✓", "release_date": "1999-05-01", "poster_path": "/a.jpg", "original_language": "en", "vote_average": 7.5, "genre_ids": [1, 2]})]
            } else if req.path == "/3/search/movie" && q.contains("beta") {
                vec![json!({"id": 200, "title": "Beta Film", "original_title": "Beta Film", "overview": "B", "release_date": "2001-01-01", "poster_path": null, "original_language": "en", "vote_average": 5.0, "genre_ids": []}),
                     json!({"id": 201, "title": "Beta Film 2", "original_title": "Beta Film 2", "overview": "B2", "release_date": "2003-01-01", "poster_path": "/b.jpg", "original_language": "en", "vote_average": 4.0, "genre_ids": []})]
            } else if req.path == "/3/search/movie" && q.contains("gamma") && year.is_some() {
                vec![json!({"id": 300, "title": "Gamma Story", "original_title": "Gamma Story", "overview": "G", "release_date": "2005-02-02", "poster_path": "/g.jpg", "original_language": "en", "vote_average": 6.0, "genre_ids": []})]
            } else {
                vec![]
            };
            serde_json::to_vec(&json!({"page": 1, "results": movies, "total_results": movies.len()})).unwrap()
        }
    }
    impl Transport for Mock {
        fn pause(&mut self, ms: u64) {
            self.pauses.push(ms);
        }
        fn get(&mut self, req: &CanonReq) -> Result<HttpResp, NetErr> {
            let id = format!("{}|{}|{}", req.path, req.params["query"], req.params.get("year").cloned().unwrap_or_default());
            if self.panic_on.as_deref().map(|p| req.params["query"].contains(p)).unwrap_or(false) {
                panic!("simulated crash");
            }
            *self.calls.entry(id.clone()).or_default() += 1;
            if let Some(q) = self.fail.get_mut(&id) {
                if !q.is_empty() {
                    return match q.remove(0) {
                        Err(e) => Err(e),
                        Ok(status) => Ok(HttpResp { status, body: b"{\"status_message\":\"x\"}".to_vec(), retry_after_secs: self.retry_after }),
                    };
                }
            }
            let mut body = Self::body(req);
            if self.echo_key {
                body = format!("{{\"results\":[],\"leak\":\"{FAKE_KEY}\"}}").into_bytes();
            }
            Ok(HttpResp { status: 200, body, retry_after_secs: None })
        }
    }

    fn run_all(tag: &str, mock: &mut Mock) -> (PathBuf, Vec<UnitIn>) {
        let us = units();
        let dir = tmp(tag);
        let mut g = Generator::begin(&dir, manifest(&us), us.clone(), clock(), Some(FAKE_KEY.into())).unwrap();
        g.run_primary(mock).unwrap();
        g.commit_recovery_set().unwrap();
        g.run_recovery(mock).unwrap();
        g.seal(&BTreeMap::from([("LANG".into(), "ja_JP".into()), ("TMDB_API_KEY".into(), FAKE_KEY.into())])).unwrap();
        (dir, us)
    }

    fn all_text(dir: &Path) -> String {
        let mut s = String::new();
        for e in walk(dir) {
            s.push_str(&String::from_utf8_lossy(&std::fs::read(&e).unwrap()));
        }
        s
    }
    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut v = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.path().is_dir() {
                v.extend(walk(&e.path()));
            } else {
                v.push(e.path());
            }
        }
        v
    }

    #[test]
    fn planned_specs_match_the_production_call_plan() {
        for u in units() {
            let snap = snapshot_from_payload(&u.payload, 1, EvidenceClass::Live).unwrap();
            let ev = snap.local_evidence();
            assert_eq!(planned_specs(&ev).len(), planned_logical_calls(&ev), "{}", u.key);
        }
    }

    #[test]
    fn a_clean_run_replays_offline_and_matches_the_artifact() {
        let (dir, us) = run_all("clean", &mut Mock::new());
        let rep = verify_and_replay(&dir, &us).unwrap();
        assert_eq!(rep, VerifyReport { units: 4, replayed_units: 4, failed_units: 0 });
        let (out, _) = read_chain(&dir.join(F_OUT)).unwrap();
        let kinds: Vec<String> = out.iter().map(|r| r["pf0b_machine"]["kind"].as_str().unwrap().to_string()).collect();
        assert_eq!(kinds[0], "AUTO", "{kinds:?}");
        assert_eq!(out[0]["pf0b_machine"]["identity"], json!({"media_type": "movie", "tmdb_id": 100}));
        assert_eq!(out[2]["technical_status"], json!("skipped_no_title"));
        assert_eq!(out[2]["pf0b_machine"]["kind"], json!("UNRESOLVED"), "0 call は失敗ではなく UNRESOLVED");
        assert_eq!(out[0]["binding_matcher"], json!(BINDING_MATCHER));
        assert!(out[0]["rules_safe"].is_object() && out[0]["rules_tags_shadow"].is_object());
        let (req, _) = read_chain(&dir.join(F_REQ)).unwrap();
        assert!(req.iter().all(|r| r["canonical_request"]["params"].get("api_key").is_none()));
    }

    #[test]
    fn no_secret_reaches_any_artifact_and_a_leaking_response_aborts_the_run() {
        let (dir, _) = run_all("secret", &mut Mock::new());
        let text = all_text(&dir);
        assert!(!text.contains(FAKE_KEY) && !text.to_lowercase().contains("api_key") && !text.to_lowercase().contains("authorization"));
        assert!(!text.contains("TMDB_API_KEY"), "env の key 名も記録しない");
        // 応答本文に key が混入したら保存せず中止
        let us = units();
        let d2 = tmp("leak");
        let mut g = Generator::begin(&d2, manifest(&us), us, clock(), Some(FAKE_KEY.into())).unwrap();
        let mut m = Mock::new();
        m.echo_key = true;
        assert!(g.run_primary(&mut m).unwrap_err().0.contains("secret_scan_hit"));
        assert!(!all_text(&d2).contains(FAKE_KEY));
    }

    #[test]
    fn raw_success_bodies_are_stored_content_addressed_and_failures_keep_bounded_evidence_only() {
        let mut m = Mock::new();
        m.fail.insert("/3/search/movie|Beta Film|".into(), vec![Ok(500), Ok(500), Ok(500)]);
        let us = units();
        let dir = tmp("raw");
        let mut g = Generator::begin(&dir, manifest(&us), us.clone(), clock(), None).unwrap();
        g.run_primary(&mut m).unwrap();
        let (req, _) = read_chain(&dir.join(F_REQ)).unwrap();
        let ok: Vec<_> = req.iter().filter(|r| r["final_status"] == json!("ok")).collect();
        assert!(!ok.is_empty());
        for r in ok {
            let h = r["raw_body_sha256"].as_str().unwrap();
            let b = std::fs::read(dir.join("raw").join(format!("{h}.bin"))).unwrap();
            assert_eq!(sha(&b), h);
        }
        let bad = req.iter().find(|r| r["final_status"] == json!("http_error")).unwrap();
        assert_eq!(bad["evidence"].as_object().unwrap().len(), 3, "status・長さ・sha だけ（本文は持たない）");
        assert!(bad["raw_body_sha256"].is_null());
    }

    #[test]
    fn the_reviewer_projection_is_the_frozen_combined_ranked_set_without_rank_score_source_or_decision() {
        let (dir, us) = run_all("proj", &mut Mock::new());
        verify_and_replay(&dir, &us).unwrap();
        let (proj, _) = read_chain(&dir.join(F_PROJ)).unwrap();
        let (cs, _) = read_chain(&dir.join(F_CAND)).unwrap();
        let p = proj.iter().find(|r| r["evaluation_unit_key"] == json!("work:1")).unwrap();
        let c = &p["candidates"][0];
        assert_eq!(c["overview"], json!("Overview A ✓"));
        assert_eq!(c["poster_path"], json!("/a.jpg"));
        assert_eq!(c["release_date"], json!("1999-05-01"));
        assert_eq!(c["year"], json!(1999));
        assert_eq!(c["release_year_from_date"], json!(1999));
        let keys: BTreeSet<&str> = c.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, BTreeSet::from(["tmdb_id", "media_type", "title", "original_title", "year", "release_date", "release_year_from_date", "year_consistent_with_release_date", "overview", "poster_path"]));
        let text = serde_json::to_string(&p["candidates"]).unwrap();
        for bad in ["confidence", "reasons", "rank", "score", "query_source", "decision"] {
            assert!(!text.contains(bad), "{bad}");
        }
        // 提示集合 = shadow が判定に使った combined ranked
        let ranked: BTreeSet<i64> = cs.iter().find(|r| r["evaluation_unit_key"] == json!("work:2")).unwrap()["combined"]["ranked"].as_array().unwrap().iter()
            .map(|r| cs.iter().find(|x| x["evaluation_unit_key"] == json!("work:2")).unwrap()["combined"]["all"][r["ord"].as_u64().unwrap() as usize]["tmdb_id"].as_i64().unwrap()).collect();
        let shown: BTreeSet<i64> = proj.iter().find(|r| r["evaluation_unit_key"] == json!("work:2")).unwrap()["candidates"].as_array().unwrap().iter().map(|c| c["tmdb_id"].as_i64().unwrap()).collect();
        assert_eq!(ranked, shown);
        // reviewer 表示順は rank を漏らさない（(media_type, tmdb_id) 昇順）
        let ids: Vec<i64> = proj.iter().find(|r| r["evaluation_unit_key"] == json!("work:2")).unwrap()["candidates"].as_array().unwrap().iter().map(|c| c["tmdb_id"].as_i64().unwrap()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn a_failed_year_less_retry_is_a_technical_failure_not_silently_swallowed() {
        let mut m = Mock::new();
        // Gamma は年ヒントあり → year 付き primary の後に year なし再検索が走る。後者だけ 3 回 timeout
        m.fail.insert("/3/search/movie|Gamma Story|".into(), vec![Err(NetErr::Timeout); 3]);
        let us = units();
        let dir = tmp("retryfail");
        let mut g = Generator::begin(&dir, manifest(&us), us, clock(), None).unwrap();
        g.run_primary(&mut m).unwrap();
        let (out, _) = read_chain(&dir.join(F_OUT)).unwrap();
        let g4 = out.iter().find(|r| r["evaluation_unit_key"] == json!("work:4")).unwrap();
        assert_eq!(g4["technical_status"], json!("call_failure"));
        assert_eq!(g4["retryable"], json!(true));
        assert_eq!(g4["pf0b_machine"]["kind"], json!("MISSING"));
    }

    #[test]
    fn recovery_runs_only_the_committed_retryable_units_once_and_never_touches_successful_ones() {
        let mut m = Mock::new();
        m.fail.insert("/3/search/movie|Beta Film|".into(), vec![Err(NetErr::Connect); 3]); // primary だけ失敗、recovery では成功
        let us = units();
        let dir = tmp("recovery");
        let mut g = Generator::begin(&dir, manifest(&us), us.clone(), clock(), None).unwrap();
        g.run_primary(&mut m).unwrap();
        let before: BTreeMap<String, usize> = m.calls.clone();
        let set = g.commit_recovery_set().unwrap();
        assert_eq!(set["set"]["unit_keys"], json!(["work:2"]));
        g.run_recovery(&mut m).unwrap();
        for (k, n) in &before {
            if !k.contains("Beta Film") {
                assert_eq!(m.calls[k], *n, "成功済み unit の call は増えない: {k}");
            }
        }
        assert!(g.run_recovery(&mut m).is_err(), "recovery は 1 回だけ");
        g.seal(&BTreeMap::new()).unwrap();
        let rep = verify_and_replay(&dir, &us).unwrap();
        assert_eq!(rep.failed_units, 0);
        let (out, _) = read_chain(&dir.join(F_OUT)).unwrap();
        assert_eq!(out.iter().filter(|r| r["attempt"] == json!(2)).count(), 1);
    }

    #[test]
    fn non_retryable_failures_are_not_in_the_recovery_set_and_a_residual_failure_is_a_technical_failure() {
        let mut m = Mock::new();
        m.fail.insert("/3/search/movie|Alpha Movie|1999".into(), vec![Ok(401)]); // 認証エラー等は retryable でない
        let us = units();
        let dir = tmp("residual");
        let mut g = Generator::begin(&dir, manifest(&us), us.clone(), clock(), None).unwrap();
        g.run_primary(&mut m).unwrap();
        let set = g.commit_recovery_set().unwrap();
        assert_eq!(set["set"]["unit_keys"], json!([]));
        g.seal(&BTreeMap::new()).unwrap();
        let gts: BTreeMap<String, Gt> = us.iter().map(|u| (u.key.clone(), Gt::NoMatch)).collect();
        let fi = final_input(&dir, &us, &gts).unwrap();
        assert!(fi.run_failure.is_some());
        let policy = PolicyConfig::test_only(1, 0.1, 0.95, 0.5, None, UnevaluableAuto::Exclude);
        assert!(matches!(evaluate(&fi, &policy).1, Decision::TechnicalFailure(_)));
    }

    #[test]
    fn a_clean_artifact_converts_to_a_pf0b_final_input_that_evaluates() {
        let (dir, us) = run_all("adapter", &mut Mock::new());
        let gts: BTreeMap<String, Gt> = [("work:1", Gt::Positive("movie".into(), 100)), ("work:2", Gt::Positive("movie".into(), 200)), ("work:3", Gt::NoMatch), ("work:4", Gt::Positive("movie".into(), 300))]
            .into_iter().map(|(k, g)| (k.to_string(), g)).collect();
        let fi = final_input(&dir, &us, &gts).unwrap();
        assert!(fi.run_failure.is_none());
        assert_eq!(fi.expected_membership_commitment, membership_commitment(&us.iter().map(|u| u.key.clone()).collect::<Vec<_>>(), VER));
        let policy = PolicyConfig::test_only(1, 0.1, 0.95, 0.5, None, UnevaluableAuto::Exclude);
        let (_, d) = evaluate(&fi, &policy);
        assert!(!matches!(d, Decision::TechnicalFailure(_)), "{d:?}");
        // GT が足りない / 余分な key は拒否
        let mut short = gts.clone();
        short.remove("work:4");
        assert!(final_input(&dir, &us, &short).is_err());
    }

    #[test]
    fn membership_authority_is_the_pf0b_commitment_and_auxiliary_hashes_do_not_matter() {
        let us = units();
        let mut m = manifest(&us);
        m["membership"]["pf0h_unit_set_sha256_aux"] = json!("f".repeat(64)); // 補助値を変えても通る
        m["membership"]["payload_set_sha256_aux"] = json!("e".repeat(64));
        assert!(Generator::begin(&tmp("aux"), m, us.clone(), clock(), None).is_ok());
        let mut bad = manifest(&us);
        bad["membership"]["commitment"] = json!("0".repeat(64));
        assert!(Generator::begin(&tmp("badm"), bad, us.clone(), clock(), None).is_err());
        let mut fewer = us.clone();
        fewer.pop();
        assert!(Generator::begin(&tmp("fewer"), manifest(&us), fewer, clock(), None).is_err(), "unit が足りなければ起動しない");
        let mut nopin = manifest(&us);
        nopin["membership"]["pf0h_final_artifact_sha256"] = json!("");
        assert!(Generator::begin(&tmp("nopin"), nopin, us.clone(), clock(), None).is_err());
        let mut wrong = manifest(&us);
        wrong["binding_matcher"] = json!(REFERENCE_MATCHER);
        assert!(Generator::begin(&tmp("wrongm"), wrong, us.clone(), clock(), None).is_err());
        let mut r1 = manifest(&us);
        r1["rules_1_in_final"] = json!(true);
        assert!(Generator::begin(&tmp("r1"), r1, us, clock(), None).is_err());
    }

    #[test]
    fn one_manifest_is_one_session_a_second_start_or_a_sealed_rewrite_is_refused() {
        let (dir, us) = run_all("single", &mut Mock::new());
        assert!(Generator::begin(&dir, manifest(&us), us.clone(), clock(), None).is_err());
        let prov = read_json(&dir.join("RUN_PROVENANCE.json")).unwrap();
        assert!(valid_time(prov["session"]["started_at_utc"].as_str().unwrap()) && valid_time(prov["session"]["ended_at_utc"].as_str().unwrap()));
        let us2 = units();
        let d2 = tmp("order");
        let mut g = Generator::begin(&d2, manifest(&us2), us2, clock(), None).unwrap();
        assert!(g.run_recovery(&mut Mock::new()).is_err(), "対象集合の commitment 前は recovery 不可");
        g.run_primary(&mut Mock::new()).unwrap();
        assert!(g.run_primary(&mut Mock::new()).is_err(), "primary は 1 回");
        assert!(g.seal(&BTreeMap::new()).is_err(), "recovery 対象の確定前は封印不可");
    }

    #[test]
    fn tampering_with_any_artifact_or_the_payload_set_is_detected() {
        for target in [F_OUT, F_REQ, F_CAND, F_PROJ, F_EVAL, "MANIFEST.json"] {
            let (dir, us) = run_all("tamper", &mut Mock::new());
            let p = dir.join(target);
            let t = std::fs::read_to_string(&p).unwrap();
            std::fs::write(&p, t.replacen("100", "101", 1).replacen("\"code_commit\":\"0aca", "\"code_commit\":\"0acb", 1)).unwrap();
            if std::fs::read_to_string(&p).unwrap() != t {
                assert!(verify_and_replay(&dir, &us).is_err(), "{target}");
            }
        }
        let (dir, us) = run_all("tamper_raw", &mut Mock::new());
        let raw = walk(&dir.join("raw")).remove(0);
        std::fs::write(&raw, b"{\"results\":[]}").unwrap();
        assert!(verify_and_replay(&dir, &us).is_err());
        let (dir2, mut us2) = run_all("tamper_payload", &mut Mock::new());
        us2[0].payload["work"]["title_guess"] = json!("Changed");
        assert!(verify_and_replay(&dir2, &us2).is_err());
        let (dir3, us3) = run_all("tamper_swap", &mut Mock::new());
        let p = dir3.join(F_OUT);
        let mut lines: Vec<String> = std::fs::read_to_string(&p).unwrap().lines().map(String::from).collect();
        lines.swap(0, 1);
        std::fs::write(&p, lines.join("\n") + "\n").unwrap();
        assert!(verify_and_replay(&dir3, &us3).is_err());
    }

    #[test]
    fn replay_is_deterministic_and_independent_of_the_mock() {
        let (dir, us) = run_all("det", &mut Mock::new());
        let a = verify_and_replay(&dir, &us).unwrap();
        let b = verify_and_replay(&dir, &us).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn input_mismatch_is_a_non_retryable_failure_and_makes_no_tmdb_call() {
        let mut us = units();
        us[1].matcher_input_sha256 = "0".repeat(64);
        let mut m = Mock::new();
        let dir = tmp("inmis");
        let mut mf = manifest(&us);
        mf["membership"]["commitment"] = json!(membership_commitment(&us.iter().map(|u| u.key.clone()).collect::<Vec<_>>(), VER));
        let mut g = Generator::begin(&dir, mf, us.clone(), clock(), None).unwrap();
        g.run_primary(&mut m).unwrap();
        assert!(!m.calls.keys().any(|k| k.contains("Beta")));
        let (out, _) = read_chain(&dir.join(F_OUT)).unwrap();
        let o = out.iter().find(|r| r["evaluation_unit_key"] == json!("work:2")).unwrap();
        assert_eq!((o["technical_status"].as_str(), o["retryable"].clone()), (Some("input_mismatch"), json!(false)));
    }


    // ─── r2 追加裁定: Q11 parity / Q12 release_date / Q13 resume / Q14 transport policy ────────────

    fn region(src: &str, start: &str) -> String {
        let src = src.replace('\r', "");
        let i = src.find(start).expect(start);
        let j = src[i..].find("\n}\n").unwrap() + i + 3;
        src[i..j].to_string()
    }

    /// production の純粋ロジックの pin。**変わったら parity を再確認するまで落ちる**
    /// （live `TmdbClient` は base URL が差し替えられず、`fetch_candidates` を mock で直接は走らせられないための代替）
    #[test]
    fn the_production_functions_derive_mirrors_are_pinned_by_source_hash() {
        let src = include_str!("../commands/tmdb.rs");
        let pins = [
            ("pub(crate) async fn fetch_candidates(", "4bad9f3297d4d96e9cae7f28694b5839fddf453f53076a46ac94c8084f751a91"),
            ("fn merge_candidate_sets(", "9ff51c0bf769d348648404c7a3c49cbcfcf609d5f7ce363e8654c1e63314e466"),
            ("pub(crate) fn query_plan_for(", "fe649693d8b4562fb6ca88531d5bf118205d11afaed0472f235f1449a6f02e44"),
            ("pub(crate) fn parsed_for_query(", "baf2905304e0e3e27bafbf1570758d12a5b8ee9ee7145c984a0d30f60f3e6f23"),
            ("pub(crate) fn skips_tmdb(", "5d299b363f420778097d96555bb7433172e5400298a92842b00169d7c6ceaffa"),
            ("pub(crate) fn planned_logical_calls(", "611081d75d7ede164e72a098cc3c5b9a253e3c642f0ce239bab332dba20d7527"),
        ];
        for (start, want) in pins {
            assert_eq!(sha(region(src, start).as_bytes()), want, "{start} が変わりました。derive との parity を再確認して pin を更新してください");
        }
    }

    /// 検索応答を query / year から決定的に作る fixture（id は 1.. で movie・tv・legacy・embedded 間で重なる）
    fn fixture_body(req: &CanonReq) -> Vec<u8> {
        let q = &req.params["query"];
        let year_given = req.params.contains_key("year") || req.params.contains_key("first_air_date_year");
        let base_year = if q.to_lowercase().contains("embedded") { 2004 } else { 1999 };
        let (range, is_tv) = if req.path == "/3/search/tv" { (1..=3, true) } else if year_given { (1..=8, false) } else { (5..=14, false) };
        let items: Vec<Value> = range
            .map(|i: i64| {
                // id1 は検索語そのまま、id2 は同点になりやすい同名、他は部分一致
                let title = match i { 1 => q.clone(), 2 => q.clone(), _ => format!("{q} part {i}") };
                let date = format!("{}-0{}-01", base_year + (i % 3) as i32, 1 + (i % 9));
                if is_tv {
                    json!({"id": i, "name": title, "original_name": title, "overview": "tv", "first_air_date": date, "poster_path": null, "original_language": "ja"})
                } else {
                    json!({"id": i, "title": title, "original_title": title, "overview": "mv", "release_date": date, "poster_path": "/p.jpg", "original_language": "en"})
                }
            })
            .collect();
        serde_json::to_vec(&json!({"page": 1, "results": items, "total_results": items.len()})).unwrap()
    }

    /// production `fetch_candidates` の本体を、HTTP だけ fixture に置き換えて **別途書き写した** 基準実装（derive とは独立に書いてある）
    fn oracle(ev: &LocalEvidence) -> Value {
        use crate::services::rules_v1::{self};
        let _ = rules_v1::THRESHOLD_AUTO_V1;
        // production は `search_candidates` が先に skips_tmdb で 0 call を返す
        if skips_tmdb(ev) {
            return shape(&[], &[], &[], &[], &[], Vec::new());
        }
        let (mut legacy_all, mut embedded_all) = (Vec::<TmdbCandidate>::new(), Vec::<TmdbCandidate>::new());
        let mut queries = Vec::<Value>::new();
        for input in &ev.search_inputs() {
            let plan = query_plan_for(input, ev.media_kind());
            let parsed = parsed_for_query(input);
            let source = input.source.as_str();
            let bucket = match input.source {
                QuerySource::Legacy => &mut legacy_all,
                QuerySource::Embedded => &mut embedded_all,
            };
            let get = |kind: &'static str, year: Option<i32>| {
                let mut spec = CallSpec { input_index: 0, source: "x", kind, variant: "primary", query: plan.query.clone(), year };
                spec.year = year;
                fixture_body(&spec.canon_req())
            };
            if plan.search_movie {
                queries.push(json!({"kind": "movie", "query": plan.query, "year": plan.year, "source": source}));
                let r: TmdbSearchResponse<TmdbSearchMovie> = serde_json::from_slice(&get("movie", plan.year)).unwrap();
                for x in &r.results {
                    bucket.push(score_movie(x, &parsed));
                }
                if plan.retry_without_year {
                    queries.push(json!({"kind": "movie", "query": plan.query, "year": null, "source": source}));
                    let r2: TmdbSearchResponse<TmdbSearchMovie> = serde_json::from_slice(&get("movie", None)).unwrap();
                    for x in &r2.results {
                        let scored = score_movie(x, &parsed);
                        if !bucket.iter().any(|c| c.tmdb_id == scored.tmdb_id) {
                            bucket.push(scored);
                        }
                    }
                }
            }
            if plan.search_tv {
                queries.push(json!({"kind": "tv", "query": plan.query, "year": plan.year, "source": source}));
                let r: TmdbSearchResponse<TmdbSearchTv> = serde_json::from_slice(&get("tv", plan.year)).unwrap();
                for x in &r.results {
                    bucket.push(score_tv(x, &parsed));
                }
            }
        }
        let (combined_all, sources) = merge_candidate_sets(&legacy_all, &embedded_all);
        shape(&legacy_all, &merge_and_rank(legacy_all.clone()), &combined_all, &merge_and_rank(combined_all.clone()), &sources, queries)
    }

    fn shape(la: &[TmdbCandidate], lr: &[TmdbCandidate], ca: &[TmdbCandidate], cr: &[TmdbCandidate], src: &[CandidateSource], queries: Vec<Value>) -> Value {
        json!({"queries": queries, "legacy_all": la, "legacy_ranked": lr, "combined_all": ca, "combined_ranked": cr,
               "sources": src.iter().map(|s| json!([s.tmdb_id, s.media_type, s.query_source])).collect::<Vec<_>>()})
    }

    fn derive_shape(ev: &LocalEvidence) -> (Value, Derived) {
        let mut queries = Vec::new();
        let mut fetch = |spec: &CallSpec| -> Result<Items, String> {
            queries.push(json!({"kind": spec.kind, "query": spec.query, "year": spec.year, "source": spec.source}));
            Ok(parse_items(spec.kind, &fixture_body(&spec.canon_req())).map_err(|_| "decode")?)
        };
        let d = derive(ev, &mut fetch).unwrap();
        (shape(&d.legacy_all, &d.legacy_ranked, &d.combined_all, &d.combined_ranked, &d.sources, queries), d)
    }

    #[test]
    fn derive_matches_the_production_logic_on_query_plan_candidate_set_and_ordering() {
        let tags = r#"{"v":1,"title":"Delta Embedded","date":"2004"}"#;
        let scenarios: Vec<(&str, Value)> = vec![
            ("legacy_year_retry_dedupe", payload_full(Some("Alpha Movie 1999"), "Alpha Movie 1999.mkv", "movie", None)),
            ("tv_only", payload_full(Some("Beta Series"), "Beta Series.mkv", "tv", None)),
            ("unknown_both_kinds", payload_full(Some("Gamma Thing 2005"), "Gamma Thing 2005.mkv", "unknown", None)),
            ("embedded_adds_input_and_merges", payload_full(Some("Delta Legacy 1999"), "Delta Legacy 1999.mkv", "movie", Some(tags))),
            ("embedded_only_title", payload_full(None, "", "unknown", Some(tags))),
            ("no_title_no_calls", payload_full(None, "", "movie", None)),
        ];
        for (name, p) in scenarios {
            let snap = snapshot_from_payload(&p, 1, EvidenceClass::Live).unwrap();
            let ev = snap.local_evidence();
            let (got, d) = derive_shape(&ev);
            let want = oracle(&ev);
            assert_eq!(got, want, "{name}");
            assert_eq!(got["queries"].as_array().unwrap().len(), planned_logical_calls(&ev), "{name}: query plan");
            // 非自明であること
            match name {
                "legacy_year_retry_dedupe" => assert!(got["queries"].as_array().unwrap().iter().any(|q| q["year"].is_null() && q["kind"] == "movie") && d.legacy_all.len() > 8),
                "embedded_adds_input_and_merges" => assert!(got["queries"].as_array().unwrap().iter().any(|q| q["source"] == "embedded") && d.sources.iter().any(|s| s.query_source == "both")),
                "unknown_both_kinds" => assert!(d.combined_all.iter().any(|c| c.media_type == "tv") && d.combined_all.iter().any(|c| c.media_type == "movie")),
                "no_title_no_calls" => assert!(got["queries"].as_array().unwrap().is_empty()),
                _ => {}
            }
            // matcher 層も production の関数をそのまま同じ入力で呼んだ結果と一致（binding / reference）
            let e = snap.embedded_evidence();
            let shadow = rules_tags_shadow_best_candidate(&d.combined_ranked, &d.parsed, &e);
            let (out, _, _) = bodies_ok("work:1", &d, &e, skips_tmdb(&ev)).unwrap();
            assert_eq!(out["rules_tags_shadow"]["decision"], json!(shadow.decision.as_str()), "{name}");
            assert_eq!(out["rules_tags_shadow"]["top"], json!(shadow.top.map(ident)), "{name}");
        }
        // 順序: 同点が出る fixture で ranked は confidence 降順・同点は挿入順
        let snap = snapshot_from_payload(&payload_full(Some("Alpha Movie 1999"), "a.mkv", "movie", None), 1, EvidenceClass::Live).unwrap();
        let (_, d) = derive_shape(&snap.local_evidence());
        assert!(d.legacy_ranked.len() <= RANK_CAP);
        let conf: Vec<i32> = d.legacy_ranked.iter().map(|c| c.confidence).collect();
        assert!(conf.windows(2).all(|w| w[0] >= w[1]));
        assert!(conf.windows(2).any(|w| w[0] == w[1]), "同点を含む fixture であること");
        for w in d.legacy_ranked.windows(2).filter(|w| w[0].confidence == w[1].confidence) {
            let pos = |c: &TmdbCandidate| d.legacy_all.iter().position(|a| same(a, c)).unwrap();
            assert!(pos(&w[0]) < pos(&w[1]), "同点は挿入順");
        }
    }

    #[test]
    fn release_date_is_the_frozen_response_value_stored_next_to_the_derived_year_and_never_excludes_a_unit() {
        let tags = r#"{"v":1,"title":"Delta Embedded","date":"2004"}"#;
        let us = vec![unit_full(1, payload_full(Some("Delta Legacy 1999"), "Delta Legacy 1999.mkv", "movie", Some(tags))), unit_full(2, payload_full(Some("Alpha Movie 1999"), "a.mkv", "movie", None))];
        let dir = tmp("dates");
        let mut g = Generator::begin(&dir, manifest(&us), us.clone(), clock(), None).unwrap();
        // 日付が無い／壊れた応答でも unit は除外されない
        struct Odd(Mock);
        impl Transport for Odd {
            fn get(&mut self, req: &CanonReq) -> Result<HttpResp, NetErr> {
                let mut r = fixture_body(req);
                let mut v: Value = serde_json::from_slice(&r).unwrap();
                if req.params["query"].contains("Alpha") {
                    if let Some(a) = v["results"].as_array_mut() {
                        for (i, it) in a.iter_mut().enumerate() {
                            if i % 2 == 0 {
                                it["release_date"] = json!("");
                            }
                        }
                    }
                    r = serde_json::to_vec(&v).unwrap();
                }
                Ok(HttpResp { status: 200, body: r, retry_after_secs: None })
            }
        }
        let mut t = Odd(Mock::new());
        g.run_primary(&mut t).unwrap();
        let _ = &t.0;
        let (proj, _) = read_chain(&dir.join(F_PROJ)).unwrap();
        assert_eq!(proj.len(), 2, "unit は除外されない");
        // merge で embedded が採用された candidate でも、日付は **採用された response** のもの（year と整合）
        let d1 = &proj[0]["candidates"];
        assert!(!d1.as_array().unwrap().is_empty());
        for c in d1.as_array().unwrap() {
            assert_eq!(c["year_consistent_with_release_date"], json!(true), "{c}");
            assert!(c["release_date"].is_string());
        }
        // 日付が空の response 由来の candidate は release_date をそのまま保存（空文字）し、year は既存ロジックの値、unit は残る
        let d2 = proj[1]["candidates"].as_array().unwrap();
        assert!(d2.iter().any(|c| c["release_date"] == json!("")), "空の日付も verbatim");
    }
    fn unit_full(i: i64, p: Value) -> UnitIn {
        UnitIn { key: format!("work:{i}"), matcher_input_sha256: sha_v(&p).unwrap(), payload: p, expected_decision_evidence_sha256: None }
    }

    fn crash_primary(dir: &Path, us: &[UnitIn], panic_on: &str) {
        let mut g = Generator::begin(dir, manifest(us), us.to_vec(), clock(), None).unwrap();
        let mut m = Mock::new();
        m.panic_on = Some(panic_on.into());
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| g.run_primary(&mut m)));
        assert!(r.is_err(), "crash を再現できていません");
    }

    #[test]
    fn a_crashed_primary_resumes_only_the_unexecuted_units_in_the_same_logical_run() {
        let us = units();
        let dir = tmp("resume");
        crash_primary(&dir, &us, "Gamma"); // unit 4 で crash（unit 1〜3 は完了済み）
        let mut m = Mock::new();
        let mut g = Generator::resume(&dir, manifest(&us), us.clone(), clock(), None).unwrap();
        g.run_primary(&mut m).unwrap();
        assert!(m.calls.keys().all(|k| k.contains("Gamma")), "完了済み unit は再実行しない: {:?}", m.calls.keys());
        g.commit_recovery_set().unwrap();
        g.run_recovery(&mut m).unwrap();
        g.seal(&BTreeMap::new()).unwrap();
        assert_eq!(verify_and_replay(&dir, &us).unwrap().replayed_units, 4);
        let (ui, _) = read_chain(&dir.join(F_UNIT)).unwrap();
        assert_eq!(ui.len(), 4, "unit_input は書き直さない");
        let (log, _) = read_chain(&dir.join(F_LOG)).unwrap();
        assert_eq!(log.iter().filter(|r| r["event"] == json!("resume_primary")).count(), 1);
        let prov = read_json(&dir.join("RUN_PROVENANCE.json")).unwrap();
        assert_eq!(prov["primary_resumes"], json!(1));
        let first = log[0]["at_utc"].clone();
        assert_eq!(prov["session"]["started_at_utc"], first, "session の開始は元の run_start");
    }

    #[test]
    fn resume_that_cannot_prove_continuity_is_a_technical_failure_and_cannot_be_sealed() {
        let us = units();
        // 別 manifest（code_commit 違い）
        let d1 = tmp("cont_manifest");
        crash_primary(&d1, &us, "Gamma");
        let mut other = manifest(&us);
        other["code_commit"] = json!("1".repeat(40));
        assert!(Generator::resume(&d1, other, us.clone(), clock(), None).unwrap_err().0.contains("continuity"));
        assert!(d1.join("CONTINUITY_FAILURE.json").exists());
        assert!(Generator::resume(&d1, manifest(&us), us.clone(), clock(), None).is_err(), "一度 failure になれば二度と再開できない");
        // unit 順序の入れ替え（事前 commit と不一致）
        let d2 = tmp("cont_order");
        crash_primary(&d2, &us, "Gamma");
        let mut swapped = us.clone();
        swapped.swap(0, 1);
        assert!(Generator::resume(&d2, manifest(&us), swapped, clock(), None).is_err());
        // 未完了 unit の痕跡（matcher_output だけ欠けている）
        let d3 = tmp("cont_stray");
        {
            let mut g = Generator::begin(&d3, manifest(&us), us.clone(), clock(), None).unwrap();
            g.run_primary(&mut Mock::new()).unwrap();
        }
        let p = d3.join(F_OUT);
        let lines: Vec<String> = std::fs::read_to_string(&p).unwrap().lines().map(String::from).collect();
        std::fs::write(&p, lines[..3].join("\n") + "\n").unwrap();
        assert!(Generator::resume(&d3, manifest(&us), us.clone(), clock(), None).unwrap_err().0.contains("痕跡"));
        // 完了済み記録の改ざん（chain 破断）
        let d4 = tmp("cont_chain");
        crash_primary(&d4, &us, "Gamma");
        let p = d4.join(F_OUT);
        let t = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&p, t.replacen("\"ok\"", "\"x\"", 1)).unwrap();
        assert!(Generator::resume(&d4, manifest(&us), us.clone(), clock(), None).is_err());
        // 再開回数の上限（manifest で pin）、recovery 対象確定後・封印後は再開不可
        let d5 = tmp("cont_max");
        crash_primary(&d5, &us, "Gamma");
        let mut m = Mock::new();
        m.panic_on = Some("Gamma".into());
        let mut g = Generator::resume(&d5, manifest(&us), us.clone(), clock(), None).unwrap();
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| g.run_primary(&mut m))).is_err());
        assert!(Generator::resume(&d5, manifest(&us), us.clone(), clock(), None).is_err(), "上限 1 回");
        let (d6, us6) = run_all("cont_sealed", &mut Mock::new());
        assert!(Generator::resume(&d6, manifest(&us6), us6, clock(), None).is_err());
        let d7 = tmp("cont_after_set");
        let mut g = Generator::begin(&d7, manifest(&us), us.clone(), clock(), None).unwrap();
        g.run_primary(&mut Mock::new()).unwrap();
        g.commit_recovery_set().unwrap();
        assert!(Generator::resume(&d7, manifest(&us), us.clone(), clock(), None).is_err());
        // 封印不可
        let d8 = tmp("cont_seal");
        crash_primary(&d8, &us, "Gamma");
        let mut other = manifest(&us);
        other["code_commit"] = json!("2".repeat(40));
        let _ = Generator::resume(&d8, other, us.clone(), clock(), None);
        assert!(d8.join("CONTINUITY_FAILURE.json").exists());
        assert!(verify_and_replay(&d8, &us).is_err(), "未封印の run は検証・変換できない");
    }

    #[test]
    fn the_transport_policy_is_pinned_by_hash_and_is_the_only_thing_that_drives_retries_and_pauses() {
        let us = units();
        // hash 不一致・並列度≠1・retry_after の不正値は起動しない
        let mut tampered = manifest(&us);
        tampered["transport_policy"]["max_attempts"] = json!(5);
        assert!(Generator::begin(&tmp("pol_hash"), tampered, us.clone(), clock(), None).is_err());
        let mut par = policy_json("ignore", 0);
        par["concurrency"] = json!(4);
        par["policy_sha256"] = json!(sha_v(&{ let mut b = par.clone(); b.as_object_mut().unwrap().remove("policy_sha256"); b }).unwrap());
        assert!(Generator::begin(&tmp("pol_par"), manifest_with(&us, par), us.clone(), clock(), None).is_err());
        let mut bad = policy_json("whatever", 0);
        bad["policy_sha256"] = json!(sha_v(&{ let mut b = bad.clone(); b.as_object_mut().unwrap().remove("policy_sha256"); b }).unwrap());
        assert!(Generator::begin(&tmp("pol_ra"), manifest_with(&us, bad), us.clone(), clock(), None).is_err());
        // 429 + Retry-After: ignore なら backoff だけ、honor_capped なら cap 付きで長い方
        for (mode, want) in [("ignore", 400u64), ("honor_capped", 1500u64)] {
            let dir = tmp("pol_run");
            let mut g = Generator::begin(&dir, manifest_with(&us, policy_json(mode, 1500)), us.clone(), clock(), None).unwrap();
            let mut m = Mock::new();
            m.retry_after = Some(2);
            m.fail.insert("/3/search/movie|Beta Film|".into(), vec![Ok(429)]);
            g.run_primary(&mut m).unwrap();
            assert_eq!(m.pauses, vec![want], "{mode}");
            let (req, _) = read_chain(&dir.join(F_REQ)).unwrap();
            let r = req.iter().find(|r| r["request_id"] == json!("work:2#0.movie.primary")).unwrap();
            assert_eq!(r["attempts"].as_array().unwrap().len(), 2);
            assert_eq!(r["attempts"][0]["backoff_ms"], json!(want));
            assert_eq!(r["final_status"], json!("ok"));
        }
        // 上限まで 429 → http_error（retryable → recovery 対象）。接続エラーは backoff 列に従って pause する
        let dir = tmp("pol_exhaust");
        let mut g = Generator::begin(&dir, manifest(&us), us.clone(), clock(), None).unwrap();
        let mut m = Mock::new();
        m.fail.insert("/3/search/movie|Beta Film|".into(), vec![Ok(429), Ok(429), Ok(429)]);
        m.fail.insert("/3/search/movie|Alpha Movie|1999".into(), vec![Err(NetErr::Connect), Err(NetErr::Connect), Err(NetErr::Connect)]);
        g.run_primary(&mut m).unwrap();
        assert_eq!(m.pauses, vec![400, 800, 400, 800], "最後の試行の後は待たない");
        let set = g.commit_recovery_set().unwrap();
        assert_eq!(set["set"]["unit_keys"], json!(["work:1", "work:2"]));
        // policy は MANIFEST.json に pin されたまま（封印後の検証でも再検査される）
        g.run_recovery(&mut Mock::new()).unwrap();
        g.seal(&BTreeMap::new()).unwrap();
        verify_and_replay(&dir, &us).unwrap();
        let mf = dir.join("MANIFEST.json");
        let t = std::fs::read_to_string(&mf).unwrap();
        std::fs::write(&mf, t.replace("\"max_attempts\":3", "\"max_attempts\":4")).unwrap();
        assert!(verify_and_replay(&dir, &us).is_err());
    }

    #[test]
    fn resume_policy_is_fixed_to_one_primary_resume_and_no_recovery_resume() {
        let us = units();
        for (n, r) in [(0, "forbidden"), (2, "forbidden"), (1, "allowed")] {
            let mut m = manifest(&us);
            m["resume_policy"] = json!({"max_primary_resumes": n, "recovery_resume": r});
            assert!(Generator::begin(&tmp("rp"), m, us.clone(), clock(), None).is_err(), "{n} {r}");
        }
        // recovery 中の crash: resume は禁止で CONTINUITY_FAILURE。封印・変換・FINAL 判定へは進めない
        let dir = tmp("rec_crash");
        let mut m = Mock::new();
        m.fail.insert("/3/search/movie|Beta Film|".into(), vec![Err(NetErr::Connect); 3]);
        let mut g = Generator::begin(&dir, manifest(&us), us.clone(), clock(), None).unwrap();
        g.run_primary(&mut m).unwrap();
        g.commit_recovery_set().unwrap();
        m.panic_on = Some("Beta".into());
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| g.run_recovery(&mut m))).is_err());
        let err = Generator::resume(&dir, manifest(&us), us.clone(), clock(), None).unwrap_err();
        assert!(err.0.contains("continuity") && err.0.contains("resume 禁止"), "{}", err.0);
        assert!(dir.join("CONTINUITY_FAILURE.json").exists());
        assert!(verify_and_replay(&dir, &us).is_err());
        let gts: BTreeMap<String, Gt> = us.iter().map(|u| (u.key.clone(), Gt::NoMatch)).collect();
        assert!(final_input(&dir, &us, &gts).is_err());
        // 封印済みの run に再開を試みても artifact には何も書かない
        let (d6, us6) = run_all("sealed_resume", &mut Mock::new());
        let before = walk(&d6).len();
        assert!(Generator::resume(&d6, manifest(&us6), us6, clock(), None).is_err());
        assert_eq!(walk(&d6).len(), before);
        assert!(!d6.join("CONTINUITY_FAILURE.json").exists());
    }

    #[test]
    fn the_generator_source_reads_no_gt_label_db_h1_jev_or_live_http() {
        let src = include_str!("pr4_pf0f.rs");
        let body = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        for bad in ["rusqlite", "metadata_match_labels", "gt_review", "gt_sampling", "jev_", "reqwest", "TmdbClient", "get_api_key", "std::env::var", "SystemTime", "Instant::now"] {
            assert!(!body.contains(bad), "禁止参照: {bad}");
        }
    }
}
