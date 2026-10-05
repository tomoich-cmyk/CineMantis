//! PR4-PF0A: holdout の件数と所属の commitment（**count-only・手動専用・製品には出ない**）。
//!
//! holdout を開封する作業ではない。「いまの live cohort の規模」と、「現在 eligible な評価単位の集合を固定する commitment（ハッシュ）」だけを作る。
//! **出力はこの 6 項目だけ**（+ 実行の記録）: `holdout_source_count` / `excluded_ever_strong_count` / `current_eligible_count` /
//! `membership_commitment_sha256` / `eligibility_rule_version` / `snapshot_sha256`。
//!
//! # 読まないもの・出さないもの
//!
//! 題名・work ID・task ID の一覧 / verdict / candidate / machine decision / ラベルの値・method / GT の内容 / 評価単位ごとの結果 / precision。
//! ラベルは「strong が一度でも付いたか」の存在判定（`EXISTS`）にだけ使い、列の値は取り出さない。work ID は commitment のハッシュを計算する間だけメモリに持ち、
//! 出力・保存・表示しない。
//!
//! # eligibility（既存の述語をそのまま使う）
//!
//! `gt_sampling::build_frame(conn, "live", "")`（= 既存の抽出の母集団の述語。cohort = live、file あり・利用可、source が online、他の sample で未抽出、
//! **一度でも strong ラベルが付いた作品は除外（superseded を含む）**）。source の件数と strong の件数には、同じソースファイルの `COHORT_CTE` を使う
//! （`include_str!` で取り出す。コピーしない）。`eligibility_rule_version` は、この述語を含むソース全体の SHA-256 を含む。
//!
//! # commitment
//!
//! キー = `work:<work_id>`（holdout はまだ抽出されておらず、評価単位のキー `{sample_id, task_id, work_id}` は存在しない。抽出時の frame の主キーは work_id）。
//! 現在 eligible な全キーを辞書順に並べ、`pr4-pf0a-membership-1\n` + `eligibility_rule_version\n` + `key\n`…の SHA-256。
//! **この commitment を作った時点から、最終評価の終了まで membership を変えない**（policy freeze に入れる）。production がその後変化しても、
//! 最終評価はこの commitment と snapshot の境界を基準にする。
//!
//! # 入力（環境変数。production の DB のパスを受け取る入口は無い）
//!
//! `CINEMANTIS_PR4_PF0A_ACTION=commit_cardinality` / `_SNAPSHOT_DB`（recovery 済みの immutable snapshot）/ `_SNAPSHOT_SHA256`（期待する SHA-256）/
//! `_OUTPUT_DIR` / `_SOURCE_HEAD`（40 桁）。

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::services::gt_sampling::build_frame;
use crate::services::pr4_3v_runner::open_snapshot;

pub const PROTOCOL: &str = "pr4-pf0a-holdout-cardinality-commitment-1";
const MEMBERSHIP_DOMAIN: &str = "pr4-pf0a-membership-1";
const PRODUCTION_DATA_DIR_NAME: &str = "dev.cinemantis.app";
const ENV_VARS: [&str; 5] = [
    "CINEMANTIS_PR4_PF0A_ACTION",
    "CINEMANTIS_PR4_PF0A_SNAPSHOT_DB",
    "CINEMANTIS_PR4_PF0A_SNAPSHOT_SHA256",
    "CINEMANTIS_PR4_PF0A_OUTPUT_DIR",
    "CINEMANTIS_PR4_PF0A_SOURCE_HEAD",
];
/// 出力の項目（これ以外は書かない）
const OUTPUT_KEYS: [&str; 10] = [
    "protocol",
    "generated_at_utc",
    "source_head",
    "holdout_source_count",
    "excluded_ever_strong_count",
    "current_eligible_count",
    "membership_commitment_sha256",
    "eligibility_rule_version",
    "snapshot_sha256",
    "limits",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop(pub String);

fn stop<T>(m: impl Into<String>) -> Result<T, Stop> {
    Err(Stop(m.into()))
}

fn hex_sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `gt_sampling.rs` の `COHORT_CTE` を、ソースから取り出す（コピーしない）
fn cohort_cte() -> Result<String, Stop> {
    let src = include_str!("gt_sampling.rs");
    let start = src.find("const COHORT_CTE: &str = \"").ok_or_else(|| Stop("COHORT_CTE が見つかりません".to_string()))? + "const COHORT_CTE: &str = \"".len();
    let end = src[start..].find("\";").ok_or_else(|| Stop("COHORT_CTE の終わりが見つかりません".to_string()))? + start;
    let cte = &src[start..end];
    if !cte.contains("evidence_class") || cte.contains('\\') {
        return stop("COHORT_CTE を安全に取り出せません");
    }
    Ok(cte.to_string())
}

/// 述語を含むソース全体の SHA-256 を含む版名
pub fn eligibility_rule_version() -> String {
    format!("pr4-pf0a-eligibility-1/gt_sampling_sha256={}", hex_sha(include_str!("gt_sampling.rs").as_bytes()))
}

/// 件数と commitment。**キーの一覧は持ち出さない**（この構造体にも無い）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commitment {
    pub holdout_source_count: i64,
    pub excluded_ever_strong_count: i64,
    pub current_eligible_count: i64,
    pub membership_commitment_sha256: String,
    pub eligibility_rule_version: String,
}

