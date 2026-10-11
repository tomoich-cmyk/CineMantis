//! PR4-PF0I1: sealed blind GT（PF0G）→ FINAL evaluator（PF0B）の adapter（**synthetic のみ・test ビルドだけ・製品には出ない**）。
//!
//! 契約は `docs/PR4_PF0I0_GT_EVALUATOR_ADAPTER_CONTRACT.md`、schema は `docs/pr4_pf0i0_gt_adapter_schema.json`。
//! adapter は GT 側の**変換層**で、判定ロジックを持たない。PF0B（`pr4_pf0b.rs`）は変更しない。
//! 入力は呼び出し側が渡す bytes / 値だけ。file・DB・network・環境変数に触れない（静的検査あり）。
//! 失敗したら PF0B を呼ばず、独自の失敗 envelope（`stage = gt_adapter`）を返す。key・identity・token・review_unit_id は出力に出さない。

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::services::pr4_blind_gt::{canon, derive_candidate_token, derive_review_unit_id, hex_sha, validate_unit_key};
use crate::services::pr4_pf0b::{membership_commitment, FinalInput, Gt, Machine, PolicyConfig, PolicyProvenance, Unit, UnevaluableAuto};

pub const ADAPTER_VERSION: &str = "pr4-pf0i0-gt-adapter-1";
pub const TRUSTED_GT_SOURCE: &str = "frozen_blind_h2_gt";
pub const BUNDLE_VERSION: &str = "pr4-pf0i-unblind-bundle-1";
/// PF0C の凍結 policy（`docs/pr4_final_policy.json`）の canonical SHA-256
pub const FROZEN_POLICY_SHA256: &str = "1d575b41f2fd22ebb36b8b90a7a45a8ef3e038906f3f634ecd84c472fb232ac4";
/// `build_secret_commitment = SHA256(build_secret_hex の ASCII bytes)`（PF0G は pin の値の定義を持たないため PF0I が固定する。契約 §4）
pub fn build_secret_commitment(build_secret_hex: &str) -> String {
    hex_sha(build_secret_hex.as_bytes())
}

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const MAX_TMDB_ID: i64 = 9_007_199_254_740_992;
const PIN_KEYS: [&str; 8] = [
    "protocol_sha256",
    "ui_package_sha256",
    "shuffle_seed_commitment",
    "attestation_policy_sha256",
    "unblind_map_sha256",
    "build_secret_commitment",
    "a8_leak_scan_report_sha256",
    "machine_freeze_anchor_commitment",
];
const EVENT_TYPES: [&str; 15] = [
    "init", "session_start", "session_end", "qc_plan", "lease", "lease_expired", "lease_release", "search", "decision", "correction", "qc_decision", "defer", "defer_final",
    "integrity_anomaly", "seal",
];
const CORRECTION_REASONS: [&str; 4] = ["mis_click", "misread_evidence", "new_evidence_in_tmdb", "other_with_note"];
/// ledger の構造化項目に持てない名前（PF0G1 `scan_forbidden` の KEYS から、ledger の正規フィールド `decision` / `tmdb_id` を除いたもの）
const LEDGER_FORBIDDEN_KEYS: [&str; 20] = [
    "verdict", "score", "rank", "rules", "jev", "matcher", "source", "promotion", "precision", "coverage", "abstention", "workid", "runid", "evaluationunitkey", "filepath",
    "firstseenat", "candidatecount", "buildsecret", "unblind", "candidatemapping",
];
/// 自由記述（adapter は読まない・走査しない）
const FREE_TEXT_KEYS: [&str; 6] = ["note", "query", "reviewer_id", "title", "year", "verifier_impl"];

type Codes = Vec<&'static str>;
type Res<T> = Result<T, Codes>;

fn refuse<T>(code: &'static str) -> Res<T> {
    Err(vec![code])
}

// ─── 入力 ────────────────────────────────────────────────────────────────────────────────

