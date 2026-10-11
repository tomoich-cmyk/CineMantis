//! PR4-PF0G2 ↔ PR4-PF0I の synthetic E2E（**統合 worktree 専用**）。
//!
//! PF0G2（packager・loopback server・実 `Store`）と PF0H1（final manifest SHA）のコードが必要なので、PF0I 単体の branch ではコンパイルしない
//! （`mod.rs` に登録していない）。統合手順は `docs/PR4_PF0I_FINAL_FREEZE_CANDIDATE.md`。
//! 実 ledger・H2 payload・production DB・live TMDB は使わない。`pr4_pf0g2_tests.rs` の末尾に
//! `#[path = "pr4_pf0i_e2e_tests.rs"] mod pf0i_e2e;` を足して、その child module として動かす（private な fixture を使うため）。

use super::*;
use crate::services::pr4_pf0b::{membership_commitment, Gt, PolicyConfig};
use crate::services::pr4_pf0h_close;
use crate::services::pr4_pf0i_adapter as pf0i;

const VER: &str = "pr4-h2-enrollment-rules-2/enrollment_rule_sha256=synthetic";
const POLICY_JSON: &str = include_str!("../../../../../docs/pr4_final_policy.json");

struct Run {
    b: Built,
    store: Store,
    file: Vec<u8>,
    ledger: Vec<u8>,
    bundle: Vec<u8>,
    loaded: pf0i::Pf0hLoaded,
    anchors: pf0i::Anchors,
}

fn final_membership_file(mc: &str) -> Vec<u8> {
    serde_json::to_vec_pretty(&json!({"protocol": "pr4-pf0h0-final-membership-1", "unit_count": 946, "membership_commitment_sha256": mc, "authoritative": true,
        "eligibility_rule_version": VER, "auxiliary_audit_values": {"unit_set_sha256": h("aux"), "authoritative": false}}))
    .unwrap()
}

fn prepare(env: &Env, n: usize) -> (Built, Vec<u8>, pf0i::Pf0hLoaded) {
    let keys: Vec<String> = (0..n).map(|i| format!("work:{}", i + 1)).collect();
    let mc = membership_commitment(&keys, VER);
    let file = final_membership_file(&mc);
    let loaded = pf0i::load_pf0h_final_membership(&file).unwrap();
    let mut inp = input(n);
    inp.authoritative_membership_commitment = mc;
    inp.pf0h_final_manifest_sha256 = loaded.final_manifest_sha256.clone();
    let (rev, sealed) = (env.p("reviewer_package"), env.p("sealed_unblind"));
    let out = build(&inp, &rev, &sealed).unwrap();
    let mut lits: Vec<String> = CANARIES.iter().map(|s| s.to_string()).collect();
    lits.push(SECRET.into());
    let (_, a8) = a8_leak_scan(&rev, &lits).unwrap();
    (Built { rev, sealed, ledger: env.p("ledger"), cp: env.p("cp"), cache: env.p("tmdb_cache"), out, a8, inp }, file, loaded)
}

fn sealed_run(env: &Env, n: usize) -> Run {
    let (b, file, loaded) = prepare(env, n);
    let (api, _) = make_api(&b, default_tmdb());
    let mut c = Client { api, log: vec![] };
    let sid = c.start();
    c.drive_all(&sid);
    assert_eq!(c.post("/api/session/end", json!({"session_id": sid})).0, 200);
    let mut store = c.api.into_store();
    store.seal(&pins_of(&b), &t(100_000)).unwrap();
    let hand = pf0i_handoff(&store, &b.ledger, &b.sealed).unwrap();
    let anchors = pf0i::Anchors {
        expected_membership_commitment: hand.authoritative_membership_commitment.clone(),
        expected_pf0h_final_manifest_sha256: hand.pf0h_final_manifest_sha256.clone(),
        review_seal_sha256: hand.review_seal_sha256.clone(),
        machine_freeze_anchor_commitment: h("anchor"),
        eligibility_rule_version_opaque: VER.into(),
    };
    let ledger = std::fs::read(b.ledger.join("events.jsonl")).unwrap();
    let bundle = std::fs::read(b.sealed.join("unblind_map.json")).unwrap();
    Run { b, store, file, ledger, bundle, loaded, anchors }
}

