//! PR4-PF0H1 `pr4_pf0h_close`: H2 close の成果物生成（**読み取り専用・test ビルドだけ・手動専用・製品には出ない**）。契約: `docs/PR4_PF0H0_H2_CLOSE_CONTRACT.md`（r2）。
//!
//! ledger / checkpoint chain / payload / reports を **読むだけ**で、DB・snapshot・matcher・PF0E のコードは使わない（ソース走査テストで固定）。
//! 検証 V0〜V16 が全部通ったときだけ、新規の出力 dir に `H2_CLOSE.json` / `FINAL_MEMBERSHIP_COMMITMENT.json` / `H2_LEDGER_FINAL_FREEZE.json` を書く。
//! 1 つでも失敗すれば 3 ファイルは作らず、`H2_CLOSE_STOP.json`（check id・コード・件数のみ）だけを書く。fail closed。waive できるのは pin 済みで
//! `accepted_non_material` の extra run（cadence）だけ。他の失敗は waive 不可。
//! authoritative な membership commitment は PF0B の `membership_commitment`。`unit_set_sha256` / `unit_input_sha256` は補助の監査値で、promotion 判断には使わない。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::services::pr4_pf0b::membership_commitment;

pub const PROTOCOL: &str = "pr4-pf0h0-h2-close-1";
pub const CONTRACT_VERSION: &str = "pr4-pf0h0-close-1";
pub const EXPECTED_COUNT: u64 = 946;
pub const RULE_VERSION: &str = "pr4-h2-enrollment-rules-2";
pub const PAYLOAD_SCHEMA_VERSION: &str = "pr4-h2-input-payload-1";
pub const CANONICALIZATION_VERSION: &str = "pr4-canonical-json-1";
/// PF0E enrollment tool の SHA は集合ではなく時系列で検証する（checkpoint seq 1〜3 は OLD のみ、seq 4 以降は NEW のみ）
pub const TOOL_SHA_OLD: &str = "183e0c73c14c20afd9ed34da33586f9599f96d1fff95c5c9a79d0b78855e3213";
pub const TOOL_SHA_NEW: &str = "36f869f6d197615a610a50e1a164f1492eae06dd1c0eaad778c5311890518d08";
pub const TOOL_SWITCH_SEQ: u64 = 4;
pub const PF0E3_MAIN_COMMIT: &str = "69d150d633ad1097319e27545962d989a92aebd9";
pub const PF0E3_ORIGINAL_COMMIT: &str = "98e3917fa022d98dd8c3e9293a865f8b44b6a13b";
pub const DAILY_FREEZE_AMENDMENT_2_SHA256: &str = "f4a2eb6d92010aa35425dbe7cd882143ff026f4ba6dae755bfa5ebc6e42a0263";
/// ledger に missed record の無い唯一の historical cadence exception（2026-10-10 の scheduled observation #2）。一般化しない
pub const EXCEPTION_JST_DATE: &str = "2026-10-10";
pub const EXCEPTION_AUDIT_NOTE_SHA256: &str = "5f80972600dbb92395ccdf63ecacc3b49b0fda30fabd3c86f572149e26568b68";
pub const H1_COMMITMENT: &str = "22cc00a81f3bf58049c1aaf45668e4fdf7b678662cf9c99cf2b591a8f983e031";
const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
pub const ARTIFACTS: [&str; 3] = ["H2_CLOSE.json", "FINAL_MEMBERSHIP_COMMITMENT.json", "H2_LEDGER_FINAL_FREEZE.json"];
pub const STOP_FILE: &str = "H2_CLOSE_STOP.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fail {
    pub id: &'static str,
    pub code: &'static str,
    pub observed: Option<u64>,
    pub expected: Option<u64>,
}

pub type R<T> = Result<T, Fail>;

pub fn fail<T>(id: &'static str, code: &'static str) -> R<T> {
    Err(Fail { id, code, observed: None, expected: None })
}

fn fail_n<T>(id: &'static str, code: &'static str, observed: u64, expected: u64) -> R<T> {
    Err(Fail { id, code, observed: Some(observed), expected: Some(expected) })
}

pub fn hex_sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn expected_tool_sha(checkpoint_seq: u64) -> &'static str {
    if checkpoint_seq < TOOL_SWITCH_SEQ {
        TOOL_SHA_OLD
    } else {
        TOOL_SHA_NEW
    }
}

/// この close tool 自身の sha256（ソース全体）
pub fn tool_sha256() -> String {
    hex_sha(include_str!("pr4_pf0h_close.rs").as_bytes())
}

/// canonical JSON（`pr4-canonical-json-1`: キーはバイト順・空白なし・整数のみ・null を保持）
pub fn canon(v: &Value) -> R<String> {
    Ok(match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) if n.is_i64() || n.is_u64() => n.to_string(),
        Value::Number(_) => return fail("V1", "E_LEDGER_CHAIN"),
        Value::String(s) => serde_json::to_string(s).unwrap_or_default(),
        Value::Array(a) => format!("[{}]", a.iter().map(canon).collect::<R<Vec<_>>>()?.join(",")),
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            let parts = keys.iter().map(|k| Ok(format!("{}:{}", serde_json::to_string(k).unwrap_or_default(), canon(&o[*k])?))).collect::<R<Vec<_>>>()?;
            format!("{{{}}}", parts.join(","))
        }
    })
}

fn sha_of(v: &Value) -> R<String> {
    Ok(hex_sha(canon(v)?.as_bytes()).to_string())
}

fn is_sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn valid_time(t: &str) -> bool {
    let b = t.as_bytes();
    t.len() == 20
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && b[19] == b'Z'
        && t.bytes().enumerate().all(|(i, c)| matches!(i, 4 | 7 | 10 | 13 | 16 | 19) || c.is_ascii_digit())
}

/// UTC 時刻文字列 → epoch 秒
pub fn epoch(t: &str) -> i64 {
    let n = |a: usize, b: usize| t[a..b].parse::<i64>().unwrap_or(0);
    let (y, m, d) = (n(0, 4), n(5, 7), n(8, 10));
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86400 + n(11, 13) * 3600 + n(14, 16) * 60 + n(17, 19)
}

