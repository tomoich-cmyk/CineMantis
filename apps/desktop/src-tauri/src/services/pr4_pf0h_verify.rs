//! PR4-PF0H1 `pr4_pf0h_verify`: close 成果物の **独立再検証**（読み取り専用・test ビルドだけ・手動専用）。
//!
//! `pr4_pf0h_close` のコードを一切使わず（ソース走査テストで固定）、入力（ledger / checkpoint / payload / reports / params / deviation）から事実を別実装で再導出して、
//! 3 つの成果物と突き合わせる。再導出した検証が 1 つでも失敗すれば、成果物があっても STOP。authoritative な membership commitment の関数だけは PF0B のものを共用する。
//! 入力の読み方は close と意図的に変えている（close は record を畳み込みながら検証し、verify は先に全部を読み込んでから集合・索引で検算する）。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::services::pr4_pf0b::membership_commitment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop(pub String);

type V<T> = Result<T, Stop>;

fn stop<T>(m: &str) -> V<T> {
    Err(Stop(m.to_string()))
}

fn sha(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

fn canon(v: &Value) -> V<String> {
    match v {
        Value::Null => Ok("null".into()),
        Value::Bool(b) => Ok(format!("{b}")),
        Value::Number(n) if n.is_i64() || n.is_u64() => Ok(n.to_string()),
        Value::Number(_) => stop("float"),
        Value::String(x) => Ok(serde_json::to_string(x).map_err(|e| Stop(e.to_string()))?),
        Value::Array(a) => {
            let mut parts = vec![];
            for x in a {
                parts.push(canon(x)?);
            }
            Ok(format!("[{}]", parts.join(",")))
        }
        Value::Object(o) => {
            let sorted: BTreeMap<&String, &Value> = o.iter().collect();
            let mut parts = vec![];
            for (k, x) in sorted {
                parts.push(format!("{}:{}", serde_json::to_string(k).map_err(|e| Stop(e.to_string()))?, canon(x)?));
            }
            Ok(format!("{{{}}}", parts.join(",")))
        }
    }
}

fn csha(v: &Value) -> V<String> {
    Ok(sha(canon(v)?.as_bytes()))
}

fn read(p: &Path) -> V<Vec<u8>> {
    std::fs::read(p).map_err(|e| Stop(format!("{}: {e}", p.display())))
}

fn jread(p: &Path) -> V<Value> {
    serde_json::from_slice(&read(p)?).map_err(|e| Stop(e.to_string()))
}

fn st<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

fn days_since_epoch_jst(t: &str) -> V<i64> {
    // 独立実装: 暦を 1970-01-01 からの経過日として数える（close と別アルゴリズム）
    let p: Vec<i64> = [&t[0..4], &t[5..7], &t[8..10], &t[11..13], &t[14..16], &t[17..19]].iter().map(|x| x.parse().unwrap_or(-1)).collect();
    if p.iter().any(|x| *x < 0) || t.len() != 20 {
        return stop("time");
    }
    let leap = |y: i64| (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let mut days = 0i64;
    for y in 1970..p[0] {
        days += if leap(y) { 366 } else { 365 };
    }
    let ml = [31, if leap(p[0]) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 0..(p[1] - 1) as usize {
        days += ml[m];
    }
    days += p[2] - 1;
    let secs = days * 86400 + p[3] * 3600 + p[4] * 60 + p[5] + 9 * 3600;
    Ok(secs.div_euclid(86400))
}

struct Rec {
    body: Value,
    commit: String,
}

struct Inputs {
    recs: Vec<Rec>,
    ledger_raw: Vec<u8>,
    baseline_raw: Vec<u8>,
    cps: Vec<(Value, String)>,
    cp_files: Vec<(String, String)>,
    reports: Vec<(String, Value, String)>,
    payload_names: BTreeSet<String>,
}

fn load(ledger: &Path, cdir: &Path) -> V<Inputs> {
    let ledger_raw = read(&ledger.join("ledger.jsonl"))?;
    let text = String::from_utf8(ledger_raw.clone()).map_err(|e| Stop(e.to_string()))?;
    if text.contains('\r') || !text.ends_with('\n') {
        return stop("ledger newline form");
    }
    let mut recs = vec![];
    for line in text.trim_end_matches('\n').split('\n') {
        let mut v: Value = serde_json::from_str(line).map_err(|e| Stop(e.to_string()))?;
        if canon(&v)? != line {
            return stop("ledger line not canonical");
        }
        let commit = v.as_object_mut().and_then(|o| o.remove("record_commitment")).and_then(|c| c.as_str().map(String::from)).unwrap_or_default();
        recs.push(Rec { body: v, commit });
    }
    let mut cps = vec![];
    let mut cp_files = vec![];
    let mut names: Vec<String> = std::fs::read_dir(cdir).map_err(|e| Stop(e.to_string()))?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
    names.sort();
    for n in &names {
        let raw = read(&cdir.join(n))?;
        cp_files.push((n.clone(), sha(&raw)));
        let v: Value = serde_json::from_slice(&raw).map_err(|e| Stop(e.to_string()))?;
        let h = csha(&v)?;
        cps.push((v, h));
    }
    if cp_files.iter().enumerate().any(|(i, (n, _))| *n != format!("{:04}.json", i + 1)) {
        return stop("checkpoint file names");
    }
    let mut reports = vec![];
    let rdir = ledger.join("reports");
    if rdir.is_dir() {
        let mut rn: Vec<String> = std::fs::read_dir(&rdir).map_err(|e| Stop(e.to_string()))?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
        rn.sort();
        for n in rn {
            let raw = read(&rdir.join(&n))?;
            reports.push((n, serde_json::from_slice(&raw).map_err(|e| Stop(e.to_string()))?, sha(&raw)));
        }
    }
    let mut payload_names = BTreeSet::new();
    let pdir = ledger.join("payloads");
    if pdir.is_dir() {
        for e in std::fs::read_dir(&pdir).map_err(|e| Stop(e.to_string()))?.flatten() {
            payload_names.insert(e.file_name().to_string_lossy().to_string());
        }
    }
    let known: BTreeSet<String> = std::fs::read_dir(ledger).map_err(|e| Stop(e.to_string()))?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
    if !known.is_subset(&["ledger.jsonl", "baseline_ids.json", "payloads", "reports"].iter().map(|s| s.to_string()).collect()) {
        return stop("unknown file in ledger dir");
    }
    Ok(Inputs { recs, ledger_raw, baseline_raw: read(&ledger.join("baseline_ids.json"))?, cps, cp_files, reports, payload_names })
}

fn digest(files: &[(String, String)]) -> String {
    sha(files.iter().map(|(n, h)| format!("{n}:{h}\n")).collect::<String>().as_bytes())
}

/// 再導出した事実から期待される 3 成果物（`generated_at` / `close_tool_*` / `params` は別途比較）。失敗は Stop
pub fn verify(params_path: &Path, ledger: &Path, cdir: &Path, dev_dir: &Path, artifacts: &Path, close_anchor: &str) -> V<()> {
    if artifacts.join("H2_CLOSE_STOP.json").exists() {
        return stop("STOP file present");
    }
    let [close, commit, freeze] = ["H2_CLOSE.json", "FINAL_MEMBERSHIP_COMMITMENT.json", "H2_LEDGER_FINAL_FREEZE.json"].map(|n| jread(&artifacts.join(n)));
    let (close, commit, freeze) = (close?, commit?, freeze?);
    let params_raw = read(params_path)?;
    let p: Value = serde_json::from_slice(&params_raw).map_err(|e| Stop(e.to_string()))?;
    let (rule_sha, t0) = (st(&p, "enrollment_rule_sha256").to_string(), st(&p, "t0_utc").to_string());
    // tool SHA は時系列: checkpoint seq 1〜3 は OLD のみ、seq 4 以降は NEW のみ（独立に再記述した定数）
    let tool_at = |seq: u64| if seq <= 3 { "183e0c73c14c20afd9ed34da33586f9599f96d1fff95c5c9a79d0b78855e3213" } else { "36f869f6d197615a610a50e1a164f1492eae06dd1c0eaad778c5311890518d08" };
    let tl = &p["enrollment_tool_sha_timeline"];
    if tl[0]["sha256"] != tool_at(1) || tl[0]["checkpoint_seq_to"] != 3 || tl[1]["sha256"] != tool_at(4) || tl[1]["checkpoint_seq_from"] != 4 || !tl[1]["checkpoint_seq_to"].is_null() || tl.as_array().map(|a| a.len()) != Some(2) {
        return stop("tool timeline");
    }
    let tp = &p["tool_transition_provenance"];
    if st(tp, "pf0e3_main_commit") != "69d150d633ad1097319e27545962d989a92aebd9" || st(tp, "pf0e3_original_commit") != "98e3917fa022d98dd8c3e9293a865f8b44b6a13b" || st(tp, "daily_freeze_amendment_2_sha256") != "f4a2eb6d92010aa35425dbe7cd882143ff026f4ba6dae755bfa5ebc6e42a0263" {
        return stop("tool transition provenance");
    }
    if p["expected_enrolled_count"] != 946 || st(&p, "enrollment_rule_version") != "pr4-h2-enrollment-rules-2" {
        return stop("params");
    }
    let inp = load(ledger, cdir)?;
    let n = inp.recs.len();

    // ── ledger: 先に全 commitment を索引化してから検算
    let mut prev = "0".repeat(64);
    for (i, r) in inp.recs.iter().enumerate() {
        let calc = sha(format!("{}\n{}", canon(&r.body)?, prev).as_bytes());
        if r.body["seq"] != (i as u64 + 1) || st(&r.body, "prev_commitment") != prev || calc != r.commit {
            return stop("ledger chain");
        }
        prev = r.commit.clone();
    }
    let kinds: Vec<&str> = inp.recs.iter().map(|r| st(&r.body, "kind")).collect();
    let close_positions: Vec<usize> = kinds.iter().enumerate().filter(|(_, k)| **k == "close").map(|(i, _)| i).collect();
    if close_positions != vec![n - 1] || kinds[0] != "baseline" || kinds.iter().filter(|k| **k == "baseline").count() != 1 {
        return stop("close position / baseline");
    }
    let enrolls: Vec<(usize, &Value)> = inp.recs.iter().enumerate().filter(|(_, r)| st(&r.body, "kind") == "enroll").map(|(i, r)| (i, &r.body)).collect();
    if enrolls.len() != 946 || inp.recs[n - 1].body["enrolled_count"] != 946 {
        return stop("count");
    }
    let times: Vec<&str> = inp.recs.iter().map(|r| r.body["observed_at"].as_str().or_else(|| r.body["first_seen_at"].as_str()).unwrap_or("")).collect();
    if times.windows(2).any(|w| w[0] > w[1]) || times[0] != t0 {
        return stop("time order / t0");
    }
    // baseline
    let bl = &inp.recs[0].body;
    let bids: Value = serde_json::from_slice(&inp.baseline_raw).map_err(|e| Stop(e.to_string()))?;
    let bset: BTreeSet<i64> = bids["ids"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
    let bhash = sha(std::iter::once("pr4-h2-baseline-1\n".to_string()).chain(bset.iter().map(|i| format!("work:{i}\n"))).collect::<String>().as_bytes());
    let bseq = bl["works_sequence"].as_i64().unwrap_or(-1);
    if bhash != st(bl, "baseline_membership_sha256") || bhash != st(&p["baseline"], "baseline_membership_sha256") || bset.len() as u64 != p["baseline"]["baseline_count"].as_u64().unwrap_or(u64::MAX) || bseq != p["baseline"]["works_sequence"].as_i64().unwrap_or(-2) {
        return stop("baseline");
    }
    // unit
    let mut idx: HashMap<String, usize> = HashMap::new();
    for (i, e) in &enrolls {
        let key = st(e, "evaluation_unit_key");
        let id = key.strip_prefix("work:").and_then(|d| if d.starts_with('0') || d.starts_with('+') { None } else { d.parse::<i64>().ok() });
        let Some(id) = id else { return stop("key") };
        if id <= bseq || bset.contains(&id) || idx.insert(key.to_string(), *i).is_some() {
            return stop("identity / duplicate");
        }
        let ctx = &e["enrollment_context"];
        if csha(ctx)? != st(e, "enrollment_context_sha256")
            || st(ctx, "first_seen_at") != st(e, "first_seen_at")
            || st(e, "first_seen_at") <= t0.as_str()
            || st(ctx, "enrollment_rule_version") != "pr4-h2-enrollment-rules-2"
            || st(ctx, "evidence_class_at_enrollment") != "live"
        {
            return stop("unit provenance");
        }
    }
    // payload
    let mut referenced = BTreeSet::new();
    for (_, e) in &enrolls {
        let h = st(e, "matcher_input_sha256").to_string();
        let name = format!("{h}.json");
        if !inp.payload_names.contains(&name) {
            return stop("payload missing");
        }
        if referenced.insert(h.clone()) {
            let v: Value = serde_json::from_slice(&read(&ledger.join("payloads").join(&name))?).map_err(|e| Stop(e.to_string()))?;
            if csha(&v)? != h {
                return stop("payload hash");
            }
        }
    }
    if inp.payload_names.len() != referenced.len() {
        return stop("orphan payload");
    }
    // checkpoint
    let mut cprev = "0".repeat(64);
    let mut last_count = 0u64;
    for (i, (v, h)) in inp.cps.iter().enumerate() {
        let c = v["record_count"].as_u64().unwrap_or(0);
        if v["seq"] != (i as u64 + 1) || st(v, "prev_checkpoint_sha256") != cprev || c < last_count || c == 0 || c as usize > n || st(v, "ledger_head_sha256") != inp.recs[c as usize - 1].commit || st(v, "enrollment_rule_sha256") != rule_sha || st(v, "enrollment_tool_sha256") != tool_at(i as u64 + 1) {
            return stop("checkpoint chain");
        }
        let added = &kinds[last_count as usize..c as usize];
        let ok = match st(v, "kind") {
            "baseline" => i == 0 && c == 1,
            "missed" => i > 0 && added == ["missed"],
            "enroll" => i > 0 && added.iter().all(|k| *k == "enroll" || *k == "close"),
            _ => false,
        };
        if !ok {
            return stop("checkpoint shape");
        }
        last_count = c;
        cprev = h.clone();
    }
    // 各 record の tool sha は、それを含む最初の checkpoint の seq の期待値と一致する
    for (i, r) in inp.recs.iter().enumerate() {
        let sha_here = match st(&r.body, "kind") {
            "baseline" => st(&r.body, "enrollment_tool_sha256"),
            "enroll" => st(&r.body["enrollment_context"], "enrollment_tool_sha256"),
            _ => continue,
        };
        let Some(q) = inp.cps.iter().position(|(v, _)| v["record_count"].as_u64().unwrap_or(0) as usize > i) else { return stop("record not covered by a checkpoint") };
        if sha_here != tool_at(q as u64 + 1) {
            return stop("record tool sha timeline");
        }
    }
    let (lastv, lasth) = inp.cps.last().ok_or_else(|| Stop("no checkpoint".into()))?;
    if last_count as usize != n || inp.cps.iter().filter(|(v, _)| v["record_count"].as_u64() == Some(n as u64)).count() != 1 || *lasth != close_anchor || st(lastv, "run_utc") != times[n - 1] || st(&inp.cps[0].0, "run_utc") != t0 {
        return stop("close anchor / post-close checkpoint / uncovered tail");
    }
    for a in p["auditor_anchors"].as_array().cloned().unwrap_or_default() {
        let q = a["checkpoint_seq"].as_u64().unwrap_or(0) as usize;
        if q == 0 || inp.cps.get(q - 1).map(|c| c.1.as_str()) != a["checkpoint_sha256"].as_str() {
            return stop("auditor anchor");
        }
    }
    // reports
    let mut seen = BTreeSet::new();
    for (_, r, _) in &inp.reports {
        let q = r["checkpoint_seq"].as_u64().unwrap_or(0) as usize;
        let Some((cv, ch)) = inp.cps.get(q.wrapping_sub(1)) else { return stop("report seq") };
        if !seen.insert(q) || st(r, "checkpoint_sha256") != ch || r["record_count"] != cv["record_count"] || st(r, "ledger_head_sha256") != st(cv, "ledger_head_sha256") {
            return stop("report mismatch");
        }
    }
    if seen.len() != inp.cps.len() {
        return stop("report missing");
    }
    // deviation / cadence
    let mut per_day: BTreeMap<i64, u64> = BTreeMap::new();
    for (v, _) in inp.cps.iter().skip(1) {
        *per_day.entry(days_since_epoch_jst(st(v, "run_utc"))?).or_default() += 1;
    }
    let mut pins: BTreeMap<i64, u64> = BTreeMap::new();
    let mut exposure: Vec<(String, u64, Vec<String>)> = vec![];
    for d in p["deviations"].as_array().cloned().unwrap_or_default() {
        if st(&d, "auditor_disposition") != "accepted_non_material" || st(&d, "class") != "extra_run" {
            return stop("deviation disposition");
        }
        let q = d["declared_checkpoint_seq"].as_u64().unwrap_or(0) as usize;
        let Some((cv, _)) = q.checked_sub(1).and_then(|i| inp.cps.get(i)) else { return stop("deviation seq") };
        let fname = st(&d, "deviation_file_name");
        if st(cv, "run_utc") != st(&d, "run_utc") || st(cv, "kind") == "baseline" || fname.contains(['/', '\\']) || sha(&read(&dev_dir.join(fname))?) != st(&d, "deviation_file_sha256") {
            return stop("deviation mismatch");
        }
        let lo = if q >= 2 { inp.cps[q - 2].0["record_count"].as_u64().unwrap_or(0) as usize } else { 0 };
        let hi = cv["record_count"].as_u64().unwrap_or(0) as usize;
        let keys: Vec<String> = enrolls.iter().filter(|(i, _)| *i >= lo && *i < hi).map(|(_, e)| st(e, "evaluation_unit_key").to_string()).collect();
        if keys.len() as u64 != d["declared_enrolled_in_run"].as_u64().unwrap_or(u64::MAX) {
            return stop("deviation count");
        }
        *pins.entry(days_since_epoch_jst(st(cv, "run_utc"))?).or_default() += 1;
        exposure.push((st(&d, "deviation_id").to_string(), keys.len() as u64, keys));
    }
    // ledger 外の欠落日: SHA 固定済みの 2026-10-10 の 1 件だけを許す
    let exc = p["cadence_exceptions"].as_array().cloned().unwrap_or_default();
    let mut declared: Vec<i64> = vec![];
    for e in &exc {
        if exc.len() != 1 || st(e, "jst_date") != "2026-10-10" || st(e, "type") != "missed_without_ledger_record" || st(e, "audit_note_sha256") != "5f80972600dbb92395ccdf63ecacc3b49b0fda30fabd3c86f572149e26568b68"
            || st(e, "disposition") != "accepted_non_material" || st(e, "membership_impact") != "none" || st(e, "checkpoint_impact") != "none" || st(e, "backfill") != "forbidden"
        {
            return stop("cadence exception");
        }
        declared.push(days_since_epoch_jst("2026-10-10T00:00:00Z")?);
    }
    if !exc.is_empty() && close["provenance"]["cadence_exception_preflight_sha256"] != "5f80972600dbb92395ccdf63ecacc3b49b0fda30fabd3c86f572149e26568b68" {
        return stop("exception note preflight not recorded");
    }
    if exc.len() > 1 {
        return stop("cadence exception count");
    }
    let (lo, hi) = (per_day.keys().next().copied().unwrap_or(0), per_day.keys().next_back().copied().unwrap_or(-1));
    let gaps: Vec<i64> = (lo..=hi).filter(|d| !per_day.contains_key(d)).collect();
    if gaps != declared {
        return stop("undeclared gap / exception not a gap");
    }
    if per_day.iter().any(|(d, c)| *c > 1 && pins.get(d).copied().unwrap_or(0) < c - 1) {
        return stop("undeclared deviation");
    }

    // ── 成果物との突き合わせ（authoritative commitment は PF0B の関数）
    let keys: Vec<String> = enrolls.iter().map(|(_, e)| st(e, "evaluation_unit_key").to_string()).collect();
    let version = format!("pr4-h2-enrollment-rules-2/enrollment_rule_sha256={rule_sha}");
    let head = &inp.recs[n - 1].commit;
    let eq = |a: &Value, b: &str, what: &str| -> V<()> { if a.as_str() == Some(b) { Ok(()) } else { stop(what) } };
    eq(&commit["membership_commitment_sha256"], &membership_commitment(&keys, &version), "membership_commitment")?;
    eq(&commit["eligibility_rule_version"], &version, "eligibility_rule_version")?;
    let mut sk = keys.clone();
    sk.sort();
    let set_pre = |ks: &[String]| sha(format!("pr4-pf0h0-final-membership-1\npr4-h2-enrollment-rules-2\n{}", ks.iter().map(|k| format!("{k}\n")).collect::<String>()).as_bytes());
    eq(&commit["auxiliary_audit_values"]["unit_set_sha256"], &set_pre(&sk), "unit_set_sha256")?;
    let mut rows: Vec<(String, String, String)> = enrolls.iter().map(|(_, e)| (st(e, "evaluation_unit_key").into(), st(e, "matcher_input_sha256").into(), st(e, "enrollment_context_sha256").into())).collect();
    rows.sort();
    eq(&commit["auxiliary_audit_values"]["unit_input_sha256"], &sha(format!("pr4-pf0h0-final-membership-input-1\n{}", rows.iter().map(|(k, a, b)| format!("{k}\t{a}\t{b}\n")).collect::<String>()).as_bytes()), "unit_input_sha256")?;
    if commit["unit_count"] != 946 || commit["authoritative"] != true || commit["auxiliary_audit_values"]["authoritative"] != false {
        return stop("commitment flags");
    }
    eq(&commit["ledger_head_sha256"], head, "commit ledger head")?;
    eq(&commit["close_record_commitment"], head, "close commitment")?;
    let subs = commit["deviation_subset_commitments"].as_array().cloned().unwrap_or_default();
    if subs.len() != exposure.len() {
        return stop("deviation subsets");
    }
    for (s, (id, cnt, ks)) in subs.iter().zip(exposure.iter()) {
        let mut k2 = ks.clone();
        k2.sort();
        if st(s, "deviation_id") != id || s["unit_count"] != *cnt || st(s, "unit_set_sha256") != set_pre(&k2) || s["audit_only"] != true || st(s, "promotion_use") != "forbidden" {
            return stop("deviation subset value");
        }
    }
    let l = &freeze["ledger"];
    eq(&l["ledger_jsonl_sha256"], &sha(&inp.ledger_raw), "ledger sha")?;
    if l["byte_length"] != inp.ledger_raw.len() as u64 || l["record_count"] != n as u64 {
        return stop("ledger len");
    }
    eq(&l["ledger_head_sha256"], head, "freeze head")?;
    eq(&freeze["baseline_ids_sha256"], &sha(&inp.baseline_raw), "baseline ids sha")?;
    eq(&freeze["payload_tree"]["payload_tree_sha256"], &sha(inp.payload_names.iter().map(|x| format!("{}\n", x.trim_end_matches(".json"))).collect::<String>().as_bytes()), "payload tree")?;
    if freeze["payload_tree"]["file_count"] != inp.payload_names.len() as u64 || freeze["checkpoint_chain"]["count"] != inp.cps.len() as u64 {
        return stop("freeze counts");
    }
    eq(&freeze["checkpoint_chain"]["head_sha256"], lasth, "cp head")?;
    eq(&freeze["checkpoint_chain"]["files_digest_sha256"], &digest(&inp.cp_files), "cp files digest")?;
    eq(&freeze["reports_digest_sha256"], &digest(&inp.reports.iter().map(|(n, _, h)| (n.clone(), h.clone())).collect::<Vec<_>>()), "reports digest")?;
    eq(&close["params_sha256"], &sha(&params_raw), "params sha")?;
    eq(&close["inputs"]["ledger_head_sha256"], head, "close head")?;
    eq(&close["inputs"]["checkpoint_head_sha256"], lasth, "close cp head")?;
    eq(&close["inputs"]["close_anchor_checkpoint_sha256"], close_anchor, "close anchor")?;
    if close["status"] != "PASS" || close["counts"]["enrolled_count"] != 946 || close["inputs"]["ledger_record_count"] != n as u64 || close["inputs"]["close_record_seq"] != n as u64 {
        return stop("close values");
    }
    for v in [&close, &commit, &freeze] {
        let t = v.to_string();
        if ["work:", "work_id", "tmdb", "title", "verdict", "candidate", "precision", "label"].iter().any(|b| t.contains(b)) {
            return stop("output not clean");
        }
    }
    Ok(())
}
