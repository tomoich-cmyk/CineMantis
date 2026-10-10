//! PR4-PF0F2: TMDB transport / wire-parity gate（**loopback mock HTTP のみ・test ビルドだけ・製品には出ない**）。
//!
//! PF0F1 の `CanonReq`（api_key を含まない正規化 request）を実際の HTTP request に変換する wire semantics が、
//! production の `TmdbClient`（`services/tmdb_client.rs`）と一致することを示す。live TMDB・実 key・production DB・H2 実データには触れない。
//!
//! 一致の示し方（production の base URL は const で差し替えられず、URL は `search_movie` / `search_tv` の中で inline に組まれるため）:
//! 1. production ソースから `format!` のテンプレートを **機械的に抽出**し、production の `urlencoding`（visibility のみ変更）で描画した URL を「production の URL」とする。
//! 2. test 側の wire builder（[`wire_target`]）の出力がそれと **バイト一致**すること、loopback server が実際に受信した request-target もバイト一致することを確認する。
//! 3. production ソースの該当断片は SHA-256 で pin し（tripwire）、未知の URL builder・変更された builder・変更された retry/timeout 定数は STOP する。
//!    ※ source SHA の一致は意味的な同一性の証明ではなく tripwire にすぎない。意味的な一致は 2. の wire 比較が担う。

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::services::pr4_pf0a::Stop;
use crate::services::pr4_pf0e::canon;
use crate::services::pr4_pf0f::{CanonReq, HttpResp, NetErr, Transport};
use crate::services::tmdb_client::urlencoding;

/// production の `tmdb_client.rs`（この gate が pin する対象）
const PROD_SRC: &str = include_str!("tmdb_client.rs");

/// production の TMDB path 接頭辞（`TMDB_API_BASE` = `https://api.themoviedb.org` + この path 接頭辞）
const API_PREFIX: &str = "/3";

fn sha(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
fn stop<T>(m: impl Into<String>) -> Result<T, Stop> {
    Err(Stop(m.into()))
}

// ─── source tripwire ────────────────────────────────────────────────────────────────────────────

/// CRLF → LF、行末空白除去。fragment の SHA はこの正規形で取る
fn norm(s: &str) -> String {
    s.replace("\r\n", "\n").lines().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n")
}

/// `start` から始まり、`ends` のどれかが最初に現れる直前までを返す（宣言の取り違えを防ぐため、start は一意でなければならない）
fn fragment<'a>(src: &'a str, start: &str, ends: &[&str]) -> Result<&'a str, Stop> {
    if src.matches(start).count() != 1 {
        return stop(format!("tripwire: `{start}` が一意に見つかりません"));
    }
    let i = src.find(start).unwrap();
    let rest = &src[i..];
    let n = ends.iter().filter_map(|e| rest[start.len()..].find(e).map(|k| k + start.len())).min().unwrap_or(rest.len());
    Ok(&rest[..n])
}

/// pin 対象の断片名 → (開始, 終了候補)。URL builder・送信・retry・timeout・エンコードを覆う
const FRAGMENTS: [(&str, &str, &[&str]); 8] = [
    ("const_base", "const TMDB_API_BASE", &["\n"]),
    ("const_attempts", "pub const MAX_HTTP_ATTEMPTS", &["\n"]),
    ("client_new", "pub fn new(api_key: String)", &["// ───"]),
    ("search_movie", "pub async fn search_movie(", &["pub async fn search_tv("]),
    ("search_tv", "pub async fn search_tv(", &["// ───"]),
    ("fetch_json", "async fn fetch_json<", &["async fn send_with_retry("]),
    ("send_with_retry", "async fn send_with_retry(", &["pub async fn get_movie_detail("]),
    ("urlencoding", "fn urlencoding(", &["\n}\n"]),
];

/// 正規化済み断片の SHA-256（PF0F2 で pin した値。production がここを変えたら STOP する）
const FRAGMENT_PINS: [(&str, &str); 8] = [
    ("const_base", "8da986d9df202c671bf3cc317e01204091ce532f0cc7dd4e73388f546ae3e4a0"),
    ("const_attempts", "709134494f2f3a7bb91cd32aa82cceabadfad5ca7d95611613b9eb62cae86b26"),
    ("client_new", "b66dde983fd36086bf4f9e40a6ea65637510ed8e6c4494d106b59c7953b28e1a"),
    ("search_movie", "32b91cfc706682be7bbbc90ffe056cdf7747ad3246a6af9ea46e6d21639fcd3f"),
    ("search_tv", "e389a5b3716fd72207aa2ca5bad66a35f01cf5efba2845d8abcbdc67e5283266"),
    ("fetch_json", "aeff34b083cffda9f9b542de9724e4a76cd80ba493d83ede8fd95fd7722499c1"),
    ("send_with_retry", "346286d25e4f62bfb542dfaceaa38f9a8e7467a9fcf489adcbd0cc2a9273afde"),
    ("urlencoding", "82bd4f33031ebf4480948de0c69df5c1cc6d1bdab845352fcbb15fba4b659293"),
];

/// `TMDB_API_BASE}/` を含む行（= production に存在する URL builder の全数）。未知の builder が増えたら STOP
const URL_BUILDER_INVENTORY: [&str; 9] = [
    "\"{TMDB_API_BASE}/search/movie?api_key={}&query={}&language=ja-JP&include_adult=false\",",
    "\"{TMDB_API_BASE}/search/tv?api_key={}&query={}&language=ja-JP&include_adult=false\",",
    "\"{TMDB_API_BASE}/movie/{tmdb_id}?api_key={}&language=ja-JP\",",
    "\"{TMDB_API_BASE}/tv/{tmdb_id}?api_key={}&language=ja-JP\",",
    "\"{TMDB_API_BASE}/movie/{tmdb_id}/credits?api_key={}&language=ja-JP\",",
    "\"{TMDB_API_BASE}/tv/{tmdb_id}/credits?api_key={}&language=ja-JP\",",
    "\"{TMDB_API_BASE}/person/{tmdb_id}?api_key={}&language=ja-JP\",",
    "let url = format!(\"{TMDB_IMAGE_BASE}{poster_path}\");",
    "\"&year={y}\"",
];