/// membership のハッシュ。キーは辞書順に並べる（入力の順序に依存しない）
pub fn membership_hash(work_ids: &[i64], version: &str) -> String {
    let mut keys: Vec<String> = work_ids.iter().map(|id| format!("work:{id}")).collect();
    keys.sort();
    let mut pre = format!("{MEMBERSHIP_DOMAIN}\n{version}\n");
    for k in keys {
        pre.push_str(&k);
        pre.push('\n');
    }
    hex_sha(pre.as_bytes())
}

/// 件数と commitment を計算する（読み取り専用の接続だけ）。ラベルは存在判定にだけ使う。
pub fn compute(conn: &Connection) -> Result<Commitment, Stop> {
    let cte = cohort_cte()?;
    let source: i64 = conn
        .query_row(&format!("{cte} SELECT COUNT(*) FROM cohort WHERE evidence_class = 'live'"), [], |r| r.get(0))
        .map_err(|e| Stop(format!("source の件数を数えられません: {e}")))?;
    // 存在判定だけ。ラベルの列（label / tmdb_id / method など）は取り出さない
    let strong: i64 = conn
        .query_row(
            &format!(
                "{cte} SELECT COUNT(*) FROM cohort c WHERE c.evidence_class = 'live'
                    AND EXISTS (SELECT 1 FROM metadata_match_labels l WHERE l.work_id = c.work_id AND l.strength = 'strong')"
            ),
            [],
            |r| r.get(0),
        )
        .map_err(|e| Stop(format!("strong の件数を数えられません: {e}")))?;
    // 既存の抽出の述語そのもの。work ID はここでメモリに持ち、ハッシュにだけ使う
    let ids: Vec<i64> = build_frame(conn, "live", "").map_err(|e| Stop(format!("frame を作れません: {e:?}")))?.into_iter().map(|row| row.work_id).collect();
    let version = eligibility_rule_version();
    Ok(Commitment {
        holdout_source_count: source,
        excluded_ever_strong_count: strong,
        current_eligible_count: ids.len() as i64,
        membership_commitment_sha256: membership_hash(&ids, &version),
        eligibility_rule_version: version,
    })
}

#[derive(Debug, Clone)]
pub struct Config {
    pub snapshot_db: PathBuf,
    pub expected_snapshot_sha256: String,
    pub output_dir: PathBuf,
    pub source_head: String,
}

impl Config {
    pub fn from_env(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, Stop> {
        let need = |n: &str| -> Result<String, Stop> {
            match get(n) {
                Some(v) if !v.trim().is_empty() => Ok(v),
                _ => stop(format!("環境変数 {n} が未設定です")),
            }
        };
        if need(ENV_VARS[0])? != "commit_cardinality" {
            return stop("この手順は CINEMANTIS_PR4_PF0A_ACTION=commit_cardinality のときだけ動きます");
        }
        Ok(Config {
            snapshot_db: PathBuf::from(need(ENV_VARS[1])?),
            expected_snapshot_sha256: need(ENV_VARS[2])?,
            output_dir: PathBuf::from(need(ENV_VARS[3])?),
            source_head: need(ENV_VARS[4])?,
        })
    }
}

fn sidecar_exists(p: &Path, suffix: &str) -> bool {
    let mut n = p.as_os_str().to_os_string();
    n.push(suffix);
    Path::new(&n).exists()
}

/// 実行前の確認（どれか 1 つでも違えば STOP）。production のアプリデータ置き場の中の DB は渡せない。
pub fn preflight(cfg: &Config) -> Result<String, Stop> {
    if !is_hex(&cfg.source_head, 40) || !is_hex(&cfg.expected_snapshot_sha256, 64) {
        return stop("source HEAD / snapshot SHA-256 の形式が不正です");
    }
    if !cfg.snapshot_db.is_file() {
        return stop("snapshot が存在しません");
    }
    let canonical = cfg.snapshot_db.canonicalize().map_err(|e| Stop(e.to_string()))?;
    if canonical.components().any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case(PRODUCTION_DATA_DIR_NAME)) {
        return stop("snapshot が production のアプリデータ置き場の中にあります");
    }
    if sidecar_exists(&cfg.snapshot_db, "-wal") || sidecar_exists(&cfg.snapshot_db, "-shm") {
        return stop("snapshot の隣に -wal / -shm があります");
    }
    let sha = hex_sha(&std::fs::read(&cfg.snapshot_db).map_err(|e| Stop(e.to_string()))?);
    if sha != cfg.expected_snapshot_sha256 {
        return stop("snapshot の SHA-256 が期待値と違います");
    }
    Ok(sha)
}

