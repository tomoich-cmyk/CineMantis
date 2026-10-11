//! PR4-PF0H1 の synthetic test。**実 H2・実 production・DB に触れない。** 一時 dir に合成 ledger / checkpoint / payload / reports / params / deviation を作る。
//! ケース番号は `docs/PR4_PF0H0_H2_CLOSE_CONTRACT.md` §8（T01〜T48）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

use crate::services::pr4_pf0h_close::{self as close, canon, hex_sha, Fail, Inputs};
use crate::services::pr4_pf0h_verify as verify;

static SEQ: AtomicUsize = AtomicUsize::new(0);
const T0: &str = "2026-10-07T22:23:21Z";
const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const BASELINE_SEQ: i64 = 25;
const GEN_AT: &str = "2026-11-01T00:00:00Z";
const HEAD40: &str = "0aca746e2d0fee12ecfc660c5fc2e40d619158c6";

const OLD: &str = "183e0c73c14c20afd9ed34da33586f9599f96d1fff95c5c9a79d0b78855e3213";
const NEW: &str = "36f869f6d197615a610a50e1a164f1492eae06dd1c0eaad778c5311890518d08";
const NOTE_SHA: &str = "5f80972600dbb92395ccdf63ecacc3b49b0fda30fabd3c86f572149e26568b68";

/// checkpoint seq 1〜3 は OLD、seq 4 以降は NEW
fn tool_for(cp_seq: usize) -> String {
    (if cp_seq <= 3 { OLD } else { NEW }).to_string()
}
fn rule() -> String {
    hex_sha(b"rules")
}

#[derive(Clone)]
struct Run {
    kind: &'static str,
    n_new: usize,
    close: bool,
    time: String,
}

fn day_time(k: usize, hhmm: &str) -> String {
    format!("2026-10-{:02}T{hhmm}:00Z", 7 + k)
}

/// 100 件ずつ積む日次 run（3 日目に missed を 1 回）。`close` なら最後の run が close
fn plan(total: usize, close_at_end: bool) -> Vec<Run> {
    plan_gap(total, close_at_end, None)
}

/// `skip_k` の日は run を走らせない（ledger にも checkpoint にも現れない欠落日）。k=1 は JST 2026-10-09
fn plan_gap(total: usize, close_at_end: bool, skip_k: Option<usize>) -> Vec<Run> {
    let mut runs = vec![];
    let (mut left, mut k) = (total, 1usize);
    while left > 0 {
        if Some(k) == skip_k {
            k += 1;
            continue;
        }
        if k == 5 {
            runs.push(Run { kind: "missed", n_new: 0, close: false, time: day_time(k, "22:30") });
            k += 1;
            continue;
        }
        let n = left.min(100);
        left -= n;
        runs.push(Run { kind: "enroll", n_new: n, close: left == 0 && close_at_end, time: day_time(k, "22:30") });
        k += 1;
    }
    runs
}

struct Pin {
    run_index: usize,
    disposition: &'static str,
    delta: i64,
    class: &'static str,
}

struct Opts {
    plan: Vec<Run>,
    mutate: Option<Box<dyn Fn(&mut Vec<Value>)>>,
    anchors: Option<Vec<(u64, String)>>,
    drop_last_cp: bool,
    pins: Vec<Pin>,
    salt: u64,
    tools: fn(usize) -> String,
    exception: bool,
    exception_sha: &'static str,
}

fn opts(plan: Vec<Run>) -> Opts {
    Opts { plan, mutate: None, anchors: None, drop_last_cp: false, pins: vec![], salt: 0, tools: tool_for, exception: false, exception_sha: NOTE_SHA }
}