fn fragment_sha(src: &str, name: &str) -> Result<String, Stop> {
    let src = &src.replace("\r\n", "\n");
    let (_, start, ends) = FRAGMENTS.iter().find(|f| f.0 == name).ok_or_else(|| Stop(format!("unknown fragment {name}")))?;
    Ok(sha(norm(fragment(src, start, ends)?).as_bytes()))
}

/// production ソースの tripwire。pin 済み断片の変更・未知の URL builder の追加で STOP する
pub fn check_source(src: &str) -> Result<(), Stop> {
    let src = &src.replace("\r\n", "\n");
    for (name, want) in FRAGMENT_PINS {
        let got = fragment_sha(src, name)?;
        if got != want {
            return stop(format!("tripwire: tmdb_client.rs の `{name}` が変わりました（{got}）"));
        }
    }
    // URL builder の全数（api.themoviedb.org / image.tmdb.org の host を持つ行は const 2 本だけ）
    let hosts: Vec<&str> = src.lines().filter(|l| l.contains("themoviedb.org") || l.contains("tmdb.org")).map(str::trim).collect();
    if hosts.len() != 2 {
        return stop(format!("tripwire: host を含む行が {} 行（想定 2 行）", hosts.len()));
    }
    let mut found: Vec<String> = src
        .lines()
        .map(str::trim)
        .filter(|l| l.contains("TMDB_API_BASE}") || l.contains("TMDB_IMAGE_BASE}") || l.contains("&year={") || l.contains("first_air_date_year="))
        .map(str::to_string)
        .collect();
    found.retain(|l| !l.starts_with("//"));
    // search 用の year / first_air_date_year は push_str 行の中。inventory には正規化した形で照合する
    let mut want: Vec<String> = URL_BUILDER_INVENTORY.iter().map(|s| s.to_string()).collect();
    want.push("url.push_str(&format!(\"&first_air_date_year={y}\"));".into());
    want.push("url.push_str(&format!(\"&year={y}\"));".into());
    let found_set: std::collections::BTreeSet<String> = found
        .into_iter()
        .filter(|l| !l.starts_with("const ") && !l.starts_with("pub const "))
        .map(|l| if l == "\"&year={y}\"" { "\"&year={y}\"".to_string() } else { l })
        .collect();
    for l in &found_set {
        if !want.contains(l) {
            return stop(format!("tripwire: 未知の URL builder 行: {l}"));
        }
    }
    Ok(())
}

/// production のソースから抽出した、search の format テンプレート
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProdTemplate {
    pub base: String,
    pub main: String,
    pub year_suffix: String,
}

fn extract_template(src: &str, fn_name: &str, next: &str) -> Result<ProdTemplate, Stop> {
    let src = &src.replace("\r\n", "\n");
    let base_re = regex::Regex::new(r#"const TMDB_API_BASE: &str = "([^"]*)";"#).unwrap();
    let base = base_re.captures(src).map(|c| c[1].to_string()).ok_or_else(|| Stop("TMDB_API_BASE が見つかりません".into()))?;
    let f = fragment(src, &format!("pub async fn {fn_name}("), &[next])?;
    let main_re = regex::Regex::new(r#"format!\(\s*"([^"]*)""#).unwrap();
    let year_re = regex::Regex::new(r#"push_str\(&format!\("([^"]*)"\)\)"#).unwrap();
    let main = main_re.captures(f).map(|c| c[1].to_string()).ok_or_else(|| Stop(format!("{fn_name}: format! が見つかりません")))?;
    let year_suffix = year_re.captures(f).map(|c| c[1].to_string()).ok_or_else(|| Stop(format!("{fn_name}: year push_str が見つかりません")))?;
    if main.matches("{}").count() != 2 {
        return stop(format!("{fn_name}: placeholder が api_key / query の 2 個ではありません"));
    }
    Ok(ProdTemplate { base, main, year_suffix })
}

/// production の search URL（key 入り・完全な URL）を、production の template と `urlencoding` で再構成する
pub fn prod_url(src: &str, kind: &str, query: &str, year: Option<i32>, api_key: &str) -> Result<String, Stop> {
    let t = match kind {
        "movie" => extract_template(src, "search_movie", "pub async fn search_tv(")?,
        "tv" => extract_template(src, "search_tv", "// ───")?,
        _ => return stop("kind は movie | tv"),
    };
    let main = t.main.replacen("{TMDB_API_BASE}", &t.base, 1).replacen("{}", api_key, 1).replacen("{}", &urlencoding(query), 1);
    Ok(match year {
        Some(y) => format!("{main}{}", t.year_suffix.replace("{y}", &y.to_string())),
        None => main,
    })
}

/// production の retry / timeout 定数（ソースから抽出）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProdPolicy {
    pub max_attempts: usize,
    pub backoff_ms: Vec<u64>,
    pub timeout_secs: u64,
    pub retries_http_status: bool,
}

pub fn prod_policy(src: &str) -> Result<ProdPolicy, Stop> {
    let src = &src.replace("\r\n", "\n");
    let n = regex::Regex::new(r"pub const MAX_HTTP_ATTEMPTS: usize = (\d+);").unwrap().captures(src).and_then(|c| c[1].parse().ok());
    let unit = regex::Regex::new(r"from_millis\(\s*(\d+) \* \(attempt \+ 1\) as u64").unwrap().captures(src).and_then(|c| c[1].parse::<u64>().ok());
    let to = regex::Regex::new(r"\.timeout\(std::time::Duration::from_secs\((\d+)\)\)").unwrap().captures(src).and_then(|c| c[1].parse().ok());
    let (Some(max_attempts), Some(unit), Some(timeout_secs)) = (n, unit, to) else {
        return stop("production の retry / timeout 定数を抽出できません");
    };
    let send = fragment(src, "async fn send_with_retry(", &["pub async fn get_movie_detail("])?;
    // production は HTTP status では再試行しない（Ok(response) をそのまま返す）。接続系の Err だけ再試行する
    let retries_http_status = !send.contains("Ok(response) => return Ok(response)");
    Ok(ProdPolicy { max_attempts, backoff_ms: (1..max_attempts as u64).map(|k| unit * k).collect(), timeout_secs, retries_http_status })
}

/// manifest の transport_policy が production の定数と一致するか（max_attempts / backoff / timeout）
pub fn policy_parity(manifest: &Value, src: &str) -> Result<(), Stop> {
    let p = prod_policy(src)?;
    let m = &manifest["transport_policy"];
    if m["max_attempts"].as_u64() != Some(p.max_attempts as u64) {
        return stop("max_attempts が production と一致しません");
    }
    let mb: Vec<u64> = m["backoff_ms"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).collect()).unwrap_or_default();
    if mb != p.backoff_ms {
        return stop("backoff_ms が production と一致しません");
    }
    if m["timeout_secs"].as_u64() != Some(p.timeout_secs) {
        return stop("timeout_secs が production と一致しません");
    }
    if m["concurrency"] != json!(1) {
        return stop("concurrency は 1 固定");
    }
    Ok(())
}

// ─── wire builder（CanonReq → request-target） ───────────────────────────────────────────────────

/// `CanonReq` を検査して実際に送る request-target（path + query。**key 入り**）を作る。
/// 順序は production と同じ: `api_key, query, language, include_adult, [year | first_air_date_year]`。
/// 未知の path・未知/欠落/変更された param は STOP。
pub fn wire_target(req: &CanonReq, api_key: &str) -> Result<String, Stop> {
    let (kind, year_key) = match req.path {
        "/3/search/movie" => ("movie", "year"),
        "/3/search/tv" => ("tv", "first_air_date_year"),
        other => return stop(format!("未知の path: {other}")),
    };
    let mut allowed: Vec<&str> = vec!["include_adult", "language", "query"];
    if req.params.contains_key(year_key) {
        allowed.push(year_key);
    }
    allowed.sort();
    let got: Vec<&str> = req.params.keys().map(String::as_str).collect();
    if got != allowed {
        return stop(format!("未知/欠落の param: {got:?}"));
    }
    if req.params["language"] != "ja-JP" || req.params["include_adult"] != "false" {
        return stop("language=ja-JP / include_adult=false 以外は送りません");
    }
    if api_key.is_empty() || api_key.chars().any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
        return stop("api_key の形式が想定外です（production は key を encode しないため、安全な文字だけ許可）");
    }
    let mut t = format!(
        "{API_PREFIX}/search/{kind}?api_key={api_key}&query={}&language=ja-JP&include_adult=false",
        urlencoding(&req.params["query"])
    );
    if let Some(y) = req.params.get(year_key) {
        let y: i32 = y.parse().map_err(|_| Stop("year が整数ではありません".into()))?;
        t.push_str(&format!("&{year_key}={y}"));
    }
    Ok(t)
}