/// 出力の JSON（OUTPUT_KEYS だけ。配列・キー一覧を含まない）
pub fn output_json(c: &Commitment, cfg: &Config, snapshot_sha: &str, generated_at: &str) -> Value {
    json!({
        "protocol": PROTOCOL,
        "generated_at_utc": generated_at,
        "source_head": cfg.source_head,
        "holdout_source_count": c.holdout_source_count,
        "excluded_ever_strong_count": c.excluded_ever_strong_count,
        "current_eligible_count": c.current_eligible_count,
        "membership_commitment_sha256": c.membership_commitment_sha256,
        "eligibility_rule_version": c.eligibility_rule_version,
        "snapshot_sha256": snapshot_sha,
        "limits": "count-only; no per-unit data; membership keys are not stored; this is not a holdout evaluation and not a performance result",
    })
}

pub fn assert_output_clean(v: &Value) -> Result<(), Stop> {
    let obj = v.as_object().ok_or_else(|| Stop("出力が object ではありません".to_string()))?;
    if obj.len() != OUTPUT_KEYS.len() || !OUTPUT_KEYS.iter().all(|k| obj.contains_key(*k)) {
        return stop("出力の項目が想定と違います");
    }
    if obj.values().any(|x| x.is_array() || x.is_object()) {
        return stop("出力に配列 / object が含まれています（キーの一覧を出さない）");
    }
    Ok(())
}