struct Fx {
    base: PathBuf,
    ledger: PathBuf,
    cps: PathBuf,
    devs: PathBuf,
    params: PathBuf,
    anchor: String,
    cp_shas: Vec<String>,
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pr4_pf0h_{tag}_{}_{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sha_v(v: &Value) -> String {
    hex_sha(canon(v).unwrap().as_bytes())
}

fn fix_ctx(b: &mut Value) {
    let h = sha_v(&b["enrollment_context"]);
    b["enrollment_context_sha256"] = json!(h);
}

fn chain(bodies: &mut [Value]) -> Vec<String> {
    let mut prev = GENESIS.to_string();
    let mut commits = vec![];
    for (i, b) in bodies.iter_mut().enumerate() {
        b["seq"] = json!(i as u64 + 1);
        b["prev_commitment"] = json!(prev);
        b.as_object_mut().unwrap().remove("record_commitment");
        let c = hex_sha(format!("{}\n{}", canon(b).unwrap(), prev).as_bytes());
        commits.push(c.clone());
        prev = c;
    }
    commits
}

fn write_ledger(dir: &Path, bodies: &[Value], commits: &[String]) {
    let mut text = String::new();
    for (b, c) in bodies.iter().zip(commits) {
        let mut r = b.clone();
        r["record_commitment"] = json!(c);
        text.push_str(&canon(&r).unwrap());
        text.push('\n');
    }
    std::fs::write(dir.join("ledger.jsonl"), text).unwrap();
}

fn build(tag: &str, o: Opts) -> Fx {
    let base = tmp(tag);
    let (ledger, cps, devs) = (base.join("ledger"), base.join("checkpoints"), base.join("devs"));
    for d in [&ledger, &cps, &devs, &ledger.join("payloads"), &ledger.join("reports")] {
        std::fs::create_dir_all(d).unwrap();
    }
    let ids: Vec<i64> = (1..=20).collect();
    std::fs::write(ledger.join("baseline_ids.json"), canon(&json!({"ids": ids})).unwrap()).unwrap();
    let bhash = hex_sha(std::iter::once("pr4-h2-baseline-1\n".to_string()).chain(ids.iter().map(|i| format!("work:{i}\n"))).collect::<String>().as_bytes());
    let mut bodies: Vec<Value> = vec![json!({"kind": "baseline", "observed_at": T0, "baseline_count": 20, "works_sequence": BASELINE_SEQ, "baseline_membership_sha256": bhash, "enrollment_rule_version": "pr4-h2-enrollment-rules-2", "enrollment_tool_sha256": (o.tools)(1)})];
    let mut ends = vec![];
    let mut unit = 0usize;
    for (run_i, r) in o.plan.iter().enumerate() {
        for _ in 0..r.n_new {
            let payload = json!({"u": unit, "salt": o.salt});
            let ps = sha_v(&payload);
            std::fs::write(ledger.join("payloads").join(format!("{ps}.json")), canon(&payload).unwrap()).unwrap();
            let ctx = json!({"evidence_class_at_enrollment": "live", "first_seen_at": r.time, "enrollment_rule_version": "pr4-h2-enrollment-rules-2", "enrollment_tool_sha256": (o.tools)(run_i + 2),
                "payload_schema_version": "pr4-h2-input-payload-1", "canonicalization_version": "pr4-canonical-json-1", "local_evidence_sha256": hex_sha(b"l"), "embedded_evidence_sha256": hex_sha(b"e"), "decision_evidence_sha256": hex_sha(b"d")});
            let mut b = json!({"kind": "enroll", "evaluation_unit_key": format!("work:{}", BASELINE_SEQ as usize + 1 + unit), "first_seen_at": r.time, "matcher_input_sha256": ps, "enrollment_context": ctx});
            fix_ctx(&mut b);
            bodies.push(b);
            unit += 1;
        }
        if r.close {
            bodies.push(json!({"kind": "close", "observed_at": r.time, "enrolled_count": unit}));
        }
        if r.kind == "missed" {
            bodies.push(json!({"kind": "missed", "observed_at": r.time}));
        }
        ends.push(bodies.len());
    }
    if let Some(m) = &o.mutate {
        m(&mut bodies);
    }
    let commits = chain(&mut bodies);
    write_ledger(&ledger, &bodies, &commits);
    // checkpoints
    let mut cp_shas: Vec<String> = vec![];
    let mut prev = GENESIS.to_string();
    let mut rows: Vec<(String, &str, usize)> = vec![(T0.to_string(), "baseline", 1)];
    for (r, e) in o.plan.iter().zip(&ends) {
        rows.push((r.time.clone(), r.kind, *e));
    }
    if o.drop_last_cp {
        rows.pop();
    }
    for (i, (t, kind, count)) in rows.iter().enumerate() {
        let count = (*count).min(commits.len());
        let v = json!({"seq": i + 1, "run_utc": t, "kind": kind, "record_count": count, "ledger_head_sha256": commits[count - 1], "works_sequence": BASELINE_SEQ,
            "enrollment_rule_sha256": rule(), "enrollment_tool_sha256": (o.tools)(i + 1), "tool_head": HEAD40, "prev_checkpoint_sha256": prev});
        let h = sha_v(&v);
        std::fs::write(cps.join(format!("{:04}.json", i + 1)), canon(&v).unwrap() + "\n").unwrap();
        let total = bodies.iter().take(count).filter(|b| b["kind"] == "enroll").count();
        let rep = json!({"protocol": "pr4-pf0e-enrollment-run-1", "kind": kind, "observed_at": t, "record_count": count, "ledger_head_sha256": commits[count - 1], "total_enrolled": total,
            "closed": bodies.iter().take(count).any(|b| b["kind"] == "close"), "checkpoint_seq": i + 1, "checkpoint_sha256": h});
        std::fs::write(ledger.join("reports").join(format!("{}_{}.json", t.replace(':', ""), count)), serde_json::to_string_pretty(&rep).unwrap() + "\n").unwrap();
        cp_shas.push(h.clone());
        prev = h;
    }
    // deviation files + params
    let mut devs_json = vec![];
    for (n, p) in o.pins.iter().enumerate() {
        let run = &o.plan[p.run_index];
        let name = format!("PR4_PF0E_DEVIATION_SYNTH_{n}.txt");
        let content = format!("synthetic deviation {n}");
        std::fs::write(devs.join(&name), &content).unwrap();
        devs_json.push(json!({"deviation_id": format!("PR4_PF0E_DEVIATION_SYNTH_{n}"), "class": p.class, "run_utc": run.time, "declared_checkpoint_seq": p.run_index + 2,
            "declared_enrolled_in_run": (run.n_new as i64 + p.delta).max(0), "deviation_file_name": name, "deviation_file_sha256": hex_sha(content.as_bytes()), "auditor_disposition": p.disposition}));
    }
    let anchors = o.anchors.clone().unwrap_or_else(|| vec![(2, cp_shas[1].clone())]);
    let params = json!({
        "contract_version": "pr4-pf0h0-close-1", "expected_enrolled_count": 946, "enrollment_rule_version": "pr4-h2-enrollment-rules-2", "enrollment_rule_sha256": rule(),
        "enrollment_tool_sha_timeline": [{"checkpoint_seq_from": 1, "checkpoint_seq_to": 3, "sha256": OLD}, {"checkpoint_seq_from": 4, "checkpoint_seq_to": null, "sha256": NEW}],
        "tool_transition_provenance": {"pf0e3_main_commit": "69d150d633ad1097319e27545962d989a92aebd9", "pf0e3_original_commit": "98e3917fa022d98dd8c3e9293a865f8b44b6a13b", "daily_freeze_amendment_2_sha256": "f4a2eb6d92010aa35425dbe7cd882143ff026f4ba6dae755bfa5ebc6e42a0263"},
        "cadence_exceptions": if o.exception { vec![json!({"jst_date": "2026-10-10", "type": "missed_without_ledger_record", "audit_note_sha256": o.exception_sha, "disposition": "accepted_non_material", "membership_impact": "none", "checkpoint_impact": "none", "backfill": "forbidden"})] } else { vec![] },
        "t0_utc": T0,
        "baseline": {"baseline_count": 20, "baseline_membership_sha256": bhash, "works_sequence": BASELINE_SEQ, "snapshot_sha256": hex_sha(b"snap")},
        "cadence": {"frequency": "DAILY", "slot_local": "07:30", "slot_tz": "Asia/Tokyo", "day_boundary_tz": "Asia/Tokyo"},
        "auditor_anchors": anchors.iter().map(|(q, h)| json!({"checkpoint_seq": q, "checkpoint_sha256": h})).collect::<Vec<_>>(),
        "deviations": devs_json, "known_tool_quirks": [],
    });
    let params_path = base.join("params.json");
    std::fs::write(&params_path, serde_json::to_string_pretty(&params).unwrap()).unwrap();
    Fx { anchor: cp_shas.last().cloned().unwrap(), cp_shas, base, ledger, cps, devs, params: params_path }
}

fn good() -> Fx {
    build("good", opts(plan(946, true)))
}

fn go(fx: &Fx, out: &str) -> Result<[PathBuf; 3], Fail> {
    go_anchor(fx, out, &fx.anchor)
}

fn go_anchor(fx: &Fx, out: &str, anchor: &str) -> Result<[PathBuf; 3], Fail> {
    let o = fx.base.join(out);
    close::run(&Inputs { params_path: &fx.params, ledger_dir: &fx.ledger, checkpoint_dir: &fx.cps, deviation_dir: &fx.devs, out_dir: &o, close_anchor_sha256: anchor, generated_at: GEN_AT, tool_head: HEAD40, enrollment_tool_head: HEAD40, exception_note_preflight_sha256: Some(NOTE_SHA) })
}

fn stops(fx: &Fx, code: &str) {
    let r = go(fx, "out_stop");
    let f = r.expect_err(&format!("expected STOP {code}"));
    assert_eq!(f.code, code, "{f:?}");
    let o = fx.base.join("out_stop");
    for a in close::ARTIFACTS {
        assert!(!o.join(a).exists(), "STOP なのに {a} がある");
    }
    assert!(o.join(close::STOP_FILE).exists());
}

fn read_lines(fx: &Fx) -> Vec<String> {
    std::fs::read_to_string(fx.ledger.join("ledger.jsonl")).unwrap().lines().map(String::from).collect()
}

fn write_lines(fx: &Fx, l: &[String]) {
    std::fs::write(fx.ledger.join("ledger.jsonl"), l.iter().map(|x| format!("{x}\n")).collect::<String>()).unwrap();
}

/// 全 record を parse し直して chain だけ再計算する（偽造の再現）
fn rechain_ledger(fx: &Fx, edit: impl Fn(&mut Vec<Value>)) {
    let mut bodies: Vec<Value> = read_lines(fx).iter().map(|l| serde_json::from_str(l).unwrap()).collect();
    for b in bodies.iter_mut() {
        b.as_object_mut().unwrap().remove("record_commitment");
    }
    edit(&mut bodies);
    let c = chain(&mut bodies);
    write_ledger(&fx.ledger, &bodies, &c);
}

fn cp_edit(fx: &Fx, n: usize, f: impl Fn(&mut Value)) {
    let p = fx.cps.join(format!("{n:04}.json"));
    let mut v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    f(&mut v);
    std::fs::write(&p, canon(&v).unwrap() + "\n").unwrap();
}

fn rechain_cps(fx: &Fx) {
    let mut prev = GENESIS.to_string();
    let mut i = 1;
    while fx.cps.join(format!("{i:04}.json")).exists() {
        cp_edit(fx, i, |v| v["prev_checkpoint_sha256"] = json!(prev));
        let v: Value = serde_json::from_slice(&std::fs::read(fx.cps.join(format!("{i:04}.json"))).unwrap()).unwrap();
        prev = sha_v(&v);
        i += 1;
    }
}

fn set_params(fx: &Fx, f: impl Fn(&mut Value)) {
    let mut v: Value = serde_json::from_slice(&std::fs::read(&fx.params).unwrap()).unwrap();
    f(&mut v);
    std::fs::write(&fx.params, serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

fn mutating(f: impl Fn(&mut Vec<Value>) + 'static) -> Opts {
    let mut o = opts(plan(946, true));
    o.mutate = Some(Box::new(f));
    o
}

fn two_runs_one_day() -> Vec<Run> {
    // 9 日目と同じ JST 暦日に extra run（定刻 22:30Z = 07:30 JST、追加 23:00Z = 08:00 JST）
    let mut p = plan(946, false);
    let last = p.len() - 1;
    let t = p[last].time.clone();
    p[last].n_new -= 10;
    p.push(Run { kind: "enroll", n_new: 10, close: true, time: t.replace("22:30", "23:00") });
    p
}

// ── T01 / T42 / T44 / T48: 正常系 ─────────────────────────────────────────────────────────────

#[test]
fn t01_exactly_946_passes_and_is_deterministic_clean_and_read_only() {
    let fx = good();
    let before = (close::dir_listing(&fx.ledger).unwrap(), close::dir_listing(&fx.cps).unwrap());
    let a = go(&fx, "out1").expect("PASS");
    assert_eq!(before, (close::dir_listing(&fx.ledger).unwrap(), close::dir_listing(&fx.cps).unwrap()), "入力は不変");
    let b = go_with_gen(&fx, "out2", "2026-12-31T00:00:00Z");
    let read = |p: &PathBuf| std::fs::read_to_string(p).unwrap();
    assert_eq!(read(&a[1]), read(&b[1]), "commitment は決定的");
    assert_eq!(read(&a[2]), read(&b[2]), "freeze は決定的");
    let (ca, cb) = (read(&a[0]).replace(GEN_AT, "X"), read(&b[0]).replace("2026-12-31T00:00:00Z", "X"));
    assert_eq!(ca, cb, "H2_CLOSE は generated_at だけ違う");
    let c: Value = serde_json::from_str(&read(&a[1])).unwrap();
    assert_eq!(c["unit_count"], 946);
    assert_eq!(c["authoritative"], true);
    assert_eq!(c["auxiliary_audit_values"]["authoritative"], false);
    let keys: Vec<String> = (0..946).map(|u| format!("work:{}", 26 + u)).collect();
    assert_eq!(c["membership_commitment_sha256"], crate::services::pr4_pf0b::membership_commitment(&keys, &close::eligibility_rule_version(&rule())), "authoritative = PF0B の関数");
    let cl: Value = serde_json::from_str(&read(&a[0])).unwrap();
    assert_eq!((cl["status"].clone(), cl["counts"]["enrolled_count"].clone(), cl["counts"]["missed_count"].clone()), (json!("PASS"), json!(946), json!(1)));
    for p in &a {
        let t = read(p);
        assert!(!t.contains("work:") && !t.contains("title") && !t.contains("label"), "出力は清浄");
    }
    verify::verify(&fx.params, &fx.ledger, &fx.cps, &fx.devs, fx.base.join("out1").as_path(), &fx.anchor).expect("独立 verify も PASS");
}

fn go_with_gen(fx: &Fx, out: &str, gen: &str) -> [PathBuf; 3] {
    let o = fx.base.join(out);
    close::run(&Inputs { params_path: &fx.params, ledger_dir: &fx.ledger, checkpoint_dir: &fx.cps, deviation_dir: &fx.devs, out_dir: &o, close_anchor_sha256: &fx.anchor, generated_at: gen, tool_head: HEAD40, enrollment_tool_head: HEAD40, exception_note_preflight_sha256: Some(NOTE_SHA) }).unwrap()
}

// ── T02〜T08: 件数・close ────────────────────────────────────────────────────────────────────

#[test]
fn t02_to_t05_wrong_counts_and_missing_close_stop() {
    stops(&build("c945", opts(plan(945, false))), "E_NO_CLOSE");
    stops(&build("c945x", opts(plan(945, true))), "E_COUNT");
    stops(&build("c947x", opts(plan(947, true))), "E_COUNT");
    stops(&build("c946n", opts(plan(946, false))), "E_NO_CLOSE");
}

#[test]
fn t06_to_t08_anything_after_close_stops() {
    let mut p = plan(946, true);
    p.push(Run { kind: "enroll", n_new: 1, close: false, time: day_time(20, "22:30") });
    stops(&build("post_enroll", opts(p)), "E_RECORD_AFTER_CLOSE");
    let mut p = plan(946, true);
    p.push(Run { kind: "missed", n_new: 0, close: false, time: day_time(20, "22:30") });
    stops(&build("post_missed", opts(p)), "E_RECORD_AFTER_CLOSE");
    // close 後の no-op checkpoint も許さない（ledger は変わらず checkpoint だけ増える）
    let mut p = plan(946, true);
    p.push(Run { kind: "enroll", n_new: 0, close: false, time: day_time(20, "22:30") });
    stops(&build("post_noop", opts(p)), "E_POSTCLOSE_CHECKPOINT");
}

// ── T10〜T16: identity と payload ────────────────────────────────────────────────────────────

#[test]
fn t10_to_t12_identity_violations_stop() {
    stops(&build("dup", mutating(|b| b[100]["evaluation_unit_key"] = b[99]["evaluation_unit_key"].clone())), "E_DUPLICATE_UNIT");
    stops(&build("overlap", mutating(|b| b[100]["evaluation_unit_key"] = json!("work:5"))), "E_BASELINE_OVERLAP");
    stops(&build("seqle", mutating(|b| b[100]["evaluation_unit_key"] = json!("work:25"))), "E_IDENTITY_ASSUMPTION");
}

#[test]
fn t13_to_t16_payload_problems_stop() {
    let fx = good();
    let mut files: Vec<_> = std::fs::read_dir(fx.ledger.join("payloads")).unwrap().flatten().collect();
    files.sort_by_key(|e| e.file_name());
    std::fs::remove_file(files[7].path()).unwrap();
    stops(&fx, "E_PAYLOAD_MISSING");
    let fx = good();
    let p = std::fs::read_dir(fx.ledger.join("payloads")).unwrap().flatten().next().unwrap().path();
    std::fs::write(&p, "{\"u\":999,\"salt\":0}").unwrap();
    stops(&fx, "E_PAYLOAD_HASH");
    let fx = good();
    std::fs::write(fx.ledger.join("payloads").join(format!("{}.json", hex_sha(b"orphan"))), "{}").unwrap();
    stops(&fx, "E_ORPHAN_PAYLOAD");
}

// ── T17〜T20: ledger の改変 ─────────────────────────────────────────────────────────────────

#[test]
fn t17_to_t20_ledger_tampering_stops() {
    let fx = good();
    let mut l = read_lines(&fx);
    l[50] = l[50].replace("\"seq\":51", "\"seq\":52");
    write_lines(&fx, &l);
    stops(&fx, "E_LEDGER_CHAIN");
    let fx = good();
    let mut l = read_lines(&fx);
    l.remove(30);
    write_lines(&fx, &l);
    stops(&fx, "E_LEDGER_CHAIN");
    let fx = good();
    let mut l = read_lines(&fx);
    l.swap(40, 41);
    write_lines(&fx, &l);
    stops(&fx, "E_LEDGER_CHAIN");
    // 末尾（close）を削除: checkpoint が先行
    let fx = good();
    let mut l = read_lines(&fx);
    l.pop();
    write_lines(&fx, &l);
    let r = go(&fx, "o").unwrap_err();
    assert!(["E_NO_CLOSE", "E_CHECKPOINT_LEDGER_MISMATCH"].contains(&r.code), "{r:?}");
}

#[test]
fn t20_forged_rechain_is_caught_by_the_checkpoint_chain() {
    let fx = good();
    // 値は変えず chain も同じ → 通る（成立確認）。値を変えて再計算 → checkpoint の head と不一致
    rechain_ledger(&fx, |b| {
        b[200]["enrollment_context"]["local_evidence_sha256"] = json!(hex_sha(b"forged"));
        let h = sha_v(&b[200]["enrollment_context"]);
        b[200]["enrollment_context_sha256"] = json!(h);
    });
    stops(&fx, "E_CHECKPOINT_LEDGER_MISMATCH");
}

#[test]
fn t21_wholesale_forgery_of_ledger_and_checkpoints_is_caught_by_the_external_anchor() {
    let real = good();
    let mut o = opts(plan(946, true));
    o.salt = 7; // 全 payload / hash が別物の、内部整合した偽造一式
    o.anchors = Some(vec![(2, real.cp_shas[1].clone())]);
    let forged = build("forged", o);
    stops(&forged, "E_ANCHOR_MISMATCH");
}

// ── T22〜T26: checkpoint ─────────────────────────────────────────────────────────────────────

#[test]
fn t22_t23_checkpoint_rollback_stops() {
    let fx = good();
    let last = fx.cp_shas.len();
    std::fs::remove_file(fx.cps.join(format!("{last:04}.json"))).unwrap();
    stops(&fx, "E_UNCOVERED_TAIL");
    let fx = good();
    let last = fx.cp_shas.len();
    for i in last - 2..=last {
        std::fs::remove_file(fx.cps.join(format!("{i:04}.json"))).unwrap();
    }
    stops(&fx, "E_UNCOVERED_TAIL");
    // ledger と checkpoint の両方を close 前まで巻き戻す
    let fx = good();
    let last = fx.cp_shas.len();
    std::fs::remove_file(fx.cps.join(format!("{last:04}.json"))).unwrap();
    let mut l = read_lines(&fx);
    l.truncate(l.len() - 47);
    write_lines(&fx, &l);
    std::fs::remove_file(fx.ledger.join("reports").join(std::fs::read_dir(fx.ledger.join("reports")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).max().unwrap())).unwrap();
    let r = go(&fx, "o").unwrap_err();
    assert!(["E_NO_CLOSE", "E_ANCHOR_MISMATCH"].contains(&r.code), "{r:?}");
}

#[test]
fn t24_t25_checkpoint_tampering_stops() {
    let fx = good();
    cp_edit(&fx, 3, |v| v["works_sequence"] = json!(999));
    stops(&fx, "E_CHECKPOINT_CHAIN");
    let fx = good();
    std::fs::remove_file(fx.cps.join("0005.json")).unwrap();
    stops(&fx, "E_CHECKPOINT_CHAIN");
    let fx = good();
    cp_edit(&fx, 4, |v| v["ledger_head_sha256"] = json!(hex_sha(b"x")));
    rechain_cps(&fx);
    stops(&fx, "E_CHECKPOINT_LEDGER_MISMATCH");
}

#[test]
fn t26_ledger_ahead_of_the_checkpoints_stops() {
    let mut o = opts(plan(946, true));
    o.drop_last_cp = true;
    stops(&build("ahead", o), "E_UNCOVERED_TAIL");
}

// ── T27〜T30, T37, T38, T39, T47: provenance・params・構造 ─────────────────────────────────

#[test]
fn t27_to_t30_provenance_mismatches_stop() {
    let fx = good();
    set_params(&fx, |v| v["enrollment_rule_sha256"] = json!(hex_sha(b"other-rules")));
    stops(&fx, "E_RULE_PROVENANCE");
    let fx = good();
    set_params(&fx, |v| v["t0_utc"] = json!("2026-10-07T22:23:22Z"));
    stops(&fx, "E_T0_MISMATCH");
    let fx = good();
    std::fs::write(fx.ledger.join("baseline_ids.json"), canon(&json!({"ids": (1..=19).collect::<Vec<i64>>()})).unwrap()).unwrap();
    stops(&fx, "E_BASELINE_MISMATCH");
}

#[test]
fn t37_t38_t39_anchor_unknown_files_and_reports() {
    let fx = good();
    let r = go_anchor(&fx, "o", &hex_sha(b"nope")).unwrap_err();
    assert_eq!(r.code, "E_ANCHOR_MISMATCH");
    let fx = good();
    std::fs::write(fx.ledger.join("stray.txt"), "x").unwrap();
    stops(&fx, "E_UNKNOWN_FILE");
    let fx = good();
    let first = std::fs::read_dir(fx.ledger.join("reports")).unwrap().flatten().map(|e| e.path()).min().unwrap();
    let mut v: Value = serde_json::from_slice(&std::fs::read(&first).unwrap()).unwrap();
    v["record_count"] = json!(2);
    std::fs::write(&first, serde_json::to_string(&v).unwrap()).unwrap();
    stops(&fx, "E_REPORT_MISMATCH");
    let fx = good();
    let first = std::fs::read_dir(fx.ledger.join("reports")).unwrap().flatten().map(|e| e.path()).min().unwrap();
    std::fs::remove_file(first).unwrap();
    stops(&fx, "E_REPORT_MISSING");
}

#[test]
fn t47_incomplete_or_wrong_params_stop() {
    for edit in [
        (|v: &mut Value| v["expected_enrolled_count"] = json!(945)) as fn(&mut Value),
        |v| {
            v.as_object_mut().unwrap().remove("baseline");
        },
        |v| v["auditor_anchors"] = json!([]),
        |v| v["enrollment_rule_version"] = json!("pr4-h2-enrollment-rules-1"),
    ] {
        let fx = good();
        set_params(&fx, edit);
        stops(&fx, "E_PARAMS_INCOMPLETE");
    }
}

// ── T31〜T36: deviation ─────────────────────────────────────────────────────────────────────

fn extra(pin: Option<Pin>) -> Opts {
    let mut o = opts(two_runs_one_day());
    if let Some(p) = pin {
        o.pins.push(p);
    }
    o
}

fn pin(disposition: &'static str, delta: i64, class: &'static str) -> Pin {
    // 追加 run は最後の run（plan の末尾）
    Pin { run_index: usize::MAX, disposition, delta, class }
}

fn extra_fx(tag: &str, disposition: &'static str, delta: i64, class: &'static str, pinned: bool) -> Fx {
    let mut o = extra(None);
    if pinned {
        let idx = o.plan.len() - 1;
        o.pins.push(Pin { run_index: idx, ..pin(disposition, delta, class) });
    }
    build(tag, o)
}

#[test]
fn t31_declared_accepted_non_material_extra_run_passes_with_disclosure() {
    let fx = extra_fx("dev_ok", "accepted_non_material", 0, "extra_run", true);
    let out = go(&fx, "o").expect("PASS");
    let cl: Value = serde_json::from_slice(&std::fs::read(&out[0]).unwrap()).unwrap();
    assert_eq!(cl["deviation_exposure"][0]["enrolled_in_run"], 10);
    assert_eq!(cl["deviation_exposure"][0]["share_of_final"], "10/946");
    let cm: Value = serde_json::from_slice(&std::fs::read(&out[1]).unwrap()).unwrap();
    assert_eq!(cm["deviation_subset_commitments"][0]["unit_count"], 10);
    assert_eq!(cm["deviation_subset_commitments"][0]["audit_only"], true);
    assert_eq!(cm["deviation_subset_commitments"][0]["promotion_use"], "forbidden");
    verify::verify(&fx.params, &fx.ledger, &fx.cps, &fx.devs, fx.base.join("o").as_path(), &fx.anchor).expect("verify も PASS");
}

#[test]
fn t32_to_t36_deviation_failures_stop() {
    stops(&extra_fx("dev_none", "accepted_non_material", 0, "extra_run", false), "E_UNDECLARED_DEVIATION");
    stops(&extra_fx("dev_count", "accepted_non_material", 1, "extra_run", true), "E_DEVIATION_MISMATCH");
    let fx = extra_fx("dev_file", "accepted_non_material", 0, "extra_run", true);
    std::fs::write(fx.devs.join("PR4_PF0E_DEVIATION_SYNTH_0.txt"), "changed").unwrap();
    stops(&fx, "E_DEVIATION_MISMATCH");
    stops(&extra_fx("dev_block", "blocking", 0, "extra_run", true), "E_DEVIATION_NOT_ACCEPTED");
    stops(&extra_fx("dev_unres", "unresolved", 0, "extra_run", true), "E_DEVIATION_NOT_ACCEPTED");
    // 他の class で hard 失敗を waive することはできない
    stops(&extra_fx("dev_class", "accepted_non_material", 0, "ledger_chain", true), "E_PARAMS_INCOMPLETE");
    // pin 済みでも hard の失敗（payload 欠落）は waive されない
    let fx = extra_fx("dev_hard", "accepted_non_material", 0, "extra_run", true);
    let p = std::fs::read_dir(fx.ledger.join("payloads")).unwrap().flatten().next().unwrap().path();
    std::fs::remove_file(p).unwrap();
    stops(&fx, "E_PAYLOAD_MISSING");
}

// ── T40, T41: 実行中の変化・上書き拒否・原子性 ───────────────────────────────────────────────

#[test]
fn t40_input_changing_during_the_run_stops_and_t41_outputs_are_never_overwritten() {
    let fx = good();
    let out = fx.base.join("o40");
    let mk = |o: &Path| -> Inputs<'_> {
        // Inputs は借用なので、ここでは fx の参照を使う
        Inputs { params_path: &fx.params, ledger_dir: &fx.ledger, checkpoint_dir: &fx.cps, deviation_dir: &fx.devs, out_dir: Box::leak(Box::new(o.to_path_buf())), close_anchor_sha256: &fx.anchor, generated_at: GEN_AT, tool_head: HEAD40, enrollment_tool_head: HEAD40, exception_note_preflight_sha256: Some(NOTE_SHA) }
    };
    let r = close::run_with_hook(&mk(&out), &|| std::fs::write(fx.ledger.join("reports").join("late.json"), "{}").unwrap());
    assert_eq!(r.unwrap_err().code, "E_INPUT_CHANGED");
    assert!(!out.join(close::ARTIFACTS[0]).exists());
    // 既に成果物がある dir には書かない
    let fx2 = good();
    go(&fx2, "o41").unwrap();
    let again = go(&fx2, "o41").unwrap_err();
    assert_eq!(again.code, "E_OUTPUT_EXISTS");
    // STOP 後の dir（STOP ファイルのみ）にも書き足さない
    let fx3 = build("c945", opts(plan(945, true)));
    let _ = go(&fx3, "o41");
    assert_eq!(go(&fx3, "o41").unwrap_err().code, "E_OUTPUT_EXISTS");
}

// ── T43: commitment の感度 ─────────────────────────────────────────────────────────────────

#[test]
fn t43_commitments_are_order_independent_and_membership_sensitive() {
    let keys: Vec<String> = (0..50).map(|i| format!("work:{}", 100 + i)).collect();
    let mut rev = keys.clone();
    rev.reverse();
    let v = close::eligibility_rule_version(&rule());
    use crate::services::pr4_pf0b::membership_commitment as mc;
    assert_eq!(mc(&keys, &v), mc(&rev, &v));
    assert_eq!(close::unit_set_sha256(&keys), close::unit_set_sha256(&rev));
    let mut swapped = keys.clone();
    swapped[3] = "work:9999".into();
    assert_ne!(mc(&keys, &v), mc(&swapped, &v));
    assert_ne!(close::unit_set_sha256(&keys), close::unit_set_sha256(&swapped));
    assert_ne!(mc(&keys, &v), mc(&keys, "other"));
    // key は辞書順（数値順ではない）: work:10 < work:9
    let mut k2 = vec!["work:9".to_string(), "work:10".to_string()];
    k2.sort();
    assert_eq!(k2[0], "work:10");
}

// ── T45: ソース走査 ───────────────────────────────────────────────────────────────────────────

#[test]
fn t45_the_tools_touch_no_database_matcher_label_or_enrollment_tool_and_are_separate() {
    let close_src = include_str!("pr4_pf0h_close.rs");
    let verify_src = include_str!("pr4_pf0h_verify.rs");
    for (name, src) in [("close", close_src), ("verify", verify_src)] {
        for banned in ["rusqlite", "Connection", "pr4_pf0e", "prematch", "matcher_decision", "gt_sampling", "label_", "std::process", "reqwest", "TcpStream"] {
            assert!(!src.contains(banned), "{name} が {banned} を含む");
        }
    }
    assert!(!verify_src.contains("pr4_pf0h_close::"), "verify は close のコードを使わない");
    assert!(!close_src.contains("pr4_pf0h_verify::"), "close は verify を使わない");
    // verify は一切書かない
    for banned in ["fs::write", "create_dir", "remove_", "rename", "OpenOptions"] {
        assert!(!verify_src.contains(banned), "verify が {banned} を含む");
    }
    // close が書くのは出力 dir だけ（入力 dir への書き込み API を使わない）
    assert!(!close_src.contains("ledger.join(\"ledger.jsonl\")).write"));
}

// ── T46: PF0E の verify_ledger との判定一致 ──────────────────────────────────────────────────

#[test]
fn t46_ledger_judgment_agrees_with_the_enrollment_tools_own_verifier() {
    use crate::services::pr4_pf0e::verify_ledger;
    let fx = good();
    assert!(verify_ledger(&fx.ledger, None).is_ok());
    assert!(go(&fx, "ok").is_ok());
    for tamper in 0..3 {
        let fx = good();
        let mut l = read_lines(&fx);
        match tamper {
            0 => l[10] = l[10].replace("work:", "work: "),
            1 => {
                l.remove(5);
            }
            _ => l.swap(60, 61),
        }
        write_lines(&fx, &l);
        assert!(verify_ledger(&fx.ledger, None).is_err(), "pf0e も壊れを検出");
        assert!(go(&fx, "bad").is_err(), "close も STOP");
    }
    // close 後の追加: 両方 STOP
    let mut p = plan(946, true);
    p.push(Run { kind: "enroll", n_new: 1, close: false, time: day_time(20, "22:30") });
    let fx = build("post", opts(p));
    assert!(verify_ledger(&fx.ledger, None).is_err());
    assert!(go(&fx, "bad").is_err());
}

// ── verify tool 自身 ─────────────────────────────────────────────────────────────────────────

#[test]
fn the_independent_verifier_rejects_tampered_or_mismatched_artifacts_and_inputs() {
    let fx = good();
    let out = fx.base.join("v");
    go(&fx, "v").unwrap();
    let ok = |fx: &Fx| verify::verify(&fx.params, &fx.ledger, &fx.cps, &fx.devs, &out, &fx.anchor);
    assert!(ok(&fx).is_ok());
    for (file, edit) in [
        ("FINAL_MEMBERSHIP_COMMITMENT.json", (|v: &mut Value| v["membership_commitment_sha256"] = json!(hex_sha(b"x"))) as fn(&mut Value)),
        ("FINAL_MEMBERSHIP_COMMITMENT.json", |v| v["auxiliary_audit_values"]["unit_input_sha256"] = json!(hex_sha(b"x"))),
        ("H2_LEDGER_FINAL_FREEZE.json", |v| v["ledger"]["ledger_jsonl_sha256"] = json!(hex_sha(b"x"))),
        ("H2_LEDGER_FINAL_FREEZE.json", |v| v["checkpoint_chain"]["files_digest_sha256"] = json!(hex_sha(b"x"))),
        ("H2_CLOSE.json", |v| v["inputs"]["ledger_head_sha256"] = json!(hex_sha(b"x"))),
    ] {
        let orig = std::fs::read(out.join(file)).unwrap();
        let mut v: Value = serde_json::from_slice(&orig).unwrap();
        edit(&mut v);
        std::fs::write(out.join(file), serde_json::to_string_pretty(&v).unwrap()).unwrap();
        assert!(ok(&fx).is_err(), "{file} の改変を検出する");
        std::fs::write(out.join(file), orig).unwrap();
    }
    assert!(ok(&fx).is_ok());
    // 入力が後から壊れたら、成果物が残っていても verify は失敗
    let p = std::fs::read_dir(fx.ledger.join("payloads")).unwrap().flatten().next().unwrap().path();
    std::fs::write(&p, "{}").unwrap();
    assert!(ok(&fx).is_err());
    // STOP ファイルがある dir は検証しない
    let fx2 = build("c945", opts(plan(945, true)));
    let _ = go(&fx2, "s");
    assert!(verify::verify(&fx2.params, &fx2.ledger, &fx2.cps, &fx2.devs, &fx2.base.join("s"), &fx2.anchor).is_err());
    // verify の判定は close の STOP と一致する（close が STOP する入力を、成果物を偽造して通さない）
    let fx3 = build("post", { let mut p = plan(946, true); p.push(Run { kind: "enroll", n_new: 0, close: false, time: day_time(20, "22:30") }); opts(p) });
    let forged = fx3.base.join("f");
    std::fs::create_dir_all(&forged).unwrap();
    for n in close::ARTIFACTS {
        std::fs::copy(out.join(n), forged.join(n)).unwrap();
    }
    assert!(verify::verify(&fx3.params, &fx3.ledger, &fx3.cps, &fx3.devs, &forged, &fx3.anchor).is_err());
}

// ── tool SHA の時系列（seq 1〜3 = OLD / seq 4 以降 = NEW）──────────────────────────────────────

fn with_tools(f: fn(usize) -> String) -> Opts {
    let mut o = opts(plan(946, true));
    o.tools = f;
    o
}

#[test]
fn tool_sha_is_checked_as_a_timeline_not_a_set() {
    // (a) seq 1〜3 は OLD、seq 4 以降は NEW → PASS（good() がそれ）。境界を明示的に確認
    let fx = good();
    for (i, expected) in [(1, OLD), (2, OLD), (3, OLD), (4, NEW), (5, NEW)] {
        let v: Value = serde_json::from_slice(&std::fs::read(fx.cps.join(format!("{i:04}.json"))).unwrap()).unwrap();
        assert_eq!(v["enrollment_tool_sha256"], expected, "checkpoint {i}");
    }
    assert!(go(&fx, "ok").is_ok());
    // (b) seq 4 以降に OLD へ戻る
    stops(&build("t_back", with_tools(|q| if q >= 6 { OLD.into() } else { tool_for(q) })), "E_TOOL_SHA");
    stops(&build("t_back4", with_tools(|q| if q == 4 { OLD.into() } else { tool_for(q) })), "E_TOOL_SHA");
    // (c) seq 3 以前に NEW が現れる
    stops(&build("t_early", with_tools(|q| if q == 3 { NEW.into() } else { tool_for(q) })), "E_TOOL_SHA");
    stops(&build("t_early1", with_tools(|_| NEW.into())), "E_TOOL_SHA");
    // (d) unknown SHA（seq 1〜3 側でも 4 以降側でも）
    stops(&build("t_unk_old", with_tools(|q| if q == 2 { hex_sha(b"unknown").into() } else { tool_for(q) })), "E_TOOL_SHA");
    stops(&build("t_unk_new", with_tools(|q| if q == 7 { hex_sha(b"unknown").into() } else { tool_for(q) })), "E_TOOL_SHA");
    // 全部 OLD（transition せず）も STOP（seq 4 以降は NEW のみ）
    stops(&build("t_allold", with_tools(|_| OLD.into())), "E_TOOL_SHA");
    // checkpoint は正しく、ledger record だけが違う（record の run の seq で判定される）
    stops(&build("t_rec", mutating(|b| {
        b[500]["enrollment_context"]["enrollment_tool_sha256"] = json!(OLD);
        fix_ctx(&mut b[500]);
    })), "E_TOOL_SHA");
    stops(&build("t_rec_early", mutating(|b| {
        b[5]["enrollment_context"]["enrollment_tool_sha256"] = json!(NEW);
        fix_ctx(&mut b[5]);
    })), "E_TOOL_SHA");
    // transition provenance は params に pin 済みの値と一致しなければ params が不完全
    let fx = good();
    set_params(&fx, |v| v["tool_transition_provenance"]["pf0e3_main_commit"] = json!("0".repeat(40)));
    stops(&fx, "E_PARAMS_INCOMPLETE");
    let fx = good();
    set_params(&fx, |v| v["enrollment_tool_sha_timeline"][1]["checkpoint_seq_from"] = json!(3));
    stops(&fx, "E_PARAMS_INCOMPLETE");
    // 独立 verify も同じ判定
    let ok = good();
    let out = go(&ok, "vv").unwrap();
    assert!(out.len() == 3);
    verify::verify(&ok.params, &ok.ledger, &ok.cps, &ok.devs, &ok.base.join("vv"), &ok.anchor).unwrap();
}

// ── 2026-10-10 の ledger 外 missed（SHA 固定済みの historical exception 1 件のみ）────────────────

fn gap_fx(tag: &str, skip_k: usize, exception: bool, sha: &'static str) -> Fx {
    let mut o = opts(plan_gap(946, true, Some(skip_k)));
    o.exception = exception;
    o.exception_sha = sha;
    build(tag, o)
}

#[test]
fn the_2026_10_10_missed_without_a_ledger_record_is_the_only_recognised_gap() {
    // 10/09 = #1, 10/10 = #2 (MISSED, ledger に無い), 10/11 = #3。pin 済み SHA のときだけ PASS
    let fx = gap_fx("gap_ok", 2, true, NOTE_SHA);
    let out = go(&fx, "o").expect("PASS");
    let cl: Value = serde_json::from_slice(&std::fs::read(&out[0]).unwrap()).unwrap();
    assert_eq!(cl["counts"]["cadence_gap_days"], 1);
    assert_eq!(cl["provenance"]["cadence_exception_disclosed"], 1);
    assert_eq!(cl["provenance"]["tool_transition_provenance"]["switch_checkpoint_seq"], 4);
    verify::verify(&fx.params, &fx.ledger, &fx.cps, &fx.devs, &fx.base.join("o"), &fx.anchor).expect("verify も PASS");
    // 同じ欠落でも exception を pin していなければ STOP
    stops(&gap_fx("gap_unpinned", 2, false, NOTE_SHA), "E_UNDECLARED_GAP");
    // SHA 不一致の exception は params が不正
    stops(&gap_fx("gap_badsha", 2, true, "0000000000000000000000000000000000000000000000000000000000000001"), "E_PARAMS_INCOMPLETE");
    // 別日の ledger 外 missed（10/11 が欠落）は、10/10 の exception があっても STOP
    stops(&gap_fx("gap_otherday", 3, true, NOTE_SHA), "E_UNDECLARED_GAP");
    stops(&gap_fx("gap_otherday2", 4, false, NOTE_SHA), "E_UNDECLARED_GAP");
    // 欠落が 2 日（10/10 と別日）なら exception があっても STOP
    let mut o = opts({
        let mut p = plan_gap(946, true, Some(2));
        let n = p.len();
        p[n - 1].time = day_time(30, "22:30");
        p
    });
    o.exception = true;
    stops(&build("gap_two", o), "E_UNDECLARED_GAP");
    // exception を pin したが 10/10 は実在した（欠落していない）→ 宣言と事実が合わない
    let mut o = opts(plan(946, true));
    o.exception = true;
    stops(&build("gap_not_gap", o), "E_DEVIATION_MISMATCH");
    // exception は 1 件だけ（一般化しない）
    let fx = gap_fx("gap_two_exc", 2, true, NOTE_SHA);
    set_params(&fx, |v| {
        let e = v["cadence_exceptions"][0].clone();
        v["cadence_exceptions"] = json!([e.clone(), e]);
    });
    stops(&fx, "E_PARAMS_INCOMPLETE");
    // 別日付の exception を pin しようとしても params 不正
    let fx = gap_fx("gap_date", 2, true, NOTE_SHA);
    set_params(&fx, |v| v["cadence_exceptions"][0]["jst_date"] = json!("2026-10-11"));
    stops(&fx, "E_PARAMS_INCOMPLETE");
    // disposition / impact / backfill が違えば params 不正
    for (k, val) in [("disposition", "blocking"), ("membership_impact", "some"), ("checkpoint_impact", "some"), ("backfill", "allowed"), ("type", "other")] {
        let fx = gap_fx("gap_field", 2, true, NOTE_SHA);
        set_params(&fx, |v| v["cadence_exceptions"][0][k] = json!(val));
        stops(&fx, "E_PARAMS_INCOMPLETE");
    }
    // V6(c) chain coverage は exception があっても hard のまま
    let mut o = opts(plan_gap(946, true, Some(2)));
    o.exception = true;
    o.drop_last_cp = true;
    stops(&build("gap_uncovered", o), "E_UNCOVERED_TAIL");
}

#[test]
fn the_exception_note_preflight_is_required_and_recorded_when_the_exception_is_pinned() {
    let fx = gap_fx("pre", 2, true, NOTE_SHA);
    let run = |out: &str, pf: Option<&str>| {
        close::run(&Inputs { params_path: &fx.params, ledger_dir: &fx.ledger, checkpoint_dir: &fx.cps, deviation_dir: &fx.devs, out_dir: &fx.base.join(out), close_anchor_sha256: &fx.anchor, generated_at: GEN_AT, tool_head: HEAD40, enrollment_tool_head: HEAD40, exception_note_preflight_sha256: pf })
    };
    assert_eq!(run("a", None).unwrap_err().code, "E_EXCEPTION_PREFLIGHT");
    assert_eq!(run("b", Some(&hex_sha(b"other"))).unwrap_err().code, "E_EXCEPTION_PREFLIGHT");
    let out = run("c", Some(NOTE_SHA)).unwrap();
    let cl: Value = serde_json::from_slice(&std::fs::read(&out[0]).unwrap()).unwrap();
    assert_eq!(cl["provenance"]["cadence_exception_preflight_sha256"], NOTE_SHA);
    verify::verify(&fx.params, &fx.ledger, &fx.cps, &fx.devs, &fx.base.join("c"), &fx.anchor).unwrap();
    // 記録が消された成果物は verify が拒否する
    let mut v = cl.clone();
    v["provenance"]["cadence_exception_preflight_sha256"] = Value::Null;
    std::fs::write(&out[0], serde_json::to_string_pretty(&v).unwrap()).unwrap();
    assert!(verify::verify(&fx.params, &fx.ledger, &fx.cps, &fx.devs, &fx.base.join("c"), &fx.anchor).is_err());
}

/// Condition A: authoritative membership commitment は PF0B の既存定義と byte-for-byte 一致する（close の出力・verify の受理値の両方）
#[test]
fn the_authoritative_membership_commitment_is_byte_identical_to_the_pf0b_definition() {
    use crate::services::pr4_pf0b::membership_commitment as pf0b;
    let fx = good();
    let out = go(&fx, "parity").unwrap();
    let c: Value = serde_json::from_slice(&std::fs::read(&out[1]).unwrap()).unwrap();
    let keys: Vec<String> = (0..946).map(|u| format!("work:{}", 26 + u)).collect();
    let version = close::eligibility_rule_version(&rule());
    // (1) PF0B の関数そのもの
    assert_eq!(c["membership_commitment_sha256"], pf0b(&keys, &version));
    // (2) PF0B の定義（pre-image）を手で組んだ値。domain = pr4-pf0a-membership-1、key は byte 順
    let mut sorted = keys.clone();
    sorted.sort();
    let pre = format!("pr4-pf0a-membership-1
{version}
{}", sorted.iter().map(|k| format!("{k}
")).collect::<String>());
    assert_eq!(c["membership_commitment_sha256"], hex_sha(pre.as_bytes()));
    // (3) 補助値は authoritative と別物で、authoritative として扱われない
    assert_ne!(c["membership_commitment_sha256"], c["auxiliary_audit_values"]["unit_set_sha256"]);
    assert_eq!(c["authoritative"], true);
    assert_eq!(c["auxiliary_audit_values"]["authoritative"], false);
    // (4) verify は PF0B と食い違う値（補助値を正本にすり替える等）を拒否する
    let aux = c["auxiliary_audit_values"]["unit_set_sha256"].clone();
    let mut v = c.clone();
    v["membership_commitment_sha256"] = aux;
    std::fs::write(&out[1], serde_json::to_string_pretty(&v).unwrap()).unwrap();
    assert!(verify::verify(&fx.params, &fx.ledger, &fx.cps, &fx.devs, &fx.base.join("parity"), &fx.anchor).is_err());
    // (5) 版名が違えば commitment も違う（version は provenance だが再計算の入力には入る）
    assert_ne!(pf0b(&keys, &version), pf0b(&keys, "other-version"));
}

/// PF0H final manifest SHA の凍結規則: canonical SHA256(FINAL_MEMBERSHIP_COMMITMENT.json)。raw byte の体裁に依存しない
#[test]
fn the_final_manifest_sha_is_the_canonical_sha_and_ignores_formatting() {
    // golden: canon = {"a":1,"b":["x",null,true]} （キー昇順・空白なし）
    let g = close::final_manifest_sha256(br#"{ "b": ["x", null, true], "a": 1 }"#).unwrap();
    assert_eq!(g, hex_sha(br#"{"a":1,"b":["x",null,true]}"#));
    assert_eq!(g, "f3b8a4ec6b2ad1666747731d5601847491e0ae81f0585f2b1edb8aa823e8f3ff");
    let fx = good();
    let out = go(&fx, "fm").unwrap();
    let raw = std::fs::read(&out[1]).unwrap();
    let sha = close::final_manifest_sha256(&raw).unwrap();
    let v: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(sha, hex_sha(canon(&v).unwrap().as_bytes()));
    // 体裁（pretty / compact / CRLF）が違っても同じ
    assert_eq!(sha, close::final_manifest_sha256(serde_json::to_string(&v).unwrap().as_bytes()).unwrap());
    assert_eq!(sha, close::final_manifest_sha256(serde_json::to_string_pretty(&v).unwrap().replace('\n', "\r\n").as_bytes()).unwrap());
    // 内容が違えば変わる。H2_CLOSE.json の raw SHA とは別物
    let mut w = v.clone();
    w["unit_count"] = json!(945);
    assert_ne!(sha, close::final_manifest_sha256(w.to_string().as_bytes()).unwrap());
    assert_ne!(sha, hex_sha(&std::fs::read(&out[0]).unwrap()));
    // 2 回の close で FINAL_MEMBERSHIP_COMMITMENT は完全一致 → SHA も一致
    let out2 = go_with_gen(&fx, "fm2", "2027-01-01T00:00:00Z");
    assert_eq!(sha, close::final_manifest_sha256(&std::fs::read(&out2[1]).unwrap()).unwrap());
}