fn pct_decode(s: &str) -> Result<String, Stop> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let h = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()).ok_or_else(|| Stop("壊れた percent-encoding".into()))?;
                out.push(h);
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8(out).map_err(|_| Stop("UTF-8 ではありません".into()))
}

/// 実際に送られた request-target から、artifact の `canonical_request` と同じ形（key 抜き）を復元する。
/// 戻り値: (canonical_request JSON, 送られた api_key, 実際の param 順)
pub fn reconstruct(target: &str) -> Result<(Value, String, Vec<String>), Stop> {
    let (path, query) = target.split_once('?').ok_or_else(|| Stop("query がありません".into()))?;
    let mut params = BTreeMap::new();
    let mut key = String::new();
    let mut order = Vec::new();
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').ok_or_else(|| Stop("壊れた param".into()))?;
        order.push(k.to_string());
        if k == "api_key" {
            key = v.to_string();
        } else if params.insert(k.to_string(), pct_decode(v)?).is_some() {
            return stop("param の重複");
        }
    }
    Ok((json!({"method": "GET", "path": path, "params": params}), key, order))
}

/// artifact に残す request 証跡の SHA（PF0F1 の `canonical_request_sha256` と同じ定義）
pub fn canonical_request_sha(v: &Value) -> Result<String, Stop> {
    Ok(sha(canon(v)?.as_bytes()))
}

/// 送られた request-target の key を伏せた形（artifact に載せてよい形）
pub fn redact(target: &str) -> String {
    target
        .split_once('?')
        .map(|(p, q)| {
            let q = q.split('&').map(|kv| if kv.starts_with("api_key=") { "api_key=<redacted>".to_string() } else { kv.to_string() }).collect::<Vec<_>>().join("&");
            format!("{p}?{q}")
        })
        .unwrap_or_else(|| target.to_string())
}

// ─── loopback mock server ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub target: String,
    pub headers: BTreeMap<String, String>,
}

#[allow(dead_code)]
pub enum Act {
    Body(Vec<u8>),
    Status(u16, Option<u32>),
    /// 応答しない（client が切断するまで待つ）
    Hang,
    /// 応答せずに接続を閉じる
    Drop,
}