fn jst_day(t: &str) -> i64 {
    (epoch(t) + 9 * 3600).div_euclid(86400)
}

fn rd(p: &Path) -> R<Vec<u8>> {
    match std::fs::read(p) {
        Ok(b) => Ok(b),
        Err(_) => fail("V0", "E_UNREADABLE"),
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

// ─── params ───────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Dev {
    pub id: String,
    pub run_utc: String,
    pub seq: u64,
    pub enrolled: u64,
    pub file_name: String,
    pub file_sha: String,
    pub disposition: String,
}

#[derive(Debug, Clone)]
pub struct Params {
    pub sha: String,
    pub rule_sha: String,
    pub exception_note_file: Option<String>,
    pub t0: String,
    pub baseline_count: u64,
    pub baseline_sha: String,
    pub baseline_seq: i64,
    pub anchors: Vec<(u64, String)>,
    pub devs: Vec<Dev>,
}

pub fn parse_params(bytes: &[u8]) -> R<Params> {
    let bad = || fail::<Params>("V0", "E_PARAMS_INCOMPLETE");
    let Ok(v) = serde_json::from_slice::<Value>(bytes) else { return bad() };
    let u = |x: &Value| x.as_u64();
    if s(&v, "contract_version") != CONTRACT_VERSION || u(&v["expected_enrolled_count"]) != Some(EXPECTED_COUNT) || s(&v, "enrollment_rule_version") != RULE_VERSION || !is_sha(s(&v, "enrollment_rule_sha256")) || !valid_time(s(&v, "t0_utc")) {
        return bad();
    }
    let timeline = json!([
        {"checkpoint_seq_from": 1, "checkpoint_seq_to": TOOL_SWITCH_SEQ - 1, "sha256": TOOL_SHA_OLD},
        {"checkpoint_seq_from": TOOL_SWITCH_SEQ, "checkpoint_seq_to": null, "sha256": TOOL_SHA_NEW},
    ]);
    let prov = &v["tool_transition_provenance"];
    if v["enrollment_tool_sha_timeline"] != timeline
        || s(prov, "pf0e3_main_commit") != PF0E3_MAIN_COMMIT
        || s(prov, "pf0e3_original_commit") != PF0E3_ORIGINAL_COMMIT
        || s(prov, "daily_freeze_amendment_2_sha256") != DAILY_FREEZE_AMENDMENT_2_SHA256
    {
        return bad();
    }
    // ledger 外の missed は、この 1 件の historical exception だけ（0 件か 1 件）
    let exc = v["cadence_exceptions"].as_array().cloned().unwrap_or_default();
    let exception_note_file = match exc.as_slice() {
        [] => None,
        [e] if s(e, "jst_date") == EXCEPTION_JST_DATE
            && s(e, "type") == "missed_without_ledger_record"
            && s(e, "audit_note_sha256") == EXCEPTION_AUDIT_NOTE_SHA256
            && s(e, "disposition") == "accepted_non_material"
            && s(e, "membership_impact") == "none"
            && s(e, "checkpoint_impact") == "none"
            && s(e, "backfill") == "forbidden"
            && s(e, "audit_note_sha256") == EXCEPTION_AUDIT_NOTE_SHA256 =>
        {
            Some(EXCEPTION_AUDIT_NOTE_SHA256.to_string())
        }
        _ => return bad(),
    };
    let b = &v["baseline"];
    if u(&b["baseline_count"]).is_none() || !is_sha(s(b, "baseline_membership_sha256")) || b["works_sequence"].as_i64().is_none() || !is_sha(s(b, "snapshot_sha256")) {
        return bad();
    }
    let c = &v["cadence"];
    if s(c, "frequency") != "DAILY" || s(c, "slot_local") != "07:30" || s(c, "slot_tz") != "Asia/Tokyo" || s(c, "day_boundary_tz") != "Asia/Tokyo" {
        return bad();
    }
    let mut anchors = Vec::new();
    for a in v["auditor_anchors"].as_array().cloned().unwrap_or_default() {
        match (u(&a["checkpoint_seq"]), s(&a, "checkpoint_sha256")) {
            (Some(q), h) if q >= 1 && is_sha(h) => anchors.push((q, h.to_string())),
            _ => return bad(),
        }
    }
    if anchors.is_empty() {
        return bad();
    }
    let mut devs = Vec::new();
    for d in v["deviations"].as_array().cloned().unwrap_or_default() {
        let disposition = s(&d, "auditor_disposition");
        let name = s(&d, "deviation_file_name");
        if s(&d, "class") != "extra_run"
            || s(&d, "deviation_id").is_empty()
            || !valid_time(s(&d, "run_utc"))
            || u(&d["declared_checkpoint_seq"]).is_none()
            || u(&d["declared_enrolled_in_run"]).is_none()
            || !is_sha(s(&d, "deviation_file_sha256"))
            || name.is_empty()
            || name.contains(['/', '\\'])
            || !["accepted_non_material", "blocking", "unresolved"].contains(&disposition)
        {
            return bad();
        }
        devs.push(Dev { id: s(&d, "deviation_id").into(), run_utc: s(&d, "run_utc").into(), seq: u(&d["declared_checkpoint_seq"]).unwrap_or(0), enrolled: u(&d["declared_enrolled_in_run"]).unwrap_or(0), file_name: name.into(), file_sha: s(&d, "deviation_file_sha256").into(), disposition: disposition.into() });
    }
    Ok(Params {
        sha: hex_sha(bytes),
        rule_sha: s(&v, "enrollment_rule_sha256").into(),
        exception_note_file,
        t0: s(&v, "t0_utc").into(),
        baseline_count: u(&b["baseline_count"]).unwrap_or(0),
        baseline_sha: s(b, "baseline_membership_sha256").into(),
        baseline_seq: b["works_sequence"].as_i64().unwrap_or(0),
        anchors,
        devs,
    })
}

// ─── ディレクトリの指紋 ──────────────────────────────────────────────────────────────────────

fn walk(base: &Path, rel: &Path, out: &mut Vec<(String, bool)>) -> R<()> {
    let Ok(rd) = std::fs::read_dir(base.join(rel)) else { return fail("V0", "E_UNREADABLE") };
    for e in rd.flatten() {
        let r = rel.join(e.file_name());
        let is_dir = e.path().is_dir();
        out.push((r.to_string_lossy().replace('\\', "/"), is_dir));
        if is_dir {
            walk(base, &r, out)?;
        }
    }
    Ok(())
}

/// 全ファイルの `相対パス:sha256` を昇順に並べた一覧（V15 と freeze の digest に使う）
pub fn dir_listing(base: &Path) -> R<Vec<(String, String)>> {
    let mut all = Vec::new();
    walk(base, Path::new(""), &mut all)?;
    all.sort();
    let mut out = Vec::new();
    for (r, is_dir) in all {
        if !is_dir {
            out.push((r.clone(), hex_sha(&rd(&base.join(&r))?)));
        }
    }
    Ok(out)
}

fn listing_digest(l: &[(String, String)]) -> String {
    hex_sha(l.iter().map(|(n, h)| format!("{n}:{h}\n")).collect::<String>().as_bytes())
}

// ─── 解析 ─────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Unit {
    pub key: String,
    pub input_sha: String,
    pub ctx_sha: String,
    pub rec_seq: u64,
}