/// FINAL runner が外部（audit anchor chain / PF0H の正式 loader）から渡す期待値。ledger / seal の自己申告だけは信じない
#[derive(Debug, Clone)]
pub struct Anchors {
    /// PF0H の authoritative membership commitment の期待値（audit anchor 側）
    pub expected_membership_commitment: String,
    /// PF0H final manifest SHA の期待値（audit anchor 側）
    pub expected_pf0h_final_manifest_sha256: String,
    /// seal event の `event_commitment`（別経路で記録した値）
    pub review_seal_sha256: String,
    /// PF0G の pin `machine_freeze_anchor_commitment` の期待値
    pub machine_freeze_anchor_commitment: String,
    /// `membership_commitment` の第 2 引数にそのまま渡す。adapter は解釈・分解・比較しない
    pub eligibility_rule_version_opaque: String,
}

/// PF0H の（唯一の）正式 loader が出した 2 値。adapter は PF0H 内部の version や補助 hash を解釈しない
#[derive(Debug, Clone)]
pub struct Pf0hLoaded {
    pub authoritative_membership_commitment: String,
    pub final_manifest_sha256: String,
}

pub struct AdapterInput<'a> {
    pub ledger_bytes: &'a [u8],
    pub unblind_bundle_bytes: &'a [u8],
    pub anchors: &'a Anchors,
    pub pf0h: &'a Pf0hLoaded,
    pub policy: &'a PolicyConfig,
}