pub fn run(cfg: &Config, generated_at: &str, run_id: &str) -> Result<(PathBuf, Value), Stop> {
    if run_id.is_empty() || !run_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return stop("run id が不正です");
    }
    let sha = preflight(cfg)?;
    let conn = open_snapshot(&cfg.snapshot_db).map_err(|e| Stop(e.0))?;
    let c = compute(&conn)?;
    drop(conn);
    let out = output_json(&c, cfg, &sha, generated_at);
    assert_output_clean(&out)?;
    let dir = cfg.output_dir.join(run_id);
    if dir.exists() {
        return stop("出力先が既にあります（上書きしない）");
    }
    std::fs::create_dir_all(&dir).map_err(|e| Stop(e.to_string()))?;
    std::fs::write(dir.join("holdout_cardinality_commitment.json"), serde_json::to_string_pretty(&out).unwrap_or_default() + "\n").map_err(|e| Stop(e.to_string()))?;
    Ok((dir, out))
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
#[ignore = "manual: needs a recovered immutable snapshot; CINEMANTIS_PR4_PF0A_ACTION=commit_cardinality"]
fn manual_pr4_pf0a_commit_cardinality() {
    let cfg = Config::from_env(&|n| std::env::var(n).ok()).unwrap_or_else(|e| panic!("STOP: {}", e.0));
    let (iso, id) = utc_now();
    let (dir, out) = run(&cfg, &iso, &id).unwrap_or_else(|e| panic!("STOP: {}", e.0));
    println!("holdout_source_count = {}", out["holdout_source_count"]);
    println!("excluded_ever_strong_count = {}", out["excluded_ever_strong_count"]);
    println!("current_eligible_count = {}", out["current_eligible_count"]);
    println!("membership_commitment_sha256 = {}", out["membership_commitment_sha256"]);
    println!("eligibility_rule_version = {}", out["eligibility_rule_version"]);
    println!("snapshot_sha256 = {}", out["snapshot_sha256"]);
    println!("output = {}", dir.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::scan::insert_scanned_work;
    use crate::db::test_support::{insert_source, open_migrated};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);
    const HEAD: &str = "30eca9ddd196d20f367ee0c05b044746ce6d3a7e";

    /// live の work（file あり・scan 時に original を取得済み）
    fn live_work(conn: &Connection, index: usize, available: bool) -> i64 {
        make_work(conn, index, available, true)
    }

    fn make_work(conn: &Connection, index: usize, available: bool, captured: bool) -> i64 {
        let root = r"D:\Live";
        let source_id = conn.query_row("SELECT id FROM sources WHERE root_path = ?1", [root], |r| r.get::<_, i64>(0)).unwrap_or_else(|_| insert_source(conn, root));
        let work = insert_scanned_work(conn, "movie", "unknown", &format!("w{index}"), "unknown").unwrap();
        conn.execute("INSERT INTO files (source_id, file_path, file_name, extension, container) VALUES (?1, ?2, ?3, 'mkv', 'mkv')", rusqlite::params![source_id, format!(r"D:\Live\{index}.mkv"), format!("{index}.mkv")]).unwrap();
        let file = conn.last_insert_rowid();
        conn.execute("INSERT INTO work_parts (work_id, file_id) VALUES (?1, ?2)", rusqlite::params![work, file]).unwrap();
        conn.execute("UPDATE files SET original_file_name = file_name, original_rel_path = file_name, original_captured = ?3, availability_status = ?2 WHERE id = ?1", rusqlite::params![file, if available { "available" } else { "missing" }, captured as i64]).unwrap();
        work
    }

    fn reconstructed_work(conn: &Connection, index: usize) -> i64 {
        make_work(conn, 1000 + index, true, false)
    }

    fn label(conn: &Connection, work: i64, strength: &str, method: &str, superseded: bool) {
        conn.execute(
            "INSERT INTO metadata_match_labels (work_id, label, tmdb_id, media_type, method, strength, superseded_at) VALUES (?1, 'tmdb', 1, 'movie', ?2, ?3, ?4)",
            rusqlite::params![work, method, strength, if superseded { Some("2026-01-01T00:00:00Z") } else { None }],
        )
        .unwrap();
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pr4_pf0a_{tag}_{}_{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn fixture_db() -> (Connection, Vec<i64>) {
        let conn = open_migrated();
        // live 6 件: [0] 通常 / [1] strong（superseded でも除外）/ [2] weak（除外しない）/ [3] 利用不可 / [4] 通常 / [5] 通常。+ reconstructed 2 件（source に数えない）
        let w: Vec<i64> = vec![live_work(&conn, 1, true), live_work(&conn, 2, true), live_work(&conn, 3, true), live_work(&conn, 4, false), live_work(&conn, 5, true), live_work(&conn, 6, true)];
        label(&conn, w[1], "strong", "manual_apply", true);
        label(&conn, w[2], "weak", "lock", false);
        reconstructed_work(&conn, 1);
        reconstructed_work(&conn, 2);
        (conn, w)
    }

    fn snapshot_cfg(conn: &Connection) -> Config {
        let dir = tmp("snap");
        let path = dir.join("snapshot.db");
        conn.execute_batch(&format!("VACUUM INTO '{}'", path.to_string_lossy().replace('\'', "''"))).unwrap();
        let sha = hex_sha(&std::fs::read(&path).unwrap());
        Config { snapshot_db: path, expected_snapshot_sha256: sha, output_dir: dir.join("out"), source_head: HEAD.to_string() }
    }

    #[test]
    fn counts_use_the_existing_predicates() {
        let (conn, w) = fixture_db();
        let c = compute(&conn).unwrap();
        assert_eq!(c.holdout_source_count, 6, "live cohort の作品数（reconstructed は数えない）");
        assert_eq!(c.excluded_ever_strong_count, 1, "strong が一度でも付いた（superseded を含む）。weak は数えない");
        assert_eq!(c.current_eligible_count, 4, "6 - strong 1 - 利用不可 1（weak は残る）");
        let frame_ids: Vec<i64> = build_frame(&conn, "live", "").unwrap().into_iter().map(|r| r.work_id).collect();
        assert!(frame_ids.contains(&w[2]) && !frame_ids.contains(&w[1]) && !frame_ids.contains(&w[3]));
        assert!(c.eligibility_rule_version.starts_with("pr4-pf0a-eligibility-1/gt_sampling_sha256=") && c.eligibility_rule_version.len() > 60);
    }

    #[test]
    fn the_commitment_is_deterministic_order_independent_and_sensitive_to_membership() {
        let (conn, _) = fixture_db();
        let a = compute(&conn).unwrap();
        assert_eq!(a, compute(&conn).unwrap());
        // 独立に再計算（辞書順・domain・版名つき）
        let ids: Vec<i64> = build_frame(&conn, "live", "").unwrap().into_iter().map(|r| r.work_id).collect();
        let mut keys: Vec<String> = ids.iter().map(|i| format!("work:{i}")).collect();
        keys.sort();
        let pre = format!("pr4-pf0a-membership-1\n{}\n{}\n", a.eligibility_rule_version, keys.join("\n"));
        assert_eq!(a.membership_commitment_sha256, hex_sha(pre.as_bytes()));
        let mut rev = ids.clone();
        rev.reverse();
        assert_eq!(membership_hash(&rev, &a.eligibility_rule_version), a.membership_commitment_sha256);
        // membership が 1 件変われば commitment も変わる（ラベルの追加 / 作品の追加）
        let (conn2, w2) = fixture_db();
        label(&conn2, w2[0], "strong", "manual_apply", false);
        let b = compute(&conn2).unwrap();
        assert_eq!((b.current_eligible_count, b.excluded_ever_strong_count), (3, 2));
        assert_ne!(b.membership_commitment_sha256, a.membership_commitment_sha256);
        let (conn3, _) = fixture_db();
        live_work(&conn3, 99, true);
        assert_ne!(compute(&conn3).unwrap().membership_commitment_sha256, a.membership_commitment_sha256);
        // 版名が違えば commitment も違う
        assert_ne!(membership_hash(&ids, "other-version"), a.membership_commitment_sha256);
    }

    #[test]
    fn the_output_contains_exactly_the_allowed_items_and_no_key_list() {
        let (conn, w) = fixture_db();
        let cfg = snapshot_cfg(&conn);
        let (dir, out) = run(&cfg, "2026-10-06T00:00:00Z", "RUN1").unwrap();
        assert!(assert_output_clean(&out).is_ok());
        let text = std::fs::read_to_string(dir.join("holdout_cardinality_commitment.json")).unwrap();
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(files.len(), 1, "出力は 1 ファイルだけ");
        let v: Value = serde_json::from_str(&text).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        let mut expected = OUTPUT_KEYS.to_vec();
        expected.sort();
        assert_eq!(keys, expected);
        for banned in ["work_id", "task_id", "title", "verdict", "candidate", "decision", "method", "label", "precision", "tmdb", "sample_id", "\"work:"] {
            assert!(!text.contains(banned), "{banned}");
        }
        // 個別の作品 ID が出力に現れない（ID と一致する長い数字列も無い）
        for id in &w {
            assert!(!text.contains(&format!("work:{id}")));
        }
        assert_eq!(v["holdout_source_count"], 6);
        assert_eq!(v["current_eligible_count"], 4);
        assert_eq!(v["snapshot_sha256"], json!(cfg.expected_snapshot_sha256));
        assert!(v["limits"].as_str().unwrap().contains("not a holdout evaluation"));
        assert!(run(&cfg, "t", "RUN1").is_err(), "同じ run id には上書きしない");
    }

    #[test]
    fn the_checker_rejects_extra_items_and_lists() {
        let (conn, _) = fixture_db();
        let cfg = snapshot_cfg(&conn);
        let c = compute(&conn).unwrap();
        let good = output_json(&c, &cfg, &cfg.expected_snapshot_sha256, "t");
        assert!(assert_output_clean(&good).is_ok());
        let mut extra = good.clone();
        extra["per_unit"] = json!("x");
        let mut list = good.clone();
        list["limits"] = json!(["work:1"]);
        let mut missing = good.clone();
        missing.as_object_mut().unwrap().remove("snapshot_sha256");
        for bad in [extra, list, missing] {
            assert!(assert_output_clean(&bad).is_err());
        }
    }

    #[test]
    fn preflight_stops_on_bad_snapshots_and_missing_environment() {
        let (conn, _) = fixture_db();
        let cfg = snapshot_cfg(&conn);
        assert!(preflight(&cfg).is_ok());
        let mut c = cfg.clone();
        c.expected_snapshot_sha256 = "0".repeat(64);
        assert!(matches!(run(&c, "t", "R"), Err(Stop(m)) if m.contains("SHA-256")));
        let mut c = cfg.clone();
        c.source_head = "short".to_string();
        assert!(run(&c, "t", "R").is_err());
        let mut c = cfg.clone();
        c.snapshot_db = cfg.snapshot_db.with_file_name("none.db");
        assert!(matches!(run(&c, "t", "R"), Err(Stop(m)) if m.contains("存在しません")));
        for suffix in ["-wal", "-shm"] {
            let (conn, _) = fixture_db();
            let g = snapshot_cfg(&conn);
            let mut n = g.snapshot_db.as_os_str().to_os_string();
            n.push(suffix);
            std::fs::write(PathBuf::from(n), b"").unwrap();
            assert!(matches!(run(&g, "t", "R"), Err(Stop(m)) if m.contains("-wal / -shm")), "{suffix}");
            assert!(!g.output_dir.exists());
        }
        let inside = cfg.snapshot_db.parent().unwrap().join(PRODUCTION_DATA_DIR_NAME);
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::copy(&cfg.snapshot_db, inside.join("snapshot.db")).unwrap();
        let mut c = cfg.clone();
        c.snapshot_db = inside.join("snapshot.db");
        assert!(matches!(run(&c, "t", "R"), Err(Stop(m)) if m.contains("production")));
        // 環境変数
        let all = |drop: Option<&str>, action: &str| Config::from_env(&|k| if Some(k) == drop { None } else if k == ENV_VARS[0] { Some(action.to_string()) } else { Some("x".to_string()) });
        assert!(all(None, "commit_cardinality").is_ok());
        assert!(all(None, "other").is_err());
        for k in ENV_VARS {
            assert!(all(Some(k), "commit_cardinality").is_err(), "{k}");
        }
        assert!(ENV_VARS.iter().all(|k| !k.to_lowercase().contains("production")));
    }

    #[test]
    fn the_snapshot_is_never_modified_and_nothing_is_written_outside_the_output_dir() {
        let (conn, _) = fixture_db();
        let cfg = snapshot_cfg(&conn);
        let before = std::fs::read(&cfg.snapshot_db).unwrap();
        let dir = cfg.snapshot_db.parent().unwrap().to_path_buf();
        let listing = |d: &Path| -> Vec<String> { let mut v: Vec<String> = std::fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect(); v.sort(); v };
        let l0 = listing(&dir);
        run(&cfg, "t", "RUN1").unwrap();
        assert_eq!(std::fs::read(&cfg.snapshot_db).unwrap(), before);
        let l1 = listing(&dir);
        assert_eq!(l1.iter().filter(|n| !l0.contains(n)).collect::<Vec<_>>(), vec![&"out".to_string()]);
        assert!(!l1.iter().any(|n| n.ends_with("-wal") || n.ends_with("-shm")));
    }

    #[test]
    fn the_source_reads_no_label_columns_and_has_no_writes_network_or_per_unit_output() {
        let src = include_str!("pr4_pf0a.rs");
        let code = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        let body: String = code.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        // ラベルは EXISTS の中だけ（列の値を SELECT しない）
        assert!(body.contains("EXISTS (SELECT 1 FROM metadata_match_labels l WHERE l.work_id = c.work_id AND l.strength = 'strong')"));
        assert_eq!(body.matches("metadata_match_labels").count(), 1);
        for banned in ["tmdb_id", "metadata_match_verdicts", "metadata_match_candidates", "metadata_review_tasks", "reasons_json", "INSERT ", "UPDATE ", "DELETE ", "DROP ", "VACUUM", "Command", "reqwest", "TcpStream", "dbg!"] {
            assert!(!body.contains(banned), "{banned}");
        }
        // 出力の println は件数・commitment・版名・SHA・出力先だけ
        let prints: Vec<&str> = code.lines().filter(|l| l.contains("println!")).collect();
        assert_eq!(prints.len(), 7);
        assert!(prints.iter().all(|l| ["holdout_source_count", "excluded_ever_strong_count", "current_eligible_count", "membership_commitment_sha256", "eligibility_rule_version", "snapshot_sha256", "output ="].iter().any(|k| l.contains(k))));
        let m = include_str!("mod.rs");
        assert!(m.contains("#[cfg(test)]\npub mod pr4_pf0a;"));
    }

    #[test]
    fn the_cohort_cte_is_taken_from_the_source_not_copied() {
        let cte = cohort_cte().unwrap();
        assert!(cte.contains("WITH part AS") && cte.contains("'live'") && cte.contains("historical_audit_only"));
        let src = include_str!("pr4_pf0a.rs");
        assert!(!src.split("#[cfg(test)]\nmod tests").next().unwrap().contains("missing_original"), "CTE の本文をコピーしない");
    }
}