#[derive(Debug, Clone)]
pub struct Cp {
    pub seq: u64,
    pub run_utc: String,
    pub kind: String,
    pub count: u64,
    pub head: String,
    pub sha: String,
}

#[derive(Debug, Clone)]
pub struct Analysis {
    pub params: Params,
    pub ledger_raw_sha: String,
    pub ledger_len: u64,
    pub records: u64,
    pub head: String,
    pub close_seq: u64,
    pub close_observed: String,
    pub close_commitment: String,
    pub missed: u64,
    pub baseline_ids_sha: String,
    pub units: Vec<Unit>,
    pub shared_payload_units: u64,
    pub payload_files: Vec<String>,
    pub cps: Vec<Cp>,
    pub cp_files_digest: String,
    pub reports_digest: String,
    pub cadence_gap_days: u64,
    pub exposure: Vec<(Dev, u64, Vec<String>)>,
    pub tool_ok: bool,
    pub note_preflight: Option<String>,
    pub rec_tools: Vec<(u64, String)>,
    pub gap_days_disclosed: u64,
}

struct Led {
    commits: Vec<String>,
    kinds: Vec<String>,
}

fn rule_ok(v: &Value) -> bool {
    s(v, "enrollment_rule_version") == RULE_VERSION
}

fn parse_ledger(dir: &Path, p: &Params, a: &mut Analysis) -> R<Led> {
    let raw = rd(&dir.join("ledger.jsonl"))?;
    a.ledger_raw_sha = hex_sha(&raw);
    a.ledger_len = raw.len() as u64;
    let Ok(text) = String::from_utf8(raw) else { return fail("V1", "E_LEDGER_CHAIN") };
    if text.contains('\r') || !text.ends_with('\n') {
        return fail("V1", "E_LEDGER_CHAIN");
    }
    let baseline_raw = rd(&dir.join("baseline_ids.json"))?;
    a.baseline_ids_sha = hex_sha(&baseline_raw);
    let ids: BTreeSet<i64> = match serde_json::from_slice::<Value>(&baseline_raw) {
        Ok(v) => v["ids"].as_array().map(|x| x.iter().filter_map(Value::as_i64).collect()).unwrap_or_default(),
        Err(_) => return fail("V4", "E_BASELINE_MISMATCH"),
    };
    let mut bpre = String::from("pr4-h2-baseline-1\n");
    for id in &ids {
        bpre.push_str(&format!("work:{id}\n"));
    }
    let mut led = Led { commits: Vec::new(), kinds: Vec::new() };
    let mut head = GENESIS.to_string();
    let (mut closed, mut last_t, mut close_enrolled) = (false, String::new(), None::<u64>);
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for (i, line) in text.split_terminator('\n').enumerate() {
        let Ok(rec) = serde_json::from_str::<Value>(line) else { return fail("V1", "E_LEDGER_CHAIN") };
        if canon(&rec)? != line {
            return fail("V1", "E_LEDGER_CHAIN");
        }
        let mut body = rec.clone();
        let stored = body.as_object_mut().and_then(|o| o.remove("record_commitment")).and_then(|x| x.as_str().map(str::to_string)).unwrap_or_default();
        if body["seq"].as_u64() != Some(i as u64 + 1) || s(&body, "prev_commitment") != head || hex_sha(format!("{}\n{}", canon(&body)?, head).as_bytes()) != stored {
            return fail("V1", "E_LEDGER_CHAIN");
        }
        if closed {
            return fail("V2", "E_RECORD_AFTER_CLOSE");
        }
        let kind = s(&body, "kind").to_string();
        if (i == 0) != (kind == "baseline") {
            return fail("V1", "E_LEDGER_CHAIN");
        }
        let t = body["observed_at"].as_str().or_else(|| body["first_seen_at"].as_str()).unwrap_or("").to_string();
        if !valid_time(&t) || t < last_t {
            return fail("V14", "E_TIME_ORDER");
        }
        match kind.as_str() {
            "baseline" => {
                if t != p.t0 {
                    return fail("V12", "E_T0_MISMATCH");
                }
                if hex_sha(bpre.as_bytes()) != s(&body, "baseline_membership_sha256")
                    || s(&body, "baseline_membership_sha256") != p.baseline_sha
                    || ids.len() as u64 != p.baseline_count
                    || body["baseline_count"].as_u64() != Some(p.baseline_count)
                    || body["works_sequence"].as_i64() != Some(p.baseline_seq)
                {
                    return fail("V12", "E_BASELINE_MISMATCH");
                }
                if !rule_ok(&body) {
                    return fail("V10", "E_RULE_PROVENANCE");
                }
                a.rec_tools.push((1, s(&body, "enrollment_tool_sha256").to_string()));
            }
            "enroll" => {
                let key = s(&body, "evaluation_unit_key").to_string();
                let id = key.strip_prefix("work:").filter(|d| !d.is_empty() && !d.starts_with('0') && d.bytes().all(|c| c.is_ascii_digit())).and_then(|d| d.parse::<i64>().ok());
                let Some(id) = id else { return fail("V4", "E_DUPLICATE_UNIT") };
                if ids.contains(&id) {
                    return fail("V4", "E_BASELINE_OVERLAP");
                }
                if id <= p.baseline_seq {
                    return fail("V4", "E_IDENTITY_ASSUMPTION");
                }
                if !keys.insert(key.clone()) {
                    return fail("V4", "E_DUPLICATE_UNIT");
                }
                if t <= p.t0.clone() || s(&body, "first_seen_at") != t {
                    return fail("V12", "E_T0_MISMATCH");
                }
                let ctx = &body["enrollment_context"];
                if sha_of(ctx)? != s(&body, "enrollment_context_sha256") || s(ctx, "first_seen_at") != t {
                    return fail("V7", "E_CONTEXT_HASH");
                }
                if !rule_ok(ctx) || s(ctx, "payload_schema_version") != PAYLOAD_SCHEMA_VERSION || s(ctx, "canonicalization_version") != CANONICALIZATION_VERSION || s(ctx, "evidence_class_at_enrollment") != "live" {
                    return fail("V10", "E_RULE_PROVENANCE");
                }
                a.rec_tools.push((i as u64 + 1, s(ctx, "enrollment_tool_sha256").to_string()));
                if !is_sha(s(&body, "matcher_input_sha256")) {
                    return fail("V7", "E_PAYLOAD_MISSING");
                }
                a.units.push(Unit { key, input_sha: s(&body, "matcher_input_sha256").into(), ctx_sha: s(&body, "enrollment_context_sha256").into(), rec_seq: i as u64 + 1 });
            }
            "missed" => a.missed += 1,
            "close" => {
                closed = true;
                close_enrolled = body["enrolled_count"].as_u64();
                a.close_seq = i as u64 + 1;
                a.close_observed = t.clone();
                a.close_commitment = stored.clone();
            }
            _ => return fail("V1", "E_LEDGER_CHAIN"),
        }
        last_t = t;
        head = stored.clone();
        led.commits.push(stored);
        led.kinds.push(kind);
    }
    if led.commits.is_empty() {
        return fail("V1", "E_LEDGER_CHAIN");
    }
    a.records = led.commits.len() as u64;
    a.head = head;
    if !closed {
        return fail("V2", "E_NO_CLOSE");
    }
    let n = a.units.len() as u64;
    if close_enrolled != Some(n) {
        return fail_n("V2", "E_CLOSE_COUNT", n, close_enrolled.unwrap_or(0));
    }
    if n != EXPECTED_COUNT {
        return fail_n("V3", "E_COUNT", n, EXPECTED_COUNT);
    }
    Ok(led)
}