pub struct Loopback {
    pub addr: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    script: Arc<Mutex<VecDeque<Act>>>,
    pub max_inflight: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

fn default_body(target: &str) -> Vec<u8> {
    let q = reconstruct(target).ok().and_then(|(v, _, _)| v["params"]["query"].as_str().map(str::to_lowercase)).unwrap_or_default();
    let movie = target.starts_with("/3/search/movie");
    let results = if movie && q.contains("alpha") {
        vec![json!({"id": 100, "title": "Alpha Movie", "original_title": "Alpha Movie", "overview": "o", "release_date": "1999-05-01", "poster_path": "/a.jpg", "original_language": "en", "vote_average": 7.5, "genre_ids": [1]})]
    } else {
        vec![]
    };
    serde_json::to_vec(&json!({"page": 1, "results": results, "total_results": results.len()})).unwrap()
}

fn read_head(s: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut b = [0u8; 1024];
    loop {
        let n = s.read(&mut b).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&b[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Some(String::from_utf8_lossy(&buf).to_string());
        }
        if buf.len() > 64 * 1024 {
            return None;
        }
    }
}

impl Loopback {
    pub fn start(script: Vec<Act>) -> Loopback {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(script.into_iter().collect::<VecDeque<_>>()));
        let inflight = Arc::new(AtomicUsize::new(0));
        let max_inflight = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen2, script2, inflight2, max2, stop2) = (seen.clone(), script.clone(), inflight.clone(), max_inflight.clone(), stop.clone());
        std::thread::spawn(move || {
            for conn in l.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut s) = conn else { continue };
                let (seen, script, inflight, max) = (seen2.clone(), script2.clone(), inflight2.clone(), max2.clone());
                std::thread::spawn(move || {
                    let _ = s.set_read_timeout(Some(Duration::from_millis(50)));
                    let cur = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                    max.fetch_max(cur, Ordering::SeqCst);
                    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                    if let Some(head) = read_head(&mut s) {
                        let mut lines = head.split("\r\n");
                        let first = lines.next().unwrap_or_default().to_string();
                        let mut p = first.split(' ');
                        let (method, target) = (p.next().unwrap_or_default().to_string(), p.next().unwrap_or_default().to_string());
                        let headers: BTreeMap<String, String> =
                            lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string())).collect();
                        let act = {
                            let mut g = seen.lock().unwrap();
                            g.push(Seen { method, target: target.clone(), headers });
                            script.lock().unwrap().pop_front()
                        };
                        let write = |s: &mut TcpStream, status: u16, extra: &str, body: &[u8]| {
                            let h = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n", body.len());
                            let _ = s.write_all(h.as_bytes());
                            let _ = s.write_all(body);
                            let _ = s.flush();
                        };
                        match act {
                            None => write(&mut s, 200, "", &default_body(&target)),
                            Some(Act::Body(b)) => write(&mut s, 200, "", &b),
                            Some(Act::Status(c, ra)) => {
                                let extra = ra.map(|r| format!("Retry-After: {r}\r\n")).unwrap_or_default();
                                write(&mut s, c, &extra, b"{\"status_message\":\"x\"}")
                            }
                            Some(Act::Drop) => {}
                            Some(Act::Hang) => {
                                // client が切断（timeout）するまで待つ。切断を見たら in-flight から外す
                                let _ = s.set_read_timeout(Some(Duration::from_millis(50)));
                                let mut b = [0u8; 16];
                                for _ in 0..200 {
                                    match s.read(&mut b) {
                                        Ok(0) => break,
                                        Ok(_) => {}
                                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
                                        Err(_) => break,
                                    }
                                }
                            }
                        }
                    }
                    inflight.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        Loopback { addr, seen, script, max_inflight, stop }
    }
    pub fn targets(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().map(|s| s.target.clone()).collect()
    }
    pub fn remaining_script(&self) -> usize {
        self.script.lock().unwrap().len()
    }
}
impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(&self.addr);
    }
}

// ─── 実 HTTP transport（loopback 向け） ──────────────────────────────────────────────────────────

/// `CanonReq` を [`wire_target`] で実 request にして loopback へ送る。再試行・待ちは generator（policy）の仕事で、ここでは 1 request = 1 送信。
/// api_key は送信にだけ使い、Debug にも記録にも出さない。
pub struct WireTransport {
    rt: tokio::runtime::Runtime,
    client: reqwest::Client,
    base: String,
    key: String,
    pub pauses: Vec<u64>,
    pub sent: usize,
    pub stops: Vec<String>,
    /// 送信直前に target を書き換える（tamper 検出テスト用）
    pub tamper: Option<fn(&str) -> String>,
}
impl std::fmt::Debug for WireTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WireTransport(base={}, key=<redacted>, sent={})", self.base, self.sent)
    }
}
impl WireTransport {
    pub fn new(addr: &str, key: &str, timeout: Duration) -> WireTransport {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        // production と同じ組み立て（Client::builder().timeout(..).build()）。proxy を拾わない（loopback 専用）
        let client = reqwest::Client::builder().timeout(timeout).no_proxy().build().unwrap();
        WireTransport { rt, client, base: format!("http://{addr}"), key: key.to_string(), pauses: Vec::new(), sent: 0, stops: Vec::new(), tamper: None }
    }
}
impl Transport for WireTransport {
    fn pause(&mut self, ms: u64) {
        self.pauses.push(ms);
    }
    fn get(&mut self, req: &CanonReq) -> Result<HttpResp, NetErr> {
        let mut target = match wire_target(req, &self.key) {
            Ok(t) => t,
            Err(Stop(m)) => {
                self.stops.push(m);
                return Err(NetErr::Other);
            }
        };
        if let Some(f) = self.tamper {
            target = f(&target);
        }
        let url = format!("{}{target}", self.base);
        self.sent += 1;
        let client = self.client.clone();
        self.rt.block_on(async move {
            match client.get(&url).send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let retry_after_secs = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
                    match resp.bytes().await {
                        Ok(b) => Ok(HttpResp { status, body: b.to_vec(), retry_after_secs }),
                        Err(e) => Err(classify(&e)),
                    }
                }
                Err(e) => Err(classify(&e)),
            }
        })
    }
}
/// production の `send_with_retry` と同じ分類
fn classify(e: &reqwest::Error) -> NetErr {
    if e.is_timeout() {
        NetErr::Timeout
    } else if e.is_connect() {
        NetErr::Connect
    } else if e.is_request() {
        NetErr::Interrupted
    } else {
        NetErr::Other
    }
}