fn adapt_run(ledger: &[u8], bundle: &[u8], anchors: &pf0i::Anchors, loaded: &pf0i::Pf0hLoaded) -> pf0i::AdapterOutcome {
    let policy = PolicyConfig::from_frozen_json(POLICY_JSON).unwrap();
    pf0i::adapt(&pf0i::AdapterInput { ledger_bytes: ledger, unblind_bundle_bytes: bundle, anchors, pf0h: loaded, policy: &policy })
}

#[test]
fn e1_real_store_ledger_and_packager_bundle_are_accepted_and_counts_match_pf0g1_audit() {
    let env = Env::new("pf0i_e1");
    let r = sealed_run(&env, 30);
    let o = adapt_run(&r.ledger, &r.bundle, &r.anchors, &r.loaded);
    assert!(o.report.ready, "{:?}", o.report.refusal_codes);
    let audit = r.store.completion_audit();
    let c = &o.report.counts;
    assert_eq!(c.n_members as usize, audit.unit_count);
    assert_eq!((c.n_confirm as usize, c.n_pick_other as usize, c.n_none as usize, c.n_defer_final as usize), (audit.confirm, audit.pick_other, audit.none, audit.defer_final));
    assert_eq!(c.n_corrections_applied, audit.corrections as u64);
    // confirm の identity は packager が提示した候補のどれか
    let g = o.gt.unwrap();
    let shown: HashMap<String, BTreeSet<(String, i64)>> = r
        .b
        .inp
        .units
        .iter()
        .map(|u| (u.evaluation_unit_key.clone(), u.projection["candidates"].as_array().unwrap().iter().map(|c| (c["media_type"].as_str().unwrap().to_string(), c["tmdb_id"].as_i64().unwrap())).collect()))
        .collect();
    let in_shown = g.entries().iter().filter(|(k, gt)| matches!(gt, Gt::Positive(mt, id) if shown[k].contains(&(mt.clone(), *id)))).count();
    assert_eq!(in_shown, audit.confirm);
}

#[test]
fn e2_definitions_agree_across_packages() {
    let env = Env::new("pf0i_e2");
    let r = sealed_run(&env, 12);
    // build_secret_commitment（Human 決定の定義）が PF0G2 の実装と一致
    assert_eq!(pf0i::build_secret_commitment(SECRET), build_secret_commitment(SECRET));
    assert_eq!(r.b.out.build_secret_commitment, pf0i::build_secret_commitment(SECRET));
    // PF0H1 の実装と PF0I loader が同じ final manifest SHA を出す（規則 pr4-pf0h1-final-manifest-sha-1）
    assert_eq!(pr4_pf0h_close::final_manifest_sha256(&r.file).unwrap(), r.loaded.final_manifest_sha256);
    let v: Value = serde_json::from_slice(&r.file).unwrap();
    assert_eq!(pr4_pf0h_close::final_manifest_sha256(serde_json::to_string(&v).unwrap().as_bytes()).unwrap(), r.loaded.final_manifest_sha256);
    // PF0G2 handoff の値 = PF0I の anchors
    assert_eq!(r.anchors.expected_membership_commitment, r.loaded.authoritative_membership_commitment);
    assert_eq!(r.anchors.expected_pf0h_final_manifest_sha256, r.loaded.final_manifest_sha256);
    // bundle の SHA は ledger の pin（PF0G2 が出した値）
    let pins = pins_from_ledger(&r.b.ledger).unwrap();
    assert_eq!(hex_sha(&r.bundle), pins.unblind_map_sha256);
    // seal SHA = seal event の event_commitment
    let text = String::from_utf8(r.ledger.clone()).unwrap();
    let last: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert_eq!(last["event_commitment"], json!(r.anchors.review_seal_sha256));
}