fn check_payloads(dir: &Path, a: &mut Analysis) -> R<()> {
    let pdir = dir.join("payloads");
    let mut present: BTreeSet<String> = BTreeSet::new();
    if let Ok(rd) = std::fs::read_dir(&pdir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            match name.strip_suffix(".json") {
                Some(h) if is_sha(h) && e.path().is_file() => {
                    present.insert(h.to_string());
                }
                _ => return fail("V0", "E_UNKNOWN_FILE"),
            }
        }
    }
    let mut verified: BTreeSet<String> = BTreeSet::new();
    let mut referenced: BTreeMap<String, u64> = BTreeMap::new();
    for u in &a.units {
        *referenced.entry(u.input_sha.clone()).or_default() += 1;
        if !present.contains(&u.input_sha) {
            return fail("V7", "E_PAYLOAD_MISSING");
        }
        if verified.insert(u.input_sha.clone()) {
            let bytes = rd(&pdir.join(format!("{}.json", u.input_sha)))?;
            let ok = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| sha_of(&v).ok()).map(|h| h == u.input_sha).unwrap_or(false);
            if !ok {
                return fail("V7", "E_PAYLOAD_HASH");
            }
        }
    }
    if present.iter().any(|h| !referenced.contains_key(h)) {
        return fail("V8", "E_ORPHAN_PAYLOAD");
    }
    a.shared_payload_units = referenced.values().filter(|c| **c > 1).sum();
    a.payload_files = present.into_iter().collect();
    Ok(())
}

