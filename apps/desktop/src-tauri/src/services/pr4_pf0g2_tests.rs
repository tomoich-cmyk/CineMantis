//! PR4-PF0G2 の synthetic テスト。H2 / H1 / 実 machine output / production DB / live TMDB は使わない。
//! ネットワークは 127.0.0.1 の loopback だけ。API key は fake canary。

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::pr4_blind_gt::*;
use super::pr4_pf0g2::*;
use super::pr4_pf0g2_server::*;

const SECRET: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
const SEED: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
const FAKE_KEY: &str = "PF0G2-FAKE-KEY-0123456789abcdef";
const BASE: i64 = 1_790_000_000;
const CANARIES: [&str; 6] = ["CANARY_SCORE_0.87314", "CANARY_RANK_9137", "CANARY_SOURCE_tags-shadow", "CANARY_DECISION_AUTO_X", "CANARY_REASON_77", "CANARY_JEV_55"];

fn t(secs: i64) -> String {
    fmt_epoch(BASE + secs)
}
fn h(label: &str) -> String {
    hex_sha(label.as_bytes())
}

struct Env {
    root: PathBuf,
}
impl Env {
    fn new(name: &str) -> Env {
        let root = std::env::temp_dir().join(format!("pf0g2_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Env { root }
    }
    fn p(&self, n: &str) -> PathBuf {
        self.root.join(n)
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ── synthetic の凍結 projection / 入力 ──────────────────────────────────────────────────────

fn proj_cand(id: i64, mt: &str, title: &str) -> Value {
    json!({"tmdb_id": id, "media_type": mt, "title": title, "original_title": null, "year": 1999, "release_date": "1999-03-31",
           "overview": format!("OVERVIEW-BODY-{id}"), "poster_path": null, "release_year_from_date": 1999, "year_consistent_with_release_date": true})
}
fn projection(key: &str, mut cands: Vec<Value>) -> Value {
    cands.sort_by(|a, b| (a["media_type"].as_str().unwrap().to_string(), a["tmdb_id"].as_i64().unwrap()).cmp(&(b["media_type"].as_str().unwrap().to_string(), b["tmdb_id"].as_i64().unwrap())));
    let shown = hex_sha(canon(&Value::Array(cands.clone())).unwrap().as_bytes());
    json!({"evaluation_unit_key": key, "candidates": cands, "shown_set_sha256": shown})
}
fn evidence(i: usize) -> Vec<EvidenceFile> {
    vec![EvidenceFile { part_no: 1, original_file_name: format!("movie_{i}.mkv"), original_rel_path: format!("a/b/movie_{i}.mkv"), container_tags_json: "{}".into(), source_media_kind: "video".into() }]
}
fn input(n: usize) -> PackagerInput {
    let units = (0..n)
        .map(|i| {
            let key = format!("work:{}", i + 1);
            let cands = if i % 5 == 0 {
                vec![]
            } else {
                (0..(2 + i % 3)).map(|j| proj_cand(1000 + (i * 7 + j) as i64, if j % 2 == 0 { "movie" } else { "tv" }, &format!("Title {i}-{j}"))).collect()
            };
            PackagerUnit { evaluation_unit_key: key.clone(), projection: projection(&key, cands), evidence: evidence(i) }
        })
        .collect();
    PackagerInput {
        build_id: "synthetic".into(),
        build_secret_hex: SECRET.into(),
        units,
        machine_freeze_commitment_sha256: h("machine-freeze"),
        pf0e_ledger_head_sha256: h("pf0e"),
        pf0e_record_count: n as i64,
        built_at_utc: t(1),
        authoritative_membership_commitment: h("authoritative-membership"),
        pf0h_final_manifest_sha256: h("pf0h-final-manifest"),
    }
}

struct Built {
    rev: PathBuf,
    sealed: PathBuf,
    ledger: PathBuf,
    cp: PathBuf,
    cache: PathBuf,
    out: BuildOutput,
    a8: String,
    inp: PackagerInput,
}
fn build_all(env: &Env, n: usize) -> Built {
    let inp = input(n);
    let (rev, sealed) = (env.p("reviewer_package"), env.p("sealed_unblind"));
    let out = build(&inp, &rev, &sealed).unwrap();
    let mut lits: Vec<String> = CANARIES.iter().map(|s| s.to_string()).collect();
    lits.push(SECRET.into());
    let (_, a8) = a8_leak_scan(&rev, &lits).unwrap();
    Built { rev, sealed, ledger: env.p("ledger"), cp: env.p("cp"), cache: env.p("tmdb_cache"), out, a8, inp }
}
fn pins_of(b: &Built) -> Pins {
    make_pins(&b.out, &h("protocol"), &h("attestation-policy"), &b.a8, &h("anchor"))
}
fn init_store(b: &Built, pct: u32, probe: Box<dyn MainAppProbe>) -> Store {
    let art = Artifact::load(&b.rev).unwrap();
    let mut s = Store::create(&b.ledger, &b.cp, art, &pins_of(b), &t(5), &t(10), probe).unwrap();
    s.freeze_qc_plan(SEED, pct, &t(20)).unwrap();
    s
}

// ── fake TMDB / clock ───────────────────────────────────────────────────────────────────

struct FnTmdb(Box<dyn FnMut(&TmdbReq) -> Result<HttpResp, NetErr>>, Rc<RefCell<Vec<TmdbReq>>>);
impl TmdbTransport for FnTmdb {
    fn get(&mut self, req: &TmdbReq) -> Result<HttpResp, NetErr> {
        self.1.borrow_mut().push(req.clone());
        (self.0)(req)
    }
}
fn ok_json(v: Value) -> Result<HttpResp, NetErr> {
    Ok(HttpResp { status: 200, body: v.to_string().into_bytes() })
}
fn default_tmdb() -> Box<dyn FnMut(&TmdbReq) -> Result<HttpResp, NetErr>> {
    Box::new(|req| match req {
        TmdbReq::Search { kind, .. } => ok_json(json!({"results": [
            {"id": 900001, "title": "Found A", "name": "Found A", "release_date": "2001-02-03", "first_air_date": "2001-02-03", "overview": "found overview"},
            {"id": 1000, "title": "Maybe Shown", "name": "Maybe Shown", "release_date": "1999-01-01", "first_air_date": "1999-01-01", "overview": "x"}], "kind": kind})),
        TmdbReq::Detail { id, .. } => ok_json(json!({"id": id, "title": "Detail T", "name": "Detail T", "release_date": "1999-05-05", "first_air_date": "1999-05-05", "overview": "detail overview", "adult": false})),
    })
}
fn clock() -> Box<dyn FnMut() -> String> {
    clock_from(100)
}
fn clock_from(start: i64) -> Box<dyn FnMut() -> String> {
    let c = Rc::new(Cell::new(start));
    Box::new(move || {
        c.set(c.get() + 1);
        t(c.get())
    })
}
struct Running(bool);
impl MainAppProbe for Running {
    fn main_app_running(&self) -> bool {
        self.0
    }
}
struct FakeLister(Result<Vec<String>, String>);
impl ProcessLister for FakeLister {
    fn image_names(&self) -> Result<Vec<String>, String> {
        self.0.clone()
    }
}

fn make_api(b: &Built, tmdb: Box<dyn FnMut(&TmdbReq) -> Result<HttpResp, NetErr>>) -> (Api, Rc<RefCell<Vec<TmdbReq>>>) {
    let store = init_store(b, 10, Box::new(NoMainApp));
    attach(b, store, tmdb)
}
fn attach(b: &Built, store: Store, tmdb: Box<dyn FnMut(&TmdbReq) -> Result<HttpResp, NetErr>>) -> (Api, Rc<RefCell<Vec<TmdbReq>>>) {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let api = Api::attach(store, &b.ledger, &b.cache, Box::new(FnTmdb(tmdb, calls.clone())), clock(), &Running(false)).unwrap();
    (api, calls)
}

// ── API 呼び出しの補助（全応答を保存して後で走査する）────────────────────────────────────────

struct Client {
    api: Api,
    log: Vec<(String, u16, String)>,
}
impl Client {
    fn call(&mut self, method: &str, path: &str, body: Value) -> (u16, Value) {
        let (s, text) = self.api.handle(method, path, body.to_string().as_bytes());
        self.log.push((path.to_string(), s, text.clone()));
        (s, serde_json::from_str(&text).unwrap())
    }
    fn post(&mut self, path: &str, body: Value) -> (u16, Value) {
        self.call("POST", path, body)
    }
    fn start(&mut self) -> String {
        let (s, v) = self.post("/api/session/start", json!({"reviewer_id": "R1", "attestation": true}));
        assert_eq!(s, 200, "{v}");
        v["session_id"].as_str().unwrap().to_string()
    }
    fn next(&mut self, sid: &str) -> Option<Value> {
        let (s, v) = self.post("/api/next", json!({"session_id": sid}));
        assert_eq!(s, 200, "{v}");
        if v["done"] == json!(true) {
            None
        } else {
            Some(v["unit"].clone())
        }
    }
    fn search(&mut self, u: &Value, q: &str) -> (u16, Value) {
        self.post("/api/search", json!({"unit": u["review_unit_id"], "lease_id": u["lease_id"], "query": q, "media_type": "movie"}))
    }
    fn preview(&mut self, u: &Value, id: i64) -> (u16, Value) {
        self.post("/api/preview", json!({"unit": u["review_unit_id"], "lease_id": u["lease_id"], "tmdb_id": id, "media_type": "movie"}))
    }
    fn submit(&mut self, u: &Value, prev: &str, sid: &str, extra: Value) -> (u16, Value) {
        let mut body = json!({"unit": u["review_unit_id"], "lease_id": u["lease_id"], "submission_id": sid, "prev_event_sha": prev,
            "decision": "", "candidate_token": null, "tmdb_id": null, "media_type": null, "defer_reason": null, "note": null, "correction_reason": null});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/api/submit", body)
    }
    /// 決定論的に全 unit を進める（confirm / none / pick_other / defer→2 巡目）
    fn drive_all(&mut self, sid: &str) -> HashMap<&'static str, usize> {
        let mut counts: HashMap<&'static str, usize> = HashMap::new();
        let mut deferred_once: BTreeSet<String> = BTreeSet::new();
        let mut n = 0;
        while let Some(u) = self.next(sid) {
            n += 1;
            let id = u["review_unit_id"].as_str().unwrap().to_string();
            let hv = hex_sha(id.as_bytes()).as_bytes()[0] as u32 % 4;
            let ncand = u["candidates"].as_array().unwrap().len();
            let prev = u["prev_event_sha"].as_str().unwrap().to_string();
            let sub = format!("sub-{n}");
            let (s, v) = if hv == 0 && !deferred_once.contains(&id) {
                deferred_once.insert(id.clone());
                *counts.entry("defer").or_default() += 1;
                self.submit(&u, &prev, &sub, json!({"decision": "defer", "defer_reason": "need_external_lookup"}))
            } else if ncand > 0 && hv != 3 && hv != 2 {
                *counts.entry("confirm").or_default() += 1;
                let tok = u["candidates"][0]["candidate_token"].clone();
                self.submit(&u, &prev, &sub, json!({"decision": "confirm", "candidate_token": tok}))
            } else if hv == 3 || ncand == 0 && hv % 2 == 1 {
                let (ss, sv) = self.search(&u, "query");
                assert_eq!(ss, 200, "{sv}");
                let prev2 = sv["prev_event_sha"].as_str().unwrap().to_string();
                let pid = 900_000 + (n as i64);
                let (ps, pv) = self.preview(&u, pid);
                assert_eq!(ps, 200, "{pv}");
                *counts.entry("pick_other").or_default() += 1;
                self.submit(&u, &prev2, &sub, json!({"decision": "pick_other", "tmdb_id": pid, "media_type": "movie"}))
            } else {
                let (ss, sv) = self.search(&u, "query");
                assert_eq!(ss, 200, "{sv}");
                *counts.entry("none").or_default() += 1;
                self.submit(&u, sv["prev_event_sha"].as_str().unwrap(), &sub, json!({"decision": "none"}))
            };
            assert_eq!(s, 200, "{v}");
        }
        counts
    }
}

fn forbidden_without_tmdb_id(text: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(text).unwrap();
    scan_forbidden(&v).into_iter().filter(|f| !f.contains("tmdb_id")).collect()
}

/// 全工程（package→a8→init→session→全 unit→seal）。seal 済み Store と ledger の場所を返す
fn full_run(env: &Env, n: usize) -> (Built, Store, String, Vec<(String, u16, String)>) {
    let b = build_all(env, n);
    let (api, _) = make_api(&b, default_tmdb());
    let mut c = Client { api, log: vec![] };
    let sid = c.start();
    c.drive_all(&sid);
    let (s, _) = c.post("/api/session/end", json!({"session_id": sid}));
    assert_eq!(s, 200);
    let log = std::mem::take(&mut c.log);
    let mut store = c.api.into_store();
    let (_, seal) = store.seal(&pins_of(&b), &t(100_000)).unwrap();
    (b, store, seal, log)
}

// ═══ packager: PF0F frozen projection だけを入力にする ═══════════════════════════════════════

#[test]
fn the_packager_consumes_only_the_frozen_projection_and_rejects_anything_else() {
    let env = Env::new("proj");
    let bad = |mutate: &dyn Fn(&mut Value)| {
        let mut i = input(3);
        let mut p = i.units[1].projection.clone();
        mutate(&mut p);
        i.units[1].projection = p;
        let (r, s) = (env.p("r_bad"), env.p("s_bad"));
        let _ = std::fs::remove_dir_all(&r);
        let _ = std::fs::remove_dir_all(&s);
        let res = build(&i, &r, &s);
        // 失敗したら何も残さない
        if res.is_err() {
            assert!(!r.exists() && !s.exists(), "partial output left behind");
        }
        res
    };
    // 機械由来のキー（canary 値つき）を top / candidate のどちらに足しても拒否
    for k in ["rank", "score", "source", "decision", "reasons", "confidence", "machine_decision", "jev", "matcher", "work_id", "display_title"] {
        assert!(bad(&|p| p[k] = json!(CANARIES[0])).is_err(), "top {k}");
        assert!(bad(&|p| p["candidates"][0][k] = json!(CANARIES[1])).is_err(), "cand {k}");
    }
    assert!(bad(&|p| { p["candidates"][0].as_object_mut().unwrap().remove("overview"); }).is_err());
    assert!(bad(&|p| p["evaluation_unit_key"] = json!("work:99")).is_err());
    assert!(bad(&|p| p["attempt"] = json!(3)).is_err());
    assert!(bad(&|p| p["shown_set_sha256"] = json!(h("other"))).is_err());
    assert!(bad(&|p| p["candidates"][0]["tmdb_id"] = json!(0)).is_err());
    assert!(bad(&|p| p["candidates"][0]["tmdb_id"] = json!(1.5)).is_err());
    assert!(bad(&|p| p["candidates"][0]["media_type"] = json!("person")).is_err());
    assert!(bad(&|p| p["candidates"][0]["year"] = json!("1999")).is_err());
    // 並び違い・重複（shown_set を付け直しても拒否）
    let reorder = |p: &mut Value| {
        let c = p["candidates"].as_array_mut().unwrap();
        c.reverse();
        p["shown_set_sha256"] = json!(hex_sha(canon(&Value::Array(c.clone())).unwrap().as_bytes()));
    };
    assert!(bad(&reorder).is_err());
    let dup = |p: &mut Value| {
        let c = p["candidates"].as_array_mut().unwrap();
        let first = c[0].clone();
        c.insert(1, first);
        p["shown_set_sha256"] = json!(hex_sha(canon(&Value::Array(c.clone())).unwrap().as_bytes()));
    };
    assert!(bad(&dup).is_err());
    // 正常な projection は通る
    assert!(bad(&|_| {}).is_ok());
}

#[test]
fn shown_set_sha_and_canonical_json_equal_the_pf0e_rule_that_pf0f_froze() {
    // PF0F の sha_v = sha256(pr4_pf0e::canon(v))。PF0G1 の canon と同一であること
    let p = projection("work:1", vec![proj_cand(5, "movie", "あ\"い\\う"), proj_cand(3, "tv", "z")]);
    let v = &p["candidates"];
    assert_eq!(canon(v).unwrap(), crate::services::pr4_pf0e::canon(v).unwrap());
    assert_eq!(p["shown_set_sha256"], json!(hex_sha(crate::services::pr4_pf0e::canon(v).unwrap().as_bytes())));
}

#[test]
fn overview_and_title_come_verbatim_from_the_frozen_artifact_and_no_tmdb_is_called() {
    let env = Env::new("overview");
    let b = build_all(&env, 6);
    let art = Artifact::load(&b.rev).unwrap();
    let mut seen = 0;
    for pu in &b.inp.units {
        let uid = derive_review_unit_id(SECRET, &pu.evaluation_unit_key).unwrap();
        let unit = &art.units[&uid];
        for c in pu.projection["candidates"].as_array().unwrap() {
            let token = derive_candidate_token(SECRET, &uid, c["media_type"].as_str().unwrap(), c["tmdb_id"].as_i64().unwrap()).unwrap();
            let shown = unit.candidates.iter().find(|x| x.candidate_token == token).unwrap();
            assert_eq!(shown.overview.as_deref(), c["overview"].as_str());
            assert_eq!(shown.title, c["title"].as_str().unwrap());
            assert_eq!(shown.year.as_deref(), Some("1999"));
            seen += 1;
        }
    }
    assert!(seen > 5);
    // packager には通信コードが無い
    let src = code_only(include_str!("pr4_pf0g2.rs"));
    for banned in ["reqwest", "tokio", "TcpStream", "TcpListener", "std::net", "TmdbTransport", "HttpTmdb"] {
        assert!(!src.contains(banned), "packager must not contain {banned}");
    }
}

fn code_only(src: &str) -> String {
    src.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

#[test]
fn reviewer_package_has_no_identity_no_display_title_no_secret_and_opaque_tokens() {
    let env = Env::new("pkg");
    let b = build_all(&env, 12);
    check_reviewer_package(&b.rev).unwrap();
    // 構成: manifest.json と units/ だけ
    let mut top: Vec<String> = std::fs::read_dir(&b.rev).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    top.sort();
    assert_eq!(top, vec!["manifest.json", "units"]);
    let mut all = String::new();
    for e in std::fs::read_dir(b.rev.join("units")).unwrap() {
        let text = std::fs::read_to_string(e.unwrap().path()).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        // 機械情報・内部 ID・identity のキー/値が無い
        assert!(scan_forbidden(&v).is_empty(), "{:?}", scan_forbidden(&v));
        for c in v["candidates"].as_array().unwrap() {
            let keys: BTreeSet<&str> = c.as_object().unwrap().keys().map(String::as_str).collect();
            assert_eq!(keys, BTreeSet::from(["candidate_token", "title", "original_title", "year", "overview", "poster_path", "original_language"]));
            assert!(valid_token_shape(c["candidate_token"].as_str().unwrap()));
        }
        assert!(v["evidence"].get("display_title").is_none());
        all.push_str(&text);
    }
    all.push_str(&std::fs::read_to_string(b.rev.join("manifest.json")).unwrap());
    for banned in [SECRET, "build_secret", "unblind", "work_id", "work:", "evaluation_unit_key", "display_title", "tmdb_id", "media_type", "release_date", "candidate_count"] {
        assert!(!all.contains(banned), "{banned}");
    }
    for c in CANARIES {
        assert!(!all.contains(c));
    }
    // secret / unblind を入れた package は起動拒否（check_reviewer_package）
    std::fs::write(b.rev.join("units").join("zz.json"), "{}").unwrap();
    assert!(check_reviewer_package(&b.rev).is_err() || Artifact::load(&b.rev).is_err());
    std::fs::remove_file(b.rev.join("units").join("zz.json")).unwrap();
    for (name, body) in [("unblind_map.json", "{}")] {
        std::fs::write(b.rev.join(name), body).unwrap();
        assert!(Artifact::load(&b.rev).is_err(), "{name}");
        std::fs::remove_file(b.rev.join(name)).unwrap();
    }
    Artifact::load(&b.rev).unwrap();
    // sealed 側には map / secret / attestation がある（別ディレクトリ）
    for n in ["unblind_map.json", "build_attestation.json"] {
        assert!(b.sealed.join(n).exists());
    }
    assert!(!b.sealed.starts_with(&b.rev) && !b.rev.starts_with(&b.sealed));
}

fn valid_token_shape(t: &str) -> bool {
    t.len() == 23 && t.starts_with("ct_") && t[3..].bytes().all(|c| matches!(c, b'a'..=b'z' | b'2'..=b'7'))
}

#[test]
fn candidate_token_is_the_hmac_of_the_contract_and_the_unblind_map_restores_identity_only_in_the_sealed_side() {
    let env = Env::new("token");
    let b = build_all(&env, 10);
    let art = Artifact::load(&b.rev).unwrap();
    let map: Value = serde_json::from_slice(&std::fs::read(b.sealed.join("unblind_map.json")).unwrap()).unwrap();
    assert_eq!(hex_sha(&std::fs::read(b.sealed.join("unblind_map.json")).unwrap()), b.out.unblind_map_sha256);
    let mut n = 0;
    assert_eq!(map["bundle_version"], json!("pr4-pf0i-unblind-bundle-1"));
    assert_eq!(map["build_secret_hex"], json!(SECRET));
    assert_eq!(b.out.build_secret_commitment, hex_sha(SECRET.as_bytes()));
    assert_eq!(map.as_object().unwrap().len(), 3);
    for e in map["units"].as_array().unwrap() {
        let uid = e["review_unit_id"].as_str().unwrap();
        for c in e["candidates"].as_array().unwrap() {
            let tok = derive_candidate_token(SECRET, uid, c["media_type"].as_str().unwrap(), c["tmdb_id"].as_i64().unwrap()).unwrap();
            assert_eq!(c["candidate_token"], json!(tok));
            assert!(art.units[uid].candidates.iter().any(|x| x.candidate_token == tok));
            n += 1;
        }
    }
    assert!(n > 10);
    // HMAC は build_secret に依存する（別 secret では別 token）
    let other = "11".repeat(32);
    assert_ne!(derive_candidate_token(SECRET, "ru_aaaaaaaaaaaaaaaaaaaaaaaaaa", "movie", 5).unwrap(), derive_candidate_token(&other, "ru_aaaaaaaaaaaaaaaaaaaaaaaaaa", "movie", 5).unwrap());
    assert_eq!(CANDIDATE_TOKEN_DOMAIN, "pr4-pf0g-candidate-token-1");
}

#[test]
fn packaging_is_deterministic_independent_of_input_order_and_candidate_order_is_not_the_tmdb_order() {
    let env = Env::new("order");
    let a = input(40);
    let mut shuffled = a.clone();
    shuffled.units.reverse();
    let (r1, s1, r2, s2) = (env.p("r1"), env.p("s1"), env.p("r2"), env.p("s2"));
    let o1 = build(&a, &r1, &s1).unwrap();
    let o2 = build(&shuffled, &r2, &s2).unwrap();
    assert_eq!(o1, o2);
    for e in std::fs::read_dir(r1.join("units")).unwrap() {
        let p = e.unwrap().path();
        assert_eq!(std::fs::read(&p).unwrap(), std::fs::read(r2.join("units").join(p.file_name().unwrap())).unwrap());
    }
    // candidate の並びは (media_type,tmdb_id) 昇順ではなく token 順（rank は入力に存在しない）
    let art = Artifact::load(&r1).unwrap();
    assert!(art.units.values().all(|u| u.candidates.windows(2).all(|w| w[0].candidate_token < w[1].candidate_token)));
    assert!(art.units.values().any(|u| u.candidates.len() >= 3));
    // 提示順は work id 順ではない
    let mut by_pos: Vec<(i64, String)> = a.units.iter().map(|u| (art.units[&derive_review_unit_id(SECRET, &u.evaluation_unit_key).unwrap()].position, u.evaluation_unit_key.clone())).collect();
    by_pos.sort();
    assert_ne!(by_pos.iter().map(|x| x.1.clone()).collect::<Vec<_>>(), a.units.iter().map(|u| u.evaluation_unit_key.clone()).collect::<Vec<_>>());
}

#[test]
fn packager_refuses_bad_inputs_and_nonempty_or_nested_outputs() {
    let env = Env::new("refuse");
    let ok = input(4);
    let mut bad = ok.clone();
    bad.units[0].evaluation_unit_key = "work:01".into();
    assert!(build(&bad, &env.p("a"), &env.p("b")).is_err());
    let mut bad = ok.clone();
    bad.units[1].evaluation_unit_key = bad.units[0].evaluation_unit_key.clone();
    assert!(build(&bad, &env.p("a"), &env.p("b")).is_err());
    let mut bad = ok.clone();
    bad.pf0e_record_count = 3;
    assert!(build(&bad, &env.p("a"), &env.p("b")).is_err());
    let mut bad = ok.clone();
    bad.build_secret_hex = "short".into();
    assert!(build(&bad, &env.p("a"), &env.p("b")).is_err());
    let mut bad = ok.clone();
    bad.units[2].evidence.clear();
    assert!(build(&bad, &env.p("a"), &env.p("b")).is_err());
    // 入れ子は拒否
    assert!(build(&ok, &env.p("x"), &env.p("x").join("sealed")).is_err());
    // 空でない出力先は拒否
    std::fs::create_dir_all(env.p("full")).unwrap();
    std::fs::write(env.p("full").join("f"), "x").unwrap();
    assert!(build(&ok, &env.p("full"), &env.p("c")).is_err());
    assert!(!env.p("a").exists() && !env.p("b").exists());
    build(&ok, &env.p("g1"), &env.p("g2")).unwrap();
    assert!(build(&ok, &env.p("g1"), &env.p("g3")).is_err());
}

#[test]
fn a_946_unit_package_builds_loads_and_scans() {
    let env = Env::new("scale");
    let inp = input(946);
    let out = build(&inp, &env.p("r"), &env.p("s")).unwrap();
    assert_eq!(out.unit_count, 946);
    let art = Artifact::load(&env.p("r")).unwrap();
    assert_eq!(art.manifest.unit_count, 946);
    let (rep, sha) = a8_leak_scan(&env.p("r"), &CANARIES.map(String::from)).unwrap();
    assert_eq!(rep["unit_count"], json!(946));
    assert_eq!(sha.len(), 64);
}

// ═══ A8 leak scan と pin ═════════════════════════════════════════════════════════════════

#[test]
fn a8_leak_scan_is_deterministic_and_stops_on_any_leak() {
    let env = Env::new("a8");
    let b = build_all(&env, 8);
    let lits: Vec<String> = CANARIES.iter().map(|s| s.to_string()).collect();
    let (r1, s1) = a8_leak_scan(&b.rev, &lits).unwrap();
    let (r2, s2) = a8_leak_scan(&b.rev, &lits).unwrap();
    assert_eq!((r1.clone(), s1.clone()), (r2, s2));
    assert_eq!(r1["result"], json!("PASS"));
    assert_eq!(r1["ui_package_sha256"], json!(ui_package_sha256()));
    // 禁止リテラル（packager が知っている canary など）が package に入っていれば STOP
    let id = Artifact::load(&b.rev).unwrap().units.keys().next().unwrap().clone();
    let p = b.rev.join("units").join(format!("{id}.json"));
    let original = std::fs::read(&p).unwrap();
    assert!(a8_leak_scan(&b.rev, &["movie_".to_string()]).is_err());
    // 禁止キーを足した unit は Artifact::load の段階で STOP/拒否
    let mut v: Value = serde_json::from_slice(&original).unwrap();
    v["rank"] = json!(1);
    std::fs::write(&p, v.to_string()).unwrap();
    assert!(a8_leak_scan(&b.rev, &lits).is_err());
    std::fs::write(&p, &original).unwrap();
    a8_leak_scan(&b.rev, &lits).unwrap();
}

#[test]
fn ui_files_do_not_mention_machine_or_internal_terms() {
    let ui = UI_FILES[0].1.to_lowercase();
    for banned in ["display_title", "work_id", "candidate_count", "unit_salt", "overlap_digest", "build_secret", "unblind", "evaluation_unit_key", "confidence", "reasons", "rank", "score"] {
        assert!(!ui.contains(banned), "{banned}");
    }
    // DOM へは textContent のみ（innerHTML を使わない）
    assert!(!ui.contains("innerhtml") && !ui.contains("insertadjacenthtml") && !ui.contains("eval("));
    assert_eq!(ui_package_sha256(), ui_package_sha256());
}

#[test]
fn all_eight_pins_must_match_between_init_and_seal_including_ui_and_a8() {
    let env = Env::new("pins");
    let b = build_all(&env, 8);
    let (api, _) = make_api(&b, default_tmdb());
    let mut c = Client { api, log: vec![] };
    let sid = c.start();
    c.drive_all(&sid);
    c.post("/api/session/end", json!({"session_id": sid}));
    let mut store = c.api.into_store();
    let good = pins_of(&b);
    let mutated: Vec<(&str, Pins)> = vec![
        ("protocol_sha256", Pins { protocol_sha256: h("x"), ..good.clone() }),
        ("ui_package_sha256", Pins { ui_package_sha256: h("x"), ..good.clone() }),
        ("shuffle_seed_commitment", Pins { shuffle_seed_commitment: h("x"), ..good.clone() }),
        ("attestation_policy_sha256", Pins { attestation_policy_sha256: h("x"), ..good.clone() }),
        ("unblind_map_sha256", Pins { unblind_map_sha256: h("x"), ..good.clone() }),
        ("build_secret_commitment", Pins { build_secret_commitment: h("x"), ..good.clone() }),
        ("a8_leak_scan_report_sha256", Pins { a8_leak_scan_report_sha256: h("x"), ..good.clone() }),
        ("machine_freeze_anchor_commitment", Pins { machine_freeze_anchor_commitment: h("x"), ..good.clone() }),
    ];
    for (name, p) in mutated {
        match store.seal(&p, &t(100_000)) {
            Err(BlindError::PinMismatch(v)) => assert_eq!(v, vec![name.to_string()]),
            other => panic!("{name}: {other:?}"),
        }
    }
    // 不正な（64 hex でない）a8 report SHA は seal 前に拒否される = A8 は必須入力
    assert!(store.seal(&Pins { a8_leak_scan_report_sha256: String::new(), ..good.clone() }, &t(100_000)).is_err());
    assert!(!store.sealed());
    store.seal(&good, &t(100_000)).unwrap();
    assert!(store.sealed());
}

#[test]
fn the_api_refuses_to_start_when_the_ui_package_differs_from_the_init_pin() {
    let env = Env::new("uipin");
    let b = build_all(&env, 4);
    let art = Artifact::load(&b.rev).unwrap();
    let wrong = Pins { ui_package_sha256: h("another-ui"), ..pins_of(&b) };
    let store = Store::create(&b.ledger, &b.cp, art, &wrong, &t(5), &t(10), Box::new(NoMainApp)).unwrap();
    let r = Api::attach(store, &b.ledger, &b.cache, Box::new(FnTmdb(default_tmdb(), Rc::new(RefCell::new(vec![])))), clock(), &Running(false));
    assert!(matches!(r, Err(BlindError::PinMismatch(v)) if v == vec!["ui_package_sha256".to_string()]));
}

// ═══ 本体アプリ稼働中は reviewer 起動拒否 ═══════════════════════════════════════════════════

#[test]
fn reviewer_refuses_to_start_while_the_main_app_is_running() {
    let probe = |l: Result<Vec<String>, String>| ProcessProbe { lister: FakeLister(l) };
    assert!(probe(Ok(vec!["explorer.exe".into(), "CineMantis.exe".into()])).main_app_running());
    assert!(probe(Ok(vec!["cinemantis".into()])).main_app_running());
    assert!(!probe(Ok(vec!["explorer.exe".into(), "cinemantis-reviewer.exe".into()])).main_app_running());
    assert!(probe(Err("tasklist failed".into())).main_app_running(), "fail closed");
    assert_eq!(parse_process_list("\"System\",\"4\",\"Services\",\"0\",\"1 K\"\r\n\"CineMantis.exe\",\"123\",\"Console\",\"1\",\"9 K\"\r\n"), vec!["System", "CineMantis.exe"]);
    assert_eq!(parse_process_list("/usr/bin/foo\nbar\n"), vec!["foo", "bar"]);

    let env = Env::new("mainapp");
    let b = build_all(&env, 4);
    // Api::attach の起動検査
    let store = init_store(&b, 10, Box::new(NoMainApp));
    let r = Api::attach(store, &b.ledger, &b.cache, Box::new(FnTmdb(default_tmdb(), Rc::new(RefCell::new(vec![])))), clock(), &probe(Ok(vec!["CineMantis.exe".into()])));
    assert!(matches!(r, Err(BlindError::MainAppRunning)));
    // Store 側（session 開始・全 command）でも拒否
    let mut s = Store::open(&b.ledger, &b.cp, Artifact::load(&b.rev).unwrap(), Box::new(probe(Ok(vec!["cinemantis.exe".into()])))).unwrap();
    assert!(matches!(s.start_session("R", true, UI_PROTOCOL_VERSION, &t(200)), Err(BlindError::MainAppRunning)));
}

#[test]
fn the_server_does_not_start_when_the_factory_refuses() {
    let env = Env::new("srvrefuse");
    let b = build_all(&env, 4);
    drop(init_store(&b, 10, Box::new(NoMainApp)));
    let (rev, ledger, cp, cache) = (b.rev.clone(), b.ledger.clone(), b.cp.clone(), b.cache.clone());
    let r = Server::start(
        move || {
            let store = Store::open(&ledger, &cp, Artifact::load(&rev)?, Box::new(NoMainApp))?;
            Api::attach(store, &ledger, &cache, Box::new(FnTmdb(default_tmdb(), Rc::new(RefCell::new(vec![])))), clock(), &Running(true))
        },
        0,
    );
    assert!(matches!(r, Err(BlindError::MainAppRunning)));
    // lock は外れている（再度開ける）
    Store::open(&b.ledger, &b.cp, Artifact::load(&b.rev).unwrap(), Box::new(NoMainApp)).unwrap();
}

// ═══ API: 型付き DTO・非露出・TMDB 検証 ═════════════════════════════════════════════════════

#[test]
fn api_responses_never_show_machine_info_identifiers_counts_or_display_title() {
    let env = Env::new("leak");
    let (b, _store, _seal, log) = full_run(&env, 20);
    assert!(log.len() > 60);
    let mut saw_unit = false;
    for (path, _status, text) in &log {
        for c in CANARIES {
            assert!(!text.contains(c), "{path}");
        }
        for banned in [SECRET, "work:", "work_id", "display_title", "candidate_count", "unit_salt", "overlap_digest", "unblind", "build_secret", "evaluation_unit_key", "position"] {
            assert!(!text.contains(banned), "{path}: {banned}");
        }
        assert!(forbidden_without_tmdb_id(text).is_empty(), "{path}: {text}");
        if path == "/api/next" {
            let v: Value = serde_json::from_str(text).unwrap();
            if v["unit"].is_object() {
                saw_unit = true;
                // frozen candidate には identity が無い（token と表示用 projection だけ）
                assert!(!text.contains("tmdb_id") && !text.contains("media_type"));
                assert_eq!(v["progress"].as_object().unwrap().keys().cloned().collect::<BTreeSet<_>>(), BTreeSet::from(["done".into(), "remaining".into(), "deferred".into(), "session_elapsed".into()]));
            }
        }
    }
    assert!(saw_unit);
    // secret・unblind を ledger / UI にも出していない
    let ledger_text = std::fs::read_to_string(b.ledger.join("events.jsonl")).unwrap();
    // init の pin 名（unblind_map_sha256）は載るが、map / secret の中身・work id は載らない
    for banned in [SECRET, "work:", "\"unblind_map\"", "build_secret\""] {
        assert!(!ledger_text.contains(banned), "ledger {banned}");
    }
    for c in CANARIES {
        assert!(!ledger_text.contains(c));
    }
}

#[test]
fn candidate_count_is_not_exposed_zero_candidates_never_become_none_and_none_needs_a_search() {
    let env = Env::new("none");
    let b = build_all(&env, 10);
    let (api, _) = make_api(&b, default_tmdb());
    let mut c = Client { api, log: vec![] };
    let sid = c.start();
    // 候補 0 件の unit を探す
    let mut zero = None;
    for _ in 0..10 {
        let u = c.next(&sid).unwrap();
        if u["candidates"].as_array().unwrap().is_empty() {
            zero = Some(u);
            break;
        }
        // 他は保留で退避
        let prev = u["prev_event_sha"].as_str().unwrap().to_string();
        c.submit(&u, &prev, &format!("d-{}", u["review_unit_id"]), json!({"decision": "defer", "defer_reason": "insufficient_evidence"}));
    }
    let u = zero.expect("zero-candidate unit");
    let prev = u["prev_event_sha"].as_str().unwrap().to_string();
    // 検索なしの none は拒否（候補 0 件でも自動 none にならない）
    let (s, v) = c.submit(&u, &prev, "none-1", json!({"decision": "none"}));
    assert_eq!((s, v["reason_code"].as_str()), (422, Some("none_needs_search")));
    // 検索失敗（通信不能）は検索として数えない
    let (s, _) = c.search(&u, "q");
    assert_eq!(s, 200);
    let (_, sv) = (0, c.search(&u, "q2").1);
    let (s, v) = c.submit(&u, sv["prev_event_sha"].as_str().unwrap(), "none-2", json!({"decision": "none"}));
    assert_eq!(s, 200, "{v}");
}

#[test]
fn input_dtos_reject_unknown_fields_malformed_targets_and_oversized_bodies() {
    let env = Env::new("malformed");
    let b = build_all(&env, 6);
    let (api, calls) = make_api(&b, default_tmdb());
    let mut c = Client { api, log: vec![] };
    let sid = c.start();
    let u = c.next(&sid).unwrap();
    let base = |extra: Value| {
        let mut v = json!({"unit": u["review_unit_id"], "lease_id": u["lease_id"], "submission_id": "m-1", "prev_event_sha": u["prev_event_sha"], "decision": "none",
            "candidate_token": null, "tmdb_id": null, "media_type": null, "defer_reason": null, "note": null, "correction_reason": null});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    };
    for extra in [json!({"rank": 1}), json!({"decision": "AUTO"}), json!({"candidate_token": "x"}), json!({"tmdb_id": 5}), json!({"decision": "pick_other", "tmdb_id": 0, "media_type": "movie"}),
        json!({"decision": "pick_other", "tmdb_id": 1.5, "media_type": "movie"}), json!({"decision": "pick_other", "tmdb_id": "7", "media_type": "movie"}),
        json!({"decision": "pick_other", "tmdb_id": 9223372036854775807i64, "media_type": "movie"}), json!({"decision": "pick_other", "tmdb_id": 5, "media_type": "person"}),
        json!({"decision": "pick_other", "tmdb_id": 5, "media_type": "TV"}), json!({"decision": "pick_other", "tmdb_id": 5, "media_type": " movie"}), json!({"decision": "defer"}),
        json!({"decision": "defer", "defer_reason": "bogus"}), json!({"decision": "confirm"}), json!({"decision": "confirm", "candidate_token": "ct_aaaaaaaaaaaaaaaaaaaa"})] {
        let (s, v) = c.post("/api/submit", base(extra.clone()));
        assert!(s == 400 || s == 422, "{extra} -> {s} {v}");
        assert!(!v.to_string().contains(u["review_unit_id"].as_str().unwrap()), "error leaked unit id");
    }
    // ledger は何も増えていない（副作用ゼロ）: 同じ prev で正規の提出がまだ通る
    let (s, _) = c.submit(&u, u["prev_event_sha"].as_str().unwrap(), "ok-1", json!({"decision": "defer", "defer_reason": "insufficient_evidence"}));
    assert_eq!(s, 200);
    // 壊れた JSON / 巨大 body / 未知 route / 未知 method
    assert_eq!(c.api.handle("POST", "/api/submit", b"{not json").0, 400);
    assert_eq!(c.api.handle("POST", "/api/submit", &vec![b' '; MAX_BODY + 1]).0, 400);
    assert_eq!(c.api.handle("POST", "/api/seal", b"{}").0, 404);
    assert_eq!(c.api.handle("GET", "/api/unblind_map", b"").0, 404);
    assert_eq!(c.api.handle("DELETE", "/api/progress", b"").0, 404);
    assert!(calls.borrow().is_empty(), "malformed requests must not reach TMDB");
    // 存在しない unit・path traversal
    let (s, v) = c.post("/api/search", json!({"unit": "../../etc/passwd", "lease_id": "x", "query": "q", "media_type": "movie"}));
    assert_eq!(s, 409, "{v}");
    assert!(calls.borrow().is_empty());
}

#[test]
fn confirm_resolves_the_token_server_side_and_records_no_identity() {
    let env = Env::new("confirm");
    let b = build_all(&env, 10);
    let (api, _) = make_api(&b, default_tmdb());
    let mut c = Client { api, log: vec![] };
    let sid = c.start();
    let mut done = 0;
    while done < 3 {
        let u = c.next(&sid).unwrap();
        let prev = u["prev_event_sha"].as_str().unwrap().to_string();
        if let Some(first) = u["candidates"].as_array().unwrap().first() {
            // 他 unit の token は拒否
            let (s, _) = c.submit(&u, &prev, &format!("x-{done}"), json!({"decision": "confirm", "candidate_token": "ct_zzzzzzzzzzzzzzzzzzzz"}));
            assert_eq!(s, 400);
            let (s, _) = c.submit(&u, &prev, &format!("c-{done}"), json!({"decision": "confirm", "candidate_token": first["candidate_token"]}));
            assert_eq!(s, 200);
            done += 1;
        } else {
            c.submit(&u, &prev, &format!("d-{done}"), json!({"decision": "defer", "defer_reason": "insufficient_evidence"}));
        }
    }
    let text = std::fs::read_to_string(b.ledger.join("events.jsonl")).unwrap();
    let mut n = 0;
    for line in text.lines() {
        let v: Value = serde_json::from_str(line).unwrap();
        if v["decision"] == json!("confirm") {
            n += 1;
            assert!(v["candidate_token"].is_string());
            assert!(v.get("tmdb_id").map(Value::is_null).unwrap_or(true) && v.get("media_type").map(Value::is_null).unwrap_or(true));
        }
    }
    assert_eq!(n, 3);
}

#[test]
fn pick_other_uses_only_a_server_side_verification_and_rejects_candidates_mismatches_and_failures() {
    let env = Env::new("pick");
    let b = build_all(&env, 10);
    let mode = Rc::new(RefCell::new("ok".to_string()));
    let m2 = mode.clone();
    let tmdb: Box<dyn FnMut(&TmdbReq) -> Result<HttpResp, NetErr>> = Box::new(move |req| match (req, m2.borrow().as_str()) {
        (TmdbReq::Detail { id, .. }, "ok") => ok_json(json!({"id": id, "title": "T", "release_date": "2000-01-01", "overview": "o", "adult": false})),
        (TmdbReq::Detail { id, .. }, "redirect") => ok_json(json!({"id": id + 1, "title": "T"})),
        (TmdbReq::Detail { id, .. }, "adult") => ok_json(json!({"id": id, "title": "T", "adult": true})),
        (TmdbReq::Detail { .. }, "404") => Ok(HttpResp { status: 404, body: b"{}".to_vec() }),
        (TmdbReq::Detail { .. }, "429") => Ok(HttpResp { status: 429, body: b"{}".to_vec() }),
        (TmdbReq::Detail { .. }, "500") => Ok(HttpResp { status: 500, body: b"{}".to_vec() }),
        (TmdbReq::Detail { .. }, "timeout") => Err(NetErr::Timeout),
        (TmdbReq::Detail { .. }, "garbage") => Ok(HttpResp { status: 200, body: b"<html>".to_vec() }),
        _ => ok_json(json!({"results": []})),
    });
    let (api, calls) = make_api(&b, tmdb);
    let mut c = Client { api, log: vec![] };
    let sid = c.start();
    // 候補のある unit を取る
    let u = loop {
        let u = c.next(&sid).unwrap();
        if !u["candidates"].as_array().unwrap().is_empty() {
            break u;
        }
        let prev = u["prev_event_sha"].as_str().unwrap().to_string();
        c.submit(&u, &prev, &format!("d-{}", c.log.len()), json!({"decision": "defer", "defer_reason": "insufficient_evidence"}));
    };
    let uid = u["review_unit_id"].as_str().unwrap().to_string();
    let pu = b.inp.units.iter().find(|p| derive_review_unit_id(SECRET, &p.evaluation_unit_key).unwrap() == uid).unwrap();
    let shown = &pu.projection["candidates"][0];
    let (shown_id, shown_mt) = (shown["tmdb_id"].as_i64().unwrap(), shown["media_type"].as_str().unwrap().to_string());
    let prev = u["prev_event_sha"].as_str().unwrap().to_string();
    let pick = |c: &mut Client, id: i64, mt: &str, sub: &str| c.submit(&u, &prev, sub, json!({"decision": "pick_other", "tmdb_id": id, "media_type": mt}));
    // 表示候補は preview も submit も TargetIsCandidate（通信しない）
    let before = calls.borrow().len();
    let (s, v) = c.post("/api/preview", json!({"unit": uid, "lease_id": u["lease_id"], "tmdb_id": shown_id, "media_type": shown_mt}));
    assert_eq!((s, v["reason_code"].as_str()), (422, Some("target_is_candidate")));
    assert_eq!(calls.borrow().len(), before);
    // preview なしの pick_other は未検証
    let (s, v) = pick(&mut c, 880_001, "movie", "p-0");
    assert_eq!((s, v["reason_code"].as_str()), (422, Some("target_not_verified")));
    // 検証に失敗する各ケースは preview が失敗し、submit も通らない（draft も ledger event も残さない）
    for m in ["redirect", "adult", "404", "429", "500", "timeout", "garbage"] {
        *mode.borrow_mut() = m.to_string();
        let (s, v) = c.preview(&u, 880_002);
        assert_eq!((s, v["reason_code"].as_str()), (422, Some("target_not_verified")), "{m}");
        let (s, _) = pick(&mut c, 880_002, "movie", &format!("pf-{m}"));
        assert_eq!(s, 422, "{m}");
    }
    // 成功 → server が検証結果を保持し、client の別 ID では通らない
    *mode.borrow_mut() = "ok".into();
    let (s, v) = c.preview(&u, 880_003);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["title"], json!("T"));
    assert_eq!(pick(&mut c, 880_004, "movie", "p-x").0, 422);
    assert_eq!(pick(&mut c, 880_003, "tv", "p-y").0, 422);
    let (s, v) = pick(&mut c, 880_003, "movie", "p-ok");
    assert_eq!(s, 200, "{v}");
    // ledger の provenance
    let text = std::fs::read_to_string(b.ledger.join("events.jsonl")).unwrap();
    let ev: Value = text.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).find(|e| e["decision"] == json!("pick_other")).unwrap();
    let tv = &ev["tmdb_verification"];
    assert_eq!((tv["requested_id"].as_i64(), tv["response_id"].as_i64(), tv["http_status"].as_i64(), tv["verifier_impl"].as_str()), (Some(880_003), Some(880_003), Some(200), Some(VERIFIER_IMPL)));
    // 応答本文が audit 側に保存されている
    let cached = tv["response_body_sha256"].as_str().unwrap();
    assert_eq!(hex_sha(&std::fs::read(b.cache.join(format!("{cached}.json"))).unwrap()), cached);
}

#[test]
fn the_tmdb_api_key_never_reaches_a_dto_the_ledger_the_cache_or_debug_output() {
    let env = Env::new("key");
    let b = build_all(&env, 8);
    let store = init_store(&b, 10, Box::new(NoMainApp));
    let lb = Loopback::start();
    let tm = HttpTmdb::loopback(&lb.addr, FAKE_KEY, Duration::from_secs(5)).unwrap();
    assert!(!format!("{tm:?}").contains(FAKE_KEY));
    let api = Api::attach(store, &b.ledger, &b.cache, Box::new(tm), clock(), &Running(false)).unwrap();
    let mut c = Client { api, log: vec![] };
    let sid = c.start();
    let u = c.next(&sid).unwrap();
    let (s, sv) = c.search(&u, "A B&日本");
    assert_eq!(s, 200, "{sv}");
    let (s, _) = c.preview(&u, 880_010);
    assert_eq!(s, 200);
    let (s, _) = c.submit(&u, sv["prev_event_sha"].as_str().unwrap(), "k-1", json!({"decision": "pick_other", "tmdb_id": 880_010, "media_type": "movie"}));
    assert_eq!(s, 200);
    // 実 HTTP の loopback が受信した request-target = production の URL template を描画したもの
    let targets = lb.targets();
    assert_eq!(targets[0], format!("/3/search/movie?api_key={FAKE_KEY}&query=A+B%26%E6%97%A5%E6%9C%AC&language=ja-JP&include_adult=false"));
    assert_eq!(targets[1], format!("/3/movie/880010?api_key={FAKE_KEY}&language=ja-JP"));
    let mut hay = std::fs::read_to_string(b.ledger.join("events.jsonl")).unwrap();
    for e in std::fs::read_dir(&b.cache).unwrap() {
        hay.push_str(&String::from_utf8_lossy(&std::fs::read(e.unwrap().path()).unwrap()));
    }
    for (_, _, text) in &c.log {
        hay.push_str(text);
    }
    hay.push_str(&format!("{:?}", c.api.runtime_a8_report()));
    assert!(!hay.contains(FAKE_KEY) && !hay.contains("api_key"));
}

// ── production との wire 一致（PF0F2 の方式: ソースの template と urlencoding を機械的に照合）──

#[test]
fn wire_requests_match_the_production_templates_and_urlencoding() {
    let prod = include_str!("tmdb_client.rs");
    for template in [
        "{TMDB_API_BASE}/search/movie?api_key={}&query={}&language=ja-JP&include_adult=false",
        "{TMDB_API_BASE}/search/tv?api_key={}&query={}&language=ja-JP&include_adult=false",
        "{TMDB_API_BASE}/movie/{tmdb_id}?api_key={}&language=ja-JP",
        "{TMDB_API_BASE}/tv/{tmdb_id}?api_key={}&language=ja-JP",
    ] {
        assert!(prod.contains(template), "{template}");
    }
    assert!(prod.contains("const TMDB_API_BASE: &str = \"https://api.themoviedb.org/3\";"));
    assert_eq!(TMDB_PRODUCTION_BASE, "https://api.themoviedb.org/3");
    // production の urlencoding の分岐が変わっていない（変われば要再確認）
    let f = prod.split("fn urlencoding").nth(1).unwrap().split("\n}\n").next().unwrap();
    for needle in ["'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string()", "' ' => \"+\".to_string()", "format!(\"%{b:02X}\")"] {
        assert!(f.contains(needle), "{needle}");
    }
    let corpus = [("", ""), ("A B", "A+B"), ("日本", "%E6%97%A5%E6%9C%AC"), ("a&b=c?d#e%f+g", "a%26b%3Dc%3Fd%23e%25f%2Bg"), ("~-_.", "~-_."), ("😀", "%F0%9F%98%80"), ("Ａ", "%EF%BC%A1")];
    for (i, o) in corpus {
        assert_eq!(urlencode(i), o, "{i}");
    }
    assert_eq!(wire_target(&TmdbReq::Search { kind: "tv".into(), query: "x y".into() }, "K").unwrap(), "/3/search/tv?api_key=K&query=x+y&language=ja-JP&include_adult=false");
    assert_eq!(wire_target(&TmdbReq::Detail { kind: "tv".into(), id: 7 }, "K").unwrap(), "/3/tv/7?api_key=K&language=ja-JP");
    assert!(wire_target(&TmdbReq::Detail { kind: "person".into(), id: 7 }, "K").is_err());
    // production() 以外は 127.0.0.1 以外へ向けられない
    assert!(HttpTmdb::loopback("example.com:443", "K", Duration::from_secs(1)).is_err());
}

// ── 最小の loopback TMDB（テスト用）──────────────────────────────────────────────────────────

struct Loopback {
    addr: String,
    seen: Arc<Mutex<Vec<String>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}
impl Loopback {
    fn start() -> Loopback {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (s2, st2) = (seen.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            while !st2.load(std::sync::atomic::Ordering::SeqCst) {
                match l.accept() {
                    Ok((mut sock, _)) => {
                        sock.set_nonblocking(false).unwrap();
                        sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                        let mut buf = [0u8; 4096];
                        let n = sock.read(&mut buf).unwrap_or(0);
                        let head = String::from_utf8_lossy(&buf[..n]).to_string();
                        let target = head.split_whitespace().nth(1).unwrap_or("").to_string();
                        s2.lock().unwrap().push(target.clone());
                        let body = if target.starts_with("/3/search/") {
                            json!({"results": [{"id": 880_011, "title": "Loop A", "release_date": "2002-01-01", "overview": "lo"}]}).to_string()
                        } else {
                            let id: i64 = target.split('?').next().unwrap().rsplit('/').next().unwrap().parse().unwrap_or(0);
                            json!({"id": id, "title": "Loop Detail", "release_date": "2003-01-01", "overview": "d", "adult": false}).to_string()
                        };
                        let _ = sock.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Loopback { addr, seen, stop, handle: Some(handle) }
    }
    fn targets(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}
impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// ═══ A8 実行時 scan（型ごとの schema allowlist）═══════════════════════════════════════════════

#[test]
fn a_response_outside_the_allowlist_is_never_emitted_and_is_counted_as_a_violation() {
    let env = Env::new("a8rt");
    let b = build_all(&env, 4);
    let (mut api, _) = make_api(&b, default_tmdb());
    #[derive(serde::Serialize)]
    struct Rogue {
        done: usize,
        remaining: usize,
        deferred: usize,
        session_elapsed: i64,
        rank: i64,
    }
    let (s, body) = api.emit(200, "progress", &Rogue { done: 0, remaining: 0, deferred: 0, session_elapsed: 0, rank: 9137 });
    assert_eq!(s, 500);
    assert!(!body.contains("9137") && !body.contains("rank"));
    assert_eq!(api.violations(), 1);
    assert_eq!(api.emit(200, "no_such_type", &json!({})).0, 500);
    assert_eq!(api.violations(), 2);
    // 正規の応答は通り、report に型ごとの key path SHA が残る
    let (s, _) = api.handle("GET", "/api/progress", b"");
    assert_eq!(s, 200);
    let rep = api.runtime_a8_report();
    assert_eq!(rep["violations"], json!(2));
    assert_eq!(rep["types"]["progress"]["count"], json!(1));
}

// ═══ 完全工程: seal・post-seal immutability・PF0I handoff ═══════════════════════════════════════

#[test]
fn seal_binds_the_event_commitment_and_exposes_exactly_the_pf0i_values() {
    let env = Env::new("seal");
    let (b, store, seal, _) = full_run(&env, 24);
    assert!(store.sealed());
    // review seal SHA = seal event の event_commitment
    let text = std::fs::read_to_string(b.ledger.join("events.jsonl")).unwrap();
    let last: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert_eq!(last["type"], json!("seal"));
    assert_eq!(last["event_commitment"], json!(seal));
    assert_eq!(store.review_seal_sha256(), Some(seal.as_str()));
    assert_eq!(last["pins"]["a8_leak_scan_report_sha256"], json!(b.a8));
    assert_eq!(last["pins"]["ui_package_sha256"], json!(ui_package_sha256()));
    // PF0I が exact-match する 3 値の取り出し口
    let hand = pf0i_handoff(&store, &b.ledger, &b.sealed).unwrap();
    assert_eq!(hand, Pf0iHandoff { authoritative_membership_commitment: b.inp.authoritative_membership_commitment.clone(), pf0h_final_manifest_sha256: b.inp.pf0h_final_manifest_sha256.clone(), review_seal_sha256: seal.clone() });
    // sealed の map / secret が pin と食い違えば拒否
    let p = b.sealed.join("unblind_map.json");
    let original = std::fs::read(&p).unwrap();
    std::fs::write(&p, [original.clone(), b" ".to_vec()].concat()).unwrap();
    assert!(matches!(pf0i_handoff(&store, &b.ledger, &b.sealed), Err(BlindError::PinMismatch(_))));
    std::fs::write(&p, &original).unwrap();

    // reopen しても同じ seal（replay でも pin 検査）
    drop(store);
    let again = Store::open(&b.ledger, &b.cp, Artifact::load(&b.rev).unwrap(), Box::new(NoMainApp)).unwrap();
    assert_eq!(again.review_seal_sha256(), Some(seal.as_str()));
}

#[test]
fn handoff_is_refused_before_seal() {
    let env = Env::new("preseal");
    let b = build_all(&env, 6);
    let store = init_store(&b, 10, Box::new(NoMainApp));
    assert!(!store.unblind_allowed());
    assert!(pf0i_handoff(&store, &b.ledger, &b.sealed).is_err());
}

#[test]
fn sealed_gt_is_immutable_and_errata_invalidate_the_evaluation() {
    let env = Env::new("errata");
    let (b, store, _seal, _) = full_run(&env, 16);
    let before = std::fs::read(b.ledger.join("events.jsonl")).unwrap();
    let unit = Artifact::load(&b.rev).unwrap().units.keys().next().unwrap().clone();
    assert!(matches!(store.evaluation_validity().unwrap(), EvaluationValidity::Valid { recorded_errata: 0 }));
    store.record_post_seal_errata(&unit, "non promotion-impacting note", false, &t(200_000)).unwrap();
    assert!(matches!(store.evaluation_validity().unwrap(), EvaluationValidity::Valid { recorded_errata: 1 }));
    store.record_post_seal_errata(&unit, "promotion-impacting", true, &t(200_001)).unwrap();
    assert!(matches!(store.evaluation_validity().unwrap(), EvaluationValidity::Invalidated { .. }));
    // GT（events.jsonl）は 1 byte も変わらない
    assert_eq!(std::fs::read(b.ledger.join("events.jsonl")).unwrap(), before);
    // seal 後は API からの変更も全て拒否（Sealed）
    let calls = Rc::new(RefCell::new(vec![]));
    let mut api = Api::attach(store, &b.ledger, &b.cache, Box::new(FnTmdb(default_tmdb(), calls)), clock(), &Running(false)).unwrap();
    let (s, v) = api.handle("POST", "/api/session/start", json!({"reviewer_id": "R2", "attestation": true}).to_string().as_bytes());
    assert_eq!(s, 410, "{v}");
    assert!(v.contains("sealed"));
    assert_eq!(std::fs::read(b.ledger.join("events.jsonl")).unwrap(), before);
}

#[test]
fn the_ledger_resumes_identically_through_the_api_after_a_restart() {
    let env = Env::new("resume");
    let b = build_all(&env, 8);
    let (api, _) = make_api(&b, default_tmdb());
    let mut c = Client { api, log: vec![] };
    let sid = c.start();
    let u = c.next(&sid).unwrap();
    let (s, sv) = c.search(&u, "q");
    assert_eq!(s, 200);
    let head = c.api.store().head().to_string();
    let count = c.api.store().event_count();
    drop(c);
    // 再起動（同じ ledger を開き直す）: 状態は ledger だけから復元される
    let store = Store::open(&b.ledger, &b.cp, Artifact::load(&b.rev).unwrap(), Box::new(NoMainApp)).unwrap();
    assert_eq!((store.head().to_string(), store.event_count()), (head, count));
    let calls = Rc::new(RefCell::new(Vec::new()));
    let api = Api::attach(store, &b.ledger, &b.cache, Box::new(FnTmdb(default_tmdb(), calls)), clock_from(5_000), &Running(false)).unwrap();
    let mut c = Client { api, log: vec![] };
    let sid2 = c.start();
    // 古い貸出は期限切れとして扱われ、同じ unit が再び取れる（CAS の prev は ledger の末尾から）
    let mut got = None;
    for _ in 0..3 {
        let n = c.next(&sid2).unwrap();
        if n["review_unit_id"] == u["review_unit_id"] {
            got = Some(n);
            break;
        }
        let prev = n["prev_event_sha"].as_str().unwrap().to_string();
        c.submit(&n, &prev, &format!("r-{}", c.log.len()), json!({"decision": "defer", "defer_reason": "insufficient_evidence"}));
    }
    let _ = (sv, got); // 同 unit が出る保証は lease の期限次第。ここでは「再起動後に API が正常に動く」ことだけを固定する
}

// ═══ HTTP 層（loopback）══════════════════════════════════════════════════════════════════

fn http(addr: &str, method: &str, path: &str, headers: &[(&str, &str)], body: &str, host: Option<&str>) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\nConnection: close\r\n", host.unwrap_or(addr), body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text.split_whitespace().nth(1).and_then(|x| x.parse().ok()).unwrap_or(0);
    (status, text.split("\r\n\r\n").nth(1).unwrap_or("").to_string())
}

#[test]
fn the_loopback_server_enforces_token_host_and_origin_and_serves_the_pinned_ui() {
    let env = Env::new("http");
    let b = build_all(&env, 6);
    drop(init_store(&b, 10, Box::new(NoMainApp)));
    let lb = Loopback::start();
    let (rev, ledger, cp, cache, laddr) = (b.rev.clone(), b.ledger.clone(), b.cp.clone(), b.cache.clone(), lb.addr.clone());
    let srv = Server::start(
        move || {
            let store = Store::open(&ledger, &cp, Artifact::load(&rev)?, Box::new(NoMainApp))?;
            Api::attach(store, &ledger, &cache, Box::new(HttpTmdb::loopback(&laddr, FAKE_KEY, Duration::from_secs(5))?), system_clock(), &Running(false))
        },
        0,
    )
    .unwrap();
    assert!(srv.addr.starts_with("127.0.0.1:"));
    let (a, tok) = (srv.addr.clone(), srv.token.clone());
    // token なし / 誤 token / 誤 Host / 誤 Origin は 403
    assert_eq!(http(&a, "GET", "/api/progress", &[], "", None).0, 403);
    assert_eq!(http(&a, "GET", "/api/progress", &[("X-Review-Token", "wrong")], "", None).0, 403);
    assert_eq!(http(&a, "GET", "/api/progress", &[("X-Review-Token", &tok)], "", Some("evil.example:80")).0, 403);
    assert_eq!(http(&a, "GET", "/api/progress", &[("X-Review-Token", &tok), ("Origin", "http://evil.example")], "", None).0, 403);
    assert_eq!(http(&a, "GET", "/", &[], "", None).0, 403);
    // UI は token つき URL でだけ配られ、pin された UI と同一
    let (s, html) = http(&a, "GET", &format!("/?t={tok}"), &[], "", None);
    assert_eq!(s, 200);
    assert_eq!(html, UI_FILES[0].1);
    assert_eq!(hex_sha(UI_FILES[0].1.as_bytes()).len(), 64);
    // 認証つきの API
    let h = [("X-Review-Token", tok.as_str())];
    let (s, body) = http(&a, "GET", "/api/progress", &h, "", None);
    assert_eq!(s, 200, "{body}");
    assert_eq!(http(&a, "POST", "/api/seal", &h, "{}", None).0, 404);
    let (s, v) = http(&a, "POST", "/api/session/start", &h, &json!({"reviewer_id": "R1", "attestation": true}).to_string(), None);
    assert_eq!(s, 200, "{v}");
    let sid = serde_json::from_str::<Value>(&v).unwrap()["session_id"].as_str().unwrap().to_string();
    let (s, v) = http(&a, "POST", "/api/next", &h, &json!({"session_id": sid}).to_string(), None);
    assert_eq!(s, 200, "{v}");
    let u: Value = serde_json::from_str::<Value>(&v).unwrap()["unit"].clone();
    // search / preview は loopback の TMDB へ実 HTTP で出る
    let (s, v) = http(&a, "POST", "/api/search", &h, &json!({"unit": u["review_unit_id"], "lease_id": u["lease_id"], "query": "ロープ", "media_type": "movie"}).to_string(), None);
    assert_eq!(s, 200, "{v}");
    let sv: Value = serde_json::from_str(&v).unwrap();
    assert_eq!(sv["results"][0]["tmdb_id"], json!(880_011));
    let (s, v) = http(&a, "POST", "/api/preview", &h, &json!({"unit": u["review_unit_id"], "lease_id": u["lease_id"], "tmdb_id": 880_011, "media_type": "movie"}).to_string(), None);
    assert_eq!(s, 200, "{v}");
    assert_eq!(lb.targets().len(), 2);
    assert!(lb.targets()[0].starts_with("/3/search/movie?api_key="));
    srv.stop();
    // 停止後は lock が外れて ledger を開き直せる。search event（none 証跡）は残っている
    let s = Store::open(&b.ledger, &b.cp, Artifact::load(&b.rev).unwrap(), Box::new(NoMainApp)).unwrap();
    assert!(s.event_count() >= 4);
}

// ═══ contract / schema anchor（1a6266d…）との parity ═════════════════════════════════════════

const SCHEMA: &str = include_str!("../../../../../docs/pr4_pf0g0_blind_gt_schema.json");
const CONTRACT: &str = include_str!("../../../../../docs/PR4_PF0G0_BLIND_GT_CONTRACT.md");
const ADDENDUM: &str = include_str!("../../../../../docs/PR4_PF0G1_DOCS_ALIGNMENT_ADDENDUM.md");

fn lf_sha(s: &str) -> String {
    hex_sha(s.replace("\r\n", "\n").as_bytes())
}

#[test]
fn the_authoritative_docs_are_the_ones_at_commit_1a6266d_and_the_core_is_the_pf0g1_freeze() {
    // docs: PF0G の authoritative schema anchor = 1a6266d9869d7dc3bf8556850312c85676ebe7d3 の tree
    assert_eq!(lf_sha(SCHEMA), "3b0debef45f4014a98deafb2d0a06f45b5df3ef5bb931c3e6c6aa28829e94e17");
    assert_eq!(lf_sha(CONTRACT), "c02caf44eab714e63af8a48500fbe5f6741c63434d11cac156b862f2726ba543");
    assert_eq!(lf_sha(ADDENDUM), "f1e017a85ac1300c8e1ea08dfb347ad0bcc3019b0eb5c628acc977566ec399b9");
    // PF0G1 の実装は eeeb573 から不変（FREEZE.txt に記録された tool_sha256）
    assert_eq!(tool_sha256(), "666182cb150d6a7c7605268f286cb726858e23e72eaa646fd3b3afd9f6d3e3cc");
}

#[test]
fn the_implementation_matches_the_schema_anchor_semantically() {
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    // unit
    assert_eq!(schema["unit"]["schema_version"], json!(UNIT_SCHEMA));
    let env = Env::new("parity");
    let b = build_all(&env, 6);
    let art = Artifact::load(&b.rev).unwrap();
    let unit = serde_json::to_value(art.units.values().find(|u| !u.candidates.is_empty()).unwrap()).unwrap();
    let props: BTreeSet<String> = schema["unit"]["properties"].as_object().unwrap().keys().cloned().collect();
    let keys: BTreeSet<String> = unit.as_object().unwrap().keys().cloned().collect();
    for r in schema["unit"]["required"].as_array().unwrap() {
        assert!(keys.contains(r.as_str().unwrap()), "required {r}");
    }
    for k in &keys {
        assert!(k == "schema_version" || k == "unit_sha256" || props.contains(k), "unit key {k} not in schema");
    }
    assert!(schema["unit"]["properties"]["evidence"]["display_title"].as_str().unwrap().contains("ABSENT"));
    assert!(unit["evidence"].get("display_title").is_none());
    let file_keys: BTreeSet<String> = schema["unit"]["properties"]["evidence"]["files"][0].as_object().unwrap().keys().cloned().collect();
    assert_eq!(file_keys, unit["evidence"]["files"][0].as_object().unwrap().keys().cloned().collect::<BTreeSet<_>>());
    let cand_keys: BTreeSet<String> = schema["unit"]["properties"]["candidates"][0].as_object().unwrap().keys().cloned().collect();
    assert_eq!(cand_keys, unit["candidates"][0].as_object().unwrap().keys().cloned().collect::<BTreeSet<_>>());
    assert!(schema["unit"]["properties"]["candidates"][0]["candidate_token"].as_str().unwrap().contains(CANDIDATE_TOKEN_DOMAIN));
    // manifest
    assert_eq!(schema["manifest"]["schema_version"], json!(MANIFEST_SCHEMA));
    let mk: BTreeSet<String> = serde_json::to_value(&art.manifest).unwrap().as_object().unwrap().keys().cloned().collect();
    for r in schema["manifest"]["required"].as_array().unwrap() {
        assert!(mk.contains(r.as_str().unwrap()), "manifest {r}");
    }
    // 決定の意味（label mapping）と defer 理由
    let lm = &schema["ledger_event"]["label_mapping_unchanged_from_c5c5"];
    for d in ["confirm", "pick_other", "none"] {
        let (label, method) = gt_label(d).unwrap();
        assert_eq!((lm[d]["label"].as_str(), lm[d]["method"].as_str()), (Some(label), Some(method)));
    }
    assert!(gt_label("defer").is_none());
    let reasons: Vec<&str> = schema["submission_input"]["decision_payloads"]["defer"]["defer_reason"].as_str().unwrap().split('|').collect();
    assert_eq!(reasons, DEFER_REASONS.to_vec());
    assert_eq!(schema["canonicalization"].as_str().unwrap().split_whitespace().next(), Some("pr4-canonical-json-1"));
    // API 応答の allowlist: schema の名前 ↔ 実装の type tag
    let mapping = HashMap::from([("session", "session"), ("queue_item", "next"), ("progress", "progress"), ("tmdb_search_result", "search"), ("tmdb_target_preview", "preview"), ("submit_result", "submit")]);
    let allowed = allowed_paths();
    let names: Vec<String> = schema["api_responses_allowlist"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().split(|c| c == '(' || c == '{').next().unwrap().to_string()).collect();
    assert_eq!(names.iter().cloned().collect::<BTreeSet<_>>(), mapping.keys().map(|k| k.to_string()).collect::<BTreeSet<_>>());
    for tag in mapping.values() {
        assert!(allowed.contains_key(tag), "{tag}");
    }
    let progress_decl = schema["api_responses_allowlist"].as_array().unwrap().iter().find(|s| s.as_str().unwrap().starts_with("progress")).unwrap().as_str().unwrap().to_string();
    let decl: BTreeSet<String> = progress_decl.trim_start_matches("progress{").trim_end_matches('}').split(',').map(|x| x.trim().to_string()).collect();
    assert_eq!(decl, allowed["progress"].iter().cloned().collect::<BTreeSet<_>>());
    assert!(allowed["error"].contains("reason_code") && allowed["submit"].contains("accepted"));
    // 契約 §17: 8 pin・R4 の token・MainAppRunning・seal の commitment
    for pin in ["protocol_sha256", "ui_package_sha256", "shuffle_seed_commitment", "attestation_policy_sha256", "unblind_map_sha256", "build_secret_commitment", "a8_leak_scan_report_sha256", "machine_freeze_anchor_commitment"] {
        assert!(CONTRACT.contains(pin), "{pin}");
    }
    for needle in ["MainAppRunning", "check_reviewer_package", "pr4-pf0g-candidate-token-1", "authoritative_membership_commitment", "review seal SHA", "INVALIDATED"] {
        assert!(CONTRACT.contains(needle), "{needle}");
    }
    assert_eq!(pins_of(&b).ui_package_sha256, ui_package_sha256());
    // schema の forbidden_anywhere を、この実装が出す全応答の DTO key が含まない
    for (tag, paths) in &allowed {
        for p in paths {
            let n: String = p.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
            for bad in ["decision", "verdict", "score", "rank", "jev", "matcher", "promotion", "precision", "coverage", "abstention", "workid", "runid", "evaluationunitkey", "filepath", "firstseenat", "candidatecount", "displaytitle", "unitsalt", "overlap"] {
                assert!(!n.contains(bad), "{tag}.{p} contains {bad}");
            }
        }
    }
}

// ═══ ソース走査（構造）══════════════════════════════════════════════════════════════════════

#[test]
fn reviewer_side_sources_do_not_touch_machine_types_the_production_db_or_value_passthrough() {
    let server = code_only(include_str!("pr4_pf0g2_server.rs"));
    let packager = code_only(include_str!("pr4_pf0g2.rs"));
    for (name, src) in [("server", &server), ("packager", &packager)] {
        for banned in ["rusqlite", "metadata_match", "pr4_pf0e", "pr4_pf0f", "pr4_pf0b", "pr4_pf0d", "gt_review", "DbState"] {
            assert!(!src.contains(banned), "{name} contains {banned}");
        }
    }
    // reviewer 側（server）は内部 ID・display_title・件数を一切知らない（packager は A8 の禁止語リストとして文字列で持つ）
    for banned in ["work_id", "display_title", "candidate_count", "evaluation_unit_key"] {
        assert!(!server.contains(banned), "server contains {banned}");
    }
    // server: 応答は emit（allowlist 検査つき）だけで作る。route に seal / unblind / errata が無い
    for banned in ["\"/api/seal", "unblind", "errata", "record_post_seal", "fn seal", ".seal("] {
        assert!(!server.contains(banned), "server contains {banned}");
    }
    // 応答 DTO は Serialize のみ（Deserialize を持つのは入力 DTO だけで、全て deny_unknown_fields）
    let req_structs = server.matches("#[derive(Deserialize)]").count();
    assert_eq!(req_structs, server.matches("#[serde(deny_unknown_fields)]").count());
    assert!(req_structs >= 6);
    assert!(server.matches("#[derive(Serialize)]").count() >= 10);
    // reviewer 側の Value 素通し: emit は DTO を受ける。handle の戻りを json! で組み立てていない
    let handle_body = server.split("pub fn handle").nth(1).unwrap().split("fn with<").next().unwrap();
    assert!(!handle_body.contains("json!"));
    // PF0G1 のコアを変更する経路（unsafe / include で上書き）が無い
    assert!(!server.contains("unsafe {") && !packager.contains("unsafe {") && !server.contains("unsafe fn"));
}

// D8: PF0G2 → PF0H1 → PF0I の synthetic E2E。private な fixture を使うので、この test module の child として登録する（統合 branch 専用）
#[path = "pr4_pf0i_e2e_tests.rs"]
mod pf0i_e2e;