// ─── tests ──────────────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::pr4_pf0b::membership_commitment;
    use crate::services::pr4_pf0f::{planned_specs, read_chain, verify_and_replay, Generator, UnitIn};
    use crate::services::prematch_snapshot::EvidenceClass;
    use crate::services::pr4_pf0e::snapshot_from_payload;
    use std::path::{Path, PathBuf};

    static SEQ: AtomicUsize = AtomicUsize::new(0);
    const VER: &str = "pr4-test-eligibility-1";
    /// 実在しない canary。artifact に出たら漏洩
    const KEY: &str = "PF0F2-FAKE-KEY-0123456789abcdef";

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pr4_pf0f2_{tag}_{}_{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
    fn sha_v(v: &Value) -> String {
        canonical_request_sha(v).unwrap()
    }
    fn unit(i: i64, tg: Option<&str>, file: &str) -> UnitIn {
        let p = json!({"payload_schema_version": "pr4-h2-input-payload-1", "canonicalization_version": "pr4-canonical-json-1",
            "file_order_basis": "work_parts.part_no,files.id", "work": {"title_guess": tg},
            "files": [{"original_file_name": file, "original_rel_path": file, "original_captured": true, "renamed_by_app": null,
                       "container_tags_json": null, "tags_provenance": null, "tags_provider_hint": null, "source_media_kind": "movie", "path_derived": null}]});
        UnitIn { key: format!("work:{i}"), matcher_input_sha256: sha_v(&p), payload: p, expected_decision_evidence_sha256: None }
    }
    fn units() -> Vec<UnitIn> {
        vec![
            unit(1, Some("Alpha Movie 1999"), "Alpha Movie 1999.mkv"),
            unit(2, Some("Beta Film"), "Beta Film.mkv"),
            unit(3, None, ""),
            unit(4, Some("Gamma Story 2005"), "Gamma Story 2005.mkv"),
        ]
    }
    fn policy_json(retry_after: &str, cap: u64) -> Value {
        let mut p = json!({"version": "transport-policy-1", "max_attempts": 3, "backoff_ms": [400, 800], "retry_statuses": [429, 500, 502, 503, 504],
                           "retry_after": retry_after, "retry_after_cap_ms": cap, "concurrency": 1, "timeout_secs": 30, "pacing_ms": 0});
        p["policy_sha256"] = json!(sha_v(&p));
        p
    }
    fn manifest(us: &[UnitIn], policy: Value) -> Value {
        let keys: Vec<String> = us.iter().map(|u| u.key.clone()).collect();
        json!({"unit_order_sha256": sha_v(&json!(keys)), "transport_policy": policy, "resume_policy": {"max_primary_resumes": 1, "recovery_resume": "forbidden"},
               "contract_version": crate::services::pr4_pf0f::CONTRACT_VERSION, "run_id": "00000000-0000-4000-8000-000000000002",
               "binding_matcher": crate::services::pr4_pf0f::BINDING_MATCHER, "reference_matcher": crate::services::pr4_pf0f::REFERENCE_MATCHER, "rules_1_in_final": false,
               "code_commit": "5a31b5fa513243f7cbe6b411d19b7ca5dd1beaaf",
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
    fn req(kind: &str, query: &str, year: Option<i32>) -> CanonReq {
        let mut params = BTreeMap::new();
        params.insert("include_adult".to_string(), "false".to_string());
        params.insert("language".to_string(), "ja-JP".to_string());
        params.insert("query".to_string(), query.to_string());
        if let Some(y) = year {
            params.insert(if kind == "movie" { "year" } else { "first_air_date_year" }.to_string(), y.to_string());
        }
        CanonReq { path: if kind == "movie" { "/3/search/movie" } else { "/3/search/tv" }, params }
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
    fn all_text(dir: &Path) -> String {
        walk(dir).iter().map(|p| String::from_utf8_lossy(&std::fs::read(p).unwrap()).to_string()).collect()
    }
    fn tmdb_requests(dir: &Path) -> Vec<Value> {
        read_chain(&dir.join("tmdb_request.jsonl")).unwrap().0
    }
    fn short() -> Duration {
        Duration::from_millis(250)
    }
    /// 1 unit を primary で流す。戻り値: (dir, transport, loopback)
    fn run_one(tag: &str, script: Vec<Act>, policy: Value, timeout: Duration, title: &str) -> (PathBuf, WireTransport, Loopback) {
        let lb = Loopback::start(script);
        let us = vec![unit(1, Some(title), &format!("{title}.mkv"))];
        let dir = tmp(tag);
        let mut g = Generator::begin(&dir, manifest(&us, policy), us, clock(), Some(KEY.into())).unwrap();
        let mut t = WireTransport::new(&lb.addr, KEY, timeout);
        g.run_primary(&mut t).unwrap();
        (dir, t, lb)
    }

    // ── corpus ──
    fn corpus() -> Vec<(&'static str, String, Option<i32>)> {
        let qs = [
            "Alpha Movie", "ab", "a b  c", "日本語タイトル", "千と千尋の神隠し", "Amélie", "A&B=C?D#E", "100% 本物", "a+b", "x/y\\z", "'quote\" <tag>", "~-_.", "😀 emoji", "", "  lead trail  ", "ｆｕｌｌｗｉｄｔｈ", "tab\there", "%2B", "日本語 + English & 記号?",
        ];
        let mut v = Vec::new();
        for kind in ["movie", "tv"] {
            for q in qs {
                v.push((kind, q.to_string(), None));
                v.push((kind, q.to_string(), Some(1999)));
            }
        }
        v
    }

    // ── 1. endpoint / year / language / adult / encoding: production との一致 ──
    #[test]
    fn wire_builder_is_byte_identical_to_the_production_url_for_movie_tv_year_and_encoding() {
        for (kind, q, year) in corpus() {
            let prod = prod_url(PROD_SRC, kind, &q, year, KEY).unwrap();
            let want = prod.strip_prefix("https://api.themoviedb.org").unwrap().to_string();
            let got = wire_target(&req(kind, &q, year), KEY).unwrap();
            assert_eq!(got, want, "{kind} {q:?} {year:?}");
            // 固定項目
            assert!(got.contains("&language=ja-JP&include_adult=false"));
            assert!(got.starts_with(&format!("/3/search/{kind}?api_key={KEY}&query=")));
            assert_eq!(got.contains("&year=1999") || got.contains("&first_air_date_year=1999"), year.is_some());
            match (kind, year) {
                ("movie", Some(_)) => assert!(got.ends_with("&year=1999") && !got.contains("first_air_date_year")),
                ("tv", Some(_)) => assert!(got.ends_with("&first_air_date_year=1999") && !got.contains("&year=")),
                _ => assert!(!got.contains("year")),
            }
        }
    }

    #[test]
    fn loopback_receives_exactly_the_production_bytes_and_the_query_round_trips() {
        let lb = Loopback::start(vec![]);
        let mut t = WireTransport::new(&lb.addr, KEY, short());
        let cases = corpus();
        for (kind, q, year) in &cases {
            let r = t.get(&req(kind, q, *year)).unwrap();
            assert_eq!(r.status, 200);
        }
        assert!(t.stops.is_empty());
        let seen = lb.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), cases.len());
        for (s, (kind, q, year)) in seen.iter().zip(&cases) {
            let prod = prod_url(PROD_SRC, kind, q, *year, KEY).unwrap();
            assert_eq!(s.method, "GET");
            assert_eq!(format!("https://api.themoviedb.org{}", s.target), prod, "受信した request-target が production の URL と一致しません");
            // percent-decode すると元の query に戻る（artifact の canonical_request と実送信が同じ意味）
            let (canon_v, key, order) = reconstruct(&s.target).unwrap();
            assert_eq!(canon_v["params"]["query"], json!(q));
            assert_eq!(key, KEY);
            assert_eq!(order[0], "api_key");
            assert_eq!(&order[1..4], ["query", "language", "include_adult"]);
            // 余計な認証 header は付かない（key は query のみ。production と同じ）
            assert!(!s.headers.contains_key("authorization") && !s.headers.contains_key("x-api-key"));
        }
    }

    // ── 2. 未知/変更の request は STOP ──
    #[test]
    fn unknown_or_altered_requests_stop_before_anything_is_sent() {
        let ok = req("movie", "x", Some(2000));
        assert!(wire_target(&ok, KEY).is_ok());
        let mut bad: Vec<CanonReq> = Vec::new();
        let mut r = ok.clone();
        r.path = "/3/search/person";
        bad.push(r);
        let mut r = ok.clone();
        r.path = "/3/search/movie/../tv";
        bad.push(r);
        for (k, v) in [("language", "en-US"), ("include_adult", "true")] {
            let mut r = ok.clone();
            r.params.insert(k.into(), v.into());
            bad.push(r);
        }
        for (k, v) in [("page", "2"), ("region", "JP"), ("first_air_date_year", "2000")] {
            let mut r = ok.clone();
            r.params.insert(k.into(), v.into());
            bad.push(r);
        }
        let mut r = ok.clone();
        r.params.remove("language");
        bad.push(r);
        let mut r = ok.clone();
        r.params.insert("year".into(), "19x9".into());
        bad.push(r);
        for r in &bad {
            assert!(wire_target(r, KEY).is_err(), "{:?}", r);
        }
        // key の形式が想定外（encode されない）なら送らない
        assert!(wire_target(&ok, "a&b").is_err() && wire_target(&ok, "").is_err());
        // transport 経由でも何も送られない
        let lb = Loopback::start(vec![]);
        let mut t = WireTransport::new(&lb.addr, KEY, short());
        for r in &bad {
            assert!(t.get(r).is_err());
        }
        assert_eq!(t.sent, 0);
        assert_eq!(t.stops.len(), bad.len());
        assert!(lb.targets().is_empty());
    }

    // ── 3. source tripwire ──
    /// pin を作り直すときだけ手で実行する（`cargo test --lib dump_fragment_pins -- --ignored --nocapture`）。通常 run では実行しない
    #[test]
    #[ignore]
    fn dump_fragment_pins() {
        for (name, _) in FRAGMENT_PINS {
            println!("(\"{name}\", \"{}\"),", fragment_sha(PROD_SRC, name).unwrap());
        }
        println!("whole_file_sha256={}", sha(norm(PROD_SRC).as_bytes()));
    }

    #[test]
    fn production_source_matches_the_pinned_fragments() {
        check_source(PROD_SRC).unwrap();
    }

    #[test]
    fn tripwire_stops_on_any_change_to_a_url_builder_or_the_sender() {
        let mutations: Vec<(&str, String)> = vec![
            ("language", PROD_SRC.replacen("language=ja-JP&include_adult=false", "language=en-US&include_adult=false", 1)),
            ("adult", PROD_SRC.replacen("include_adult=false", "include_adult=true", 1)),
            ("order", PROD_SRC.replacen("/search/movie?api_key={}&query={}", "/search/movie?query={}&api_key={}", 1)),
            ("year", PROD_SRC.replacen("&year={y}", "&primary_release_year={y}", 1)),
            ("encode_space", PROD_SRC.replacen("' ' => \"+\".to_string()", "' ' => \"%20\".to_string()", 1)),
            ("attempts", PROD_SRC.replacen("MAX_HTTP_ATTEMPTS: usize = 3", "MAX_HTTP_ATTEMPTS: usize = 5", 1)),
            ("backoff", PROD_SRC.replacen("400 * (attempt + 1)", "100 * (attempt + 1)", 1)),
            ("timeout", PROD_SRC.replacen("from_secs(30)", "from_secs(5)", 1)),
            ("base", PROD_SRC.replacen("https://api.themoviedb.org/3", "https://example.invalid/3", 1)),
            ("status_retry", PROD_SRC.replacen("Ok(response) => return Ok(response),", "Ok(response) if response.status().is_success() => return Ok(response),", 1)),
            ("new_builder", PROD_SRC.replacen("// ─── Credits", "pub async fn trending(&self) { let _u = format!(\"{TMDB_API_BASE}/trending/all/day?api_key={}\", self.api_key); }\n// ─── Credits", 1)),
            ("new_host", format!("{PROD_SRC}\nconst X: &str = \"https://api.other-tmdb.org/3\";\n")),
        ];
        for (name, src) in &mutations {
            assert_ne!(src, PROD_SRC, "mutation {name} が当たっていません");
            assert!(check_source(src).is_err(), "tripwire が {name} を検出しません");
        }
        // CRLF 化だけでは落ちない（正規化済み）
        check_source(&PROD_SRC.replace('\n', "\r\n")).unwrap();
    }

    #[test]
    fn production_retry_and_timeout_constants_match_the_manifest_policy() {
        let p = prod_policy(PROD_SRC).unwrap();
        assert_eq!(p, ProdPolicy { max_attempts: 3, backoff_ms: vec![400, 800], timeout_secs: 30, retries_http_status: false });
        let us = units();
        policy_parity(&manifest(&us, policy_json("ignore", 0)), PROD_SRC).unwrap();
        // manifest 側が production とずれたら STOP
        for (k, v) in [("max_attempts", json!(2)), ("backoff_ms", json!([100, 200])), ("timeout_secs", json!(10)), ("concurrency", json!(2))] {
            let mut m = manifest(&us, policy_json("ignore", 0));
            m["transport_policy"][k] = v;
            assert!(policy_parity(&m, PROD_SRC).is_err(), "{k}");
        }
        // production の retry 変更は tripwire 側でも parity 側でも検出される
        assert!(policy_parity(&manifest(&us, policy_json("ignore", 0)), &PROD_SRC.replacen("from_secs(30)", "from_secs(31)", 1)).is_err());
    }

    // ── 4. 全体 run: 順序・concurrency・artifact SHA・key 非残存 ──
    fn full_run(tag: &str, lb: &Loopback, tamper: Option<fn(&str) -> String>) -> (PathBuf, Vec<UnitIn>, WireTransport) {
        let us = units();
        let dir = tmp(tag);
        let mut g = Generator::begin(&dir, manifest(&us, policy_json("ignore", 0)), us.clone(), clock(), Some(KEY.into())).unwrap();
        let mut t = WireTransport::new(&lb.addr, KEY, short());
        t.tamper = tamper;
        g.run_primary(&mut t).unwrap();
        g.commit_recovery_set().unwrap();
        g.run_recovery(&mut t).unwrap();
        g.seal(&BTreeMap::from([("LANG".into(), "ja_JP".into()), ("TMDB_API_KEY".into(), KEY.into())])).unwrap();
        (dir, us, t)
    }

    /// artifact の attempts を展開した順の (canonical_request_sha256) 列
    fn artifact_sequence(dir: &Path) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for rec in tmdb_requests(dir) {
            let sha = rec["canonical_request_sha256"].as_str().unwrap().to_string();
            let id = rec["request_id"].as_str().unwrap().to_string();
            for _ in rec["attempts"].as_array().unwrap() {
                out.push((id.clone(), sha.clone()));
            }
        }
        out
    }

    #[test]
    fn artifact_request_sha_equals_the_sha_of_what_was_actually_sent_in_the_same_order() {
        let lb = Loopback::start(vec![]);
        let (dir, us, t) = full_run("sha", &lb, None);
        assert!(t.stops.is_empty());
        let sent = lb.seen.lock().unwrap().clone();
        let art = artifact_sequence(&dir);
        assert!(!art.is_empty());
        assert_eq!(sent.len(), art.len(), "artifact の attempt 数と実送信数が違います");
        for (s, (_id, want)) in sent.iter().zip(&art) {
            let (v, key, _) = reconstruct(&s.target).unwrap();
            assert_eq!(&sha_v(&v), want, "{}", redact(&s.target));
            assert_eq!(key, KEY);
        }
        // 順序: 計画（planned_specs を unit 順に）と一致
        let mut planned = Vec::new();
        for u in &us {
            let snap = snapshot_from_payload(&u.payload, 1, EvidenceClass::Live).unwrap();
            for s in planned_specs(&snap.local_evidence()) {
                planned.push(s);
            }
        }
        let first_attempt: Vec<String> = tmdb_requests(&dir).iter().map(|r| r["request_id"].as_str().unwrap().to_string()).collect();
        assert!(first_attempt.iter().any(|id| id.starts_with("work:1#")) && !first_attempt.iter().any(|id| id.starts_with("work:3#")), "work:3 は 0 call のはず");
        let keys_in_order: Vec<&str> = first_attempt.iter().map(|id| id.split('#').next().unwrap()).collect();
        let mut sorted = keys_in_order.clone();
        sorted.sort();
        assert_eq!(keys_in_order, sorted, "unit 順に送られていません");
        assert_eq!(t.sent, sent.len());
        // 実送信した path + 固定 param
        for s in &sent {
            assert!(s.target.starts_with("/3/search/movie?") || s.target.starts_with("/3/search/tv?"));
        }
        // concurrency=1（server 側で同時 in-flight を数えた）
        assert_eq!(lb.max_inflight.load(Ordering::SeqCst), 1);
        verify_and_replay(&dir, &us).unwrap();
    }

    #[test]
    fn the_api_key_is_sent_but_never_left_in_any_artifact_or_debug_output() {
        let lb = Loopback::start(vec![]);
        let (dir, _us, t) = full_run("key", &lb, None);
        assert!(lb.targets().iter().all(|x| x.contains(&format!("api_key={KEY}"))), "key は送信に使われる");
        let text = all_text(&dir);
        assert!(!text.contains(KEY), "artifact に key が残っています");
        assert!(!text.to_lowercase().contains("api_key="), "artifact に api_key= が残っています");
        assert!(!text.contains("127.0.0.1"), "loopback の addr が artifact に残っています");
        assert!(!format!("{t:?}").contains(KEY));
        // 証跡として残せる形（redact 済み）には key が無く、SHA は送信内容に結び付く
        for x in lb.targets() {
            let r = redact(&x);
            assert!(!r.contains(KEY) && r.contains("api_key=<redacted>"));
        }
    }

    #[test]
    fn a_transport_that_sends_something_other_than_the_artifact_request_is_detected() {
        // 実送信を書き換える（language を落とす）と、artifact の SHA と一致しなくなる
        let lb = Loopback::start(vec![]);
        let (dir, _us, _t) = full_run("tamper", &lb, Some(|t| t.replacen("&language=ja-JP", "&language=en-US", 1)));
        let sent = lb.seen.lock().unwrap().clone();
        let art = artifact_sequence(&dir);
        let mismatches = sent.iter().zip(&art).filter(|(s, (_, want))| sha_v(&reconstruct(&s.target).unwrap().0) != *want).count();
        assert_eq!(mismatches, sent.len(), "改ざんが 1 件も検出されない");
    }

    // ── 5. retry / backoff / timeout（manifest 固定の policy） ──
    fn first_req(dir: &Path) -> Value {
        tmdb_requests(dir).remove(0)
    }
    fn attempts(rec: &Value) -> Vec<(u64, String, Option<u64>)> {
        rec["attempts"].as_array().unwrap().iter().map(|a| (a["n"].as_u64().unwrap(), a["outcome"].as_str().unwrap().to_string(), a["backoff_ms"].as_u64())).collect()
    }

    #[test]
    fn http_429_is_retried_with_the_pinned_backoff_then_succeeds() {
        let (dir, t, lb) = run_one("r429", vec![Act::Status(429, None), Act::Status(429, None)], policy_json("ignore", 0), short(), "Alpha Movie 1999");
        let rec = first_req(&dir);
        assert_eq!(rec["final_status"], "ok");
        assert_eq!(attempts(&rec).len(), 3);
        assert_eq!(t.pauses[..2], [400, 800]);
        let ts = lb.targets();
        assert_eq!(ts[0], ts[1]);
        assert_eq!(ts[1], ts[2], "同じ request の再送のはず");
        assert_eq!(lb.max_inflight.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn http_5xx_exhausts_exactly_max_attempts_with_two_pauses_and_no_pause_after_the_last() {
        for code in [500u16, 502, 503, 504] {
            let (dir, t, lb) = run_one(&format!("r{code}"), (0..3).map(|_| Act::Status(code, None)).collect(), policy_json("ignore", 0), short(), "Alpha Movie 1999");
            let rec = first_req(&dir);
            assert_eq!(rec["final_status"], "http_error", "{code}");
            assert_eq!(attempts(&rec), vec![(1, "response".into(), Some(400)), (2, "response".into(), Some(800)), (3, "response".into(), None)]);
            assert_eq!(t.pauses[..2], [400, 800]);
            assert_eq!(lb.targets().iter().take(3).collect::<std::collections::BTreeSet<_>>().len(), 1);
            assert_eq!(lb.remaining_script(), 0);
        }
    }

    #[test]
    fn non_retryable_statuses_are_sent_once() {
        for code in [400u16, 401, 404] {
            let (dir, t, lb) = run_one(&format!("n{code}"), vec![Act::Status(code, None)], policy_json("ignore", 0), short(), "Alpha Movie 1999");
            let rec = first_req(&dir);
            assert_eq!(attempts(&rec).len(), 1, "{code}");
            assert_eq!(rec["final_status"], "http_error");
            assert!(t.pauses.is_empty());
            let _ = lb;
        }
    }

    #[test]
    fn a_timeout_is_retried_with_the_pinned_backoff_and_ends_as_network_error() {
        let (dir, t, lb) = run_one("tmo", vec![Act::Hang, Act::Hang, Act::Hang], policy_json("ignore", 0), Duration::from_millis(150), "Alpha Movie 1999");
        let rec = first_req(&dir);
        assert_eq!(rec["final_status"], "network_error");
        assert_eq!(attempts(&rec), vec![(1, "timeout".into(), Some(400)), (2, "timeout".into(), Some(800)), (3, "timeout".into(), None)]);
        assert_eq!(t.pauses[..2], [400, 800]);
        assert!(lb.targets().len() >= 3);
        assert_eq!(lb.max_inflight.load(Ordering::SeqCst), 1, "timeout した接続が残ったまま次を送っていない");
    }

    #[test]
    fn a_timeout_then_success_recovers_within_the_budget() {
        let (dir, t, _lb) = run_one("tmo_ok", vec![Act::Hang], policy_json("ignore", 0), Duration::from_millis(150), "Alpha Movie 1999");
        let rec = first_req(&dir);
        assert_eq!(rec["final_status"], "ok");
        assert_eq!(attempts(&rec)[0], (1, "timeout".into(), Some(400)));
        assert_eq!(t.pauses[0], 400);
    }

    #[test]
    fn a_dropped_connection_is_retried_like_any_network_error() {
        let (dir, t, _lb) = run_one("drop", vec![Act::Drop, Act::Drop, Act::Drop], policy_json("ignore", 0), short(), "Alpha Movie 1999");
        let rec = first_req(&dir);
        assert_eq!(rec["final_status"], "network_error");
        assert_eq!(attempts(&rec).len(), 3);
        assert_eq!(t.pauses[..2], [400, 800]);
    }

    #[test]
    fn connection_refused_is_a_connect_error_with_the_same_budget() {
        let dead = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().to_string()
        };
        let us = vec![unit(1, Some("Alpha Movie 1999"), "Alpha Movie 1999.mkv")];
        let dir = tmp("refused");
        let mut g = Generator::begin(&dir, manifest(&us, policy_json("ignore", 0)), us, clock(), Some(KEY.into())).unwrap();
        let mut t = WireTransport::new(&dead, KEY, Duration::from_secs(8));
        g.run_primary(&mut t).unwrap();
        let rec = first_req(&dir);
        assert_eq!(attempts(&rec).iter().map(|a| a.1.as_str()).collect::<Vec<_>>(), ["connect", "connect", "connect"]);
        assert_eq!(t.pauses[..2], [400, 800]);
    }

    #[test]
    fn retry_after_is_ignored_or_honored_only_as_the_manifest_says_and_is_capped() {
        // ignore: Retry-After があっても 400
        let (_d, t, _) = run_one("ra_ign", vec![Act::Status(429, Some(7))], policy_json("ignore", 0), short(), "Alpha Movie 1999");
        assert_eq!(t.pauses[0], 400);
        // honor_capped: max(backoff, min(Retry-After, cap))
        let (_d, t, _) = run_one("ra_hon", vec![Act::Status(429, Some(2))], policy_json("honor_capped", 5000), short(), "Alpha Movie 1999");
        assert_eq!(t.pauses[0], 2000);
        let (_d, t, _) = run_one("ra_cap", vec![Act::Status(429, Some(60))], policy_json("honor_capped", 5000), short(), "Alpha Movie 1999");
        assert_eq!(t.pauses[0], 5000);
    }

    #[test]
    fn a_tampered_policy_is_refused_before_any_request_is_sent() {
        let lb = Loopback::start(vec![]);
        let us = vec![unit(1, Some("Alpha Movie 1999"), "Alpha Movie 1999.mkv")];
        let mut m = manifest(&us, policy_json("ignore", 0));
        m["transport_policy"]["max_attempts"] = json!(5);
        assert!(Generator::begin(&tmp("tp"), m, us.clone(), clock(), Some(KEY.into())).is_err());
        let mut m = manifest(&us, policy_json("ignore", 0));
        m["transport_policy"]["concurrency"] = json!(4);
        assert!(Generator::begin(&tmp("tp2"), m, us, clock(), Some(KEY.into())).is_err());
        assert!(lb.targets().is_empty());
    }

    #[test]
    fn production_urlencoding_is_the_one_the_wire_builder_uses() {
        // visibility だけ公開した production の関数を直接呼んで、wire builder と同じ出力であることを確認
        for q in ["a b", "日本語", "A&B", "😀", "~-_."] {
            let t = wire_target(&req("movie", q, None), KEY).unwrap();
            assert!(t.contains(&format!("&query={}&language", urlencoding(q))));
        }
        assert_eq!(urlencoding("a b&c"), "a+b%26c");
        assert_eq!(urlencoding("日"), "%E6%97%A5");
        assert_eq!(urlencoding("😀"), "%F0%9F%98%80");
    }
}