fn check_checkpoints(cdir: &Path, p: &Params, led: &Led, a: &mut Analysis, close_anchor: &str) -> R<()> {
    let mut names: Vec<String> = Vec::new();
    let Ok(rdir) = std::fs::read_dir(cdir) else { return fail("V0", "E_UNREADABLE") };
    for e in rdir.flatten() {
        if !e.path().is_file() {
            return fail("V0", "E_UNKNOWN_FILE");
        }
        names.push(e.file_name().to_string_lossy().to_string());
    }
    names.sort();
    if names.is_empty() {
        return fail("V5", "E_CHECKPOINT_CHAIN");
    }
    let mut prev_sha = GENESIS.to_string();
    let mut files: Vec<(String, String)> = Vec::new();
    for (i, name) in names.iter().enumerate() {
        if *name != format!("{:04}.json", i + 1) {
            return fail("V5", "E_CHECKPOINT_CHAIN");
        }
        let raw = rd(&cdir.join(name))?;
        files.push((name.clone(), hex_sha(&raw)));
        let Ok(v) = serde_json::from_slice::<Value>(&raw) else { return fail("V5", "E_CHECKPOINT_CHAIN") };
        let sha = sha_of(&v)?;
        let (count, kind) = (v["record_count"].as_u64().unwrap_or(0), s(&v, "kind").to_string());
        if v["seq"].as_u64() != Some(i as u64 + 1) || s(&v, "prev_checkpoint_sha256") != prev_sha || !valid_time(s(&v, "run_utc")) {
            return fail("V5", "E_CHECKPOINT_CHAIN");
        }
        if s(&v, "enrollment_rule_sha256") != p.rule_sha {
            return fail("V10", "E_RULE_PROVENANCE");
        }
        if s(&v, "enrollment_tool_sha256") != expected_tool_sha(i as u64 + 1) {
            return fail("V5", "E_TOOL_SHA");
        }
        let prev_count = a.cps.last().map(|c| c.count).unwrap_or(0);
        if count < prev_count || count == 0 || count as usize > led.commits.len() || s(&v, "ledger_head_sha256") != led.commits[count as usize - 1] {
            return fail("V5", "E_CHECKPOINT_LEDGER_MISMATCH");
        }
        let added = &led.kinds[prev_count as usize..count as usize];
        let shape_ok = match kind.as_str() {
            "baseline" => i == 0 && count == 1,
            "missed" => i > 0 && added.len() == 1 && added[0] == "missed",
            "enroll" => i > 0 && added.iter().all(|k| k == "enroll" || k == "close"),
            _ => false,
        };
        if !shape_ok {
            return fail("V5", "E_CHECKPOINT_LEDGER_MISMATCH");
        }
        a.cps.push(Cp { seq: i as u64 + 1, run_utc: s(&v, "run_utc").into(), kind, count, head: s(&v, "ledger_head_sha256").into(), sha: sha.clone() });
        prev_sha = sha;
    }
    a.cp_files_digest = listing_digest(&files);
    // 各 ledger record は、それを含む最初の checkpoint の run で書かれた。その seq の tool sha だけが許される（時系列）
    for (rec_seq, sha) in a.rec_tools.clone() {
        let Some(cp) = a.cps.iter().find(|c| c.count >= rec_seq) else { return fail("V6", "E_UNCOVERED_TAIL") };
        if sha != expected_tool_sha(cp.seq) {
            return fail("V10", "E_TOOL_SHA");
        }
    }
    if a.cps[0].run_utc != p.t0 {
        return fail("V12", "E_T0_MISMATCH");
    }
    for (q, h) in &p.anchors {
        if a.cps.get(*q as usize - 1).map(|c| &c.sha) != Some(h) {
            return fail("V6", "E_ANCHOR_MISMATCH");
        }
    }
    let last = a.cps.last().cloned().unwrap_or_else(|| unreachable!());
    // V6: close は最後の run の checkpoint で終わり、その後に checkpoint は無い
    let first_full = a.cps.iter().find(|c| c.count == a.records).cloned();
    let Some(first_full) = first_full else { return fail("V6", "E_UNCOVERED_TAIL") };
    if last.count != a.records {
        return fail("V6", "E_UNCOVERED_TAIL");
    }
    if first_full.seq != last.seq {
        return fail("V6", "E_POSTCLOSE_CHECKPOINT");
    }
    if last.sha != close_anchor {
        return fail("V6", "E_ANCHOR_MISMATCH");
    }
    if last.run_utc != a.close_observed {
        return fail("V14", "E_TIME_ORDER");
    }
    Ok(())
}

fn check_reports(dir: &Path, a: &mut Analysis) -> R<()> {
    let rdir = dir.join("reports");
    let mut files: Vec<(String, String)> = Vec::new();
    let mut seen: BTreeSet<u64> = BTreeSet::new();
    if let Ok(rd) = std::fs::read_dir(&rdir) {
        let mut ents: Vec<_> = rd.flatten().collect();
        ents.sort_by_key(|e| e.file_name());
        for e in ents {
            if !e.path().is_file() {
                return fail("V0", "E_UNKNOWN_FILE");
            }
            let raw = rd_file(&e.path())?;
            files.push((e.file_name().to_string_lossy().to_string(), hex_sha(&raw)));
            let Ok(v) = serde_json::from_slice::<Value>(&raw) else { return fail("V13", "E_REPORT_MISMATCH") };
            let Some(q) = v["checkpoint_seq"].as_u64().filter(|q| *q >= 1 && (*q as usize) <= a.cps.len()) else { return fail("V13", "E_REPORT_MISMATCH") };
            let cp = &a.cps[q as usize - 1];
            if !seen.insert(q) || s(&v, "checkpoint_sha256") != cp.sha || v["record_count"].as_u64() != Some(cp.count) || s(&v, "ledger_head_sha256") != cp.head {
                return fail("V13", "E_REPORT_MISMATCH");
            }
            let last = q as usize == a.cps.len();
            if last && (v["closed"] != json!(true) || v["total_enrolled"].as_u64() != Some(EXPECTED_COUNT)) {
                return fail("V13", "E_REPORT_MISMATCH");
            }
        }
    }
    if seen.len() != a.cps.len() {
        return fail("V13", "E_REPORT_MISSING");
    }
    a.reports_digest = listing_digest(&files);
    Ok(())
}

fn rd_file(p: &Path) -> R<Vec<u8>> {
    rd(p)
}

fn check_deviations(p: &Params, dev_dir: &Path, a: &mut Analysis) -> R<()> {
    if p.devs.iter().any(|d| d.disposition != "accepted_non_material") {
        return fail("V11", "E_DEVIATION_NOT_ACCEPTED");
    }
    let mut by_day: BTreeMap<i64, Vec<&Cp>> = BTreeMap::new();
    for c in a.cps.iter().filter(|c| c.kind != "baseline") {
        by_day.entry(jst_day(&c.run_utc)).or_default().push(c);
    }
    let mut pinned_on_day: BTreeMap<i64, u64> = BTreeMap::new();
    let mut exposure = Vec::new();
    for d in &p.devs {
        let Some(cp) = a.cps.iter().find(|c| c.seq == d.seq) else { return fail("V11", "E_DEVIATION_MISMATCH") };
        if cp.run_utc != d.run_utc || cp.kind == "baseline" {
            return fail("V11", "E_DEVIATION_MISMATCH");
        }
        let Ok(bytes) = std::fs::read(dev_dir.join(&d.file_name)) else { return fail("V11", "E_DEVIATION_MISMATCH") };
        if hex_sha(&bytes) != d.file_sha {
            return fail("V11", "E_DEVIATION_MISMATCH");
        }
        let prev = if cp.seq >= 2 { a.cps[cp.seq as usize - 2].count } else { 0 };
        let range = |u: &&Unit| u.rec_seq > prev && u.rec_seq <= cp.count;
        let keys: Vec<String> = a.units.iter().filter(range).map(|u| u.key.clone()).collect();
        if keys.len() as u64 != d.enrolled {
            return fail_n("V11", "E_DEVIATION_MISMATCH", keys.len() as u64, d.enrolled);
        }
        *pinned_on_day.entry(jst_day(&cp.run_utc)).or_default() += 1;
        exposure.push((d.clone(), keys.len() as u64, keys));
    }
    for (day, runs) in &by_day {
        if runs.len() > 1 && pinned_on_day.get(day).copied().unwrap_or(0) < runs.len() as u64 - 1 {
            return fail("V11", "E_UNDECLARED_DEVIATION");
        }
    }
    let (first, last) = (by_day.keys().next().copied(), by_day.keys().next_back().copied());
    let gaps: Vec<i64> = match (first, last) {
        (Some(f), Some(l)) => (f..=l).filter(|d| !by_day.contains_key(d)).collect(),
        _ => Vec::new(),
    };
    a.cadence_gap_days = gaps.len() as u64;
    // ledger に記録の無い欠落日は、SHA 固定済みの historical exception 1 件だけを認める（それ以外・宣言日が実在する日は STOP）
    let declared: Vec<i64> = if p.exception_note_file.is_some() { vec![jst_day(&format!("{EXCEPTION_JST_DATE}T00:00:00Z"))] } else { vec![] };
    if let Some(g) = gaps.iter().find(|g| !declared.contains(g)) {
        let _ = g;
        return fail("V11", "E_UNDECLARED_GAP");
    }
    if declared.iter().any(|d| !gaps.contains(d)) {
        return fail("V11", "E_DEVIATION_MISMATCH");
    }
    a.gap_days_disclosed = declared.len() as u64;
    a.exposure = exposure;
    Ok(())
}

