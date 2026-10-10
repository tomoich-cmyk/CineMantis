//! PR4-PF0G1 の synthetic テスト。H2 / H1 / 実 machine output / production DB は使わない。

use super::*;

const BASE: i64 = 1_790_000_000;
const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const SEED: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
const CANARIES: [&str; 5] = ["CANARY_SCORE_0.87314", "CANARY_RANK_9137", "CANARY_SOURCE_tags-shadow", "CANARY_DECISION_AUTO_X", "CANARY_JEV_55"];

fn t(secs: i64) -> String {
    fmt_epoch(BASE + secs)
}

struct Env {
    root: PathBuf,
    dir: PathBuf,
    cp: PathBuf,
}
impl Env {
    fn new(name: &str) -> Env {
        let root = std::env::temp_dir().join(format!("pf0g1_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Env { dir: root.join("ledger"), cp: root.join("cp"), root }
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// 機械出力を **持つ** synthetic packager（テスト専用）。unit には whitelist だけを射影する
struct MachineCandidate {
    tmdb_id: i64,
    media_type: &'static str,
    #[allow(dead_code)]
    rank: i64,
    #[allow(dead_code)]
    score: &'static str,
    #[allow(dead_code)]
    source: &'static str,
}

fn artifact(n: usize) -> Artifact {
    let keys: Vec<String> = (0..n).map(|i| format!("work:{}", i + 1)).collect();
    let pos: HashMap<String, usize> = assign_positions(SECRET, &keys).unwrap().into_iter().collect();
    let mut units = Vec::new();
    for (i, k) in keys.iter().enumerate() {
        let salt = derive_unit_salt(SECRET, k).unwrap();
        let unit_id = derive_review_unit_id(SECRET, k).unwrap();
        let machine: Vec<MachineCandidate> = if i % 4 == 0 {
            vec![]
        } else {
            vec![
                MachineCandidate { tmdb_id: 1000 + i as i64 * 2, media_type: "movie", rank: 1, score: CANARIES[0], source: CANARIES[2] },
                MachineCandidate { tmdb_id: 1001 + i as i64 * 2, media_type: "tv", rank: 2, score: CANARIES[1], source: CANARIES[3] },
            ]
        };
        let candidates = machine
            .iter()
            .enumerate()
            .map(|(_j, c)| Candidate {
                candidate_token: derive_candidate_token(SECRET, &unit_id, c.media_type, c.tmdb_id).unwrap(),
                title: format!("Title {}", c.tmdb_id),
                original_title: None,
                year: Some("1999".into()),
                overview: Some("synthetic".into()),
                poster_path: None,
                original_language: Some("en".into()),
            })
            .collect();
        units.push(
            ReviewUnit {
                schema_version: UNIT_SCHEMA.into(),
                review_unit_id: derive_review_unit_id(SECRET, k).unwrap(),
                position: pos[k] as i64,
                unit_salt: salt.clone(),
                overlap_digests: {
                    let mut d: Vec<String> = machine.iter().map(|c| overlap_digest(&salt, c.media_type, c.tmdb_id).unwrap()).collect();
                    d.sort();
                    d
                },
                evidence: Evidence {
                    files: vec![EvidenceFile {
                        part_no: 1,
                        original_file_name: format!("f{i}.mkv"),
                        original_rel_path: format!("a/f{i}.mkv"),
                        container_tags_json: "{}".into(),
                        source_media_kind: "video".into(),
                    }],
                },
                candidates,
                unit_sha256: String::new(),
            }
            .sealed()
            .unwrap(),
        );
    }
    let map: BTreeMap<String, ReviewUnit> = units.iter().map(|u| (u.review_unit_id.clone(), u.clone())).collect();
    let manifest = Manifest {
        schema_version: MANIFEST_SCHEMA.into(),
        build_id: "synthetic".into(),
        unit_count: n as i64,
        order_commitment_sha256: order_commitment(&map),
        artifact_sha256: artifact_sha256(&map),
        machine_freeze_commitment_sha256: hex_sha(b"machine-freeze"),
        pf0e_ledger_head_sha256: hex_sha(b"pf0e"),
        pf0e_record_count: n as i64,
        packager_sha256: hex_sha(b"packager"),
        built_at_utc: t(1),
    };
    Artifact::from_parts(manifest, units).unwrap()
}

fn pins() -> Pins {
    let h = |l: &str| hex_sha(l.as_bytes());
    Pins {
        protocol_sha256: h("protocol"),
        ui_package_sha256: h("ui"),
        shuffle_seed_commitment: h("shuffle"),
        attestation_policy_sha256: h("attestation"),
        unblind_map_sha256: h("unblind-map"),
        build_secret_commitment: h("build-secret-commit"),
        a8_leak_scan_report_sha256: h("a8"),
        machine_freeze_anchor_commitment: h("anchor"),
    }
}
fn confirm(s: &Store, unit: &str, idx: usize) -> Decision {
    Decision::Confirm { candidate_token: s.unit(unit).unwrap().candidates[idx].candidate_token.clone() }
}
/// テスト専用: token から identity を逆引きする（reviewer 側には存在しない対応表）
fn ident(art: &Artifact, unit: &str, idx: usize) -> (i64, String) {
    let u = &art.units[unit];
    for id in 1..4000 {
        for mt in ["movie", "tv"] {
            if derive_candidate_token(SECRET, unit, mt, id).unwrap() == u.candidates[idx].candidate_token {
                return (id, mt.to_string());
            }
        }
    }
    panic!("identity not found")
}
fn sealed_store(env: &Env, art: &Artifact) -> (Store, String) {
    let mut store = Some(new_store(env, art, 25));
    let sid = start(store.as_mut().unwrap(), "A", 100);
    let mut d = Driver { art, clock: 200, deferred_once: BTreeSet::new(), sid: sid.clone(), n_sub: 0 };
    d.run(&mut store, env, false);
    let mut s = store.take().unwrap();
    s.end_session(&sid, &t(5000)).unwrap();
    let (_, seal) = s.seal(&pins(), &t(5001)).unwrap();
    (s, seal)
}

fn new_store(env: &Env, art: &Artifact, pct: u32) -> Store {
    let mut s = Store::create(&env.dir, &env.cp, art.clone(), &pins(), &t(5), &t(10), Box::new(NoMainApp)).unwrap();
    s.freeze_qc_plan(SEED, pct, &t(20)).unwrap();
    s
}
fn reopen(env: &Env, art: &Artifact) -> Store {
    Store::open(&env.dir, &env.cp, art.clone(), Box::new(NoMainApp)).unwrap()
}
fn verif(id: i64, mt: &str) -> TmdbVerification {
    TmdbVerification {
        requested_id: id,
        requested_media_type: mt.into(),
        http_status: 200,
        response_id: id,
        response_media_type: mt.into(),
        response_body_sha256: hex_sha(b"body"),
        title: "T".into(),
        year: Some("2000".into()),
        fetched_at: t(50),
        verifier_impl: "fake".into(),
    }
}
fn sub(unit: &str, sid: &str, g: &LeaseGrant, d: Decision) -> Submit {
    Submit { unit: unit.into(), submission_id: sid.into(), lease_id: g.lease_id.clone(), prev_event_sha: g.prev_event_sha.clone(), decision: d, note: None, correction_reason: None }
}
fn first_unit(s: &Store) -> String {
    match s.next() {
        Next::Unit { unit, .. } => unit,
        Next::Done => panic!("done"),
    }
}
fn start(s: &mut Store, reviewer: &str, at: i64) -> String {
    s.start_session(reviewer, true, "pr4-blind-ui-1", &t(at)).unwrap()
}
fn has_candidates(art: &Artifact, n: usize) -> String {
    art.units.values().find(|u| u.candidates.len() == n).unwrap().review_unit_id.clone()
}

/// 決定論的な方針で全 unit を進める。`reopen_each` なら 1 コマンドごとに再起動する（resume）
struct Driver<'a> {
    art: &'a Artifact,
    clock: i64,
    deferred_once: BTreeSet<String>,
    sid: String,
    n_sub: u32,
}
impl<'a> Driver<'a> {
    fn tick(&mut self) -> String {
        self.clock += 1;
        t(self.clock)
    }
    fn bounce(&self, store: &mut Option<Store>, env: &Env) {
        *store = None;
        *store = Some(reopen(env, self.art));
    }
    fn run(&mut self, store: &mut Option<Store>, env: &Env, reopen_each: bool) {
        loop {
            if reopen_each {
                self.bounce(store, env);
            }
            let s = store.as_mut().unwrap();
            let (unit, purpose) = match s.next() {
                Next::Unit { unit, purpose } => (unit, purpose),
                Next::Done => return,
            };
            let now = self.tick();
            let g = s.acquire_lease(&self.sid, &unit, purpose, &now).unwrap();
            if reopen_each {
                self.bounce(store, env);
            }
            let s = store.as_mut().unwrap();
            let u = s.unit(&unit).unwrap().clone();
            let h = hex_sha(unit.as_bytes()).as_bytes()[0] as u32 % 12;
            let mut prev = g.prev_event_sha.clone();
            let do_search = |s: &mut Store, d: &mut Self| {
                let now = d.tick();
                s.record_search(&unit, &g.lease_id, "query", "multi", 3, &now).unwrap()
            };
            let decision = if purpose == "qc" {
                prev = do_search(s, self);
                Decision::None
            } else if u.position == 0 {
                Decision::Defer { reason: "insufficient_evidence".into() }
            } else if h == 1 && !self.deferred_once.contains(&unit) {
                self.deferred_once.insert(unit.clone());
                Decision::Defer { reason: "need_external_lookup".into() }
            } else if u.candidates.is_empty() || h % 3 == 2 {
                prev = do_search(s, self);
                if h % 2 == 1 {
                    Decision::PickOther { tmdb_id: 900_000 + h as i64, media_type: "movie".into(), verification: Some(verif(900_000 + h as i64, "movie")) }
                } else {
                    Decision::None
                }
            } else if h % 3 == 1 {
                prev = do_search(s, self);
                Decision::None
            } else {
                confirm(s, &unit, 0)
            };
            self.n_sub += 1;
            let now = self.tick();
            let sb = Submit { unit: unit.clone(), submission_id: format!("sub-{}", self.n_sub), lease_id: g.lease_id.clone(), prev_event_sha: prev, decision, note: None, correction_reason: None };
            s.submit(sb, &now).unwrap();
        }
    }
}

// ── opaque id / order / qc ──────────────────────────────────────────────────────────────────

#[test]
fn opaque_ids_are_unique_secret_dependent_and_order_is_a_permutation() {
    let keys: Vec<String> = (0..200).map(|i| format!("work:{}", i + 1)).collect();
    let ids: BTreeSet<String> = keys.iter().map(|k| derive_review_unit_id(SECRET, k).unwrap()).collect();
    assert_eq!(ids.len(), 200);
    assert!(ids.iter().all(|i| valid_unit_id(i)));
    let other = "1".repeat(64);
    assert_ne!(derive_review_unit_id(SECRET, "work:1").unwrap(), derive_review_unit_id(&other, "work:1").unwrap());
    assert!(derive_review_unit_id("short", "work:1").is_err());
    let by_key: HashMap<String, usize> = assign_positions(SECRET, &keys).unwrap().into_iter().collect();
    let pos: Vec<usize> = keys.iter().map(|k| by_key[k]).collect();
    let mut sorted = pos.clone();
    sorted.sort();
    assert_eq!(sorted, (0..200).collect::<Vec<_>>());
    assert_eq!(assign_positions(SECRET, &keys).unwrap(), assign_positions(SECRET, &keys).unwrap());
    // key 順（work id 順に相当）とは一致しない
    assert_ne!(pos, (0..200).collect::<Vec<_>>());
}

#[test]
fn qc_selection_is_ceil_10pct_deterministic_and_seed_dependent() {
    let ids: Vec<String> = (0..946).map(|i| format!("ru_{i:026}")).collect();
    let a = qc_select(SEED, &ids, 10).unwrap();
    assert_eq!(a.len(), 95);
    assert_eq!(a, qc_select(SEED, &ids, 10).unwrap());
    assert_ne!(a, qc_select(SECRET, &ids, 10).unwrap());
    assert!(qc_select(SEED, &ids, 0).is_err() && qc_select("bad", &ids, 10).is_err());
}

// ── artifact ────────────────────────────────────────────────────────────────────────────────

fn write_artifact(root: &Path, a: &Artifact) {
    std::fs::create_dir_all(root.join("units")).unwrap();
    std::fs::write(root.join("manifest.json"), serde_json::to_string(&a.manifest).unwrap()).unwrap();
    for (id, u) in &a.units {
        std::fs::write(root.join("units").join(format!("{id}.json")), serde_json::to_string(u).unwrap()).unwrap();
    }
}

#[test]
fn artifact_round_trips_and_malformed_artifacts_are_rejected() {
    let env = Env::new("art");
    let a = artifact(6);
    let dir = env.root.join("artifact");
    write_artifact(&dir, &a);
    assert_eq!(Artifact::load(&dir).unwrap().manifest_sha256, a.manifest_sha256);

    let id = a.units.keys().next().unwrap().clone();
    let unit_path = dir.join("units").join(format!("{id}.json"));
    let original = std::fs::read_to_string(&unit_path).unwrap();
    for extra in ["rank", "display_title", "source", "decision"] {
        let mut v: Value = serde_json::from_str(&original).unwrap();
        v[extra] = json!("x");
        std::fs::write(&unit_path, v.to_string()).unwrap();
        assert!(Artifact::load(&dir).is_err(), "{extra}");
    }
    let mut v: Value = serde_json::from_str(&original).unwrap();
    v["evidence"]["files"][0]["original_file_name"] = json!("tampered.mkv");
    std::fs::write(&unit_path, v.to_string()).unwrap();
    assert!(Artifact::load(&dir).is_err());
    std::fs::write(&unit_path, &original).unwrap();
    std::fs::write(dir.join("units").join("extra.json"), "{}").unwrap();
    assert!(Artifact::load(&dir).is_err());
    std::fs::remove_file(dir.join("units").join("extra.json")).unwrap();
    std::fs::remove_file(&unit_path).unwrap();
    assert!(Artifact::load(&dir).is_err());
    std::fs::write(&unit_path, &original).unwrap();
    assert!(Artifact::load(&dir).is_ok());

    let two = a.units.values().find(|u| u.candidates.len() == 2).unwrap().clone();
    let mutations: [fn(&mut ReviewUnit); 5] = [
        |u| u.candidates[1].candidate_token = u.candidates[0].candidate_token.clone(),
        |u| u.candidates[0].candidate_token = "c1".into(),
        |u| u.unit_salt = "x".into(),
        |u| u.overlap_digests.pop().map(|_| ()).unwrap_or(()),
        |u| u.candidates[0].candidate_token = "ct_UPPERCASE00000000000".into(),
    ];
    for m in mutations {
        let mut u = two.clone();
        m(&mut u);
        assert!(u.sealed().unwrap().validate().is_err());
    }
    let mut m = a.manifest.clone();
    m.unit_count += 1;
    assert!(Artifact::from_parts(m, a.units.values().cloned().collect()).is_err());
    assert!(!valid_unit_id("../../etc/passwd") && !valid_unit_id("ru_UPPER00000000000000000000"));
}

// ── 通し ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn full_flow_reaches_a_sealed_audit_and_the_ledger_contains_no_machine_canary() {
    let env = Env::new("flow");
    let art = artifact(24);
    let mut store = Some(new_store(&env, &art, 25));
    let sid = start(store.as_mut().unwrap(), "A", 100);
    let mut d = Driver { art: &art, clock: 200, deferred_once: BTreeSet::new(), sid: sid.clone(), n_sub: 0 };
    d.run(&mut store, &env, false);
    let s = store.as_mut().unwrap();
    let rep = s.completion_audit();
    // session がまだ開いている（A3）だけが落ちる
    assert!(rep.failures().iter().all(|f| f.starts_with("A3_no_open_state")), "{:?}", rep.failures());
    s.end_session(&sid, &t(5000)).unwrap();
    let rep = s.completion_audit();
    assert!(rep.passed(), "{:?}", rep.failures());
    assert!(rep.confirm > 0 && rep.none > 0 && rep.pick_other > 0 && rep.defer_final > 0, "{rep:?}");
    assert!(!rep.independent_second_review); // 同じ reviewer が QC → independent と呼ばない
    assert!(!s.unblind_allowed());
    let (rep, review_seal) = s.seal(&pins(), &t(5001)).unwrap();
    assert_eq!(rep.unit_count, 24);
    assert_eq!(Some(review_seal.as_str()), s.review_seal_sha256());
    assert_eq!(review_seal, s.head()); // seal event の commitment = 外部 audit anchor に固定する値
    assert!(s.unblind_allowed() && s.sealed());
    assert!(matches!(s.start_session("A", true, "x", &t(5002)), Err(BlindError::Sealed)));
    let bytes = std::fs::read_to_string(env.dir.join(LEDGER_FILE)).unwrap();
    for c in CANARIES {
        assert!(!bytes.contains(c), "{c}");
    }
    drop(store);
    assert!(reopen(&env, &art).sealed());
}

#[test]
fn units_serialize_without_machine_information() {
    let art = artifact(8);
    for u in art.units.values() {
        let text = serde_json::to_string(u).unwrap();
        for c in CANARIES {
            assert!(!text.contains(c));
        }
        assert!(scan_forbidden(&serde_json::to_value(u).unwrap()).is_empty(), "{text}");
        assert!(!text.contains("display_title") && !text.contains("candidate_count") && !text.contains("work_id"));
    }
    assert!(!scan_forbidden(&json!({"rules_rank": 1})).is_empty());
    assert!(!scan_forbidden(&json!({"x": "AUTO"})).is_empty());
    assert!(!scan_forbidden(&json!({"matcher_score": 0})).is_empty());
    assert!(scan_forbidden(&json!({"source_media_kind": "video", "overview": "rank and file"})).is_empty());
}

// ── semantics ───────────────────────────────────────────────────────────────────────────────

#[test]
fn none_needs_a_search_and_zero_candidates_never_implies_none() {
    let env = Env::new("none");
    let art = artifact(8);
    let mut s = new_store(&env, &art, 25);
    let sid = start(&mut s, "A", 100);
    let empty = art.units.values().find(|u| u.candidates.is_empty()).unwrap().review_unit_id.clone();
    let g = s.acquire_lease(&sid, &empty, "review", &t(101)).unwrap();
    assert_eq!(s.progress().0, 0); // 候補 0 件でも自動で resolved にならない
    assert_eq!(s.submit(sub(&empty, "a", &g, Decision::None), &t(102)), Err(BlindError::NoneNeedsSearch));
    let prev = s.record_search(&empty, &g.lease_id, "foo", "multi", 0, &t(103)).unwrap();
    let g2 = LeaseGrant { prev_event_sha: prev, ..g };
    assert!(matches!(s.submit(sub(&empty, "a", &g2, Decision::None), &t(104)), Ok(SubmitOutcome::Recorded { .. })));
    let last: Value = serde_json::from_str(std::fs::read_to_string(env.dir.join(LEDGER_FILE)).unwrap().lines().last().unwrap()).unwrap();
    assert!(last["tmdb_id"].is_null() && last["media_type"].is_null());
}

#[test]
fn confirm_takes_identity_from_the_artifact_and_rejects_unknown_refs() {
    let env = Env::new("confirm");
    let art = artifact(8);
    let mut s = new_store(&env, &art, 25);
    let sid = start(&mut s, "A", 100);
    let id = has_candidates(&art, 2);
    let u = art.units[&id].clone();
    let g = s.acquire_lease(&sid, &id, "review", &t(101)).unwrap();
    for bad in ["c9", "", "c1; DROP", "../c1"] {
        assert!(s.submit(sub(&id, "x", &g, Decision::Confirm { candidate_token: bad.into() }), &t(102)).is_err());
    }
    s.submit(sub(&id, "ok", &g, confirm(&s, &id, 1)), &t(103)).unwrap();
    let ev: Value = serde_json::from_str(std::fs::read_to_string(env.dir.join(LEDGER_FILE)).unwrap().lines().last().unwrap()).unwrap();
    // confirm の event は opaque token だけを持ち、index（c1/c2）も identity も持たない
    assert_eq!(ev["candidate_token"], json!(u.candidates[1].candidate_token));
    for k in ["tmdb_id", "media_type", "candidate_ref", "rank"] {
        assert!(ev.get(k).is_none(), "{k}");
    }
}

#[test]
fn pick_other_requires_a_matching_verification_and_a_non_candidate() {
    let env = Env::new("pick");
    let art = artifact(8);
    let mut s = new_store(&env, &art, 25);
    let sid = start(&mut s, "A", 100);
    let id = has_candidates(&art, 2);
    let u = art.units[&id].clone();
    let g = s.acquire_lease(&sid, &id, "review", &t(101)).unwrap();
    let po = |tid: i64, mt: &str, v: Option<TmdbVerification>| Decision::PickOther { tmdb_id: tid, media_type: mt.into(), verification: v };
    let (cid, cmt) = ident(&art, &id, 0);
    assert_eq!(s.submit(sub(&id, "1", &g, po(cid, &cmt, Some(verif(cid, &cmt)))), &t(102)), Err(BlindError::TargetIsCandidate));
    assert!(matches!(s.submit(sub(&id, "2", &g, po(5, "movie", None)), &t(103)), Err(BlindError::TargetNotVerified(_))));
    for (tid, mt) in [(0, "movie"), (-1, "movie"), (MAX_TMDB_ID + 1, "movie"), (i64::MAX, "tv"), (5, ""), (5, "person"), (5, "TV"), (5, " movie")] {
        assert!(matches!(s.submit(sub(&id, "3", &g, po(tid, mt, Some(verif(tid, mt)))), &t(104)), Err(BlindError::Invalid(_))), "{tid} {mt}");
    }
    let mut bad = verif(5, "movie");
    bad.response_id = 6; // 別作品が返った
    assert!(matches!(s.submit(sub(&id, "4", &g, po(5, "movie", Some(bad))), &t(105)), Err(BlindError::TargetNotVerified(_))));
    let mut bad = verif(5, "movie");
    bad.requested_id = 7;
    assert!(s.submit(sub(&id, "4b", &g, po(5, "movie", Some(bad))), &t(105)).is_err());
    let mut bad = verif(5, "movie");
    bad.http_status = 404;
    assert!(s.submit(sub(&id, "5", &g, po(5, "movie", Some(bad))), &t(106)).is_err());
    let mut bad = verif(5, "movie");
    bad.response_media_type = "tv".into();
    assert!(s.submit(sub(&id, "6", &g, po(5, "movie", Some(bad))), &t(107)).is_err());
    let mut bad = verif(5, "movie");
    bad.response_body_sha256 = "zz".into();
    assert!(s.submit(sub(&id, "7", &g, po(5, "movie", Some(bad))), &t(108)).is_err());
    assert_eq!(s.progress().0, 0); // 失敗は何も書かない
    s.submit(sub(&id, "8", &g, po(5, "movie", Some(verif(5, "movie")))), &t(109)).unwrap();
}

#[test]
fn defer_goes_to_a_second_pass_only_after_the_main_pass_and_then_becomes_final() {
    let env = Env::new("defer");
    let art = artifact(4);
    let mut s = new_store(&env, &art, 50);
    let sid = start(&mut s, "A", 100);
    let first = first_unit(&s);
    let g = s.acquire_lease(&sid, &first, "review", &t(101)).unwrap();
    assert!(s.submit(sub(&first, "d0", &g, Decision::Defer { reason: "bogus".into() }), &t(102)).is_err());
    assert!(s.submit(sub(&first, "d0", &g, Decision::Defer { reason: "other_with_note".into() }), &t(102)).is_err()); // note 必須
    s.submit(sub(&first, "d1", &g, Decision::Defer { reason: "insufficient_evidence".into() }), &t(103)).unwrap();
    assert_ne!(first_unit(&s), first); // 本流が残っている間、deferred は出ない
    assert!(matches!(s.acquire_lease(&sid, &first, "review", &t(104)), Err(BlindError::UnitNotAvailable(_))));
    let mut clock = 110;
    loop {
        let u = first_unit(&s);
        if u == first {
            break;
        }
        clock += 1;
        let mut g = s.acquire_lease(&sid, &u, "review", &t(clock)).unwrap();
        let unit = s.unit(&u).unwrap().clone();
        let dec = if unit.candidates.is_empty() {
            clock += 1;
            g.prev_event_sha = s.record_search(&u, &g.lease_id, "q", "multi", 0, &t(clock)).unwrap();
            Decision::None
        } else {
            confirm(&s, &u, 0)
        };
        clock += 1;
        s.submit(sub(&u, &format!("m{clock}"), &g, dec), &t(clock)).unwrap();
    }
    // 2 巡目: 再度 defer → defer_final（第 3 巡は無い）
    let g = s.acquire_lease(&sid, &first, "review", &t(300)).unwrap();
    s.submit(sub(&first, "d2", &g, Decision::Defer { reason: "multiple_equally_valid_targets".into() }), &t(301)).unwrap();
    assert_eq!(s.progress(), (4, 0, 0));
    assert!(matches!(s.acquire_lease(&sid, &first, "review", &t(302)), Err(BlindError::UnitNotAvailable(_))));
    let text = std::fs::read_to_string(env.dir.join(LEDGER_FILE)).unwrap();
    assert!(text.contains("\"type\":\"defer\"") && text.contains("\"type\":\"defer_final\""));
}

// ── duplicate / concurrency / CAS / lease ───────────────────────────────────────────────────

#[test]
fn duplicate_submission_is_idempotent_and_a_resolved_unit_rejects_new_submissions() {
    let env = Env::new("dup");
    let art = artifact(6);
    let mut s = new_store(&env, &art, 50);
    let sid = start(&mut s, "A", 100);
    let u = has_candidates(&art, 2);
    let g = s.acquire_lease(&sid, &u, "review", &t(101)).unwrap();
    let d0 = confirm(&s, &u, 0);
    let d = || d0.clone();
    assert!(matches!(s.submit(sub(&u, "same", &g, d()), &t(102)), Ok(SubmitOutcome::Recorded { .. })));
    let n = s.event_count();
    assert!(matches!(s.submit(sub(&u, "same", &g, d()), &t(103)), Ok(SubmitOutcome::Duplicate { .. })));
    assert_eq!(s.event_count(), n);
    assert!(matches!(s.submit(sub(&u, "other", &g, d()), &t(104)), Err(BlindError::UnitAlreadyResolved(_))));
    assert_eq!(s.event_count(), n);
    // 同じ submission_id で内容が違う → 異常として ledger に残り、以後は全て STOP
    assert_eq!(s.submit(sub(&u, "same", &g, Decision::None), &t(105)), Err(BlindError::Poisoned));
    assert_eq!(s.event_count(), n + 1);
    assert!(matches!(s.start_session("A", true, "x", &t(106)), Err(BlindError::Poisoned)));
    drop(s);
    let s2 = reopen(&env, &art);
    let rep = s2.completion_audit();
    assert!(rep.checks.iter().any(|c| c.0 == "A7_no_duplicate_no_anomaly" && !c.1));
}

#[test]
fn stale_cas_is_a_conflict_with_no_side_effect() {
    let env = Env::new("cas");
    let art = artifact(6);
    let mut s = new_store(&env, &art, 50);
    let sid = start(&mut s, "A", 100);
    let u = has_candidates(&art, 2);
    let g = s.acquire_lease(&sid, &u, "review", &t(101)).unwrap();
    let mut stale = sub(&u, "s", &g, confirm(&s, &u, 0));
    stale.prev_event_sha = hex_sha(b"stale");
    let n = s.event_count();
    assert_eq!(s.submit(stale, &t(102)), Err(BlindError::Conflict));
    assert_eq!(s.event_count(), n);
    let fresh = s.record_search(&u, &g.lease_id, "q", "multi", 1, &t(103)).unwrap();
    assert_eq!(s.submit(sub(&u, "t", &g, confirm(&s, &u, 0)), &t(104)), Err(BlindError::Conflict));
    let g2 = LeaseGrant { prev_event_sha: fresh, ..g };
    assert!(s.submit(sub(&u, "t", &g2, confirm(&s, &u, 0)), &t(105)).is_ok());
}

#[test]
fn only_one_process_one_session_and_one_lease_per_unit_and_leases_expire() {
    let env = Env::new("conc");
    let art = artifact(6);
    let mut s = new_store(&env, &art, 50);
    assert!(matches!(Store::open(&env.dir, &env.cp, art.clone(), Box::new(NoMainApp)), Err(BlindError::Locked)));
    let sid = start(&mut s, "A", 100);
    let u = has_candidates(&art, 2);
    let g = s.acquire_lease(&sid, &u, "review", &t(101)).unwrap();
    assert!(s.acquire_lease(&sid, &u, "review", &t(102)).is_err());
    let other = art.units.keys().find(|k| **k != u).unwrap().clone();
    assert!(matches!(s.acquire_lease(&sid, &other, "review", &t(102)), Err(BlindError::LeaseInvalid(_))));
    let late = 101 + LEASE_SECONDS + 1;
    let ok0 = confirm(&s, &u, 0);
    let ok = || ok0.clone();
    assert!(matches!(s.submit(sub(&u, "late", &g, ok()), &t(late)), Err(BlindError::LeaseInvalid(_))));
    assert_eq!(s.progress().0, 0);
    let g2 = s.acquire_lease(&sid, &u, "review", &t(late + 1)).unwrap();
    assert_ne!(g2.lease_id, g.lease_id);
    assert!(std::fs::read_to_string(env.dir.join(LEDGER_FILE)).unwrap().contains("\"type\":\"lease_expired\""));
    assert!(s.submit(sub(&u, "old", &g, ok()), &t(late + 2)).is_err()); // 失効した旧 lease では出せない
}

#[test]
fn session_limits_and_main_app_stop() {
    let env = Env::new("lim");
    let art = artifact(60);
    let mut s = new_store(&env, &art, 10);
    let sid = start(&mut s, "A", 100);
    let ids: Vec<String> = art.units.keys().cloned().collect();
    for (i, u) in ids.iter().take(SESSION_MAX_UNITS as usize).enumerate() {
        let g = s.acquire_lease(&sid, u, "review", &t(101 + i as i64 * 2)).unwrap();
        s.release_lease(u, &g.lease_id, &t(102 + i as i64 * 2)).unwrap();
    }
    assert!(matches!(s.acquire_lease(&sid, &ids[50], "review", &t(900)), Err(BlindError::SessionLimit(_))));
    s.end_session(&sid, &t(901)).unwrap();
    let sid2 = start(&mut s, "A", 1000);
    // 90 分を超えた session は新しい lease を取れない
    assert!(matches!(s.acquire_lease(&sid2, &ids[50], "review", &t(1000 + SESSION_MAX_SECONDS + 5)), Err(BlindError::SessionLimit(_))));

    struct Running;
    impl MainAppProbe for Running {
        fn main_app_running(&self) -> bool {
            true
        }
    }
    let env2 = Env::new("lim2");
    let mut s2 = Store::create(&env2.dir, &env2.cp, artifact(6), &pins(), &t(5), &t(10), Box::new(Running)).unwrap();
    s2.freeze_qc_plan(SEED, 50, &t(20)).unwrap();
    assert!(matches!(s2.start_session("A", true, "x", &t(100)), Err(BlindError::MainAppRunning)));
}

#[test]
fn ordering_rules_for_anchor_qc_plan_and_attestation() {
    let env = Env::new("order");
    let art = artifact(6);
    let mut s = Store::create(&env.dir, &env.cp, art.clone(), &pins(), &t(500), &t(10), Box::new(NoMainApp)).unwrap();
    assert!(s.start_session("A", true, "x", &t(600)).is_err()); // qc_plan の前は不可
    s.freeze_qc_plan(SEED, 50, &t(20)).unwrap();
    assert!(matches!(s.start_session("A", false, "x", &t(600)), Err(BlindError::Invalid(_))));
    assert!(s.start_session("A", true, "x", &t(400)).is_err()); // machine freeze の anchor より前は不可
    let sid = start(&mut s, "A", 600);
    assert!(s.freeze_qc_plan(SECRET, 50, &t(601)).is_err()); // review 開始後は seed を変えられない
    s.end_session(&sid, &t(602)).unwrap();
    assert!(s.start_session("A", true, "x", &t(100)).is_err()); // 時刻の巻き戻し
}

// ── resume / tamper ─────────────────────────────────────────────────────────────────────────

#[test]
fn resume_after_every_command_gives_the_same_ledger_as_an_uninterrupted_run() {
    let art = artifact(10);
    let finish = |reopen_each: bool| {
        let env = Env::new(if reopen_each { "res_a" } else { "res_b" });
        let mut store = Some(new_store(&env, &art, 30));
        let sid = start(store.as_mut().unwrap(), "A", 100);
        let mut d = Driver { art: &art, clock: 200, deferred_once: BTreeSet::new(), sid: sid.clone(), n_sub: 0 };
        d.run(&mut store, &env, reopen_each);
        let s = store.as_mut().unwrap();
        s.end_session(&sid, &t(9000)).unwrap();
        assert!(s.completion_audit().passed(), "{:?}", s.completion_audit().failures());
        let (head, count) = (s.head().to_string(), s.event_count());
        drop(store);
        let again = reopen(&env, &art);
        assert_eq!((again.head().to_string(), again.event_count()), (head.clone(), count));
        (head, count)
    };
    assert_eq!(finish(true), finish(false));
}

#[test]
fn crash_with_an_open_session_and_lease_is_recovered_on_the_next_start() {
    let env = Env::new("crash");
    let art = artifact(6);
    let mut s = new_store(&env, &art, 50);
    let sid = start(&mut s, "A", 100);
    let u = first_unit(&s);
    s.acquire_lease(&sid, &u, "review", &t(101)).unwrap();
    drop(s); // 強制終了相当（session も lease も開いたまま）
    let mut s = reopen(&env, &art);
    let sid2 = s.start_session("A", true, "x", &t(200)).unwrap();
    assert_ne!(sid, sid2);
    assert!(std::fs::read_to_string(env.dir.join(LEDGER_FILE)).unwrap().contains("crash_recovered"));
    assert!(s.acquire_lease(&sid2, &u, "review", &t(201)).is_ok());
}

#[test]
fn torn_tail_tampering_deletion_and_reordering_are_detected() {
    let env = Env::new("tamper");
    let art = artifact(6);
    let mut s = new_store(&env, &art, 50);
    let sid = start(&mut s, "A", 100);
    let u = first_unit(&s);
    s.acquire_lease(&sid, &u, "review", &t(101)).unwrap();
    s.end_session(&sid, &t(102)).unwrap(); // checkpoint も書く
    drop(s);
    let path = env.dir.join(LEDGER_FILE);
    let good = std::fs::read_to_string(&path).unwrap();
    let open = || Store::open(&env.dir, &env.cp, art.clone(), Box::new(NoMainApp));

    assert!(open().is_ok());
    std::fs::write(&path, format!("{good}{{\"type\":\"sea")).unwrap();
    assert!(matches!(open(), Err(BlindError::TornTail)));
    std::fs::write(&path, good.replacen("qc_plan", "qc_pl4n", 1)).unwrap();
    assert!(matches!(open(), Err(BlindError::Tampered(_))));
    let lines: Vec<&str> = good.lines().collect();
    let mid: Vec<&str> = lines.iter().enumerate().filter(|(i, _)| *i != 2).map(|(_, l)| *l).collect();
    std::fs::write(&path, format!("{}\n", mid.join("\n"))).unwrap();
    assert!(matches!(open(), Err(BlindError::Tampered(_))));
    let mut re = lines.clone();
    re.swap(2, 3);
    std::fs::write(&path, format!("{}\n", re.join("\n"))).unwrap();
    assert!(matches!(open(), Err(BlindError::Tampered(_))));
    // 末尾削除は checkpoint が覚えている件数で検出
    let cut = lines[..lines.len() - 2].join("\n");
    std::fs::write(&path, format!("{cut}\n")).unwrap();
    assert!(matches!(open(), Err(BlindError::Tampered(_))));
    // checkpoint の改変
    std::fs::write(&path, &good).unwrap();
    let cp0 = env.cp.join("cp_000000.json");
    let txt = std::fs::read_to_string(&cp0).unwrap();
    std::fs::write(&cp0, txt.replace("event_count", "event_cxunt")).unwrap();
    assert!(matches!(open(), Err(BlindError::Tampered(_))));
    std::fs::write(&cp0, &txt).unwrap();
    assert!(open().is_ok());
    // 意味の偽造: hash を正しく作り直しても、遷移として不正な event は replay で拒否される
    let head = serde_json::from_str::<Value>(lines.last().unwrap()).unwrap()["event_commitment"].as_str().unwrap().to_string();
    let forged = with_commitment(json!({"type": "decision", "ts": t(103), "unit": u, "submission_id": "x"}), lines.len() as u64, &head).unwrap();
    std::fs::write(&path, format!("{good}{}\n", canon(&forged).unwrap())).unwrap();
    assert!(open().is_err());
}

// ── correction ──────────────────────────────────────────────────────────────────────────────

#[test]
fn correction_keeps_full_history_requires_a_reason_and_ends_at_seal() {
    let env = Env::new("corr");
    let art = artifact(6);
    let mut s = new_store(&env, &art, 50);
    let sid = start(&mut s, "A", 100);
    let u = has_candidates(&art, 2);
    let g = s.acquire_lease(&sid, &u, "review", &t(101)).unwrap();
    s.submit(sub(&u, "c0", &g, confirm(&s, &u, 0)), &t(102)).unwrap();
    let mut no_lease = sub(&u, "c1", &g, confirm(&s, &u, 1));
    no_lease.correction_reason = Some("mis_click".into());
    assert!(s.submit(no_lease, &t(103)).is_err());
    let cg = s.acquire_lease(&sid, &u, "correction", &t(104)).unwrap();
    assert!(s.submit(sub(&u, "c2", &cg, confirm(&s, &u, 1)), &t(105)).is_err()); // reason なし
    let mut b2 = sub(&u, "c3", &cg, confirm(&s, &u, 1));
    b2.correction_reason = Some("whim".into());
    assert!(s.submit(b2, &t(106)).is_err());
    let mut b3 = sub(&u, "c4", &cg, confirm(&s, &u, 1));
    b3.correction_reason = Some("other_with_note".into());
    assert!(s.submit(b3, &t(107)).is_err()); // note 必須
    let mut ok = sub(&u, "c5", &cg, confirm(&s, &u, 1));
    ok.correction_reason = Some("mis_click".into());
    ok.note = Some("押し間違い".into());
    s.submit(ok, &t(108)).unwrap();
    assert_eq!(s.unit_corrections(&u), 1);
    assert_eq!(s.completion_audit().corrections, 1);
    let text = std::fs::read_to_string(env.dir.join(LEDGER_FILE)).unwrap();
    assert!(text.contains("\"type\":\"decision\"") && text.contains("\"type\":\"correction\"") && text.contains("mis_click"));
    // defer への訂正は不可
    let cg2 = s.acquire_lease(&sid, &u, "correction", &t(109)).unwrap();
    let mut d = sub(&u, "c6", &cg2, Decision::Defer { reason: "insufficient_evidence".into() });
    d.correction_reason = Some("mis_click".into());
    assert!(s.submit(d, &t(110)).is_err());
}

// ── completion audit / seal / QC ────────────────────────────────────────────────────────────

#[test]
fn seal_is_refused_until_every_audit_passes_and_independence_needs_a_different_reviewer() {
    let env = Env::new("seal");
    let art = artifact(12);
    let mut s = new_store(&env, &art, 25);
    let sid = start(&mut s, "A", 100);
    assert!(matches!(s.seal(&pins(), &t(101)), Err(BlindError::AuditFailed(_))));
    // 本流だけ終える（QC 未了）
    let mut clock = 200;
    let mut n = 0;
    while let Next::Unit { unit, purpose: "review" } = s.next() {
        clock += 1;
        let mut g = s.acquire_lease(&sid, &unit, "review", &t(clock)).unwrap();
        let u = s.unit(&unit).unwrap().clone();
        n += 1;
        let dec = if u.candidates.is_empty() {
            clock += 1;
            g.prev_event_sha = s.record_search(&unit, &g.lease_id, "q", "multi", 0, &t(clock)).unwrap();
            Decision::None
        } else {
            confirm(&s, &unit, 0)
        };
        clock += 1;
        s.submit(sub(&unit, &format!("m{n}"), &g, dec), &t(clock)).unwrap();
    }
    let rep = s.completion_audit();
    assert!(rep.failures().iter().any(|f| f.starts_with("A11_qc_complete")), "{:?}", rep.failures());
    assert!(rep.failures().iter().any(|f| f.starts_with("A3_no_open_state"))); // session がまだ開いている
    assert!(s.seal(&pins(), &t(clock + 1)).is_err());
    s.end_session(&sid, &t(clock + 2)).unwrap();
    assert!(s.seal(&pins(), &t(clock + 3)).is_err()); // QC がまだ

    // 別の reviewer B が QC を行う → independent_second_review = true
    let sid_b = start(&mut s, "B", 3000);
    let mut clock = 3100;
    while let Next::Unit { unit, purpose } = s.next() {
        assert_eq!(purpose, "qc");
        clock += 1;
        let mut g = s.acquire_lease(&sid_b, &unit, "qc", &t(clock)).unwrap();
        clock += 1;
        g.prev_event_sha = s.record_search(&unit, &g.lease_id, "q", "multi", 0, &t(clock)).unwrap();
        clock += 1;
        s.submit(sub(&unit, &format!("qc-{unit}"), &g, Decision::None), &t(clock)).unwrap();
    }
    s.end_session(&sid_b, &t(clock + 1)).unwrap();
    // leak scan の hash が不正なら seal できない
    // pin が 1 つでも init と違えば seal できない（8 個すべて）
    let base = pins();
    let mutations: [(&str, fn(&mut Pins)); 8] = [
        ("protocol_sha256", |p| p.protocol_sha256 = hex_sha(b"x")),
        ("ui_package_sha256", |p| p.ui_package_sha256 = hex_sha(b"x")),
        ("shuffle_seed_commitment", |p| p.shuffle_seed_commitment = hex_sha(b"x")),
        ("attestation_policy_sha256", |p| p.attestation_policy_sha256 = hex_sha(b"x")),
        ("unblind_map_sha256", |p| p.unblind_map_sha256 = hex_sha(b"x")),
        ("build_secret_commitment", |p| p.build_secret_commitment = hex_sha(b"x")),
        ("a8_leak_scan_report_sha256", |p| p.a8_leak_scan_report_sha256 = hex_sha(b"x")),
        ("machine_freeze_anchor_commitment", |p| p.machine_freeze_anchor_commitment = hex_sha(b"x")),
    ];
    for (name, m) in mutations {
        let mut bad = base.clone();
        m(&mut bad);
        assert_eq!(s.seal(&bad, &t(clock + 2)), Err(BlindError::PinMismatch(vec![name.to_string()])), "{name}");
        assert!(!s.sealed());
    }
    let mut not_hex = base.clone();
    not_hex.unblind_map_sha256 = "not-a-hash".into();
    assert!(matches!(s.seal(&not_hex, &t(clock + 2)), Err(BlindError::Invalid(_))));
    let (rep, _) = s.seal(&base, &t(clock + 3)).unwrap();
    assert!(rep.independent_second_review);
    assert_eq!((rep.qc_total, rep.qc_done), (3, 3));
    assert!(s.unblind_allowed());
}

#[test]
fn qc_does_not_change_the_effective_decision_and_same_reviewer_is_not_independent() {
    let env = Env::new("qc");
    let art = artifact(8);
    let mut store = Some(new_store(&env, &art, 25));
    let sid = start(store.as_mut().unwrap(), "A", 100);
    let mut d = Driver { art: &art, clock: 200, deferred_once: BTreeSet::new(), sid: sid.clone(), n_sub: 0 };
    d.run(&mut store, &env, false);
    let s = store.as_mut().unwrap();
    s.end_session(&sid, &t(9000)).unwrap();
    let rep = s.completion_audit();
    assert!(rep.passed(), "{:?}", rep.failures());
    assert_eq!(rep.qc_done, rep.qc_total);
    assert!(rep.qc_agree <= rep.qc_total);
    assert_eq!(rep.confirm + rep.pick_other + rep.none + rep.defer_final, 8); // QC で decision 数は増えない
    assert!(!rep.independent_second_review);
}

// ── 構造・parity ────────────────────────────────────────────────────────────────────────────

#[test]
fn core_does_not_reference_production_db_or_machine_types() {
    let src = include_str!("pr4_blind_gt.rs");
    for line in src.lines().filter(|l| l.trim_start().starts_with("use ")) {
        assert!(!line.contains("crate::") && !line.contains("rusqlite") && !line.contains("tauri") && !line.contains("reqwest"), "{line}");
    }
    for line in src.lines().filter(|l| !l.trim_start().starts_with("//")) {
        for banned in ["rules_rank", "matcher_score", "rules_safe", "rules_tags_shadow", "metadata_match_", "metadata_review_tasks", "pr4_pf0e", "gt_review::", "audit_run"] {
            assert!(!line.contains(banned), "{line}");
        }
    }
}

#[test]
fn label_mapping_matches_the_c5c5_review_semantics() {
    use crate::services::match_history::LabelMethod;
    assert_eq!(gt_label("confirm"), Some(("tmdb", LabelMethod::ReviewConfirm.as_str())));
    assert_eq!(gt_label("pick_other"), Some(("tmdb", LabelMethod::ReviewPickOther.as_str())));
    assert_eq!(gt_label("none"), Some(("none", LabelMethod::ReviewNone.as_str())));
    assert_eq!(gt_label("defer"), None);
    assert_eq!(REVIEW_PROTOCOL_VERSION, crate::services::match_history::REVIEW_PROTOCOL_VERSION);
}

#[test]
fn time_helpers_round_trip() {
    for e in [0i64, 951_782_400, BASE, 1_900_000_000] {
        assert_eq!(epoch(&fmt_epoch(e)).unwrap(), e);
    }
    assert!(epoch("2026-13-01T00:00:00Z").is_err() && epoch("2026-10-09 00:00:00").is_err());
}

// ── PF0G1 final checks（監査 HOLD の 4 点）──────────────────────────────────────────────────

#[test]
fn confirm_is_recorded_as_an_opaque_token_only_and_a_forged_identity_is_rejected_on_replay() {
    let env = Env::new("token");
    let art = artifact(6);
    let mut s = new_store(&env, &art, 50);
    let sid = start(&mut s, "A", 100);
    let id = has_candidates(&art, 2);
    let g = s.acquire_lease(&sid, &id, "review", &t(101)).unwrap();
    s.submit(sub(&id, "k", &g, confirm(&s, &id, 1)), &t(102)).unwrap();
    s.end_session(&sid, &t(103)).unwrap();
    drop(s);
    let path = env.dir.join(LEDGER_FILE);
    let text = std::fs::read_to_string(&path).unwrap();
    // token は index でも identity でもない
    let tok = &art.units[&id].candidates[1].candidate_token;
    assert!(valid_token(tok) && !tok.contains("c2"));
    for line in text.lines() {
        let ev: Value = serde_json::from_str(line).unwrap();
        if ev["type"] == "decision" {
            assert_eq!(ev["candidate_token"], json!(tok));
            assert!(ev.get("tmdb_id").is_none() && ev.get("media_type").is_none() && ev.get("candidate_ref").is_none());
        }
    }
    // unit の JSON にも identity は無い（token → identity の対応は unblind map だけ）
    assert!(!serde_json::to_string(&art.units[&id]).unwrap().contains("tmdb_id"));
    // hash を正しく作り直しても、identity 付きの confirm は replay で拒否される
    let lines: Vec<&str> = text.lines().collect();
    let last_decision = lines.iter().position(|l| l.contains("\"type\":\"decision\"")).unwrap();
    let mut forged: Value = serde_json::from_str(lines[last_decision]).unwrap();
    forged["tmdb_id"] = json!(1234);
    forged.as_object_mut().unwrap().remove("event_commitment");
    let prev = serde_json::from_str::<Value>(lines[last_decision - 1]).unwrap()["event_commitment"].as_str().unwrap().to_string();
    let mut body = forged.clone();
    body.as_object_mut().unwrap().remove("schema_version");
    body.as_object_mut().unwrap().remove("seq");
    body.as_object_mut().unwrap().remove("prev_commitment");
    let forged = with_commitment(body, last_decision as u64, &prev).unwrap();
    let mut out: Vec<String> = lines[..last_decision].iter().map(|l| l.to_string()).collect();
    out.push(canon(&forged).unwrap());
    std::fs::write(&path, format!("{}\n", out.join("\n"))).unwrap();
    std::fs::remove_dir_all(&env.cp).unwrap();
    std::fs::create_dir_all(&env.cp).unwrap();
    assert!(matches!(Store::open(&env.dir, &env.cp, art.clone(), Box::new(NoMainApp)), Err(BlindError::Stop(m)) if m.contains("identity")));
}

#[test]
fn pick_other_overlap_with_a_candidate_is_decided_by_token_not_identity() {
    let art = artifact(6);
    let id = has_candidates(&art, 2);
    let u = &art.units[&id];
    let (cid, cmt) = ident(&art, &id, 1);
    assert!(u.has_target(cid, &cmt));
    assert!(!u.has_target(cid + 100_000, &cmt));
    assert!(!u.has_target(cid, if cmt == "movie" { "tv" } else { "movie" })); // media_type も identity の一部
    // token は unit ごとに違う（別 unit では同じ作品でも別 token）
    let other = art.units.values().find(|x| x.review_unit_id != id && !x.candidates.is_empty()).unwrap();
    assert_ne!(derive_candidate_token(SECRET, &id, &cmt, cid).unwrap(), derive_candidate_token(SECRET, &other.review_unit_id, &cmt, cid).unwrap());
}

#[test]
fn reviewer_package_must_not_contain_build_secret_unblind_map_work_ids_or_identity_mappings() {
    let env = Env::new("pkg");
    let a = artifact(6);
    let dir = env.root.join("artifact");
    write_artifact(&dir, &a);
    assert!(check_reviewer_package(&dir).is_ok());
    assert!(Artifact::load(&dir).is_ok());

    // 余分なファイル（unblind map / secret）
    for name in ["unblind_map.json", "build_secret.txt", "notes.md"] {
        std::fs::write(dir.join(name), "{}").unwrap();
        assert!(matches!(Artifact::load(&dir), Err(BlindError::Stop(_))), "{name}");
        std::fs::remove_file(dir.join(name)).unwrap();
    }
    // 余分なディレクトリ
    std::fs::create_dir_all(dir.join("sealed_unblind")).unwrap();
    assert!(matches!(Artifact::load(&dir), Err(BlindError::Stop(_))));
    std::fs::remove_dir_all(dir.join("sealed_unblind")).unwrap();
    // units の中のサブディレクトリ
    std::fs::create_dir_all(dir.join("units").join("sub")).unwrap();
    assert!(matches!(Artifact::load(&dir), Err(BlindError::Stop(_))));
    std::fs::remove_dir_all(dir.join("units").join("sub")).unwrap();

    // 既存ファイルの中身に禁止情報が混ざった場合（manifest / unit）
    let manifest_path = dir.join("manifest.json");
    let manifest = std::fs::read_to_string(&manifest_path).unwrap();
    for needle in ["build_secret", "unblind_map", "work_id", "evaluation_unit_key", "candidate_mapping"] {
        std::fs::write(&manifest_path, manifest.replacen("synthetic", needle, 1)).unwrap();
        assert!(matches!(Artifact::load(&dir), Err(BlindError::Stop(_))), "{needle}");
    }
    std::fs::write(&manifest_path, &manifest).unwrap();
    let id = a.units.keys().next().unwrap().clone();
    let unit_path = dir.join("units").join(format!("{id}.json"));
    let unit = std::fs::read_to_string(&unit_path).unwrap();
    for (from, to) in [("a/f", "work:12345/f"), ("video", "work_id"), ("\"candidates\"", "\"tmdb_id\":1,\"candidates\"")] {
        std::fs::write(&unit_path, unit.replacen(from, to, 1)).unwrap();
        assert!(matches!(Artifact::load(&dir), Err(BlindError::Stop(_)) | Err(BlindError::Invalid(_))), "{to}");
    }
    std::fs::write(&unit_path, &unit).unwrap();
    assert!(Artifact::load(&dir).is_ok());
    // key だけの検出器
    assert!(!scan_forbidden_keys(&json!({"build_secret": "x"})).is_empty());
    assert!(!scan_forbidden_keys(&json!({"unblind_map": {}})).is_empty());
    assert!(!scan_forbidden_keys(&json!({"candidate_mapping": []})).is_empty());
    assert!(!scan_forbidden(&json!({"x": "work:42"})).is_empty());
    assert!(scan_forbidden_keys(&json!({"title": "Review"})).is_empty()); // 作品名 "Review" は誤検出しない
}

#[test]
fn init_pins_all_eight_and_seal_must_match_them_exactly_and_survive_reopen() {
    let env = Env::new("pins");
    let art = artifact(12);
    let (s, seal) = sealed_store(&env, &art);
    // init event に 8 個の pin が全部ある
    let text = std::fs::read_to_string(env.dir.join(LEDGER_FILE)).unwrap();
    let init: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    let p = init["pins"].as_object().unwrap();
    assert_eq!(p.len(), 8);
    for k in ["protocol_sha256", "ui_package_sha256", "shuffle_seed_commitment", "attestation_policy_sha256", "unblind_map_sha256", "build_secret_commitment", "a8_leak_scan_report_sha256", "machine_freeze_anchor_commitment"] {
        assert!(p.get(k).and_then(Value::as_str).map(is_hex64).unwrap_or(false), "{k}");
    }
    // seal event も同じ pin を持ち、event_commitment が review_seal_sha256
    let last: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert_eq!(last["type"], "seal");
    assert_eq!(last["pins"], init["pins"]);
    assert_eq!(last["event_commitment"], json!(seal));
    drop(s);
    let again = reopen(&env, &art);
    assert_eq!(again.review_seal_sha256(), Some(seal.as_str()));
    // pin が 64 桁 hex でなければ ledger を作れない
    let env2 = Env::new("pins2");
    let mut bad = pins();
    bad.build_secret_commitment = "short".into();
    assert!(Store::create(&env2.dir, &env2.cp, art.clone(), &bad, &t(5), &t(10), Box::new(NoMainApp)).is_err());
}

#[test]
fn a_forged_seal_with_different_pins_is_rejected_on_replay() {
    let env = Env::new("pins_forge");
    let art = artifact(12);
    let (s, _) = sealed_store(&env, &art);
    drop(s);
    let path = env.dir.join(LEDGER_FILE);
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len();
    let mut seal: Value = serde_json::from_str(lines[n - 1]).unwrap();
    seal["pins"]["unblind_map_sha256"] = json!(hex_sha(b"swapped-after-the-fact"));
    for k in ["event_commitment", "schema_version", "seq", "prev_commitment"] {
        seal.as_object_mut().unwrap().remove(k);
    }
    let prev = serde_json::from_str::<Value>(lines[n - 2]).unwrap()["event_commitment"].as_str().unwrap().to_string();
    let forged = with_commitment(seal, (n - 1) as u64, &prev).unwrap();
    let mut out: Vec<String> = lines[..n - 1].iter().map(|l| l.to_string()).collect();
    out.push(canon(&forged).unwrap());
    std::fs::write(&path, format!("{}\n", out.join("\n"))).unwrap();
    std::fs::remove_dir_all(&env.cp).unwrap();
    std::fs::create_dir_all(&env.cp).unwrap();
    assert!(matches!(Store::open(&env.dir, &env.cp, art.clone(), Box::new(NoMainApp)), Err(BlindError::PinMismatch(_))));
}

#[test]
fn post_seal_errata_never_rewrites_the_gt_and_a_promotion_impacting_one_invalidates_the_evaluation() {
    let env = Env::new("errata");
    let art = artifact(12);
    let (s, seal) = sealed_store(&env, &art);
    let ledger = env.dir.join(LEDGER_FILE);
    let before = std::fs::read(&ledger).unwrap();
    let unit = art.units.keys().next().unwrap().clone();
    assert_eq!(s.evaluation_validity().unwrap(), EvaluationValidity::Valid { recorded_errata: 0 });

    // seal 後は correction も新しい lease も拒否（GT は immutable）
    let mut s = s;
    assert!(matches!(s.start_session("A", true, "x", &t(6000)), Err(BlindError::Sealed)));
    assert!(matches!(s.acquire_lease("S1", &unit, "correction", &t(6000)), Err(BlindError::Sealed)));

    // 影響しない errata は記録だけ。評価は Valid のまま
    assert_eq!(s.record_post_seal_errata(&unit, "表記の誤り（GT に影響なし）", false, &t(7000)).unwrap(), 0);
    assert_eq!(s.evaluation_validity().unwrap(), EvaluationValidity::Valid { recorded_errata: 1 });
    // promotion に影響し得る errata → INVALIDATED
    assert_eq!(s.record_post_seal_errata(&unit, "正解の取り違え", true, &t(7001)).unwrap(), 1);
    assert_eq!(s.evaluation_validity().unwrap(), EvaluationValidity::Invalidated { errata_seq: 1 });
    // sealed GT のバイト列は 1 バイトも変わっていない
    assert_eq!(std::fs::read(&ledger).unwrap(), before);
    assert_eq!(s.review_seal_sha256(), Some(seal.as_str()));
    // 不正な入力
    assert!(s.record_post_seal_errata("ru_nonexistent", "x", true, &t(7002)).is_err());
    assert!(s.record_post_seal_errata(&unit, "  ", true, &t(7002)).is_err());
    drop(s);

    // 再起動しても INVALIDATED のまま。errata の改変は検出される
    let again = reopen(&env, &art);
    assert_eq!(again.evaluation_validity().unwrap(), EvaluationValidity::Invalidated { errata_seq: 1 });
    drop(again);
    let ep = env.dir.join("errata.jsonl");
    let txt = std::fs::read_to_string(&ep).unwrap();
    std::fs::write(&ep, txt.replacen("\"promotion_impacting\":true", "\"promotion_impacting\":false", 1)).unwrap();
    let tampered = reopen(&env, &art);
    assert!(matches!(tampered.evaluation_validity(), Err(BlindError::Tampered(_))));
    // seal 前は errata を使えない（correction を使う）
    let env2 = Env::new("errata2");
    let s2 = new_store(&env2, &art, 25);
    assert!(s2.record_post_seal_errata(&unit, "x", true, &t(30)).is_err());
}

#[test]
fn hmac_matches_rfc4231_and_candidate_tokens_depend_on_every_input() {
    let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
    let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    let u1 = derive_review_unit_id(SECRET, "work:1").unwrap();
    let u2 = derive_review_unit_id(SECRET, "work:2").unwrap();
    let base = derive_candidate_token(SECRET, &u1, "movie", 603).unwrap();
    assert!(valid_token(&base));
    assert_eq!(base, derive_candidate_token(SECRET, &u1, "movie", 603).unwrap());
    assert_ne!(base, derive_candidate_token(SECRET, &u2, "movie", 603).unwrap());
    assert_ne!(base, derive_candidate_token(SECRET, &u1, "tv", 603).unwrap());
    assert_ne!(base, derive_candidate_token(SECRET, &u1, "movie", 604).unwrap());
    assert_ne!(base, derive_candidate_token(&"1".repeat(64), &u1, "movie", 603).unwrap());
    assert!(derive_candidate_token(SECRET, "ru_bad", "movie", 603).is_err());
    assert_eq!(CANDIDATE_TOKEN_DOMAIN, "pr4-pf0g-candidate-token-1");
}

#[test]
fn unit_key_must_be_work_colon_base10_without_zero_sign_or_space() {
    for ok in ["work:1", "work:42", "work:1000000"] {
        assert!(validate_unit_key(ok).is_ok(), "{ok}");
    }
    for bad in ["", "work:", "work:0", "work:01", "work:+1", "work:-1", "work: 1", "work:1 ", " work:1", "work:1.0", "Work:1", "work:1e3", "work:\u{FF11}", "1", "work:1234567890123456789"] {
        assert!(validate_unit_key(bad).is_err(), "{bad:?}");
        assert!(derive_review_unit_id(SECRET, bad).is_err(), "{bad:?}");
    }
}
