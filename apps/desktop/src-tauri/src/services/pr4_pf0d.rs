//! PR4-PF0D: H2 蓄積の feasibility census（**count-only・手動専用・test ビルドだけ・製品には出ない**）。
//!
//! H2 を実際に集める作業ではない。**供給の速さと、現在のプールの規模を件数だけで確かめる。** 出力は月別の件数と全体の件数だけで、
//! ID・判定・候補・性能・ラベルの値は出さない（読まない）。入力は PF0A と同じく、recovery 済みの immutable snapshot だけ
//! （production の DB を SQLite として開かない）。
//!
//! **注意**: `eligible_now` は H1 時代の述語（ever-strong 除外）の件数で、**H2 の enrollment predicate ではない**。
//! H2 は「T0 以後に母集団へ初めて入った時点で登録」し、strong ラベル等を selection に使わない（PF0D 契約）。
//! このモジュールは `#[cfg(test)]` かつ手動専用で、製品には出ない。
//!
//! # 出力
//! - `live_cohort_total` / `ever_strong_total` / `eligible_now`（= PF0A と同じ述語 `build_frame(conn, "live", "")` の件数）
//! - `h1_commitment_matches`: 現在の eligible の集合が H1 の commitment と一致するか（一致すれば、H1 の外に、いま eligible な live 作品は無い）
//! - 月別: 作成月ごとの live cohort の作品数 / うち strong ラベルが一度でも付いた数 / うち現在 eligible の数
//! - 直近 12 か月の live 作品の流入（合計・月平均）
//!
//! # 入力（環境変数）
//! `CINEMANTIS_PR4_PF0D_ACTION=census` / `_SNAPSHOT_DB` / `_SNAPSHOT_SHA256` / `_OUTPUT_DIR` / `_SOURCE_HEAD`。production の DB のパスを受け取る入口は無い。

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::services::gt_sampling::build_frame;
use crate::services::pr4_3v_runner::open_snapshot;
use crate::services::pr4_pf0a::{eligibility_rule_version, membership_hash, preflight, Config, Stop};

pub const PROTOCOL: &str = "pr4-pf0d-h2-feasibility-census-1";
/// PF0A で固定した H1 の membership commitment
pub const H1_COMMITMENT: &str = "22cc00a81f3bf58049c1aaf45668e4fdf7b678662cf9c99cf2b591a8f983e031";
const ENV_VARS: [&str; 5] = [
    "CINEMANTIS_PR4_PF0D_ACTION",
    "CINEMANTIS_PR4_PF0D_SNAPSHOT_DB",
    "CINEMANTIS_PR4_PF0D_SNAPSHOT_SHA256",
    "CINEMANTIS_PR4_PF0D_OUTPUT_DIR",
    "CINEMANTIS_PR4_PF0D_SOURCE_HEAD",
];
const OUTPUT_KEYS: [&str; 13] = [
    "protocol",
    "generated_at_utc",
    "source_head",
    "snapshot_sha256",
    "live_cohort_total",
    "ever_strong_total",
    "eligible_now",
    "h1_commitment_matches",
    "eligible_outside_h1",
    "monthly",
    "trailing_12_months",
    "eligibility_rule_version",
    "limits",
];