fn structure_ok(ledger: &Path, cdir: &Path, out: &Path) -> R<()> {
    let (Ok(l), Ok(c)) = (ledger.canonicalize(), cdir.canonicalize()) else { return fail("V0", "E_UNREADABLE") };
    if l.starts_with(&c) || c.starts_with(&l) {
        return fail("V0", "E_UNKNOWN_FILE");
    }
    if let Ok(o) = out.canonicalize() {
        if o.starts_with(&l) || o.starts_with(&c) || l.starts_with(&o) || c.starts_with(&o) {
            return fail("V0", "E_UNKNOWN_FILE");
        }
    }
    let Ok(rd) = std::fs::read_dir(ledger) else { return fail("V0", "E_UNREADABLE") };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        let okk = match n.as_str() {
            "ledger.jsonl" | "baseline_ids.json" => e.path().is_file(),
            "payloads" | "reports" => e.path().is_dir(),
            _ => false,
        };
        if !okk {
            return fail("V0", "E_UNKNOWN_FILE");
        }
    }
    Ok(())
}

// ─── commitments ─────────────────────────────────────────────────────────────────────────────

/// authoritative: PF0B の `membership_commitment`。版名は rule version + rule sha
pub fn eligibility_rule_version(rule_sha: &str) -> String {
    format!("{RULE_VERSION}/enrollment_rule_sha256={rule_sha}")
}

/// 補助の監査値（authoritative ではない）: key 集合
pub fn unit_set_sha256(keys: &[String]) -> String {
    let mut k: Vec<&String> = keys.iter().collect();
    k.sort();
    let mut pre = format!("pr4-pf0h0-final-membership-1\n{RULE_VERSION}\n");
    for x in k {
        pre.push_str(x);
        pre.push('\n');
    }
    hex_sha(pre.as_bytes())
}

fn unit_input_sha256(units: &[Unit]) -> String {
    let mut u: Vec<&Unit> = units.iter().collect();
    u.sort_by(|a, b| a.key.cmp(&b.key));
    let mut pre = String::from("pr4-pf0h0-final-membership-input-1\n");
    for x in u {
        pre.push_str(&format!("{}\t{}\t{}\n", x.key, x.input_sha, x.ctx_sha));
    }
    hex_sha(pre.as_bytes())
}

fn payload_tree_sha(files: &[String]) -> String {
    hex_sha(files.iter().map(|h| format!("{h}\n")).collect::<String>().as_bytes())
}

pub fn analyze(params_bytes: &[u8], ledger: &Path, cdir: &Path, dev_dir: &Path, out: &Path, close_anchor: &str, note_preflight: Option<&str>) -> R<Analysis> {
    let p = parse_params(params_bytes)?;
    // audit-side preflight（PR4_PF0E_MISSED_NOTE_20261010.json の実在と SHA-256 再計算）の結果。本文は読まない・解釈しない
    if p.exception_note_file.is_some() && note_preflight != Some(EXCEPTION_AUDIT_NOTE_SHA256) {
        return fail("V11", "E_EXCEPTION_PREFLIGHT");
    }
    if !is_sha(close_anchor) {
        return fail("V6", "E_ANCHOR_MISMATCH");
    }
    structure_ok(ledger, cdir, out)?;
    let mut a = Analysis {
        params: p.clone(), ledger_raw_sha: String::new(), ledger_len: 0, records: 0, head: String::new(), close_seq: 0, close_observed: String::new(), close_commitment: String::new(), missed: 0, baseline_ids_sha: String::new(),
        units: Vec::new(), shared_payload_units: 0, payload_files: Vec::new(), cps: Vec::new(), cp_files_digest: String::new(), reports_digest: String::new(), cadence_gap_days: 0, exposure: Vec::new(), tool_ok: true, note_preflight: note_preflight.map(str::to_string), rec_tools: Vec::new(), gap_days_disclosed: 0,
    };
    let led = parse_ledger(ledger, &p, &mut a)?;
    check_payloads(ledger, &mut a)?;
    check_checkpoints(cdir, &p, &led, &mut a, close_anchor)?;
    check_reports(ledger, &mut a)?;
    check_deviations(&p, dev_dir, &mut a)?;
    Ok(a)
}