#[test]
fn e3_every_fail_closed_path_on_real_artifacts() {
    let env = Env::new("pf0i_e3");
    let r = sealed_run(&env, 20);
    let refused = |code: &str, ledger: &[u8], bundle: &[u8], anchors: &pf0i::Anchors, loaded: &pf0i::Pf0hLoaded| {
        let o = adapt_run(ledger, bundle, anchors, loaded);
        assert!(!o.report.ready && o.gt.is_none(), "{code}");
        assert!(o.report.refusal_codes.contains(&code), "want {code} got {:?}", o.report.refusal_codes);
        assert_eq!(o.report.failure_envelope()["stage"], "gt_adapter");
    };
    // bundle の 1 byte（末尾に空白）
    let mut b2 = r.bundle.clone();
    b2.push(b' ');
    refused("unblind_bundle_sha_mismatch", &r.ledger, &b2, &r.anchors, &r.loaded);
    // ledger の 1 byte
    let mut l2 = r.ledger.clone();
    l2[40] = if l2[40] == b'0' { b'1' } else { b'0' };
    assert!(!adapt_run(&l2, &r.bundle, &r.anchors, &r.loaded).report.ready);
    // 末尾の seal を落とす
    let text = String::from_utf8(r.ledger.clone()).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    let seal_line = lines.pop().unwrap().to_string();
    refused("seal_missing_or_duplicated", (lines.join("\n") + "\n").as_bytes(), &r.bundle, &r.anchors, &r.loaded);
    // anchors
    let mut a = r.anchors.clone();
    a.review_seal_sha256 = h("other");
    refused("review_seal_sha_mismatch", &r.ledger, &r.bundle, &a, &r.loaded);
    let mut a = r.anchors.clone();
    a.expected_membership_commitment = h("other");
    refused("membership_commitment_mismatch", &r.ledger, &r.bundle, &a, &r.loaded);
    let mut a = r.anchors.clone();
    a.eligibility_rule_version_opaque = "other-version".into();
    refused("membership_commitment_mismatch", &r.ledger, &r.bundle, &a, &r.loaded);
    let mut a = r.anchors.clone();
    a.machine_freeze_anchor_commitment = h("other");
    refused("machine_freeze_commitment_mismatch", &r.ledger, &r.bundle, &a, &r.loaded);
    let mut a = r.anchors.clone();
    a.expected_pf0h_final_manifest_sha256 = hex_sha(&r.file); // raw SHA は binding anchor ではない
    refused("pf0h_final_manifest_mismatch", &r.ledger, &r.bundle, &a, &r.loaded);
    // loader: 別ファイル（H2_CLOSE 風）は拒否
    assert!(pf0i::load_pf0h_final_membership(br#"{"protocol":"x"}"#).is_err());
    // 別 secret の bundle（pin と合わない）
    let mut bv: Value = serde_json::from_slice(&r.bundle).unwrap();
    bv["build_secret_hex"] = json!(h("other-secret"));
    refused("unblind_bundle_sha_mismatch", &r.ledger, canon(&bv).unwrap().as_bytes(), &r.anchors, &r.loaded);
    assert!(!seal_line.is_empty());
    // errata は adapter の外（FINAL runner の責務）。PF0G1 の evaluation_validity が Invalidated になることだけ確認
    let first_unit: Value = serde_json::from_str(text.lines().find(|l| l.contains("\"type\":\"decision\"")).unwrap()).unwrap();
    r.store.record_post_seal_errata(first_unit["unit"].as_str().unwrap(), "synthetic errata", true, &t(100_100)).unwrap();
    assert!(matches!(r.store.evaluation_validity().unwrap(), EvaluationValidity::Invalidated { .. }));
}

#[test]
fn e4_pre_seal_ledger_is_refused() {
    let env = Env::new("pf0i_e4");
    let (b, _file, loaded) = prepare(&env, 8);
    let store = init_store(&b, 10, Box::new(NoMainApp));
    assert!(pf0i_handoff(&store, &b.ledger, &b.sealed).is_err()); // PF0G2 側が先に拒否
    let ledger = std::fs::read(b.ledger.join("events.jsonl")).unwrap();
    let bundle = std::fs::read(b.sealed.join("unblind_map.json")).unwrap();
    let anchors = pf0i::Anchors {
        expected_membership_commitment: loaded.authoritative_membership_commitment.clone(),
        expected_pf0h_final_manifest_sha256: loaded.final_manifest_sha256.clone(),
        review_seal_sha256: h("no-seal-yet"),
        machine_freeze_anchor_commitment: h("anchor"),
        eligibility_rule_version_opaque: VER.into(),
    };
    let o = adapt_run(&ledger, &bundle, &anchors, &loaded);
    assert!(!o.report.ready);
    assert!(o.report.refusal_codes.contains(&"seal_missing_or_duplicated"), "{:?}", o.report.refusal_codes);
}