fn cohort_cte() -> Result<String, Stop> {
    let src = include_str!("gt_sampling.rs");
    let marker = "const COHORT_CTE: &str = \"";
    let start = src.find(marker).ok_or_else(|| Stop("COHORT_CTE が見つかりません".into()))? + marker.len();
    let end = src[start..].find("\";").ok_or_else(|| Stop("COHORT_CTE の終わりが見つかりません".into()))? + start;
    let cte = &src[start..end];
    if !cte.contains("evidence_class") || cte.contains('\\') {
        return Err(Stop("COHORT_CTE を安全に取り出せません".into()));
    }
    Ok(cte.to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Month {
    pub month: String,
    pub live_works: i64,
    pub ever_strong: i64,
    pub eligible_now: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Census {
    pub live_cohort_total: i64,
    pub ever_strong_total: i64,
    pub eligible_now: i64,
    pub h1_commitment_matches: bool,
    pub months: Vec<Month>,
}

/// 件数だけを集計する。work ID はメモリの中で eligible かどうかの照合にだけ使い、出力しない。
pub fn compute(conn: &Connection) -> Result<Census, Stop> {
    let cte = cohort_cte()?;
    let eligible: Vec<i64> = build_frame(conn, "live", "").map_err(|e| Stop(format!("frame を作れません: {e:?}")))?.into_iter().map(|r| r.work_id).collect();
    let eligible_set: HashSet<i64> = eligible.iter().copied().collect();
    let h1_matches = membership_hash(&eligible, &eligibility_rule_version()) == H1_COMMITMENT;
    // 作品の ID・作成月・strong の有無（存在判定だけ。ラベルの列の値は取り出さない）
    let mut stmt = conn
        .prepare(&format!(
            "{cte} SELECT w.id, substr(w.created_at, 1, 7),
                    EXISTS (SELECT 1 FROM metadata_match_labels l WHERE l.work_id = c.work_id AND l.strength = 'strong')
               FROM cohort c JOIN works w ON w.id = c.work_id WHERE c.evidence_class = 'live'"
        ))
        .map_err(|e| Stop(e.to_string()))?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)? != 0))).map_err(|e| Stop(e.to_string()))?;
    let mut by_month: BTreeMap<String, Month> = BTreeMap::new();
    let (mut total, mut strong) = (0i64, 0i64);
    for row in rows {
        let (id, month, is_strong) = row.map_err(|e| Stop(e.to_string()))?;
        let key = month.filter(|m| m.len() == 7).unwrap_or_else(|| "unknown".to_string());
        let m = by_month.entry(key.clone()).or_insert(Month { month: key, live_works: 0, ever_strong: 0, eligible_now: 0 });
        m.live_works += 1;
        total += 1;
        if is_strong {
            m.ever_strong += 1;
            strong += 1;
        }
        if eligible_set.contains(&id) {
            m.eligible_now += 1;
        }
    }
    Ok(Census { live_cohort_total: total, ever_strong_total: strong, eligible_now: eligible.len() as i64, h1_commitment_matches: h1_matches, months: by_month.into_values().collect() })
}

/// 直近 12 か月（観測の最後の月から数える）の流入。月は `YYYY-MM`。`unknown` は含めない
fn trailing_12(months: &[Month]) -> Value {
    let dated: Vec<&Month> = months.iter().filter(|m| m.month != "unknown").collect();
    let Some(last) = dated.last() else { return json!({"months_window": 0, "live_works_created": 0, "mean_per_month": null}) };
    let parse = |s: &str| -> Option<i64> { Some(s.get(0..4)?.parse::<i64>().ok()? * 12 + s.get(5..7)?.parse::<i64>().ok()? - 1) };
    let end = parse(&last.month).unwrap_or(0);
    let window: i64 = dated.iter().filter(|m| parse(&m.month).is_some_and(|x| x > end - 12)).map(|m| m.live_works).sum();
    json!({"window_ends": last.month, "months_window": 12, "live_works_created": window, "mean_per_month": window as f64 / 12.0})
}

pub fn output_json(c: &Census, cfg: &Config, snapshot_sha: &str, generated_at: &str) -> Value {
    json!({
        "protocol": PROTOCOL,
        "generated_at_utc": generated_at,
        "source_head": cfg.source_head,
        "snapshot_sha256": snapshot_sha,
        "live_cohort_total": c.live_cohort_total,
        "ever_strong_total": c.ever_strong_total,
        "eligible_now": c.eligible_now,
        "h1_commitment_matches": c.h1_commitment_matches,
        "eligible_outside_h1": if c.h1_commitment_matches { json!(0) } else { Value::Null },
        "monthly": c.months.iter().map(|m| json!({"month": m.month, "live_works": m.live_works, "ever_strong": m.ever_strong, "eligible_now": m.eligible_now})).collect::<Vec<_>>(),
        "trailing_12_months": trailing_12(&c.months),
        "eligibility_rule_version": eligibility_rule_version(),
        "limits": "count-only; no identifiers, no judgments, no performance figures; not a holdout evaluation; does not enrol anything",
    })
}

