//! PR4-PF0I1 の synthetic テスト。実 ledger・payload・production DB・H2・H1・sealed holdout は使わない。fixture は全て in-memory の合成。

use super::*;
use crate::services::pr4_pf0b::{evaluate, Decision};
use std::collections::HashMap;

const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const VER: &str = "opaque-eligibility-version-for-test";
const POLICY_JSON: &str = include_str!("../../../../../docs/pr4_final_policy.json");
const NOTE_CANARY: &str = "CANARY_NOTE work:99 AUTO";

fn h(l: &str) -> String {
    hex_sha(l.as_bytes())
}

#[derive(Clone, Debug)]
enum Step {
    Confirm(usize),
    Pick(i64),
    NoneM,
    Defer,
    DeferFinal,
    Correct(Box<Step>),
    Qc(Box<Step>),
}

struct Fx {
    n: usize,
    bodies: Vec<Value>,
    bundle: Value,
    init_over: Vec<(&'static str, String)>,
    seal_over: Vec<(&'static str, String)>,
    anchors: Anchors,
    pf0h: Pf0hLoaded,
    sub: u32,
}

struct Rendered {
    ledger: String,
    bundle: Vec<u8>,
    anchors: Anchors,
    pf0h: Pf0hLoaded,
}

fn work(i: usize) -> String {
    format!("work:{}", i + 1)
}
fn cands(i: usize) -> Vec<(&'static str, i64)> {
    if i % 5 == 4 {
        vec![]
    } else {
        vec![("movie", 1000 + 2 * i as i64), ("tv", 1001 + 2 * i as i64)]
    }
}

impl Fx {
    fn new(n: usize, script: &[(usize, Vec<Step>)]) -> Fx {
        let sc: HashMap<usize, Vec<Step>> = script.iter().cloned().collect();
        let mut units = Vec::new();
        let mut keys = Vec::new();
        for i in 0..n {
            let uid = derive_review_unit_id(SECRET, &work(i)).unwrap();
            let cs: Vec<Value> = cands(i)
                .iter()
                .map(|(mt, id)| json!({"candidate_token": derive_candidate_token(SECRET, &uid, mt, *id).unwrap(), "media_type": mt, "tmdb_id": id}))
                .collect();
            units.push(json!({"review_unit_id": uid, "work_key": work(i), "candidates": cs}));
            keys.push(work(i));
        }
        let anchors_membership = membership_commitment(&keys, VER);
        let mut fx = Fx {
            n,
            bodies: vec![],
            bundle: json!({"bundle_version": BUNDLE_VERSION, "build_secret_hex": SECRET, "units": units}),
            init_over: vec![],
            seal_over: vec![],
            anchors: Anchors {
                expected_membership_commitment: anchors_membership.clone(),
                expected_pf0h_final_manifest_sha256: h("pf0h-final-manifest"),
                review_seal_sha256: String::new(),
                machine_freeze_anchor_commitment: h("machine-freeze-anchor"),
                eligibility_rule_version_opaque: VER.into(),
            },
            pf0h: Pf0hLoaded { authoritative_membership_commitment: anchors_membership, final_manifest_sha256: h("pf0h-final-manifest") },
            sub: 0,
        };
        for i in 0..n {
            let steps = sc.get(&i).cloned().unwrap_or_else(|| vec![if cands(i).is_empty() { Step::NoneM } else { Step::Confirm(0) }]);
            for s in steps {
                fx.push_step(i, &s, if i % 3 == 0 { Some(NOTE_CANARY) } else { None });
            }
        }
        fx
    }

    fn uid(&self, i: usize) -> String {
        derive_review_unit_id(SECRET, &work(i)).unwrap()
    }

    fn push_step(&mut self, i: usize, s: &Step, note: Option<&str>) {
        self.sub += 1;
        let uid = self.uid(i);
        let mut e = json!({"ts": "2026-10-11T00:00:00Z", "unit": uid, "lease_id": "l1", "session_id": "s1", "submission_id": format!("sub-{}", self.sub),
            "unit_prev": h("p"), "content_sha256": h("c"), "note": note, "correction_reason": null, "artifact_sha256": h("a"), "search_count": 1,
            "review_protocol_version": "gt-review-1", "reviewer_id": "reviewer-A"});
        let mut fill = |e: &mut Value, st: &Step| match st {
            Step::Confirm(k) => {
                let (mt, id) = cands(i)[*k];
                e["decision"] = json!("confirm");
                e["candidate_token"] = json!(derive_candidate_token(SECRET, &uid, mt, id).unwrap());
            }
            Step::Pick(id) => {
                e["decision"] = json!("pick_other");
                e["candidate_token"] = Value::Null;
                e["tmdb_id"] = json!(id);
                e["media_type"] = json!("movie");
                e["tmdb_verification"] = json!({"requested_id": id, "requested_media_type": "movie", "http_status": 200, "response_id": id, "response_media_type": "movie",
                    "response_body_sha256": h("body"), "title": "T work:77", "year": "2000", "fetched_at": "2026-10-11T00:00:00Z", "verifier_impl": "fake"});
            }
            Step::NoneM => {
                e["decision"] = json!("none");
                e["candidate_token"] = Value::Null;
                e["tmdb_id"] = Value::Null;
                e["media_type"] = Value::Null;
            }
            _ => {}
        };
        match s {
            Step::Defer => {
                e["type"] = json!("defer");
                e["defer_reason"] = json!("insufficient_evidence");
            }
            Step::DeferFinal => {
                e["type"] = json!("defer_final");
                e["defer_reason"] = json!("insufficient_evidence");
            }
            Step::Correct(inner) => {
                e["type"] = json!("correction");
                e["correction_reason"] = json!("mis_click");
                fill(&mut e, inner);
            }
            Step::Qc(inner) => {
                e["type"] = json!("qc_decision");
                fill(&mut e, inner);
            }
            other => {
                e["type"] = json!("decision");
                fill(&mut e, other);
            }
        }
        self.bodies.push(e);
    }

    fn render(&mut self) -> Rendered {
        let bundle = serde_json::to_vec(&self.bundle).unwrap();
        let secret = self.bundle["build_secret_hex"].as_str().unwrap().to_string();
        let count = self.bundle["units"].as_array().unwrap().len();
        let base: Vec<(&str, String)> = vec![
            ("protocol_sha256", h("protocol")),
            ("ui_package_sha256", h("ui")),
            ("shuffle_seed_commitment", h("shuffle")),
            ("attestation_policy_sha256", h("attestation")),
            ("unblind_map_sha256", hex_sha(&bundle)),
            ("build_secret_commitment", build_secret_commitment(&secret)),
            ("a8_leak_scan_report_sha256", h("a8")),
            ("machine_freeze_anchor_commitment", self.anchors.machine_freeze_anchor_commitment.clone()),
        ];
        let pins = |over: &Vec<(&'static str, String)>| -> Value {
            let mut m = serde_json::Map::new();
            for (k, v) in &base {
                m.insert(k.to_string(), json!(v));
            }
            for (k, v) in over {
                m.insert(k.to_string(), json!(v));
            }
            Value::Object(m)
        };
        let mut all: Vec<Value> = vec![json!({"type": "init", "ts": "2026-10-11T00:00:00Z", "manifest_sha256": h("m"), "artifact_sha256": h("a"), "unit_count": count,
            "pins": pins(&self.init_over), "anchored_at": "2026-10-10T00:00:00Z", "review_protocol_version": "gt-review-1", "core_tool_sha256": h("tool")})];
        all.extend(self.bodies.iter().cloned());
        let mut ledger = String::new();
        let mut prev = GENESIS.to_string();
        let mut head_before = String::new();
        let seal_body = json!({"type": "seal", "ts": "2026-10-11T00:00:00Z", "pins": pins(&self.seal_over), "audit_report_sha256": h("audit"), "ledger_head_before": "", "unit_count": count});
        let total = all.len() + 1;
        for (seq, mut e) in all.into_iter().chain(std::iter::once(seal_body)).enumerate() {
            if seq == total - 1 {
                e["ledger_head_before"] = json!(head_before);
            }
            e["schema_version"] = json!("pr4-blind-review-event-1");
            e["seq"] = json!(seq);
            e["prev_commitment"] = json!(prev);
            let c = hex_sha(format!("{}\n{}", canon(&e).unwrap(), prev).as_bytes());
            e["event_commitment"] = json!(c);
            ledger.push_str(&canon(&e).unwrap());
            ledger.push('\n');
            head_before = c.clone();
            prev = c;
        }
        let mut anchors = self.anchors.clone();
        anchors.review_seal_sha256 = prev;
        Rendered { ledger, bundle, anchors, pf0h: self.pf0h.clone() }
    }
}

fn policy() -> PolicyConfig {
    PolicyConfig::from_frozen_json(POLICY_JSON).unwrap()
}

fn run(r: &Rendered) -> AdapterOutcome {
    run_with(r, &policy())
}
fn run_with(r: &Rendered, p: &PolicyConfig) -> AdapterOutcome {
    adapt(&AdapterInput { ledger_bytes: r.ledger.as_bytes(), unblind_bundle_bytes: &r.bundle, anchors: &r.anchors, pf0h: &r.pf0h, policy: p })
}
fn ok(fx: &mut Fx) -> (Rendered, TrustedGtSet) {
    let r = fx.render();
    let o = run(&r);
    assert!(o.report.ready, "{:?}", o.report.refusal_codes);
    let g = o.gt.unwrap();
    (r, g)
}
fn refused(r: &Rendered, code: &str) {
    let o = run(r);
    assert!(!o.report.ready, "refused のはず: {code}");
    assert!(o.gt.is_none());
    assert!(o.report.refusal_codes.contains(&code), "want {code}, got {:?}", o.report.refusal_codes);
    assert_eq!(o.report.counts, Counts::default(), "REFUSED のとき件数は 0");
}
fn gt_of(g: &TrustedGtSet, i: usize) -> Gt {
    g.entries().iter().find(|(k, _)| *k == work(i)).unwrap().1.clone()
}
fn refused_fx(mut fx: Fx, code: &str) {
    let r = fx.render();
    refused(&r, code);
}

// ── A. 写像 ──

#[test]
fn t01_terminal_decisions_map_to_pf0b_gt() {
    let mut fx = Fx::new(
        6,
        &[
            (0, vec![Step::Confirm(1)]),
            (1, vec![Step::Pick(900_001)]),
            (2, vec![Step::NoneM]),
            (3, vec![Step::Defer, Step::DeferFinal]),
            (4, vec![Step::NoneM]),
            (5, vec![Step::Defer, Step::Confirm(0)]),
        ],
    );
    let (_, g) = ok(&mut fx);
    assert_eq!(gt_of(&g, 0), Gt::Positive("tv".into(), 1001)); // identity は bundle の写像から
    assert_eq!(gt_of(&g, 1), Gt::Positive("movie".into(), 900_001));
    assert_eq!(gt_of(&g, 2), Gt::NoMatch);
    assert_eq!(gt_of(&g, 3), Gt::Unresolved);
    assert_eq!(gt_of(&g, 4), Gt::NoMatch);
    assert_eq!(gt_of(&g, 5), Gt::Positive("movie".into(), 1000 + 10));
    assert_eq!(g.trusted_gt_source(), "frozen_blind_h2_gt");
}

#[test]
fn t02_malformed_decisions_are_refused_with_specific_codes() {
    // confirm: token が集合外
    let mut fx = Fx::new(5, &[]);
    fx.bodies[0]["candidate_token"] = json!("ct_aaaaaaaaaaaaaaaaaaaa");
    refused_fx(fx, "token_not_in_presented_set");
    // confirm: token 欠落
    let mut fx = Fx::new(5, &[]);
    fx.bodies[0]["candidate_token"] = Value::Null;
    refused_fx(fx, "decision_shape_invalid");
    // confirm: inline identity（PF0G1 は confirm に identity を持たせない）
    let mut fx = Fx::new(5, &[]);
    fx.bodies[0]["tmdb_id"] = json!(1000);
    refused_fx(fx, "inline_identity_present");
    let mut fx = Fx::new(5, &[]);
    fx.bodies[0]["candidate_ref"] = json!("c1");
    refused_fx(fx, "inline_identity_present");
    // pick_other: 検証欠落 / id 不一致 / 提示集合内
    let mut fx = Fx::new(5, &[(0, vec![Step::Pick(900_001)])]);
    fx.bodies[0]["tmdb_verification"] = Value::Null;
    refused_fx(fx, "pick_other_not_verified");
    let mut fx = Fx::new(5, &[(0, vec![Step::Pick(900_001)])]);
    fx.bodies[0]["tmdb_verification"]["response_id"] = json!(5);
    refused_fx(fx, "pick_other_not_verified");
    let mut fx = Fx::new(5, &[(0, vec![Step::Pick(900_001)])]);
    fx.bodies[0]["tmdb_verification"]["http_status"] = json!(404);
    refused_fx(fx, "pick_other_not_verified");
    let mut fx = Fx::new(5, &[(0, vec![Step::Pick(1000)])]); // unit 0 の提示候補 movie:1000
    refused(&fx.render(), "pick_other_in_presented_set");
    let mut fx = Fx::new(5, &[(0, vec![Step::Pick(0)])]);
    refused(&fx.render(), "identity_invalid");
    // none: identity 同居
    let mut fx = Fx::new(5, &[(0, vec![Step::NoneM])]);
    fx.bodies[0]["tmdb_id"] = json!(5);
    refused_fx(fx, "decision_shape_invalid");
    // decision の値が未知
    let mut fx = Fx::new(5, &[]);
    fx.bodies[0]["decision"] = json!("maybe");
    refused_fx(fx, "decision_shape_invalid");
}

#[test]
fn t03_unknown_event_and_non_terminal_are_refused() {
    let mut fx = Fx::new(5, &[]);
    fx.bodies[0]["type"] = json!("mystery");
    refused_fx(fx, "unknown_event_type");
    let fx = Fx::new(5, &[(0, vec![Step::Defer])]);
    refused_fx(fx, "non_terminal_state_in_sealed_ledger");
}

#[test]
fn t04_qc_decision_never_changes_gt() {
    let mut a = Fx::new(5, &[(0, vec![Step::Confirm(0)])]);
    let (_, ga) = ok(&mut a);
    // QC が不一致（none）でも、QC が一致（confirm）でも有効 GT は同じ
    let mut b = Fx::new(5, &[(0, vec![Step::Confirm(0), Step::Qc(Box::new(Step::NoneM))])]);
    let (_, gb) = ok(&mut b);
    let mut c = Fx::new(5, &[(0, vec![Step::Confirm(0), Step::Qc(Box::new(Step::Pick(900_009)))])]);
    let (_, gc) = ok(&mut c);
    assert_eq!(ga.entries(), gb.entries());
    assert_eq!(ga.entries(), gc.entries());
    // QC は終端前には現れない
    let fx = Fx::new(5, &[(0, vec![Step::Qc(Box::new(Step::NoneM)), Step::Confirm(0)])]);
    refused_fx(fx, "non_terminal_state_in_sealed_ledger");
}

// ── B. join / membership ──

#[test]
fn t05_normal_is_ready_and_count_only_report() {
    let mut fx = Fx::new(5, &[(1, vec![Step::NoneM]), (2, vec![Step::Defer, Step::DeferFinal]), (3, vec![Step::Pick(900_001)])]);
    let r = fx.render();
    let o = run(&r);
    assert!(o.report.ready);
    let c = &o.report.counts;
    assert_eq!((c.n_members, c.n_confirm, c.n_pick_other, c.n_none, c.n_defer_final, c.n_corrections_applied), (5, 1, 1, 2, 1, 0)); // unit 4 は候補 0 件 → 既定は none
    assert_eq!(c.n_events, r.ledger.lines().count() as u64);
    let v = o.report.to_value();
    assert_eq!(v["status"], "READY");
    assert_eq!(v["anchor_checks"]["review_seal_sha256"], "match");
    assert_eq!(v["anchor_checks"]["membership_commitment"], "match");
}

#[test]
fn t06_bundle_bijection_and_unit_set() {
    // 重複 unit
    let mut fx = Fx::new(5, &[]);
    let u0 = fx.bundle["units"][0].clone();
    fx.bundle["units"].as_array_mut().unwrap().push(u0);
    refused_fx(fx, "unblind_bundle_not_bijective");
    // 重複 work_key（別 unit id）
    let mut fx = Fx::new(5, &[]);
    fx.bundle["units"][1]["work_key"] = fx.bundle["units"][0]["work_key"].clone();
    refused_fx(fx, "unblind_bundle_not_bijective");
    // unit 内の token 重複
    let mut fx = Fx::new(5, &[]);
    let c0 = fx.bundle["units"][0]["candidates"][0].clone();
    fx.bundle["units"][0]["candidates"].as_array_mut().unwrap().push(c0);
    refused_fx(fx, "candidate_token_duplicate_in_unit");
    // 余分なキー・未知 version
    let mut fx = Fx::new(5, &[]);
    fx.bundle["units"][0]["extra"] = json!(1);
    refused_fx(fx, "unblind_bundle_not_bijective");
    let mut fx = Fx::new(5, &[]);
    fx.bundle["bundle_version"] = json!("other");
    refused_fx(fx, "unblind_bundle_not_bijective");
    // ledger に在る unit が bundle に無い（bundle から 1 unit 外す）
    let mut fx = Fx::new(5, &[]);
    fx.bundle["units"].as_array_mut().unwrap().pop();
    let r = fx.render();
    let o = run(&r);
    assert!(!o.report.ready); // membership か unit の不一致で止まる
}

#[test]
fn t07_member_without_outcome_is_not_unresolved() {
    let mut fx = Fx::new(5, &[]);
    fx.bodies.remove(2); // unit 2 の outcome を欠落
    let r = fx.render();
    refused(&r, "gt_missing_for_member");
    // outcome が bundle に無い unit のもの
    let mut fx = Fx::new(5, &[]);
    fx.bodies[0]["unit"] = json!("ru_aaaaaaaaaaaaaaaaaaaaaaaaaa");
    refused_fx(fx, "review_unit_without_map_entry");
}

#[test]
fn t08_result_is_order_independent_and_membership_only_uses_pf0b() {
    let mut a = Fx::new(8, &[(2, vec![Step::NoneM])]);
    let (_, ga) = ok(&mut a);
    let mut b = Fx::new(8, &[(2, vec![Step::NoneM])]);
    b.bundle["units"].as_array_mut().unwrap().reverse();
    b.bodies.reverse(); // 各 unit は 1 event なので順序を変えても有効
    let (_, gb) = ok(&mut b);
    assert_eq!(ga.entries(), gb.entries());
    assert_eq!(ga.membership_commitment(), gb.membership_commitment());
    let src = include_str!("pr4_pf0i_adapter.rs");
    assert!(src.contains("membership_commitment(&keys"));
    assert!(!src.contains("MEMBERSHIP_DOMAIN") && !src.contains("pr4-pf0a-membership-1"), "adapter は別 hash を持たない");
}

#[test]
fn t09_membership_mismatch_variants() {
    // member の追加（bundle 側に unit を足し、pin・unit_count は整合させる）
    let mut fx = Fx::new(5, &[]);
    let uid = derive_review_unit_id(SECRET, "work:900").unwrap();
    fx.bundle["units"].as_array_mut().unwrap().push(json!({"review_unit_id": uid, "work_key": "work:900", "candidates": []}));
    fx.push_step_for_extra(&uid);
    refused_fx(fx, "membership_commitment_mismatch");
    // eligibility_rule_version_opaque が違う
    let mut fx = Fx::new(5, &[]);
    fx.anchors.eligibility_rule_version_opaque = "other".into();
    refused_fx(fx, "membership_commitment_mismatch");
    // PF0H loader の値が違う
    let mut fx = Fx::new(5, &[]);
    fx.pf0h.authoritative_membership_commitment = h("x");
    refused_fx(fx, "membership_commitment_mismatch");
    // anchor の期待値が違う
    let mut fx = Fx::new(5, &[]);
    fx.anchors.expected_membership_commitment = h("x");
    refused_fx(fx, "membership_commitment_mismatch");
    // PF0H final manifest SHA
    let mut fx = Fx::new(5, &[]);
    fx.pf0h.final_manifest_sha256 = h("other");
    refused_fx(fx, "pf0h_final_manifest_mismatch");
}

impl Fx {
    fn push_step_for_extra(&mut self, uid: &str) {
        self.sub += 1;
        self.bodies.push(json!({"type": "decision", "ts": "2026-10-11T00:00:00Z", "unit": uid, "submission_id": format!("sub-{}", self.sub), "decision": "none",
            "candidate_token": null, "tmdb_id": null, "media_type": null}));
    }
}

#[test]
fn t10_review_unit_id_and_secret_are_rederived() {
    // review_unit_id の付け替え
    let mut fx = Fx::new(5, &[]);
    let (a, b) = (fx.bundle["units"][0]["review_unit_id"].clone(), fx.bundle["units"][1]["review_unit_id"].clone());
    fx.bundle["units"][0]["review_unit_id"] = b;
    fx.bundle["units"][1]["review_unit_id"] = a;
    refused_fx(fx, "review_unit_id_not_derivable");
    // build_secret の差し替え（pin が元のまま）
    let mut fx = Fx::new(5, &[]);
    let r0 = fx.render();
    let mut r = Rendered { ledger: r0.ledger.clone(), bundle: r0.bundle.clone(), anchors: r0.anchors.clone(), pf0h: r0.pf0h.clone() };
    let mut b: Value = serde_json::from_slice(&r.bundle).unwrap();
    b["build_secret_hex"] = json!(h("other-secret"));
    r.bundle = serde_json::to_vec(&b).unwrap();
    refused(&r, "unblind_bundle_sha_mismatch"); // bundle の SHA pin が先に落ちる
    // secret と pin を整合させて差し替えても、unit id が再導出できない
    let mut fx = Fx::new(5, &[]);
    fx.bundle["build_secret_hex"] = json!(h("other-secret"));
    refused_fx(fx, "review_unit_id_not_derivable");
    // candidate の identity を書き換える（token と合わない）
    let mut fx = Fx::new(5, &[]);
    fx.bundle["units"][0]["candidates"][0]["tmdb_id"] = json!(55);
    refused_fx(fx, "candidate_token_not_derivable");
}

// ── C. SHA / seal / 順序 ──

#[test]
fn t11_every_single_tamper_is_detected() {
    let mut fx = Fx::new(4, &[(1, vec![Step::NoneM])]);
    let r = fx.render();
    let lines: Vec<String> = r.ledger.lines().map(str::to_string).collect();
    let mk = |ledger: String| Rendered { ledger, bundle: r.bundle.clone(), anchors: r.anchors.clone(), pf0h: r.pf0h.clone() };
    // 各 event の 1 byte
    for (i, l) in lines.iter().enumerate() {
        let mut b = l.clone().into_bytes();
        let p = b.len() / 2;
        b[p] = if b[p] == b'0' { b'1' } else { b'0' };
        let mut ls = lines.clone();
        ls[i] = String::from_utf8_lossy(&b).to_string();
        let o = run(&mk(ls.join("\n") + "\n"));
        assert!(!o.report.ready, "event {i} の改ざんを検出できない");
    }
    // 入替・削除・末尾追加・末尾削除・改行なし・空
    let mut ls = lines.clone();
    ls.swap(1, 2);
    assert!(!run(&mk(ls.join("\n") + "\n")).report.ready);
    let mut ls = lines.clone();
    ls.remove(2);
    assert!(!run(&mk(ls.join("\n") + "\n")).report.ready);
    let mut ls = lines.clone();
    ls.push(lines[1].clone());
    assert!(!run(&mk(ls.join("\n") + "\n")).report.ready);
    let mut ls = lines.clone();
    ls.pop();
    assert!(!run(&mk(ls.join("\n") + "\n")).report.ready);
    assert!(!run(&mk(lines.join("\n"))).report.ready);
    assert!(!run(&mk(String::new())).report.ready);
    // bundle の 1 byte
    let mut r2 = mk(r.ledger.clone());
    r2.bundle[10] ^= 1;
    refused(&r2, "unblind_bundle_sha_mismatch");
    // anchor の各項目
    for f in 0..4 {
        let mut r3 = mk(r.ledger.clone());
        match f {
            0 => r3.anchors.review_seal_sha256 = h("x"),
            1 => r3.anchors.machine_freeze_anchor_commitment = h("x"),
            2 => r3.anchors.expected_membership_commitment = h("x"),
            _ => r3.anchors.expected_pf0h_final_manifest_sha256 = h("x"),
        }
        assert!(!run(&r3).report.ready, "anchor {f}");
    }
}

#[test]
fn t12_internally_consistent_but_different_seal_is_refused() {
    let mut a = Fx::new(4, &[]);
    let ra = a.render();
    let mut b = Fx::new(4, &[]);
    b.seal_over = vec![]; // 同じ内容でも ts を変えた seal は commitment が変わる
    let mut rb = b.render();
    // seal の ts だけ変えて chain を作り直す
    let mut e: Vec<Value> = rb.ledger.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let last = e.len() - 1;
    e[last]["ts"] = json!("2026-10-12T00:00:00Z");
    let mut body = e[last].clone();
    body.as_object_mut().unwrap().remove("event_commitment");
    let prev = e[last]["prev_commitment"].as_str().unwrap().to_string();
    e[last]["event_commitment"] = json!(hex_sha(format!("{}\n{}", canon(&body).unwrap(), prev).as_bytes()));
    rb.ledger = e.iter().map(|v| canon(v).unwrap() + "\n").collect();
    rb.anchors = ra.anchors.clone(); // anchor は元の seal
    refused(&rb, "review_seal_sha_mismatch");
}

#[test]
fn t13_init_and_seal_placement() {
    // seal の後の event
    let mut fx = Fx::new(4, &[]);
    let r = fx.render();
    let mut e: Vec<Value> = r.ledger.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let seal = e.last().unwrap().clone();
    let mut extra = json!({"type": "session_end", "ts": "2026-10-12T00:00:00Z", "session_id": "s1", "reason": "normal", "schema_version": "pr4-blind-review-event-1", "seq": e.len(), "prev_commitment": seal["event_commitment"]});
    extra["event_commitment"] = json!(hex_sha(format!("{}\n{}", canon(&extra).unwrap(), seal["event_commitment"].as_str().unwrap()).as_bytes()));
    e.push(extra);
    let r2 = Rendered { ledger: e.iter().map(|v| canon(v).unwrap() + "\n").collect(), bundle: r.bundle.clone(), anchors: r.anchors.clone(), pf0h: r.pf0h.clone() };
    refused(&r2, "events_after_seal");
    // seal が無い（末尾の seal を落とす）
    let mut e2: Vec<Value> = r.ledger.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    e2.pop();
    let r3 = Rendered { ledger: e2.iter().map(|v| canon(v).unwrap() + "\n").collect(), bundle: r.bundle.clone(), anchors: r.anchors.clone(), pf0h: r.pf0h.clone() };
    refused(&r3, "seal_missing_or_duplicated");
    // init が先頭でない（init を落とす → 先頭が decision で seq/prev も壊れるので chain で落ちる）
    let mut e3: Vec<Value> = r.ledger.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    e3.remove(0);
    let r4 = Rendered { ledger: e3.iter().map(|v| canon(v).unwrap() + "\n").collect(), bundle: r.bundle.clone(), anchors: r.anchors.clone(), pf0h: r.pf0h.clone() };
    assert!(!run(&r4).report.ready);
}

#[test]
fn t14_pins_must_equal_between_init_and_seal() {
    for k in PIN_KEYS {
        let mut fx = Fx::new(4, &[]);
        fx.seal_over = vec![(k, h("changed-in-review"))];
        refused_fx(fx, "seal_pins_differ_from_init");
    }
    // 外部照合のある pin は、init と seal を揃えて変えても別のコードで落ちる
    let mut fx = Fx::new(4, &[]);
    fx.init_over = vec![("unblind_map_sha256", h("x"))];
    fx.seal_over = vec![("unblind_map_sha256", h("x"))];
    refused_fx(fx, "unblind_bundle_sha_mismatch");
    let mut fx = Fx::new(4, &[]);
    fx.init_over = vec![("build_secret_commitment", h("x"))];
    fx.seal_over = vec![("build_secret_commitment", h("x"))];
    refused_fx(fx, "build_secret_commitment_mismatch");
    let mut fx = Fx::new(4, &[]);
    fx.init_over = vec![("machine_freeze_anchor_commitment", h("x"))];
    fx.seal_over = vec![("machine_freeze_anchor_commitment", h("x"))];
    refused_fx(fx, "machine_freeze_commitment_mismatch");
    // pin が足りない
    let mut fx = Fx::new(4, &[]);
    fx.seal_over = vec![("ui_package_sha256", "short".into())];
    refused_fx(fx, "seal_pins_missing");
}

#[test]
fn t15_hash_is_checked_before_parse() {
    let mut fx = Fx::new(4, &[]);
    let mut r = fx.render();
    r.bundle = b"{ this is not json".to_vec();
    let o = run(&r);
    assert!(o.report.refusal_codes.contains(&"unblind_bundle_sha_mismatch"), "{:?}", o.report.refusal_codes);
    assert_eq!(o.report.to_value()["anchor_checks"]["unblind_bundle_sha256"], "mismatch");
}

// ── D. 複製・衝突・訂正 ──

#[test]
fn t16_duplicates_and_conflicts() {
    let fx = Fx::new(4, &[(0, vec![Step::Confirm(0), Step::Confirm(0)])]);
    refused_fx(fx, "duplicate_record_identical");
    let fx = Fx::new(4, &[(0, vec![Step::Confirm(0), Step::NoneM])]);
    refused_fx(fx, "gt_conflict");
    let fx = Fx::new(4, &[(0, vec![Step::Defer, Step::DeferFinal, Step::Confirm(0)])]);
    refused_fx(fx, "gt_conflict");
    let fx = Fx::new(4, &[(0, vec![Step::Confirm(0), Step::Defer])]);
    refused_fx(fx, "gt_conflict");
    let mut fx = Fx::new(4, &[]);
    let sid = fx.bodies[0]["submission_id"].clone();
    fx.bodies[1]["submission_id"] = sid;
    refused_fx(fx, "gt_conflict");
}

#[test]
fn t17_two_pass_and_corrections() {
    let mut fx = Fx::new(
        4,
        &[
            (0, vec![Step::Defer, Step::Confirm(1)]),
            (1, vec![Step::Defer, Step::DeferFinal, Step::Correct(Box::new(Step::Pick(900_001)))]),
            (2, vec![Step::Confirm(0), Step::Correct(Box::new(Step::NoneM)), Step::Correct(Box::new(Step::Confirm(1)))]),
        ],
    );
    let r = fx.render();
    let o = run(&r);
    assert!(o.report.ready, "{:?}", o.report.refusal_codes);
    let g = o.gt.unwrap();
    assert_eq!(gt_of(&g, 0), Gt::Positive("tv".into(), 1001));
    assert_eq!(gt_of(&g, 1), Gt::Positive("movie".into(), 900_001));
    assert_eq!(gt_of(&g, 2), Gt::Positive("tv".into(), 1005)); // 最後の correction が有効
    assert_eq!(o.report.counts.n_corrections_applied, 3); // (19)
}

#[test]
fn t18_bad_corrections() {
    let mut fx = Fx::new(4, &[(0, vec![Step::Confirm(0), Step::Correct(Box::new(Step::NoneM))])]);
    fx.bodies[1]["correction_reason"] = json!("because");
    refused_fx(fx, "correction_reason_invalid");
    let fx = Fx::new(4, &[(0, vec![Step::Correct(Box::new(Step::NoneM)), Step::Confirm(0)])]);
    refused_fx(fx, "correction_chain_invalid");
    let fx = Fx::new(4, &[(0, vec![Step::Defer, Step::Correct(Box::new(Step::NoneM))])]);
    refused_fx(fx, "correction_chain_invalid");
}

// ── E. 評価結合（PF0B + 凍結 policy）──

/// 600 unit。先頭 `n_auto` 件が AUTO（identity は gt と同じ = 正解）。gt は全て confirm(0)
fn eval_setup(script: &[(usize, Vec<Step>)]) -> (Fx, usize) {
    (Fx::new(600, script), 300)
}
fn machine_for(n_auto: usize, total: usize) -> BTreeMap<String, Machine> {
    (0..total)
        .map(|i| {
            let c = cands(i);
            let m = if i < n_auto {
                match c.first() {
                    Some((mt, id)) => Machine::Auto(Some((mt.to_string(), *id))),
                    None => Machine::Auto(None),
                }
            } else {
                Machine::Review
            };
            (work(i), m)
        })
        .collect()
}
fn evaluate_with(r: &Rendered, machine: &BTreeMap<String, Machine>) -> (Value, Decision) {
    let o = run(r);
    assert!(o.report.ready, "{:?}", o.report.refusal_codes);
    let g = o.gt.unwrap();
    let input = assemble_final_input(machine, &g, VER).unwrap();
    evaluate(&input, &policy())
}

/// unit 4, 9, ... は候補 0 件なので、GT を none にして機械も NoMatch 相当にできない。AUTO 対象は candidate 有りの unit だけにする
fn healthy_script() -> Vec<(usize, Vec<Step>)> {
    (0..600).filter(|i| i % 5 == 4).map(|i| (i, vec![Step::NoneM])).collect()
}
fn machine_auto_only_with_candidates(n_auto: usize) -> BTreeMap<String, Machine> {
    let mut left = n_auto;
    (0..600)
        .map(|i| {
            let c = cands(i);
            let m = if left > 0 && !c.is_empty() {
                left -= 1;
                Machine::Auto(Some((c[0].0.to_string(), c[0].1)))
            } else {
                Machine::Review
            };
            (work(i), m)
        })
        .collect()
}

#[test]
fn t20_promote_with_frozen_policy_when_all_auto_correct() {
    let (mut fx, _) = eval_setup(&healthy_script());
    let r = fx.render();
    let (rep, d) = evaluate_with(&r, &machine_auto_only_with_candidates(300));
    assert_eq!(d, Decision::Promote, "{rep}");
    assert_eq!(rep["policy_provenance"]["binding"], true);
}

#[test]
fn t21_unresolved_gt_blocks_only_when_auto() {
    // AUTO の unit の GT が defer_final
    let mut script = healthy_script();
    script.push((0, vec![Step::Defer, Step::DeferFinal]));
    let (mut fx, _) = eval_setup(&script);
    let r = fx.render();
    let (rep, d) = evaluate_with(&r, &machine_auto_only_with_candidates(300));
    assert!(matches!(&d, Decision::Inconclusive(v) if v.contains(&"auto_with_unresolved_gt".to_string())), "{rep}");
    assert_eq!(rep["metrics"]["precision_binding"], false);
    // AUTO でない unit（末尾）の GT が defer_final → 影響しない
    let mut script = healthy_script();
    script.push((599, vec![Step::Defer, Step::DeferFinal]));
    let (mut fx, _) = eval_setup(&script);
    let r = fx.render();
    let (_, d) = evaluate_with(&r, &machine_auto_only_with_candidates(300));
    assert_eq!(d, Decision::Promote);
}

#[test]
fn t22_auto_against_none_or_pick_other_is_wrong() {
    for step in [Step::NoneM, Step::Pick(900_001)] {
        let mut script = healthy_script();
        script.push((0, vec![step]));
        let (mut fx, _) = eval_setup(&script);
        let r = fx.render();
        let (rep, d) = evaluate_with(&r, &machine_auto_only_with_candidates(300));
        assert_ne!(d, Decision::Promote);
        assert_eq!(rep["metrics"]["n_wrong_auto"], 1, "{rep}");
    }
}

#[test]
fn t23_mutation_defer_final_as_nomatch_would_be_caught() {
    // defer_final を NoMatch に落とすと、(21) の INCONCLUSIVE が NO_PROMOTION/誤り 1 件に変わる = 差が出る
    let mut script = healthy_script();
    script.push((0, vec![Step::Defer, Step::DeferFinal]));
    let (mut fx, _) = eval_setup(&script);
    let r = fx.render();
    let o = run(&r);
    let g = o.gt.unwrap();
    assert_eq!(gt_of(&g, 0), Gt::Unresolved);
    let mut mutated = g.clone();
    for (_, gt) in mutated.entries.iter_mut() {
        if *gt == Gt::Unresolved {
            *gt = Gt::NoMatch;
        }
    }
    let m = machine_auto_only_with_candidates(300);
    let (_, d_ok) = evaluate(&assemble_final_input(&m, &g, VER).unwrap(), &policy());
    let (_, d_bad) = evaluate(&assemble_final_input(&m, &mutated, VER).unwrap(), &policy());
    assert!(matches!(d_ok, Decision::Inconclusive(_)));
    assert!(!matches!(d_bad, Decision::Inconclusive(_)), "mutation で結果が変わらない = 試験が無力");
}

#[test]
fn t24_non_frozen_policy_is_refused() {
    let mut fx = Fx::new(4, &[]);
    let r = fx.render();
    for exclude in [UnevaluableAuto::Exclude, UnevaluableAuto::CountAsWrong, UnevaluableAuto::BlockEvaluation] {
        let p = PolicyConfig::test_only(299, 0.5, 0.95, 0.99, None, exclude);
        let o = run_with(&r, &p);
        assert!(!o.report.ready);
        assert_eq!(o.report.refusal_codes, vec!["policy_not_pinned_frozen"]);
    }
    // 凍結 policy の SHA は定数と一致
    assert!(matches!(&policy().provenance, PolicyProvenance::Frozen { policy_sha256 } if policy_sha256 == FROZEN_POLICY_SHA256));
}

#[test]
fn t25_refusal_produces_own_envelope_not_pf0b_report() {
    let mut fx = Fx::new(4, &[]);
    let mut r = fx.render();
    r.anchors.review_seal_sha256 = h("wrong");
    let o = run(&r);
    let env = o.report.failure_envelope();
    assert_eq!(env["stage"], "gt_adapter");
    assert_eq!(env["status"], "TECHNICAL_FAILURE");
    assert_eq!(env["reason_code"], "review_seal_sha_mismatch");
    for k in ["report_version", "decision", "metrics", "policy_provenance", "decision_reasons"] {
        assert!(env.get(k).is_none(), "PF0B report に似せない: {k}");
    }
    // PF0B の TECHNICAL_FAILURE report とキー集合が重ならない（形が違う）
    let input = FinalInput { units: vec![], expected_membership_commitment: String::new(), eligibility_rule_version: VER.into(), run_failure: Some("x".into()) };
    let (pf0b_rep, _) = evaluate(&input, &policy());
    let pf0b_keys: BTreeSet<&String> = pf0b_rep.as_object().unwrap().keys().collect();
    assert!(env.as_object().unwrap().keys().all(|k| !pf0b_keys.contains(k)));
    // assembler の失敗
    let (_, g) = ok(&mut Fx::new(4, &[]));
    let mut m = BTreeMap::new();
    m.insert("work:999".to_string(), Machine::Review);
    assert_eq!(assemble_final_input(&m, &g, VER).unwrap_err(), "machine_output_for_non_member");
    assert_eq!(assembler_failure_envelope("machine_output_for_non_member")["stage"], "gt_adapter");
}

#[test]
fn t26_missing_machine_output_is_pf0b_technical_failure() {
    let (_, g) = ok(&mut Fx::new(4, &[]));
    let input = assemble_final_input(&BTreeMap::new(), &g, VER).unwrap();
    assert!(input.units.iter().all(|u| u.machine == Machine::Missing));
    let (_, d) = evaluate(&input, &policy());
    assert!(matches!(d, Decision::TechnicalFailure(_)));
}

// ── F. leakage / 静的性質 ──

#[test]
fn t27_ledger_structure_leak_scan() {
    for key in ["jev_score", "matcher_verdict", "rank_hint", "workid"] {
        let mut fx = Fx::new(4, &[]);
        fx.bodies[0][key] = json!("x");
        refused_fx(fx, "ledger_contains_machine_state");
    }
    let mut fx = Fx::new(4, &[]);
    fx.bodies[0]["unit_ref"] = json!("work:12");
    refused_fx(fx, "ledger_contains_work_key");
    let mut fx = Fx::new(4, &[]);
    fx.bodies[0]["flag"] = json!("AUTO");
    refused_fx(fx, "ledger_contains_machine_state");
    // 正規フィールド decision / tmdb_id / purpose=review・自由記述内の "work:" では誤検出しない（D10 の回帰）
    let mut fx = Fx::new(4, &[(0, vec![Step::Pick(900_001)])]);
    let u0 = fx.uid(0);
    fx.bodies.insert(0, json!({"type": "lease", "ts": "2026-10-11T00:00:00Z", "purpose": "review", "unit": u0, "note": "work:5 AUTO"}));
    ok(&mut fx);
}

#[test]
fn t28_outputs_do_not_leak_identity_or_free_text() {
    let mut fx = Fx::new(5, &[(1, vec![Step::Pick(900_001)]), (2, vec![Step::Defer, Step::DeferFinal])]);
    let r = fx.render();
    let o = run(&r);
    let g = o.gt.clone().unwrap();
    let m = machine_for(2, 5);
    let (final_rep, _) = evaluate(&assemble_final_input(&m, &g, VER).unwrap(), &policy());
    let mut bad = Fx::new(5, &[]);
    let mut rb = bad.render();
    rb.anchors.review_seal_sha256 = h("w");
    let failed = run(&rb);
    let mut texts = vec![o.report.to_value().to_string(), final_rep.to_string(), failed.report.to_value().to_string(), failed.report.failure_envelope().to_string()];
    texts.push(format!("{:?}", failed.report));
    let mut needles: Vec<String> = vec!["work:".into(), "tmdb".into(), "\"movie\"".into(), "\"tv\"".into(), "title".into(), "CANARY".into(), "reviewer".into(), "mis_click".into(), "2026-".into(), SECRET.into()];
    for i in 0..5 {
        needles.push(fx.uid(i));
        for (mt, id) in cands(i) {
            needles.push(derive_candidate_token(SECRET, &fx.uid(i), mt, id).unwrap());
        }
    }
    for t in &texts {
        for n in &needles {
            assert!(!t.contains(n.as_str()), "出力に {n} が出ている: {t}");
        }
    }
}

#[test]
fn t29_adapter_source_has_no_io_db_net_env_or_label_writes() {
    let src = include_str!("pr4_pf0i_adapter.rs");
    let body = src.split("#[cfg(test)]\n#[path").next().unwrap();
    for banned in [
        "rusqlite", "std::fs", "std::net", "std::env", "Command", "metadata_match", "INSERT ", "SELECT ", "UPDATE ", "DELETE ", "write_label", "reqwest", "File::", "println!", "eprintln!", "tracing::", "log::",
    ] {
        assert!(!body.contains(banned), "adapter に {banned} がある");
    }
    // pr4_pf0b.rs は adapter から変更していない（ヘルパを足さない）: adapter が使う PF0B の API は既存の pub のみ
    assert!(body.contains("use crate::services::pr4_pf0b::{membership_commitment, FinalInput, Gt, Machine, PolicyConfig, PolicyProvenance, Unit, UnevaluableAuto};"));
}

#[test]
fn t30_deterministic_and_order_independent_output() {
    let mut a = Fx::new(7, &[(3, vec![Step::NoneM])]);
    let ra = a.render();
    let (o1, o2) = (run(&ra), run(&ra));
    assert_eq!(o1.gt.unwrap().entries(), o2.gt.unwrap().entries());
    assert_eq!(o1.report.to_value(), o2.report.to_value());
    let g = run(&ra).gt.unwrap();
    let keys: Vec<&String> = g.entries().iter().map(|(k, _)| k).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}

#[test]
fn t31_adapter_is_not_in_product_build() {
    let m = include_str!("mod.rs");
    assert!(m.contains("#[cfg(test)]\npub mod pr4_pf0i_adapter;") || m.contains("#[cfg(test)]\r\npub mod pr4_pf0i_adapter;"));
}

#[test]
fn t32_trusted_gt_set_has_no_public_constructor() {
    let src = include_str!("pr4_pf0i_adapter.rs");
    let body = src.split("#[cfg(test)]\n#[path").next().unwrap();
    let def = body.split("pub struct TrustedGtSet {").nth(1).unwrap().split('}').next().unwrap();
    assert!(!def.contains("pub "), "フィールドは private");
    assert_eq!(body.matches("Ok(TrustedGtSet {").count(), 1, "構築は adapt の 1 箇所だけ");
}

#[test]
fn t33_synthetic_only_no_real_data_paths_in_test_files() {
    for (name, src) in [("adapter", include_str!("pr4_pf0i_adapter.rs")), ("tests", include_str!("pr4_pf0i_tests.rs"))] {
        for banned in ["CineMantis-audit", "pf0e\\", "holdout.sqlite", "std::fs::read", "std::fs::File"] {
            // tests 自身の banned リスト行は除外
            let n = src.lines().filter(|l| l.contains(banned) && !l.contains("for banned in")).count();
            assert_eq!(n, 0, "{name} が {banned} に触れている");
        }
    }
}

#[test]
fn t34_schema_refusal_codes_match_implementation() {
    let schema: Value = serde_json::from_str(include_str!("../../../../../docs/pr4_pf0i0_gt_adapter_schema.json")).unwrap();
    let listed: BTreeSet<String> =
        schema["definitions"]["adapter_report"]["properties"]["refusal_codes"]["items"]["enum"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    let src = include_str!("pr4_pf0i_adapter.rs");
    let body = src.split("#[cfg(test)]\n#[path").next().unwrap();
    let mut used: BTreeSet<String> = BTreeSet::new();
    for marker in ["refuse(\"", "vec![\""] {
        for part in body.split(marker).skip(1) {
            let code: String = part.chars().take_while(|c| *c != '"').collect();
            if code.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') && !code.is_empty() {
                used.insert(code);
            }
        }
    }
    // leak scan が積む 2 コードも含める
    used.insert("ledger_contains_machine_state".into());
    used.insert("ledger_contains_work_key".into());
    used.remove("unknown");
    assert_eq!(used, listed, "schema の refusal_codes と実装の差");
    assert_eq!(schema["adapter_version"], ADAPTER_VERSION);
    assert_eq!(schema["alignment"]["pf0g_authoritative_docs_commit"], "1a6266d9869d7dc3bf8556850312c85676ebe7d3");
    assert_eq!(hex_sha(include_str!("pr4_blind_gt.rs").as_bytes()), schema["alignment"]["pr4_blind_gt_rs_sha256"]);
}