pub fn build_artifacts(a: &Analysis, generated_at: &str, tool_head: &str, close_anchor: &str, enrollment_tool_head: &str) -> R<[Value; 3]> {
    let keys: Vec<String> = a.units.iter().map(|u| u.key.clone()).collect();
    let version = eligibility_rule_version(&a.params.rule_sha);
    let membership = membership_commitment(&keys, &version);
    // 2 経路目（ledger 順ではなく key 順の BTreeMap から）で authoritative commitment を自己確認する
    let sorted: BTreeSet<&String> = keys.iter().collect();
    let again = membership_commitment(&sorted.into_iter().cloned().collect::<Vec<_>>(), &version);
    if again != membership || a.units.len() as u64 != EXPECTED_COUNT {
        return fail("V9", "E_COMMITMENT_SELF_CHECK");
    }
    let last = a.cps.last().cloned().unwrap_or_else(|| unreachable!());
    let checks: Vec<Value> = (0..=16).map(|i| json!({"id": format!("V{i}"), "result": if i == 11 && !a.exposure.is_empty() { "WAIVED" } else { "PASS" }, "waiver_ref": Value::Null})).collect();
    let checks: Vec<Value> = checks
        .into_iter()
        .map(|mut c| {
            if c["result"] == "WAIVED" {
                c["waiver_ref"] = json!(a.exposure.iter().map(|(d, _, _)| d.id.clone()).collect::<Vec<_>>().join(","));
            } else {
                c.as_object_mut().map(|o| o.remove("waiver_ref"));
            }
            c
        })
        .collect();
    let exposure: Vec<Value> = a.exposure.iter().map(|(d, n, _)| json!({"deviation_id": d.id, "class": "extra_run", "enrolled_in_run": n, "share_of_final": format!("{n}/{EXPECTED_COUNT}"), "disposition": d.disposition})).collect();
    let close = json!({
        "protocol": PROTOCOL, "status": "PASS", "generated_at": generated_at, "contract_version": CONTRACT_VERSION,
        "close_tool_sha256": tool_sha256(), "close_tool_head": tool_head, "params_sha256": a.params.sha,
        "inputs": {
            "ledger_record_count": a.records, "ledger_head_sha256": a.head, "checkpoint_count": a.cps.len(), "checkpoint_head_sha256": last.sha,
            "close_record_seq": a.close_seq, "close_observed_at": a.close_observed, "close_anchor_checkpoint_seq": last.seq, "close_anchor_checkpoint_sha256": close_anchor,
        },
        "counts": {
            "baseline_count": a.params.baseline_count, "enrolled_count": a.units.len(), "missed_count": a.missed, "payload_units_present": a.units.len(),
            "shared_payload_unit_count": a.shared_payload_units, "duplicate_unit_count": 0, "cadence_gap_days": a.cadence_gap_days,
        },
        "checks": checks,
        "provenance": {"rule_sha256_matches_all_checkpoints": true, "tool_sha256_follows_timeline": a.tool_ok, "tool_transition_provenance": {"pf0e3_main_commit": PF0E3_MAIN_COMMIT, "pf0e3_original_commit": PF0E3_ORIGINAL_COMMIT, "daily_freeze_amendment_2_sha256": DAILY_FREEZE_AMENDMENT_2_SHA256, "old_tool_sha256": TOOL_SHA_OLD, "new_tool_sha256": TOOL_SHA_NEW, "switch_checkpoint_seq": TOOL_SWITCH_SEQ}, "cadence_exception_disclosed": a.gap_days_disclosed, "cadence_exception_preflight_sha256": a.note_preflight, "t0_matches": true, "baseline_matches": true},
        "deviation_exposure": exposure,
        "limits": "count-only; no unit keys, no payloads, no judgments, no machine output, no performance figures",
    });
    let subsets: Vec<Value> = a.exposure.iter().map(|(d, n, k)| json!({"deviation_id": d.id, "unit_count": n, "unit_set_sha256": unit_set_sha256(k), "audit_only": true, "promotion_use": "forbidden"})).collect();
    let commit = json!({
        "protocol": "pr4-pf0h0-final-membership-1", "unit_count": a.units.len(),
        "membership_commitment_sha256": membership, "membership_commitment_definition": "pr4_pf0b::membership_commitment (domain pr4-pf0a-membership-1)", "eligibility_rule_version": version, "authoritative": true,
        "auxiliary_audit_values": {"unit_set_sha256": unit_set_sha256(&keys), "unit_input_sha256": unit_input_sha256(&a.units), "authoritative": false},
        "ledger_head_sha256": a.head, "close_record_commitment": a.close_commitment, "baseline_membership_sha256": a.params.baseline_sha, "enrollment_rule_sha256": a.params.rule_sha,
        "h1_membership_commitment_sha256": H1_COMMITMENT, "h1_role": "legacy_sealed_diagnostic_tranche_not_primary",
        "deviation_subset_commitments": subsets,
        "key_order": "byte-lexicographic of evaluation_unit_key (not numeric)",
        "limits": "commitment only; unit keys are not stored here (the ledger is the source of truth); deviation subsets are audit-only and never inform promotion",
    });
    let freeze = json!({
        "protocol": "pr4-pf0h0-h2-ledger-final-freeze-1", "frozen_state": "CLOSED",
        "ledger": {"ledger_jsonl_sha256": a.ledger_raw_sha, "byte_length": a.ledger_len, "record_count": a.records, "ledger_head_sha256": a.head},
        "baseline_ids_sha256": a.baseline_ids_sha,
        "payload_tree": {"file_count": a.payload_files.len(), "payload_tree_sha256": payload_tree_sha(&a.payload_files)},
        "checkpoint_chain": {"count": a.cps.len(), "head_sha256": last.sha, "files_digest_sha256": a.cp_files_digest},
        "reports_digest_sha256": a.reports_digest,
        "tool_provenance": {"enrollment_tool_sha256_timeline": [{"checkpoint_seq_from": 1, "checkpoint_seq_to": TOOL_SWITCH_SEQ - 1, "sha256": TOOL_SHA_OLD}, {"checkpoint_seq_from": TOOL_SWITCH_SEQ, "checkpoint_seq_to": null, "sha256": TOOL_SHA_NEW}], "enrollment_rule_sha256": a.params.rule_sha, "enrollment_tool_head": enrollment_tool_head},
        "next_allowed_actions": ["machine_output_freeze"],
        "still_forbidden": ["performance_view", "membership_change", "enroll_run", "gt_review_before_machine_output_freeze"],
    });
    let all = [close, commit, freeze];
    for v in &all {
        assert_clean(v)?;
    }
    Ok(all)
}