/// 出力に ID・判定・候補・ラベルの値が無いこと（キーの形が固定で、月別の項目も件数だけ）
pub fn assert_output_clean(v: &Value) -> Result<(), Stop> {
    let obj = v.as_object().ok_or_else(|| Stop("出力が object ではありません".into()))?;
    if obj.len() != OUTPUT_KEYS.len() || !OUTPUT_KEYS.iter().all(|k| obj.contains_key(*k)) {
        return Err(Stop("出力の項目が想定と違います".into()));
    }
    for m in obj["monthly"].as_array().ok_or_else(|| Stop("monthly が配列ではありません".into()))? {
        let o = m.as_object().ok_or_else(|| Stop("monthly の項目が不正です".into()))?;
        if o.len() != 4 || !["month", "live_works", "ever_strong", "eligible_now"].iter().all(|k| o.contains_key(*k)) {
            return Err(Stop("monthly の項目が想定と違います".into()));
        }
    }
    let text = v.to_string();
    for banned in ["work:", "work_id", "task_id", "tmdb", "title", "verdict", "candidate", "decision", "precision"] {
        if text.contains(banned) {
            return Err(Stop(format!("出力に {banned} が含まれています")));
        }
    }
    Ok(())
}

pub fn run(cfg: &Config, generated_at: &str, run_id: &str) -> Result<(PathBuf, Value), Stop> {
    if run_id.is_empty() || !run_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(Stop("run id が不正です".into()));
    }
    let sha = preflight(cfg)?;
    let conn = open_snapshot(&cfg.snapshot_db).map_err(|e| Stop(e.0))?;
    let c = compute(&conn)?;
    drop(conn);
    let out = output_json(&c, cfg, &sha, generated_at);
    assert_output_clean(&out)?;
    let dir = cfg.output_dir.join(run_id);
    if dir.exists() {
        return Err(Stop("出力先が既にあります（上書きしない）".into()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| Stop(e.to_string()))?;
    std::fs::write(dir.join("h2_feasibility_census.json"), serde_json::to_string_pretty(&out).unwrap_or_default() + "\n").map_err(|e| Stop(e.to_string()))?;
    Ok((dir, out))
}

fn config_from_env(get: &dyn Fn(&str) -> Option<String>) -> Result<Config, Stop> {
    let need = |n: &str| -> Result<String, Stop> {
        match get(n) {
            Some(v) if !v.trim().is_empty() => Ok(v),
            _ => Err(Stop(format!("環境変数 {n} が未設定です"))),
        }
    };
    if need(ENV_VARS[0])? != "census" {
        return Err(Stop("この手順は CINEMANTIS_PR4_PF0D_ACTION=census のときだけ動きます".into()));
    }
    Ok(Config {
        snapshot_db: PathBuf::from(need(ENV_VARS[1])?),
        expected_snapshot_sha256: need(ENV_VARS[2])?,
        output_dir: PathBuf::from(need(ENV_VARS[3])?),
        source_head: need(ENV_VARS[4])?,
    })
}

fn utc_now() -> (String, String) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let (y, doy) = (yoe + era * 400, doe - (365 * yoe + yoe / 4 - yoe / 100));
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let iso = format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60);
    let id = iso.chars().filter(|c| c.is_ascii_digit() || *c == 'T' || *c == 'Z').collect();
    (iso, id)
}