// ─── 出力 ────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    NotChecked,
    Match,
    Mismatch,
}
impl Check {
    fn s(self) -> &'static str {
        match self {
            Check::NotChecked => "not_checked",
            Check::Match => "match",
            Check::Mismatch => "mismatch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorChecks {
    pub policy_pinned: Check,
    pub ledger_chain: Check,
    pub review_seal_sha256: Check,
    pub seal_pins_equal_init_pins: Check,
    pub unblind_bundle_sha256: Check,
    pub build_secret_commitment: Check,
    pub membership_commitment: Check,
    pub pf0h_final_manifest_sha256: Check,
    pub machine_freeze_anchor_commitment: Check,
}
impl Default for AnchorChecks {
    fn default() -> Self {
        let n = Check::NotChecked;
        AnchorChecks {
            policy_pinned: n,
            ledger_chain: n,
            review_seal_sha256: n,
            seal_pins_equal_init_pins: n,
            unblind_bundle_sha256: n,
            build_secret_commitment: n,
            membership_commitment: n,
            pf0h_final_manifest_sha256: n,
            machine_freeze_anchor_commitment: n,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    pub n_members: u64,
    pub n_events: u64,
    pub n_corrections_applied: u64,
    pub n_confirm: u64,
    pub n_pick_other: u64,
    pub n_none: u64,
    pub n_defer_final: u64,
}

/// count-only。key / identity / token / review_unit_id / 自由記述を含めない。REFUSED のとき件数は 0
#[derive(Debug, Clone)]
pub struct AdapterReport {
    pub ready: bool,
    pub checks: AnchorChecks,
    pub counts: Counts,
    pub refusal_codes: Vec<&'static str>,
}

impl AdapterReport {
    pub fn to_value(&self) -> Value {
        let c = &self.counts;
        let k = &self.checks;
        json!({
            "adapter_version": ADAPTER_VERSION, "trusted_gt_source": TRUSTED_GT_SOURCE,
            "status": if self.ready { "READY" } else { "REFUSED" },
            "anchor_checks": {"policy_pinned": k.policy_pinned.s(), "ledger_chain": k.ledger_chain.s(), "review_seal_sha256": k.review_seal_sha256.s(),
                "seal_pins_equal_init_pins": k.seal_pins_equal_init_pins.s(), "unblind_bundle_sha256": k.unblind_bundle_sha256.s(),
                "build_secret_commitment": k.build_secret_commitment.s(), "membership_commitment": k.membership_commitment.s(),
                "pf0h_final_manifest_sha256": k.pf0h_final_manifest_sha256.s(), "machine_freeze_anchor_commitment": k.machine_freeze_anchor_commitment.s()},
            "counts": {"n_members": c.n_members, "n_events": c.n_events, "n_corrections_applied": c.n_corrections_applied, "n_confirm": c.n_confirm,
                "n_pick_other": c.n_pick_other, "n_none": c.n_none, "n_defer_final": c.n_defer_final},
            "refusal_codes": self.refusal_codes,
        })
    }

    /// adapter が REFUSED のときの FINAL 側の出力。**PF0B の report に似せない**（`report_version` / `decision` / `metrics` を持たない独自 envelope）
    pub fn failure_envelope(&self) -> Value {
        json!({
            "stage": "gt_adapter", "status": "TECHNICAL_FAILURE", "adapter_version": ADAPTER_VERSION,
            "reason_code": self.refusal_codes.first().copied().unwrap_or("unknown"), "reason_codes": self.refusal_codes,
        })
    }
}

/// evaluator に渡してよい唯一の GT 型。コンストラクタは [`adapt`] だけ（手組みで `frozen_blind_h2_gt` を名乗れない）
#[derive(Debug, Clone)]
pub struct TrustedGtSet {
    membership_commitment: String,
    entries: Vec<(String, Gt)>,
}
impl TrustedGtSet {
    pub fn membership_commitment(&self) -> &str {
        &self.membership_commitment
    }
    pub fn entries(&self) -> &[(String, Gt)] {
        &self.entries
    }
    pub fn trusted_gt_source(&self) -> &'static str {
        TRUSTED_GT_SOURCE
    }
}

pub struct AdapterOutcome {
    pub report: AdapterReport,
    pub gt: Option<TrustedGtSet>,
}

// ─── PF0H loader（唯一）──────────────────────────────────────────────────────────────────

/// PF0H1 の凍結 commit（loader の規則の出所。anchor）。`git show` で読み取り専用に参照した
pub const PF0H1_COMMIT: &str = "81e637210f7324686f72b7f6be850950c2d9e04e";
/// `FINAL_MEMBERSHIP_COMMITMENT.json` の binding 規則: JSON を parse → canonical JSON → UTF-8 → SHA-256（体裁に依存しない）
pub const PF0H1_FINAL_MANIFEST_RULE: &str = "pr4-pf0h1-final-manifest-sha-1";
const PF0H_FINAL_MEMBERSHIP_PROTOCOL: &str = "pr4-pf0h0-final-membership-1";

/// PF0H の正式 loader（1 つだけ）。`FINAL_MEMBERSHIP_COMMITMENT.json` の bytes から 2 値を取り出す。
/// - `final_manifest_sha256` = 規則 `pr4-pf0h1-final-manifest-sha-1`（`H2_CLOSE.json` の raw SHA は使わない）
/// - `authoritative_membership_commitment` = 同ファイルの `membership_commitment_sha256`（`authoritative: true` が必須）
/// 補助値（`auxiliary_audit_values`）・`eligibility_rule_version` は読まない・解釈しない
pub fn load_pf0h_final_membership(file_bytes: &[u8]) -> Res<Pf0hLoaded> {
    let v: Value = serde_json::from_slice(file_bytes).map_err(|_| vec!["pf0h_final_manifest_invalid"])?;
    let canonical = canon(&v).map_err(|_| vec!["pf0h_final_manifest_invalid"])?;
    let commitment = v.get("membership_commitment_sha256").and_then(Value::as_str).unwrap_or("");
    if v.get("protocol").and_then(Value::as_str) != Some(PF0H_FINAL_MEMBERSHIP_PROTOCOL) || v.get("authoritative") != Some(&json!(true)) || !is_hex64(commitment) {
        return refuse("pf0h_final_manifest_invalid");
    }
    Ok(Pf0hLoaded { authoritative_membership_commitment: commitment.to_string(), final_manifest_sha256: hex_sha(canonical.as_bytes()) })
}

/// PF0H1 の commit anchor を明示して loader を呼ぶ入口。anchor が凍結 commit と違えば拒否（`pf0h_final_manifest_invalid`）
pub fn load_pf0h_final_membership_anchored(file_bytes: &[u8], pf0h1_commit_anchor: &str) -> Res<Pf0hLoaded> {
    if pf0h1_commit_anchor != PF0H1_COMMIT {
        return refuse("pf0h_final_manifest_invalid");
    }
    load_pf0h_final_membership(file_bytes)
}

// ─── unblind bundle ──────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleCandidate {
    candidate_token: String,
    media_type: String,
    tmdb_id: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleUnit {
    review_unit_id: String,
    work_key: String,
    candidates: Vec<BundleCandidate>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    bundle_version: String,
    build_secret_hex: String,
    units: Vec<BundleUnit>,
}

struct UnitMap {
    work_key: String,
    tokens: BTreeMap<String, (String, i64)>,
    identities: BTreeSet<(String, i64)>,
}

fn valid_target(media_type: &str, id: i64) -> bool {
    matches!(media_type, "movie" | "tv") && (1..=MAX_TMDB_ID).contains(&id)
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// parse の**前**に SHA を照合する。全単射・build_secret からの再導出（review_unit_id・candidate_token）を検査する
fn load_bundle(bytes: &[u8], pinned_sha: &str, pinned_secret_commitment: &str, checks: &mut AnchorChecks) -> Res<(String, BTreeMap<String, UnitMap>)> {
    if hex_sha(bytes) != pinned_sha {
        checks.unblind_bundle_sha256 = Check::Mismatch;
        return refuse("unblind_bundle_sha_mismatch");
    }
    checks.unblind_bundle_sha256 = Check::Match;
    let b: Bundle = serde_json::from_slice(bytes).map_err(|_| vec!["unblind_bundle_not_bijective"])?;
    if b.bundle_version != BUNDLE_VERSION || b.units.is_empty() || !is_hex64(&b.build_secret_hex) {
        return refuse("unblind_bundle_not_bijective");
    }
    if build_secret_commitment(&b.build_secret_hex) != pinned_secret_commitment {
        checks.build_secret_commitment = Check::Mismatch;
        return refuse("build_secret_commitment_mismatch");
    }
    checks.build_secret_commitment = Check::Match;
    let mut by_unit: BTreeMap<String, UnitMap> = BTreeMap::new();
    let mut work_keys: BTreeSet<String> = BTreeSet::new();
    for u in b.units {
        if validate_unit_key(&u.work_key).is_err() {
            return refuse("unblind_bundle_not_bijective");
        }
        if by_unit.contains_key(&u.review_unit_id) || !work_keys.insert(u.work_key.clone()) {
            return refuse("unblind_bundle_not_bijective");
        }
        match derive_review_unit_id(&b.build_secret_hex, &u.work_key) {
            Ok(id) if id == u.review_unit_id => {}
            _ => return refuse("review_unit_id_not_derivable"),
        }
        let mut tokens = BTreeMap::new();
        let mut identities = BTreeSet::new();
        for c in u.candidates {
            if !valid_target(&c.media_type, c.tmdb_id) {
                return refuse("identity_invalid");
            }
            match derive_candidate_token(&b.build_secret_hex, &u.review_unit_id, &c.media_type, c.tmdb_id) {
                Ok(t) if t == c.candidate_token => {}
                _ => return refuse("candidate_token_not_derivable"),
            }
            if tokens.insert(c.candidate_token.clone(), (c.media_type.clone(), c.tmdb_id)).is_some() || !identities.insert((c.media_type, c.tmdb_id)) {
                return refuse("candidate_token_duplicate_in_unit");
            }
        }
        by_unit.insert(u.review_unit_id.clone(), UnitMap { work_key: u.work_key, tokens, identities });
    }
    Ok((b.build_secret_hex, by_unit))
}

// ─── ledger ──────────────────────────────────────────────────────────────────────────────

fn parse_ledger(bytes: &[u8]) -> Res<Vec<Value>> {
    let text = std::str::from_utf8(bytes).map_err(|_| vec!["ledger_chain_broken"])?;
    if text.is_empty() || !text.ends_with('\n') {
        return refuse("ledger_chain_broken");
    }
    let mut out: Vec<Value> = Vec::new();
    let mut prev = GENESIS.to_string();
    for (i, line) in text[..text.len() - 1].split('\n').enumerate() {
        let v: Value = serde_json::from_str(line).map_err(|_| vec!["ledger_chain_broken"])?;
        let c = canon(&v).map_err(|_| vec!["ledger_chain_broken"])?;
        if c != line {
            return refuse("ledger_chain_broken");
        }
        match v.get("seq").and_then(Value::as_u64) {
            Some(s) if s == i as u64 => {}
            Some(_) => return refuse("ledger_seq_gap"),
            None => return refuse("ledger_chain_broken"),
        }
        if v.get("prev_commitment").and_then(Value::as_str) != Some(prev.as_str()) {
            return refuse("ledger_chain_broken");
        }
        let claimed = v.get("event_commitment").and_then(Value::as_str).unwrap_or("").to_string();
        let mut body = v.clone();
        if let Some(o) = body.as_object_mut() {
            o.remove("event_commitment");
        }
        let expect = hex_sha(format!("{}\n{}", canon(&body).map_err(|_| vec!["ledger_chain_broken"])?, prev).as_bytes());
        if claimed != expect {
            return refuse("ledger_chain_broken");
        }
        prev = claimed;
        out.push(v);
    }
    Ok(out)
}

fn ty(e: &Value) -> &str {
    e.get("type").and_then(Value::as_str).unwrap_or("")
}

/// ledger の構造化項目（キー名と enum 値。自由記述は除く）に機械状態・work key を持てない
fn scan_structure(v: &Value, out: &mut Codes) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                // 自由記述は読まない。`pins` は PIN_KEYS と値の形を別に厳密検査する（キー名に unblind / buildsecret を含むため）
                if FREE_TEXT_KEYS.contains(&k.as_str()) || k == "pins" {
                    continue;
                }
                let norm: String = k.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
                if k != "source_media_kind" && LEDGER_FORBIDDEN_KEYS.iter().any(|f| norm.contains(f)) {
                    out.push("ledger_contains_machine_state");
                }
                scan_structure(x, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| scan_structure(x, out)),
        Value::String(s) => {
            if s.starts_with("work:") {
                out.push("ledger_contains_work_key");
            }
            if matches!(s.trim().to_uppercase().as_str(), "AUTO" | "UNRESOLVED") {
                out.push("ledger_contains_machine_state");
            }
        }
        _ => {}
    }
}

fn pins_of(e: &Value) -> Option<BTreeMap<String, String>> {
    let o = e.get("pins")?.as_object()?;
    if o.len() != PIN_KEYS.len() {
        return None;
    }
    let mut m = BTreeMap::new();
    for k in PIN_KEYS {
        let s = o.get(k)?.as_str()?;
        if !is_hex64(s) {
            return None;
        }
        m.insert(k.to_string(), s.to_string());
    }
    Some(m)
}

// ─── replay（終端判断の取り出しだけ。PF0G1 の状態機械は再実装しない）───────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum Eff {
    Confirm(String),
    Pick(String, i64),
    NoneMatch,
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum St {
    Fresh,
    Deferred,
    Resolved,
    DeferFinal,
}

struct Replayed {
    eff: BTreeMap<String, Eff>,
    corrections: u64,
}

fn parse_decision(e: &Value, um: &UnitMap) -> Res<Eff> {
    let d = e.get("decision").and_then(Value::as_str).unwrap_or("");
    let non_null = |k: &str| e.get(k).map(|v| !v.is_null()).unwrap_or(false);
    match d {
        "confirm" => {
            if non_null("tmdb_id") || non_null("media_type") || e.get("candidate_ref").is_some() || non_null("tmdb_verification") {
                return refuse("inline_identity_present");
            }
            let t = e.get("candidate_token").and_then(Value::as_str).ok_or(vec!["decision_shape_invalid"])?;
            if !um.tokens.contains_key(t) {
                return refuse("token_not_in_presented_set");
            }
            Ok(Eff::Confirm(t.to_string()))
        }
        "pick_other" => {
            if non_null("candidate_token") {
                return refuse("decision_shape_invalid");
            }
            let id = e.get("tmdb_id").and_then(Value::as_i64).unwrap_or(0);
            let mt = e.get("media_type").and_then(Value::as_str).unwrap_or("");
            if !valid_target(mt, id) {
                return refuse("identity_invalid");
            }
            let v = e.get("tmdb_verification").filter(|v| v.is_object()).ok_or(vec!["pick_other_not_verified"])?;
            let ok = v.get("http_status").and_then(Value::as_i64) == Some(200)
                && v.get("requested_id").and_then(Value::as_i64) == Some(id)
                && v.get("response_id").and_then(Value::as_i64) == Some(id)
                && v.get("requested_media_type").and_then(Value::as_str) == Some(mt)
                && v.get("response_media_type").and_then(Value::as_str) == Some(mt)
                && v.get("response_body_sha256").and_then(Value::as_str).map(is_hex64).unwrap_or(false);
            if !ok {
                return refuse("pick_other_not_verified");
            }
            if um.identities.contains(&(mt.to_string(), id)) {
                return refuse("pick_other_in_presented_set");
            }
            Ok(Eff::Pick(mt.to_string(), id))
        }
        "none" => {
            if non_null("candidate_token") || non_null("tmdb_id") || non_null("media_type") {
                return refuse("decision_shape_invalid");
            }
            Ok(Eff::NoneMatch)
        }
        _ => refuse("decision_shape_invalid"),
    }
}

fn replay(events: &[Value], by_unit: &BTreeMap<String, UnitMap>) -> Res<Replayed> {
    let mut st: BTreeMap<String, St> = by_unit.keys().map(|k| (k.clone(), St::Fresh)).collect();
    let mut eff: BTreeMap<String, Eff> = BTreeMap::new();
    let mut subs: BTreeSet<String> = BTreeSet::new();
    let mut corrections = 0u64;
    for e in events {
        let t = ty(e);
        if !matches!(t, "decision" | "correction" | "qc_decision" | "defer" | "defer_final") {
            continue;
        }
        let unit = e.get("unit").and_then(Value::as_str).ok_or(vec!["decision_shape_invalid"])?;
        let um = by_unit.get(unit).ok_or(vec!["review_unit_without_map_entry"])?;
        let sub = e.get("submission_id").and_then(Value::as_str).ok_or(vec!["decision_shape_invalid"])?;
        if !subs.insert(sub.to_string()) {
            return refuse("gt_conflict");
        }
        let cur = st[unit];
        match t {
            "decision" => {
                let d = parse_decision(e, um)?;
                match cur {
                    St::Fresh | St::Deferred => {
                        st.insert(unit.to_string(), St::Resolved);
                        eff.insert(unit.to_string(), d);
                    }
                    St::Resolved if eff.get(unit) == Some(&d) => return refuse("duplicate_record_identical"),
                    _ => return refuse("gt_conflict"),
                }
            }
            "defer" => match cur {
                St::Fresh => {
                    st.insert(unit.to_string(), St::Deferred);
                }
                _ => return refuse("gt_conflict"),
            },
            "defer_final" => match cur {
                St::Deferred => {
                    st.insert(unit.to_string(), St::DeferFinal);
                    eff.insert(unit.to_string(), Eff::Unresolved);
                }
                _ => return refuse("gt_conflict"),
            },
            "correction" => {
                if !matches!(cur, St::Resolved | St::DeferFinal) {
                    return refuse("correction_chain_invalid");
                }
                if !e.get("correction_reason").and_then(Value::as_str).map(|r| CORRECTION_REASONS.contains(&r)).unwrap_or(false) {
                    return refuse("correction_reason_invalid");
                }
                let d = parse_decision(e, um)?;
                st.insert(unit.to_string(), St::Resolved);
                eff.insert(unit.to_string(), d);
                corrections += 1;
            }
            _ => {
                // qc_decision: GT に使わない。終端状態の unit にしか現れない
                if !matches!(cur, St::Resolved | St::DeferFinal) {
                    return refuse("non_terminal_state_in_sealed_ledger");
                }
            }
        }
    }
    for (unit, s) in &st {
        match s {
            St::Fresh => return refuse("gt_missing_for_member"),
            St::Deferred => return refuse("non_terminal_state_in_sealed_ledger"),
            St::Resolved | St::DeferFinal => debug_assert!(eff.contains_key(unit)),
        }
    }
    Ok(Replayed { eff, corrections })
}

// ─── adapt ───────────────────────────────────────────────────────────────────────────────

pub fn adapt(input: &AdapterInput) -> AdapterOutcome {
    let mut checks = AnchorChecks::default();
    let mut counts = Counts::default();
    match run(input, &mut checks, &mut counts) {
        Ok(gt) => AdapterOutcome { report: AdapterReport { ready: true, checks, counts, refusal_codes: vec![] }, gt: Some(gt) },
        Err(mut codes) => {
            codes.dedup();
            AdapterOutcome { report: AdapterReport { ready: false, checks, counts: Counts::default(), refusal_codes: codes }, gt: None }
        }
    }
}

fn run(input: &AdapterInput, checks: &mut AnchorChecks, counts: &mut Counts) -> Res<TrustedGtSet> {
    let a = input.anchors;
    // 1. anchors が揃っている
    let hexes = [
        &a.expected_membership_commitment,
        &a.expected_pf0h_final_manifest_sha256,
        &a.review_seal_sha256,
        &a.machine_freeze_anchor_commitment,
        &input.pf0h.authoritative_membership_commitment,
        &input.pf0h.final_manifest_sha256,
    ];
    if hexes.iter().any(|h| !is_hex64(h)) || a.eligibility_rule_version_opaque.is_empty() {
        return refuse("anchor_missing");
    }
    // 2. policy が凍結 policy そのもの（BlockEvaluation）
    let pinned = matches!(&input.policy.provenance, PolicyProvenance::Frozen { policy_sha256 } if policy_sha256 == FROZEN_POLICY_SHA256)
        && input.policy.unevaluable_auto == UnevaluableAuto::BlockEvaluation;
    checks.policy_pinned = if pinned { Check::Match } else { Check::Mismatch };
    if !pinned {
        return refuse("policy_not_pinned_frozen");
    }
    // 3. ledger の chain・位置・構造
    let events = match parse_ledger(input.ledger_bytes) {
        Ok(e) => e,
        Err(c) => {
            checks.ledger_chain = Check::Mismatch;
            return Err(c);
        }
    };
    checks.ledger_chain = Check::Match;
    counts.n_events = events.len() as u64;
    if events.iter().any(|e| !EVENT_TYPES.contains(&ty(e))) {
        return refuse("unknown_event_type");
    }
    if ty(&events[0]) != "init" || events.iter().filter(|e| ty(e) == "init").count() != 1 {
        return refuse("init_missing_or_not_first");
    }
    let seals: Vec<usize> = events.iter().enumerate().filter(|(_, e)| ty(e) == "seal").map(|(i, _)| i).collect();
    if seals.len() != 1 {
        return refuse("seal_missing_or_duplicated");
    }
    if seals[0] != events.len() - 1 {
        return refuse("events_after_seal");
    }
    let (init, seal) = (&events[0], &events[events.len() - 1]);
    // 4. review seal SHA（外部 anchor との完全一致）
    let seal_sha = seal.get("event_commitment").and_then(Value::as_str).unwrap_or("");
    checks.review_seal_sha256 = if seal_sha == a.review_seal_sha256 { Check::Match } else { Check::Mismatch };
    if seal_sha != a.review_seal_sha256 {
        return refuse("review_seal_sha_mismatch");
    }
    if events.len() < 2 || seal.get("ledger_head_before").and_then(Value::as_str) != events[events.len() - 2].get("event_commitment").and_then(Value::as_str) {
        return refuse("ledger_head_before_mismatch");
    }
    if !seal.get("audit_report_sha256").and_then(Value::as_str).map(is_hex64).unwrap_or(false) {
        return refuse("seal_pins_missing");
    }
    // 5. pin（init と seal の完全一致）
    let (Some(ip), Some(sp)) = (pins_of(init), pins_of(seal)) else { return refuse("seal_pins_missing") };
    checks.seal_pins_equal_init_pins = if ip == sp { Check::Match } else { Check::Mismatch };
    if ip != sp {
        return refuse("seal_pins_differ_from_init");
    }
    if events.iter().any(|e| ty(e) == "integrity_anomaly") {
        return refuse("integrity_anomaly_present");
    }
    checks.machine_freeze_anchor_commitment = if sp["machine_freeze_anchor_commitment"] == a.machine_freeze_anchor_commitment { Check::Match } else { Check::Mismatch };
    if checks.machine_freeze_anchor_commitment == Check::Mismatch {
        return refuse("machine_freeze_commitment_mismatch");
    }
    let mut leak = Vec::new();
    events.iter().for_each(|e| scan_structure(e, &mut leak));
    if !leak.is_empty() {
        return Err(leak);
    }
    // 6. unblind bundle（parse の前に SHA）
    let (_secret, by_unit) = load_bundle(input.unblind_bundle_bytes, &sp["unblind_map_sha256"], &sp["build_secret_commitment"], checks)?;
    let n = by_unit.len() as i64;
    if init.get("unit_count").and_then(Value::as_i64) != Some(n) || seal.get("unit_count").and_then(Value::as_i64) != Some(n) {
        return refuse("unit_count_mismatch");
    }
    // 7. membership / PF0H（完全一致だけ）
    let keys: Vec<String> = by_unit.values().map(|u| u.work_key.clone()).collect();
    let derived = membership_commitment(&keys, &a.eligibility_rule_version_opaque);
    let m_ok = derived == a.expected_membership_commitment && derived == input.pf0h.authoritative_membership_commitment;
    checks.membership_commitment = if m_ok { Check::Match } else { Check::Mismatch };
    if !m_ok {
        return refuse("membership_commitment_mismatch");
    }
    let p_ok = input.pf0h.final_manifest_sha256 == a.expected_pf0h_final_manifest_sha256;
    checks.pf0h_final_manifest_sha256 = if p_ok { Check::Match } else { Check::Mismatch };
    if !p_ok {
        return refuse("pf0h_final_manifest_mismatch");
    }
    // 8. replay と GT への写像
    let r = replay(&events, &by_unit)?;
    let mut entries: Vec<(String, Gt)> = Vec::with_capacity(by_unit.len());
    for (unit, um) in &by_unit {
        let g = match &r.eff[unit] {
            Eff::Confirm(t) => {
                counts.n_confirm += 1;
                let (mt, id) = um.tokens[t].clone();
                Gt::Positive(mt, id)
            }
            Eff::Pick(mt, id) => {
                counts.n_pick_other += 1;
                Gt::Positive(mt.clone(), *id)
            }
            Eff::NoneMatch => {
                counts.n_none += 1;
                Gt::NoMatch
            }
            Eff::Unresolved => {
                counts.n_defer_final += 1;
                Gt::Unresolved
            }
        };
        entries.push((um.work_key.clone(), g));
    }
    entries.sort_by(|x, y| x.0.cmp(&y.0));
    counts.n_members = entries.len() as u64;
    counts.n_corrections_applied = r.corrections;
    Ok(TrustedGtSet { membership_commitment: derived, entries })
}

// ─── assembler ───────────────────────────────────────────────────────────────────────────

/// 機械出力（PF0F 由来・key → Machine）と TrustedGtSet を key で結合する。units は key 昇順。
/// machine 側に member 外の key があれば失敗（`machine_output_for_non_member`）。member に machine 出力が無ければ `Machine::Missing`（PF0B が TECHNICAL_FAILURE にする）
pub fn assemble_final_input(machine: &BTreeMap<String, Machine>, gt: &TrustedGtSet, eligibility_rule_version_opaque: &str) -> Result<FinalInput, &'static str> {
    let members: BTreeSet<&String> = gt.entries.iter().map(|(k, _)| k).collect();
    if machine.keys().any(|k| !members.contains(k)) {
        return Err("machine_output_for_non_member");
    }
    let units = gt
        .entries
        .iter()
        .map(|(k, g)| Unit { key: k.clone(), machine: machine.get(k).cloned().unwrap_or(Machine::Missing), gt: g.clone() })
        .collect();
    Ok(FinalInput {
        units,
        expected_membership_commitment: gt.membership_commitment.clone(),
        eligibility_rule_version: eligibility_rule_version_opaque.to_string(),
        run_failure: None,
    })
}

/// assembler 失敗用の envelope（adapter の失敗 envelope と同じ形）
pub fn assembler_failure_envelope(code: &'static str) -> Value {
    json!({"stage": "gt_adapter", "status": "TECHNICAL_FAILURE", "adapter_version": ADAPTER_VERSION, "reason_code": code, "reason_codes": [code]})
}

#[cfg(test)]
#[path = "pr4_pf0i_tests.rs"]
mod tests;