/// V16: 出力に unit key・題名・id・パスの断片が無い
pub fn assert_clean(v: &Value) -> R<()> {
    let text = v.to_string();
    for banned in ["work:", "work_id", "tmdb", "title", "verdict", "candidate", "precision", "label", "decision", ":\\", "\\\\"] {
        if text.contains(banned) {
            return fail("V16", "E_OUTPUT_NOT_CLEAN");
        }
    }
    Ok(())
}

pub fn write_stop(out: &Path, f: &Fail) {
    let mut item = json!({"id": f.id, "code": f.code});
    if let (Some(o), Some(e)) = (f.observed, f.expected) {
        item["observed_count"] = json!(o);
        item["expected_count"] = json!(e);
    }
    let v = json!({"protocol": PROTOCOL, "status": "STOP", "failed_checks": [item], "limits": "check ids, error codes and counts only; no keys, payloads or judgments; none of the three close artifacts exist"});
    let _ = std::fs::create_dir_all(out);
    let _ = std::fs::write(out.join(STOP_FILE), serde_json::to_string_pretty(&v).unwrap_or_default() + "\n");
}

pub struct Inputs<'a> {
    pub params_path: &'a Path,
    pub ledger_dir: &'a Path,
    pub checkpoint_dir: &'a Path,
    pub deviation_dir: &'a Path,
    pub out_dir: &'a Path,
    pub close_anchor_sha256: &'a str,
    pub generated_at: &'a str,
    pub tool_head: &'a str,
    pub enrollment_tool_head: &'a str,
    /// audit-side preflight で再計算した exception note の SHA-256。exception を pin した params では必須
    pub exception_note_preflight_sha256: Option<&'a str>,
}

/// 1 回の実行。出力 dir は存在しないか空であること。PASS なら 3 ファイル、STOP なら STOP ファイルだけ。`hook` は test 用（実行中の入力変化を作る）
pub fn run_with_hook(i: &Inputs, hook: &dyn Fn()) -> Result<[PathBuf; 3], Fail> {
    if i.out_dir.exists() && std::fs::read_dir(i.out_dir).map(|mut r| r.next().is_some()).unwrap_or(true) {
        return fail("V0", "E_OUTPUT_EXISTS");
    }
    let result = (|| -> R<[Value; 3]> {
        let params = rd(i.params_path)?;
        let before = (dir_listing(i.ledger_dir)?, dir_listing(i.checkpoint_dir)?);
        let a = analyze(&params, i.ledger_dir, i.checkpoint_dir, i.deviation_dir, i.out_dir, i.close_anchor_sha256, i.exception_note_preflight_sha256)?;
        hook();
        let after = (dir_listing(i.ledger_dir)?, dir_listing(i.checkpoint_dir)?);
        if before != after {
            return fail("V15", "E_INPUT_CHANGED");
        }
        build_artifacts(&a, i.generated_at, i.tool_head, i.close_anchor_sha256, i.enrollment_tool_head)
    })();
    match result {
        Err(f) => {
            write_stop(i.out_dir, &f);
            Err(f)
        }
        Ok(arts) => {
            if std::fs::create_dir_all(i.out_dir).is_err() {
                return fail("V0", "E_OUTPUT_EXISTS");
            }
            let mut paths = Vec::new();
            for (name, v) in ARTIFACTS.iter().zip(arts.iter()) {
                let tmp = i.out_dir.join(format!("{name}.partial"));
                let text = serde_json::to_string_pretty(v).unwrap_or_default() + "\n";
                if std::fs::write(&tmp, text).is_err() {
                    for p in &paths {
                        let _ = std::fs::remove_file(p);
                    }
                    return fail("V0", "E_OUTPUT_EXISTS");
                }
                paths.push(tmp);
            }
            let mut finals = Vec::new();
            for (name, tmp) in ARTIFACTS.iter().zip(paths.iter()) {
                let dest = i.out_dir.join(name);
                if std::fs::rename(tmp, &dest).is_err() {
                    return fail("V0", "E_OUTPUT_EXISTS");
                }
                finals.push(dest);
            }
            Ok([finals[0].clone(), finals[1].clone(), finals[2].clone()])
        }
    }
}

pub fn run(i: &Inputs) -> Result<[PathBuf; 3], Fail> {
    run_with_hook(i, &|| {})
}

#[test]
#[ignore = "manual: close の実行は監査の gate 後。CINEMANTIS_PR4_PF0H_* を設定する"]
fn manual_pr4_pf0h_close() {
    let need = |n: &str| std::env::var(n).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| panic!("STOP: 環境変数 {n} が未設定です"));
    let (p, l, c, d, o) = (PathBuf::from(need("CINEMANTIS_PR4_PF0H_PARAMS")), PathBuf::from(need("CINEMANTIS_PR4_PF0H_LEDGER_DIR")), PathBuf::from(need("CINEMANTIS_PR4_PF0H_CHECKPOINT_DIR")), PathBuf::from(need("CINEMANTIS_PR4_PF0H_DEVIATION_DIR")), PathBuf::from(need("CINEMANTIS_PR4_PF0H_OUT_DIR")));
    let pf = std::env::var("CINEMANTIS_PR4_PF0H_EXCEPTION_NOTE_PREFLIGHT_SHA256").ok().filter(|v| !v.trim().is_empty());
    let (anchor, gen, head, ehead) = (need("CINEMANTIS_PR4_PF0H_CLOSE_ANCHOR_SHA256"), need("CINEMANTIS_PR4_PF0H_GENERATED_AT"), need("CINEMANTIS_PR4_PF0H_TOOL_HEAD"), need("CINEMANTIS_PR4_PF0H_ENROLLMENT_TOOL_HEAD"));
    let i = Inputs { params_path: &p, ledger_dir: &l, checkpoint_dir: &c, deviation_dir: &d, out_dir: &o, close_anchor_sha256: &anchor, generated_at: &gen, tool_head: &head, enrollment_tool_head: &ehead, exception_note_preflight_sha256: pf.as_deref() };
    match run(&i) {
        Ok(paths) => println!("PASS: {}", paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")),
        Err(f) => panic!("STOP: {} {}", f.id, f.code),
    }
}