#[test]
#[ignore = "manual: needs a recovered immutable snapshot; CINEMANTIS_PR4_PF0D_ACTION=census"]
fn manual_pr4_pf0d_census() {
    let cfg = config_from_env(&|n| std::env::var(n).ok()).unwrap_or_else(|e| panic!("STOP: {}", e.0));
    let (iso, id) = utc_now();
    let (dir, out) = run(&cfg, &iso, &id).unwrap_or_else(|e| panic!("STOP: {}", e.0));
    println!("live_cohort_total = {}", out["live_cohort_total"]);
    println!("ever_strong_total = {}", out["ever_strong_total"]);
    println!("eligible_now = {}", out["eligible_now"]);
    println!("h1_commitment_matches = {}", out["h1_commitment_matches"]);
    println!("trailing_12_months = {}", out["trailing_12_months"]);
    println!("output = {}", dir.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::scan::insert_scanned_work;
    use crate::db::test_support::{insert_source, open_migrated};
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn live_work(conn: &Connection, index: usize, month: &str) -> i64 {
        let root = r"D:\Live";
        let source_id = conn.query_row("SELECT id FROM sources WHERE root_path = ?1", [root], |r| r.get::<_, i64>(0)).unwrap_or_else(|_| insert_source(conn, root));
        let work = insert_scanned_work(conn, "movie", "unknown", &format!("w{index}"), "unknown").unwrap();
        conn.execute("INSERT INTO files (source_id, file_path, file_name, extension, container) VALUES (?1, ?2, ?3, 'mkv', 'mkv')", rusqlite::params![source_id, format!(r"D:\Live\{index}.mkv"), format!("{index}.mkv")]).unwrap();
        let file = conn.last_insert_rowid();
        conn.execute("INSERT INTO work_parts (work_id, file_id) VALUES (?1, ?2)", rusqlite::params![work, file]).unwrap();
        conn.execute("UPDATE files SET original_file_name = file_name, original_rel_path = file_name, original_captured = 1 WHERE id = ?1", [file]).unwrap();
        conn.execute("UPDATE works SET created_at = ?2 WHERE id = ?1", rusqlite::params![work, format!("{month}-15T00:00:00.000Z")]).unwrap();
        work
    }

    fn strong(conn: &Connection, work: i64) {
        conn.execute("INSERT INTO metadata_match_labels (work_id, label, tmdb_id, media_type, method, strength) VALUES (?1, 'tmdb', 1, 'movie', 'manual_apply', 'strong')", [work]).unwrap();
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pr4_pf0d_{tag}_{}_{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn fixture() -> Connection {
        let conn = open_migrated();
        // 2026-01: 3 件（1 件 strong）/ 2026-02: 2 件 / 2026-03: 4 件（2 件 strong）。全 live
        let a: Vec<i64> = (0..3).map(|i| live_work(&conn, i, "2026-01")).collect();
        for i in 0..2 {
            live_work(&conn, 10 + i, "2026-02");
        }
        let c: Vec<i64> = (0..4).map(|i| live_work(&conn, 20 + i, "2026-03")).collect();
        strong(&conn, a[0]);
        strong(&conn, c[0]);
        strong(&conn, c[1]);
        conn
    }

    fn cfg_for(conn: &Connection) -> Config {
        let dir = tmp("cfg");
        let path = dir.join("snapshot.db");
        conn.execute_batch(&format!("VACUUM INTO '{}'", path.to_string_lossy().replace('\'', "''"))).unwrap();
        let sha = format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap()));
        Config { snapshot_db: path, expected_snapshot_sha256: sha, output_dir: dir.join("out"), source_head: "5be38a1fda7c3c7172945df3811868b479cb4d23".to_string() }
    }

    #[test]
    fn the_census_counts_monthly_live_works_strong_labels_and_eligibility() {
        let conn = fixture();
        let c = compute(&conn).unwrap();
        assert_eq!((c.live_cohort_total, c.ever_strong_total, c.eligible_now), (9, 3, 6));
        assert_eq!(c.months, vec![
            Month { month: "2026-01".into(), live_works: 3, ever_strong: 1, eligible_now: 2 },
            Month { month: "2026-02".into(), live_works: 2, ever_strong: 0, eligible_now: 2 },
            Month { month: "2026-03".into(), live_works: 4, ever_strong: 2, eligible_now: 2 },
        ]);
        assert!(!c.h1_commitment_matches, "合成データは H1 とは一致しない");
    }

    #[test]
    fn the_trailing_12_month_inflow_uses_the_last_observed_month() {
        let m = |s: &str, n: i64| Month { month: s.into(), live_works: n, ever_strong: 0, eligible_now: 0 };
        let months = vec![m("2025-03", 100), m("2025-04", 12), m("2026-03", 24), m("unknown", 999)];
        let t = trailing_12(&months);
        assert_eq!(t["window_ends"], "2026-03");
        assert_eq!(t["live_works_created"], 36, "2025-04..2026-03 の 12 か月だけ（2025-03 と unknown は含めない）");
        assert!((t["mean_per_month"].as_f64().unwrap() - 3.0).abs() < 1e-12);
        assert!(trailing_12(&[])["mean_per_month"].is_null());
    }

    #[test]
    fn the_output_is_counts_only_and_the_checker_rejects_anything_else() {
        let conn = fixture();
        let cfg = cfg_for(&conn);
        let (dir, out) = run(&cfg, "2026-10-07T00:00:00Z", "RUN1").unwrap();
        assert!(assert_output_clean(&out).is_ok());
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(files.len(), 1);
        let text = std::fs::read_to_string(dir.join("h2_feasibility_census.json")).unwrap();
        for banned in ["work:", "work_id", "task_id", "tmdb", "title", "verdict", "candidate", "decision", "precision"] {
            assert!(!text.contains(banned), "{banned}");
        }
        assert_eq!(out["live_cohort_total"], 9);
        assert_eq!(out["eligible_outside_h1"], Value::Null, "H1 と一致しないときは、H1 の外の件数を断定しない");
        let mut extra = out.clone();
        extra["per_unit"] = json!([1]);
        let mut bad_month = out.clone();
        bad_month["monthly"][0]["work_ids"] = json!([1]);
        for bad in [extra, bad_month] {
            assert!(assert_output_clean(&bad).is_err());
        }
        assert!(run(&cfg, "t", "RUN1").is_err(), "上書きしない");
    }

    #[test]
    fn preflight_and_environment_checks_stop_unsafe_inputs() {
        let conn = fixture();
        let cfg = cfg_for(&conn);
        let mut c = cfg.clone();
        c.expected_snapshot_sha256 = "0".repeat(64);
        assert!(run(&c, "t", "R").is_err());
        let mut n = cfg.snapshot_db.as_os_str().to_os_string();
        n.push("-wal");
        std::fs::write(PathBuf::from(n), b"").unwrap();
        assert!(run(&cfg, "t", "R").is_err(), "隣に -wal があれば STOP");
        let all = |drop: Option<&str>, action: &str| config_from_env(&|k| if Some(k) == drop { None } else if k == ENV_VARS[0] { Some(action.to_string()) } else { Some("x".to_string()) });
        assert!(all(None, "census").is_ok() && all(None, "other").is_err());
        for k in ENV_VARS {
            assert!(all(Some(k), "census").is_err(), "{k}");
        }
        assert!(ENV_VARS.iter().all(|k| !k.to_lowercase().contains("production")));
    }

    #[test]
    fn the_source_enrols_nothing_writes_nothing_and_reads_no_label_values() {
        let src = include_str!("pr4_pf0d.rs");
        let code = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        let body: String = code.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        assert_eq!(body.matches("metadata_match_labels").count(), 1, "ラベルは EXISTS の存在判定だけ");
        for banned in ["tmdb_id", "metadata_match_verdicts", "metadata_match_candidates", "metadata_review_tasks", "reasons_json", "INSERT ", "UPDATE ", "DELETE ", "DROP ", "VACUUM", "Command", "reqwest", "TcpStream", "std::env::set"] {
            assert!(!body.contains(banned), "{banned}");
        }
        assert!(include_str!("mod.rs").contains("#[cfg(test)]\npub mod pr4_pf0d;"));
    }
}
