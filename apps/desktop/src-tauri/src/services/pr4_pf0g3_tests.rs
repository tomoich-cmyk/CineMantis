//! PR4-PF0G3: reviewer 実行ファイル（設定・probe・launch）、CSP、静的 UI の性質、実ブラウザ E2E 用 fixture server。
//! `pr4_pf0g2_tests.rs` の child module（private な fixture を使う）。実 TMDB・実 API key・production DB・H2・holdout には触れない。

use super::*;
use crate::services::pr4_pf0g3_reviewer::*;

const KEY: &str = "PF0G3-FAKE-KEY.canary_0123-abc";

#[derive(Clone)]
struct StubLister(Result<Vec<String>, String>);
impl ProcessLister for StubLister {
    fn image_names(&self) -> Result<Vec<String>, String> {
        self.0.clone()
    }
}
fn quiet() -> StubLister {
    StubLister(Ok(vec!["explorer.exe".into(), "svchost.exe".into()]))
}

fn vars(b: &Built, loopback: Option<&str>) -> Vec<(&'static str, String)> {
    let s = |p: &PathBuf| p.to_string_lossy().to_string();
    let mut v = vec![
        (ENV_REVIEWER_DIR, s(&b.rev)),
        (ENV_LEDGER_DIR, s(&b.ledger)),
        (ENV_CHECKPOINT_DIR, s(&b.cp)),
        (ENV_CACHE_DIR, s(&b.cache)),
        (ENV_TMDB_API_KEY, KEY.to_string()),
    ];
    if let Some(l) = loopback {
        v.push((ENV_TMDB_LOOPBACK, l.to_string()));
    }
    v
}
fn cfg_of(v: &[(&'static str, String)]) -> Result<RuntimeConfig, BlindError> {
    RuntimeConfig::from_lookup(&|n| v.iter().find(|(k, _)| *k == n).map(|(_, x)| x.clone()))
}
fn with(v: &[(&'static str, String)], name: &'static str, val: &str) -> Vec<(&'static str, String)> {
    let mut o: Vec<_> = v.iter().filter(|(k, _)| *k != name).cloned().collect();
    o.push((name, val.to_string()));
    o
}
fn without(v: &[(&'static str, String)], name: &str) -> Vec<(&'static str, String)> {
    v.iter().filter(|(k, _)| *k != name).cloned().collect()
}

#[test]
fn runtime_config_fails_closed_and_never_echoes_the_key() {
    let env = Env::new("g3cfg");
    let b = build_all(&env, 4);
    let good = vars(&b, None);
    let c = cfg_of(&good).unwrap();
    assert!(matches!(c.tmdb, TmdbMode::Production));
    assert_eq!(c.port, 0);
    // Debug / presence に値が出ない
    assert!(!format!("{c:?}").contains(KEY));
    let pres = format!("{:?}", env_presence(&|n| good.iter().find(|(k, _)| *k == n).map(|(_, x)| x.clone())));
    assert!(!pres.contains(KEY) && pres.contains(ENV_TMDB_API_KEY));
    // 必須の欠落
    for name in [ENV_REVIEWER_DIR, ENV_LEDGER_DIR, ENV_CHECKPOINT_DIR, ENV_CACHE_DIR, ENV_TMDB_API_KEY] {
        assert!(matches!(cfg_of(&without(&good, name)), Err(BlindError::Invalid(_))), "{name}");
    }
    // 形式不正（Err の文言に key 値が出ない）
    let long = "x".repeat(513);
    for bad in ["", "   ", "has space", "semi;colon", "日本語", long.as_str()] {
        let e = cfg_of(&with(&good, ENV_TMDB_API_KEY, bad)).unwrap_err().to_string();
        assert!(bad.trim().is_empty() || !e.contains(bad), "{e}");
    }
    for name in [ENV_REVIEWER_DIR, ENV_LEDGER_DIR, ENV_CHECKPOINT_DIR, ENV_CACHE_DIR] {
        assert!(cfg_of(&with(&good, name, "relative/dir")).is_err(), "{name}");
        assert!(cfg_of(&with(&good, name, "")).is_err(), "{name}");
    }
    for p in ["70000", "-1", "abc", "8 0"] {
        assert!(cfg_of(&with(&good, ENV_PORT, p)).is_err(), "{p}");
    }
    assert_eq!(cfg_of(&with(&good, ENV_PORT, "8123")).unwrap().port, 8123);
    // loopback 以外の TMDB 先は拒否
    for bad in ["evil.example:80", "0.0.0.0:80", "localhost:80", "127.0.0.1:x", "127.0.0.1", "http://127.0.0.1:80"] {
        assert!(cfg_of(&with(&good, ENV_TMDB_LOOPBACK, bad)).is_err(), "{bad}");
    }
    assert!(matches!(cfg_of(&with(&good, ENV_TMDB_LOOPBACK, "127.0.0.1:9999")).unwrap().tmdb, TmdbMode::Loopback(_)));
    // reviewer 側と audit 側の分離（同一・入れ子は拒否）
    let ledger = b.ledger.to_string_lossy().to_string();
    assert!(cfg_of(&with(&good, ENV_CACHE_DIR, &ledger)).is_err());
    assert!(cfg_of(&with(&good, ENV_CACHE_DIR, &b.ledger.join("cache").to_string_lossy())).is_err());
    assert!(cfg_of(&with(&good, ENV_LEDGER_DIR, &b.rev.join("ledger").to_string_lossy())).is_err());
}

#[test]
fn the_real_process_lister_works_and_the_probe_is_fail_closed() {
    let names = SystemProcessLister.image_names().expect("process list");
    assert!(names.len() > 5, "{names:?}");
    let me = std::env::current_exe().unwrap().file_name().unwrap().to_string_lossy().to_lowercase();
    assert!(names.iter().any(|n| n.to_lowercase() == me), "自プロセス {me} が一覧にない");
    let real = ProcessProbe { lister: SystemProcessLister };
    assert_eq!(real.main_app_running_checked(), names.iter().any(|n| MAIN_APP_IMAGES.contains(&n.to_lowercase().as_str())));
    assert!(ProcessProbe { lister: StubLister(Ok(vec!["CineMantis.EXE".into()])) }.main_app_running_checked());
    assert!(ProcessProbe { lister: StubLister(Err("denied".into())) }.main_app_running_checked());
    assert!(!ProcessProbe { lister: quiet() }.main_app_running_checked());
}

#[test]
fn launch_refuses_when_the_main_app_is_running_or_the_list_is_unavailable() {
    let env = Env::new("g3refuse");
    let b = build_all(&env, 4);
    drop(init_store(&b, 10, Box::new(NoMainApp)));
    let cfg = cfg_of(&vars(&b, Some("127.0.0.1:9"))).unwrap();
    for l in [StubLister(Ok(vec!["cinemantis.exe".into()])), StubLister(Err("x".into()))] {
        assert!(matches!(launch(&cfg, l), Err(BlindError::MainAppRunning)));
    }
    // 拒否は ledger に触れていない（lock が残っておらず、開き直せる）
    drop(Store::open(&b.ledger, &b.cp, Artifact::load(&b.rev).unwrap(), Box::new(NoMainApp)).unwrap());
    // reviewer package が無い / ledger が無い → 起動しない
    let missing = with(&vars(&b, Some("127.0.0.1:9")), ENV_REVIEWER_DIR, &env.p("nope").to_string_lossy());
    assert!(launch(&cfg_of(&missing).unwrap(), quiet()).is_err());
    let missing = with(&vars(&b, Some("127.0.0.1:9")), ENV_LEDGER_DIR, &env.p("no_ledger").to_string_lossy());
    assert!(launch(&cfg_of(&missing).unwrap(), quiet()).is_err());
}

fn http_full(addr: &str, path: &str, host: Option<&str>) -> (u16, String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", host.unwrap_or(addr)).as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text.split_whitespace().nth(1).and_then(|x| x.parse().ok()).unwrap_or(0);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    (status, head.to_string(), body.to_string())
}

#[test]
fn the_launched_server_serves_pinned_assets_under_a_narrow_csp_and_leaks_no_key() {
    let env = Env::new("g3srv");
    let b = build_all(&env, 6);
    drop(init_store(&b, 10, Box::new(NoMainApp)));
    let lb = Loopback::start();
    let cfg = cfg_of(&vars(&b, Some(&lb.addr))).unwrap();
    let srv = launch(&cfg, quiet()).unwrap();
    assert!(srv.addr.starts_with("127.0.0.1:"));
    let (s, head, body) = http_full(&srv.addr, &format!("/?t={}", srv.token), None);
    assert_eq!(s, 200);
    assert_eq!(body, UI_FILES[0].1);
    assert!(head.contains(&format!("Content-Security-Policy: {CSP}")));
    let (s, head, body) = http_full(&srv.addr, "/app.js", None);
    assert_eq!((s, body.as_str()), (200, UI_FILES[2].1));
    assert!(head.to_lowercase().contains("content-type: text/javascript") && head.contains(CSP));
    let (s, head, body) = http_full(&srv.addr, "/app.css", None);
    assert_eq!((s, body.as_str()), (200, UI_FILES[1].1));
    assert!(head.to_lowercase().contains("content-type: text/css"));
    // asset も Host 検査の対象。未知の path は token なしで 403（存在を教えない）
    assert_eq!(http_full(&srv.addr, "/app.js", Some("evil.example")).0, 403);
    assert_eq!(http_full(&srv.addr, "/index.html", None).0, 403);
    assert_eq!(http_full(&srv.addr, "/app.js/../x", None).0, 403);
    // API 経由の検索 → loopback の fake にしか出ない
    let h = [("X-Review-Token", srv.token.as_str())];
    let (_, v) = http(&srv.addr, "POST", "/api/session/start", &h, &json!({"reviewer_id": "R1", "attestation": true}).to_string(), None);
    let sid = serde_json::from_str::<Value>(&v).unwrap()["session_id"].as_str().unwrap().to_string();
    let (_, v) = http(&srv.addr, "POST", "/api/next", &h, &json!({"session_id": sid}).to_string(), None);
    let u = serde_json::from_str::<Value>(&v).unwrap()["unit"].clone();
    let (s, _) = http(&srv.addr, "POST", "/api/search", &h, &json!({"unit": u["review_unit_id"], "lease_id": u["lease_id"], "query": "x", "media_type": "movie"}).to_string(), None);
    assert_eq!(s, 200);
    assert!(lb.targets().iter().all(|t| t.starts_with("/3/")) && !lb.targets().is_empty());
    srv.stop();
    // key は package・ledger・cache のどこにも書かれない
    for d in [&b.rev, &b.ledger, &b.cache, &b.cp] {
        if !d.exists() {
            continue;
        }
        for e in std::fs::read_dir(d).unwrap().flatten() {
            if e.path().is_file() {
                assert!(!String::from_utf8_lossy(&std::fs::read(e.path()).unwrap()).contains(KEY), "{:?}", e.path());
            }
        }
    }
}

#[test]
fn the_static_ui_needs_no_inline_code_no_dynamic_html_and_no_external_origin() {
    let (html, css, js) = (UI_FILES[0].1, UI_FILES[1].1, UI_FILES[2].1);
    // CSP: unsafe-* なし・wildcard なし・外部 origin なし
    for bad in ["unsafe-inline", "unsafe-eval", "*", "http:", "https:", "data:", "blob:"] {
        assert!(!CSP.contains(bad), "{bad}");
    }
    for want in ["default-src 'none'", "script-src 'self'", "style-src 'self'", "connect-src 'self'", "img-src 'none'", "frame-ancestors 'none'", "base-uri 'none'", "form-action 'none'", "object-src 'none'"] {
        assert!(CSP.contains(want), "{want}");
    }
    // inline script / style / event handler なし
    let low = html.to_lowercase();
    assert_eq!(low.matches("<script").count(), 1);
    assert!(low.contains("<script src=\"/app.js\"></script>"));
    assert!(!low.contains("<style") && !low.contains(" style=") && !low.contains("javascript:"));
    let handler = regex::Regex::new(r"(?i)\son[a-z]+\s*=").unwrap();
    assert!(!handler.is_match(html), "inline event handler");
    // 外部 origin への参照なし
    for f in [html, css, js] {
        assert!(!f.contains("http://") && !f.contains("https://") && !f.contains("//cdn") && !f.contains("@import") && !f.contains("url("));
    }
    // 動的 HTML / 動的 code 生成 / 追加の network 手段なし。DOM は textContent のみ
    let jl = js.to_lowercase();
    for bad in [
        "innerhtml", "outerhtml", "insertadjacenthtml", "document.write", "eval(", "new function", "settimeout(\"", "setinterval(\"", "xmlhttprequest", "websocket", "eventsource", "sendbeacon",
        "importscripts", "new image", ".src =", ".src=", "setattribute(\"src", "setattribute(\"href", "window.open", "location.href", "localstorage", "sessionstorage", "indexeddb", "document.cookie",
    ] {
        assert!(!jl.contains(bad), "{bad}");
    }
    // network は同一 origin の相対 path への fetch のみ
    assert_eq!(js.matches("fetch(").count(), 1);
    assert!(js.contains("fetch(path,"));
    for (i, _) in js.match_indices("api(\"") {
        assert!(js[i + 5..].starts_with('/'), "{i}");
    }
    // 画像は読まない（poster は表示しない）
    assert!(!low.contains("<img") && !css.contains("background-image"));
}

#[test]
fn assets_carry_no_token_and_the_api_needs_the_per_launch_token() {
    let env = Env::new("g3tok");
    let b = build_all(&env, 4);
    drop(init_store(&b, 10, Box::new(NoMainApp)));
    let cfg = cfg_of(&vars(&b, Some("127.0.0.1:9"))).unwrap();
    let srv = launch(&cfg, quiet()).unwrap();
    for (_, body) in UI_FILES.iter() {
        assert!(!body.contains(&srv.token));
    }
    assert_eq!(http(&srv.addr, "GET", "/api/progress", &[], "", None).0, 403);
    srv.stop();
}

// ═══ 実ブラウザ E2E 用 fixture server（`--ignored`、手動）══════════════════════════════════════
// CM_PF0G3_FIXTURE_OUT=<dir> cargo test --lib pf0g3::browser_fixture_server -- --ignored --nocapture
// phase1（未 seal）→ phase1.done → operator が残りを完了して seal → phase2（sealed）→ phase2.done

fn wait_for(p: &std::path::Path) {
    let start = std::time::Instant::now();
    while !p.exists() {
        assert!(start.elapsed() < Duration::from_secs(40 * 60), "timeout waiting for {p:?}");
        std::thread::sleep(Duration::from_millis(250));
    }
}
fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    let _ = std::fs::create_dir_all(to);
    if let Ok(rd) = std::fs::read_dir(from) {
        for e in rd.flatten() {
            if e.path().is_file() {
                let _ = std::fs::copy(e.path(), to.join(e.file_name()));
            }
        }
    }
}

/// 既定は実 probe。本体 cinemantis.exe が動いている開発機では `CM_PF0G3_FIXTURE_STUB_PROBE=1` で stub にできる（ブラウザ fixture 専用。実 probe の拒否は別 test と evidence で確認）
fn launch_fixture(cfg: &RuntimeConfig) -> Result<Server, BlindError> {
    if std::env::var("CM_PF0G3_FIXTURE_STUB_PROBE").as_deref() == Ok("1") {
        launch(cfg, quiet())
    } else {
        launch(cfg, SystemProcessLister)
    }
}

#[test]
#[ignore]
fn browser_fixture_server() {
    let out = PathBuf::from(std::env::var("CM_PF0G3_FIXTURE_OUT").expect("CM_PF0G3_FIXTURE_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let env = Env::new("g3browser");
    let b = build_all(&env, 6);
    drop(init_store(&b, 10, Box::new(NoMainApp)));
    let art = Artifact::load(&b.rev).unwrap();
    let order = order_of(&b.rev);
    let lb = Loopback::start();
    let cfg = cfg_of(&vars(&b, Some(&lb.addr))).unwrap();
    let write_fixture = |phase: &str, srv: &Server| {
        let units: Vec<Value> = art.units.iter().map(|(k, u)| json!({"review_unit_id": u.review_unit_id, "candidate_tokens_in_package_order": order[k]})).collect();
        let v = json!({
            "phase": phase, "addr": srv.addr, "token": srv.token, "url": format!("http://{}/?t={}", srv.addr, srv.token),
            "fixture": {"build_id": "synthetic", "unit_count": art.units.len(), "manifest_sha256": art.manifest_sha256, "ui_package_sha256": ui_package_sha256(),
                        "csp": CSP, "ui_files": UI_FILES.iter().map(|(n, b)| json!({"name": n, "sha256": hex_sha(b.as_bytes())})).collect::<Vec<_>>()},
            "forbidden_literals": [SECRET, "CANARY_SCORE", "CANARY_RANK", "CANARY_SOURCE", "CANARY_DECISION", "CANARY_REASON", "CANARY_JEV", KEY],
            "units": units,
        });
        std::fs::write(out.join(format!("fixture_{phase}.json")), serde_json::to_string_pretty(&v).unwrap()).unwrap();
    };
    // phase 1: 未 seal
    let srv = launch_fixture(&cfg).expect("launch (is the main app running?)");
    write_fixture("phase1", &srv);
    std::fs::write(out.join("phase1.ready"), "").unwrap();
    wait_for(&out.join("phase1.done"));
    std::fs::write(out.join("tmdb_targets.json"), serde_json::to_string(&lb.targets()).unwrap()).unwrap();
    srv.stop();
    copy_dir(&b.ledger, &out.join("ledger_after_phase1"));
    // operator: 残りを in-process で完了して seal
    {
        let store = Store::open(&b.ledger, &b.cp, Artifact::load(&b.rev).unwrap(), Box::new(NoMainApp)).unwrap();
        // 実時計（server は system_clock で ledger に書いたので、固定の fake 時計だと時刻が逆行する）
        let calls = Rc::new(RefCell::new(Vec::new()));
        let api = Api::attach(store, &b.ledger, &b.cache, Box::new(FnTmdb(default_tmdb(), calls)), system_clock(), &Running(false)).unwrap();
        let mut c = Client { api, log: vec![] };
        let sid = c.start();
        c.drive_all(&sid);
        c.post("/api/session/end", json!({"session_id": sid}));
        let mut store = c.api.into_store();
        let now = (system_clock())();
        store.seal(&pins_of(&b), &now).expect("seal");
    }
    copy_dir(&b.ledger, &out.join("ledger_sealed"));
    let before = std::fs::read(b.ledger.join("events.jsonl")).ok();
    // phase 2: sealed
    let srv = launch_fixture(&cfg).expect("launch sealed");
    write_fixture("phase2", &srv);
    std::fs::write(out.join("phase2.ready"), "").unwrap();
    wait_for(&out.join("phase2.done"));
    std::fs::write(out.join("tmdb_targets_final.json"), serde_json::to_string(&lb.targets()).unwrap()).unwrap();
    srv.stop();
    let after = std::fs::read(b.ledger.join("events.jsonl")).ok();
    std::fs::write(out.join("sealed_ledger_unchanged_by_browser.json"), json!({"events_jsonl_byte_identical": before == after && before.is_some()}).to_string()).unwrap();
    copy_dir(&b.rev, &out.join("reviewer_package"));
    copy_dir(&b.cache, &out.join("tmdb_cache"));
}
