//! PR4-3V0: PR4-3 の実データ検証 runner（**手動専用・製品には出ない**）。
//!
//! このモジュールは `#[cfg(test)]` でしかコンパイルされず、Tauri command も UI も増やさない。実行する手順は `#[ignore]` 付きの
//! テスト 1 本（`manual_pr4_3v_validate`）だけで、`cargo test -- --ignored` で他の手順と一緒に動かないよう、
//! `CINEMANTIS_PR4_3V_ACTION=validate` を最初に確かめる。
//!
//! # 原則
//!
//! **production の DB を SQLite として一度も開かない。** この runner が受け取るのは、停止中の production の DB / WAL / SHM を
//! byte copy し、**コピー側だけ**で recovery して作った immutable な snapshot だけ。production の DB のパスを受け取る引数・環境変数は無い
//! （snapshot が production のアプリデータ置き場にあれば STOP する）。snapshot の作成はこの gate の対象外で、別の gate で行う。
//!
//! # 入力（環境変数。これだけ）
//!
//! | 変数 | 意味 |
//! |---|---|
//! | `CINEMANTIS_PR4_3V_ACTION` | `validate` のときだけ動く |
//! | `CINEMANTIS_PR4_3V_SNAPSHOT_DB` | recovery 済みの immutable snapshot（SQLite ファイル） |
//! | `CINEMANTIS_PR4_3V_SNAPSHOT_MANIFEST` | snapshot の manifest（JSON） |
//! | `CINEMANTIS_PR4_GT_BRIDGE` | 凍結した外部 bridge（SHA-256 を検証してから読む） |
//! | `CINEMANTIS_PR4_3V_OUTPUT_DIR` | 出力の親ディレクトリ。1 回ごとに `<UTC_RUN_ID>` の下位ディレクトリを作る |
//! | `CINEMANTIS_PR4_3V_EXPECTED_HEAD` | **評価対象**（PR4-3 の実装）の commit（40 桁）。凍結値 `67e6f41…` と一致すること |
//! | `CINEMANTIS_PR4_3V_RUNNER_HEAD` | **この runner 自体**を載せた commit（40 桁。runner の commit の後の HEAD）。manifest の記録と一致すること。評価対象とは別の SHA として記録する |
//!
//! # fail-closed の前提確認（実行前。どれか 1 つでも違えば STOP）
//!
//! snapshot が存在 / production のデータ置き場の中ではない / snapshot の SHA-256 が manifest と一致 / snapshot の隣に `-wal` `-shm` が無い /
//! expected HEAD が凍結値 / manifest の source HEAD が expected HEAD と一致 / bridge の SHA-256 が凍結値 / manifest の protocol・各 SHA の形式 /
//! cohort は A1 だけ。
//!
//! SQLite は `SQLITE_OPEN_READ_ONLY` + `?immutable=1` で開き、さらに `PRAGMA query_only = ON`（`generate_report` の中でも ON にする）。
//!
//! # 結果の読み方（順序を固定）
//!
//! 先に `diagnostic_gt.status` / `gt_universe.resolved` / `gt_universe.deferred` / `blockers` だけを見る。1 つでも blocker があれば
//! validation = BLOCKED とし、precision は評価しない。PASS（resolved 44 / deferred 6）のときだけ、precision を「参考値」として記録する
//! （44 は GT の単位数の整合性の検査で、precision の分母ではない。policy freeze の前の値で、昇格には使えない）。
//!
//! # 出力（件数・率・blocker だけ）
//!
//! `<OUTPUT_DIR>\<UTC_RUN_ID>\{snapshot_manifest.json, diagnostic_report.json, PR4_3V_RUN_REPORT.txt}`。
//! sample_id / task_id / work_id / tmdb_id / 題名 / 候補の identity は出さない（出力に含まれていれば STOP して書かない）。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::services::pr4_diagnostics::{generate_report, DiagnosticCohort, GtConfig, PINNED_BRIDGE_SHA256};

/// **評価対象**のコード（PR4-3 の実装の commit）。env の expected HEAD はこの値と一致しなければならない。
/// runner 自体は、その後の test-only の commit で、別の SHA（`RUNNER_HEAD`）として記録する（評価対象の commit ではない）。
pub const PINNED_HEAD: &str = "67e6f41bcee9746258c3fdb16c2a1d77604cae04";
pub const VALIDATION_PROTOCOL: &str = "pr4-3v-real-data-validation-1";
pub const MANIFEST_VERSION: &str = "pr4-3v-snapshot-manifest-1";
/// production のアプリデータ置き場のディレクトリ名。snapshot がこの中にあれば STOP（production の DB を渡していないことの確認）
const PRODUCTION_DATA_DIR_NAME: &str = "dev.cinemantis.app";
const ENV_VARS: [&str; 7] = [
    "CINEMANTIS_PR4_3V_ACTION",
    "CINEMANTIS_PR4_3V_SNAPSHOT_DB",
    "CINEMANTIS_PR4_3V_SNAPSHOT_MANIFEST",
    "CINEMANTIS_PR4_GT_BRIDGE",
    "CINEMANTIS_PR4_3V_OUTPUT_DIR",
    "CINEMANTIS_PR4_3V_EXPECTED_HEAD",
    "CINEMANTIS_PR4_3V_RUNNER_HEAD",
];
/// レポートに現れてはならないキー（件数・率・blocker だけを出す）
const FORBIDDEN_KEYS: [&str; 8] = ["sample_id", "task_id", "work_id", "tmdb_id", "title", "original_title", "candidate", "candidates"];
const FORBIDDEN_TEXT: [&str; 2] = ["c5c5c-redo", "c5c5-dev-expand"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop(pub String);

fn stop<T>(msg: impl Into<String>) -> Result<T, Stop> {
    Err(Stop(msg.into()))
}

/// runner の設定。**production の DB のパスを持たない。**
#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub snapshot_db: PathBuf,
    pub snapshot_manifest: PathBuf,
    pub bridge: PathBuf,
    pub output_dir: PathBuf,
    /// 評価対象（PR4-3 の実装）の commit
    pub expected_head: String,
    /// runner 自体の commit（評価対象とは別の SHA）。manifest の `runner_head` と一致すること
    pub runner_head: String,
    /// 検証に使う bridge の SHA-256 と、正式な件数（本番の値は凍結値。試験では合成値に差し替える）
    pub gt: GtExpectation,
    pub pinned_head: String,
}

#[derive(Debug, Clone)]
pub struct GtExpectation {
    pub bridge_sha256: String,
    pub population: usize,
    pub resolved: usize,
    pub deferred: usize,
}

impl GtExpectation {
    pub fn frozen() -> Self {
        GtExpectation { bridge_sha256: PINNED_BRIDGE_SHA256.to_string(), population: 50, resolved: 44, deferred: 6 }
    }
}

impl RunnerConfig {
    /// 環境変数から読む（`get` を差し替えれば試験できる）。足りなければ STOP。本番の値は凍結値を使う。
    pub fn from_env(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, Stop> {
        let need = |name: &str| -> Result<String, Stop> {
            match get(name) {
                Some(v) if !v.trim().is_empty() => Ok(v),
                _ => stop(format!("環境変数 {name} が未設定です")),
            }
        };
        let action = need(ENV_VARS[0])?;
        if action != "validate" {
            return stop(format!("この手順は CINEMANTIS_PR4_3V_ACTION=validate のときだけ動きます（いまは {action}）"));
        }
        Ok(RunnerConfig {
            snapshot_db: PathBuf::from(need(ENV_VARS[1])?),
            snapshot_manifest: PathBuf::from(need(ENV_VARS[2])?),
            bridge: PathBuf::from(need(ENV_VARS[3])?),
            output_dir: PathBuf::from(need(ENV_VARS[4])?),
            expected_head: need(ENV_VARS[5])?,
            runner_head: need(ENV_VARS[6])?,
            gt: GtExpectation::frozen(),
            pinned_head: PINNED_HEAD.to_string(),
        })
    }
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn sha256_file(path: &Path) -> Result<String, Stop> {
    let bytes = std::fs::read(path).map_err(|e| Stop(format!("{} を読めません: {e}", path.display())))?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

/// A1（開発用 audit）だけを受け付ける。それ以外（A2 など）は STOP
pub fn parse_cohort(label: &str) -> Result<DiagnosticCohort, Stop> {
    match label {
        "A1" | "A1_development_audit" => Ok(DiagnosticCohort::DevelopmentAudit),
        other => stop(format!("cohort は A1 だけです（指定: {other}）")),
    }
}

fn sidecar_exists(snapshot: &Path, suffix: &str) -> bool {
    let mut name = snapshot.as_os_str().to_os_string();
    name.push(suffix);
    Path::new(&name).exists()
}

/// 検証済みの manifest（件数・SHA・時刻だけ。production の DB の中身は持たない）
#[derive(Debug, Clone)]
pub struct Preflight {
    pub manifest: Value,
    pub snapshot_sha256: String,
    pub bridge_sha256: String,
}

fn check_file_record(rec: &Value, what: &str) -> Result<(), Stop> {
    let Some(obj) = rec.as_object() else { return stop(format!("manifest の {what} が不正です")) };
    match obj.get("status").and_then(Value::as_str) {
        Some("absent") => Ok(()),
        Some("present") => {
            if obj.get("size_bytes").and_then(Value::as_u64).is_none()
                || !obj.get("mtime_utc").is_some_and(Value::is_string)
                || !obj.get("sha256").and_then(Value::as_str).is_some_and(|s| is_hex(s, 64))
            {
                return stop(format!("manifest の {what} の size / mtime / SHA-256 が不正です"));
            }
            Ok(())
        }
        _ => stop(format!("manifest の {what} は present / absent のどちらかです（存在しないものは作らない）")),
    }
}

/// 実行前の確認。**どれか 1 つでも違えば STOP（出力は何も作らない）。**
pub fn preflight(cfg: &RunnerConfig) -> Result<Preflight, Stop> {
    if !is_hex(&cfg.pinned_head, 40) || cfg.expected_head != cfg.pinned_head {
        return stop("expected HEAD が凍結値（PR4-3 の commit）と違います");
    }
    let snapshot = &cfg.snapshot_db;
    if !snapshot.is_file() {
        return stop("snapshot が存在しません");
    }
    // production のデータ置き場の中の DB は渡さない（production を開かない原則）
    let canonical = snapshot.canonicalize().map_err(|e| Stop(format!("snapshot のパスを解決できません: {e}")))?;
    if canonical.components().any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case(PRODUCTION_DATA_DIR_NAME)) {
        return stop("snapshot が production のアプリデータ置き場の中にあります（production の DB を渡していないこと）");
    }
    if sidecar_exists(snapshot, "-wal") || sidecar_exists(snapshot, "-shm") {
        return stop("snapshot の隣に -wal / -shm があります（recovery 済みの単独ファイルであること）");
    }
    let manifest_text = std::fs::read_to_string(&cfg.snapshot_manifest).map_err(|e| Stop(format!("manifest を読めません: {e}")))?;
    let manifest: Value = serde_json::from_str(&manifest_text).map_err(|_| Stop("manifest が JSON として読めません".to_string()))?;
    let m = manifest.as_object().ok_or_else(|| Stop("manifest が object ではありません".to_string()))?;
    if m.get("manifest_version").and_then(Value::as_str) != Some(MANIFEST_VERSION) || m.get("protocol_version").and_then(Value::as_str) != Some(VALIDATION_PROTOCOL) {
        return stop("manifest の版 / protocol が想定と違います");
    }
    let text = |k: &str| m.get(k).and_then(Value::as_str);
    for k in ["raw_copy_db_sha256", "recovered_snapshot_sha256"] {
        if !text(k).is_some_and(|s| is_hex(s, 64)) {
            return stop(format!("manifest の {k} が完全な SHA-256 ではありません"));
        }
    }
    if !text("snapshot_created_at_utc").is_some_and(|s| !s.is_empty()) {
        return stop("manifest に snapshot の作成時刻がありません");
    }
    if text("source_head") != Some(cfg.expected_head.as_str()) {
        return stop("manifest の source HEAD が expected HEAD と違います");
    }
    if !is_hex(&cfg.runner_head, 40) || text("runner_head") != Some(cfg.runner_head.as_str()) {
        return stop("manifest の runner HEAD が runner HEAD（40 桁）と違います");
    }
    if text("bridge_sha256") != Some(cfg.gt.bridge_sha256.as_str()) {
        return stop("manifest の bridge SHA-256 が凍結値と違います");
    }
    let prod = m.get("production_before").and_then(Value::as_object).ok_or_else(|| Stop("manifest に production の実行前の記録がありません".to_string()))?;
    for (k, what) in [("db", "DB"), ("wal", "WAL"), ("shm", "SHM")] {
        check_file_record(prod.get(k).unwrap_or(&Value::Null), what)?;
    }
    let snapshot_sha = sha256_file(snapshot)?;
    if Some(snapshot_sha.as_str()) != text("recovered_snapshot_sha256") {
        return stop("snapshot の SHA-256 が manifest の記載と違います");
    }
    let bridge_sha = sha256_file(&cfg.bridge)?;
    if bridge_sha != cfg.gt.bridge_sha256 {
        return stop("bridge の SHA-256 が凍結値と違います");
    }
    Ok(Preflight { manifest, snapshot_sha256: snapshot_sha, bridge_sha256: bridge_sha })
}

/// snapshot を **read-only + immutable** で開く（書き込み可能な接続は作らない）。さらに `query_only`。
pub fn open_snapshot(path: &Path) -> Result<Connection, Stop> {
    let canonical = path.canonicalize().map_err(|e| Stop(format!("snapshot のパスを解決できません: {e}")))?;
    let mut uri_path = canonical.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = uri_path.strip_prefix("//?/") {
        uri_path = stripped.to_string();
    }
    let uri = format!("file:///{}?mode=ro&immutable=1", uri_path.trim_start_matches('/').replace(' ', "%20"));
    let conn = Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| Stop(format!("snapshot を read-only で開けません: {e}")))?;
    conn.execute_batch("PRAGMA query_only = ON").map_err(|e| Stop(e.to_string()))?;
    Ok(conn)
}

/// レポートに GT の identity・作品 ID・題名などが含まれていないこと（キーと既知の文字列）
pub fn assert_no_identity(report: &Value) -> Result<(), Stop> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                for (k, x) in map {
                    if FORBIDDEN_KEYS.iter().any(|f| k.eq_ignore_ascii_case(f)) {
                        out.push(k.clone());
                    }
                    walk(x, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut keys = Vec::new();
    walk(report, &mut keys);
    if !keys.is_empty() {
        return stop(format!("レポートに identity のキーが含まれています: {keys:?}"));
    }
    let text = report.to_string();
    if let Some(bad) = FORBIDDEN_TEXT.iter().find(|t| text.contains(**t)) {
        return stop(format!("レポートに identity の文字列が含まれています: {bad}"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// bridge + DB の GT が正式 closeout に収束した
    Passed,
    /// 1 つでも blocker / 件数の不一致があった。precision は評価しない
    Blocked(Vec<String>),
}

/// 結果の読み順（固定）: status → resolved → deferred → blockers。precision はこの後。
pub fn read_verdict(report: &Value, expect: &GtExpectation) -> Verdict {
    let gt = &report["diagnostic_gt"];
    let mut reasons: Vec<String> = gt["blockers"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    let status = gt["status"].as_str().unwrap_or("MISSING");
    let resolved = gt["gt_universe"]["resolved"].as_u64();
    let deferred = gt["gt_universe"]["deferred"].as_u64();
    if status != "PASS" {
        reasons.push(format!("diagnostic_gt.status = {status}"));
    }
    if resolved != Some(expect.resolved as u64) || deferred != Some(expect.deferred as u64) {
        reasons.push(format!("gt_universe resolved/deferred = {resolved:?}/{deferred:?}（期待 {}/{}）", expect.resolved, expect.deferred));
    }
    if reasons.is_empty() { Verdict::Passed } else { Verdict::Blocked(reasons) }
}

fn utc_now_iso() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // 日数 → 暦日（Howard Hinnant の civil_from_days）
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

pub fn run_id_from(iso: &str) -> String {
    iso.chars().filter(|c| c.is_ascii_digit() || *c == 'T' || *c == 'Z').collect()
}

#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub run_dir: PathBuf,
    pub verdict: Verdict,
    pub report: Value,
}

/// 検証の本体。**出力先以外へは書かない。** 失敗は STOP（出力は作らない）。`run_id` は呼び出し側が渡す（試験できるように）。
pub fn run(cfg: &RunnerConfig, cohort_label: &str, generated_at: &str, run_id: &str) -> Result<RunOutcome, Stop> {
    let cohort = parse_cohort(cohort_label)?;
    let pre = preflight(cfg)?;
    let conn = open_snapshot(&cfg.snapshot_db)?;
    let gt_cfg = GtConfig {
        bridge_path: cfg.bridge.clone(),
        expected_bridge_sha256: cfg.gt.bridge_sha256.clone(),
        expected_population: cfg.gt.population,
        expected_resolved: cfg.gt.resolved,
        expected_deferred: cfg.gt.deferred,
    };
    let report = generate_report(&conn, cohort, generated_at, &cfg.expected_head, Some(&gt_cfg)).map_err(|e| Stop(format!("レポートを作れません: {e}")))?;
    drop(conn);
    assert_no_identity(&report)?;
    if report["final_promotion"] != json!({"status": "NOT_RUN", "reason": "HOLDOUT_SEALED_POLICY_NOT_FROZEN"}) {
        return stop("final_promotion が NOT_RUN ではありません");
    }
    let verdict = read_verdict(&report, &cfg.gt);

    let run_dir = cfg.output_dir.join(run_id);
    if run_dir.exists() {
        return stop("出力先が既にあります（上書きしない）");
    }
    if !run_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') || run_id.is_empty() {
        return stop("run id が不正です");
    }
    std::fs::create_dir_all(&run_dir).map_err(|e| Stop(format!("出力先を作れません: {e}")))?;
    let write = |name: &str, text: String| std::fs::write(run_dir.join(name), text).map_err(|e| Stop(format!("{name} を書けません: {e}")));
    write("snapshot_manifest.json", serde_json::to_string_pretty(&pre.manifest).unwrap_or_default() + "\n")?;
    write("diagnostic_report.json", serde_json::to_string_pretty(&report).unwrap_or_default() + "\n")?;
    write("PR4_3V_RUN_REPORT.txt", run_report_text(&verdict, &report, &pre, cfg, generated_at))?;
    Ok(RunOutcome { run_dir, verdict, report })
}

fn run_report_text(verdict: &Verdict, report: &Value, pre: &Preflight, cfg: &RunnerConfig, generated_at: &str) -> String {
    let gt = &report["diagnostic_gt"];
    let mut out = String::new();
    out.push_str("PR4-3V real-data validation run (counts only; no GT identity)\n");
    out.push_str(&format!("protocol: {VALIDATION_PROTOCOL}\ngenerated_at: {generated_at}\nevaluation_head: {}\nrunner_head: {}\nsnapshot_sha256: {}\nbridge_sha256: {}\n\n", cfg.expected_head, cfg.runner_head, pre.snapshot_sha256, pre.bridge_sha256));
    // 読み順: status → resolved / deferred → blockers。precision はこの後
    out.push_str(&format!("diagnostic_gt.status: {}\n", gt["status"].as_str().unwrap_or("MISSING")));
    out.push_str(&format!("gt_universe: population {} / resolved {} / deferred {} (expected {}/{}/{})\n", gt["gt_universe"]["formal_population"], gt["gt_universe"]["resolved"], gt["gt_universe"]["deferred"], cfg.gt.population, cfg.gt.resolved, cfg.gt.deferred));
    out.push_str(&format!("blockers: {}\n", gt["blockers"]));
    match verdict {
        Verdict::Passed => {
            out.push_str("\nVALIDATION: PASSED (bridge + DB GT converged on the formal closeout)\n\nreference values (NOT a promotion metric; policy freeze not defined; 44 is an integrity check, not a denominator):\n");
            if let Some(profiles) = gt["profiles"].as_object() {
                for (matcher, p) in profiles {
                    out.push_str(&format!("  {matcher}: auto_evaluable {} / auto_correct {} / precision {}\n", p["auto_evaluable_count"], p["auto_correct_count"], p["diagnostic_conditional_auto_precision"]));
                }
            }
        }
        Verdict::Blocked(reasons) => {
            out.push_str("\nVALIDATION: BLOCKED (precision is not evaluated)\n");
            for r in reasons {
                out.push_str(&format!("  - {r}\n"));
            }
            out.push_str(&format!("\ngt_universe counts: {}\nexcluded_gt: {}\nunresolved_gt_count: {}\n", gt["gt_universe"], gt["excluded_gt"], gt["unresolved_gt_count"]));
        }
    }
    out.push_str("\nfinal_promotion: NOT_RUN (HOLDOUT_SEALED_POLICY_NOT_FROZEN)\nproduction DB: not opened by this runner; holdout: sealed\n");
    out
}

// ─── 手動実行（#[ignore]。snapshot 作成の gate が承認されるまで実行しない）────────────────────────────

#[test]
#[ignore = "manual: needs a recovered immutable snapshot; CINEMANTIS_PR4_3V_ACTION=validate"]
fn manual_pr4_3v_validate() {
    let cfg = RunnerConfig::from_env(&|name| std::env::var(name).ok()).unwrap_or_else(|e| panic!("STOP: {}", e.0));
    let now = utc_now_iso();
    let outcome = run(&cfg, "A1", &now, &run_id_from(&now)).unwrap_or_else(|e| panic!("STOP: {}", e.0));
    let gt = &outcome.report["diagnostic_gt"];
    println!("diagnostic_gt.status = {}", gt["status"]);
    println!("gt_universe.resolved = {} / deferred = {}", gt["gt_universe"]["resolved"], gt["gt_universe"]["deferred"]);
    println!("blockers = {}", gt["blockers"]);
    println!("validation = {:?}", outcome.verdict);
    println!("output = {}", outcome.run_dir.display());
}

// ─── テスト（合成 snapshot のみ。production の DB・実 bridge・holdout は使わない）──────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::{insert_work, open_migrated};
    use crate::services::match_history::{POLICY_VERSION, SHADOW_VERSION};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const HEAD: &str = "67e6f41bcee9746258c3fdb16c2a1d77604cae04";
    const RUNNER_HEAD: &str = "1111111111111111111111111111111111111111";
    const SAMPLE: &str = "c5c5c-redo-01";
    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pr4_3v_{tag}_{}_{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sampling() -> String {
        format!("{{\"purpose\":\"development\",\"cohort\":\"reconstructed_clean\",\"sample_id\":\"{SAMPLE}\",\"protocol_version\":\"gt-protocol-1\"}}")
    }

    /// 開発用 audit task（run つき）。`resolved_to` があれば GT レビューと同じ形で解決済みにし、AUTO の verdict を付ける
    fn unit(conn: &Connection, resolved_to: Option<i64>, auto_id: i64) -> (i64, i64, i64) {
        let work = insert_work(conn, "w");
        conn.execute(
            "INSERT INTO metadata_match_runs (work_id, batch_id, trigger_kind, mode, evidence_class, state_schema_version, input_snapshot_json,
               search_queries_json, policy_version, policy_snapshot_json, governing_matcher, decision, decision_reasons_json)
             VALUES (?1, 'b', 'batch', 'safe', 'reconstructed_clean', 'cm-prematch-2', '{}', '[]', ?2, '{}', 'rules-safe', 'AUTO', '[]')",
            rusqlite::params![work, POLICY_VERSION],
        )
        .unwrap();
        let run = conn.last_insert_rowid();
        for (m, v) in [("rules-safe", POLICY_VERSION), ("rules-tags-shadow", SHADOW_VERSION)] {
            conn.execute(
                "INSERT INTO metadata_match_verdicts (run_id, matcher, matcher_version, tmdb_id, media_type, decision, reasons_json) VALUES (?1, ?2, ?3, ?4, 'movie', 'AUTO', '[]')",
                rusqlite::params![run, m, v, auto_id],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO metadata_review_tasks (work_id, run_id, reason, sampling_json, details_json) VALUES (?1, ?2, 'audit_sample', ?3, '{\"review_protocol_version\":\"gt-review-1\",\"state\":\"ready\"}')",
            rusqlite::params![work, run, sampling()],
        )
        .unwrap();
        let task = conn.last_insert_rowid();
        if let Some(id) = resolved_to {
            conn.execute(
                "INSERT INTO metadata_match_labels (work_id, run_id, review_task_id, label, tmdb_id, media_type, method, strength) VALUES (?1, ?2, ?3, 'tmdb', ?4, 'movie', 'review_confirm', 'strong')",
                rusqlite::params![work, run, task, id],
            )
            .unwrap();
            let label = conn.last_insert_rowid();
            conn.execute(
                "UPDATE metadata_review_tasks SET resolved_at = '2026-10-06T00:00:00Z', resolution_label_id = ?2,
                   details_json = '{\"review_protocol_version\":\"gt-review-1\",\"state\":\"resolved\",\"resolution_method\":\"review_confirm\"}' WHERE id = ?1",
                rusqlite::params![task, label],
            )
            .unwrap();
        } else {
            conn.execute("UPDATE metadata_review_tasks SET details_json = '{\"review_protocol_version\":\"gt-review-1\",\"state\":\"deferred\"}' WHERE id = ?1", [task]).unwrap();
        }
        (task, work, run)
    }

    struct Fixture {
        dir: PathBuf,
        cfg: RunnerConfig,
    }

    /// 合成 snapshot（DB で 40 件解決 + bridge で 4 件解決 + 6 件 deferred = 50 / 44 / 6）。`db_resolved` で DB 側の解決件数を変えられる
    fn fixture_with(db_resolved: usize, bridge_units: usize, deferred: usize, bridge_conflict: bool) -> Fixture {
        let dir = tmp_dir("fx");
        let conn = open_migrated();
        for i in 0..db_resolved {
            unit(&conn, Some(1000 + i as i64), 1000 + i as i64);
        }
        let mut recs = Vec::new();
        for i in 0..bridge_units {
            let id = 5000 + i as i64;
            let (task, work, run) = unit(&conn, None, id);
            let tm = if bridge_conflict { id + 1 } else { id };
            recs.push(json!({"evaluation_unit": {"sample_id": SAMPLE, "task_id": task, "work_id": work}, "reviewed_run_id": run, "semantic": "POSITIVE", "media_type": "movie",
                "tmdb_id": tm, "source_stage": "D", "source_artifact": "x", "source_artifact_sha256": "a".repeat(64), "source_protocol_version": "p", "provenance_class": "FROZEN_OFF_DB_GT"}));
            let _ = (task, work, run);
        }
        for _ in 0..deferred {
            unit(&conn, None, 9000);
        }
        let snap = dir.join("snapshot.db");
        conn.execute_batch(&format!("VACUUM INTO '{}'", snap.to_string_lossy().replace('\'', "''"))).unwrap();
        drop(conn);
        let _actual = (db_resolved + bridge_units + deferred, db_resolved + bridge_units);
        let bridge = json!({
            "bridge_version": "pr4-diagnostic-gt-bridge-1",
            "formal_study": {"study": "C5c.5", "resolved": 44, "deferred": 6, "closeout_sha256": "b".repeat(64), "f_closeout_sha256": "c".repeat(64)},
            "sources": [{"name": "n", "path": "p", "sha256": "d".repeat(64)}],
            "integrity": {"off_db_records": recs.len(), "expected_final_resolved": 44, "expected_final_deferred": 6, "trusted_db_expected": db_resolved,
                          "stage_counts": {}, "holdout_scope": "x", "production_db_modified": false, "holdout_read": false},
            "records": recs,
        });
        let bridge_path = dir.join("bridge.json");
        let bridge_text = serde_json::to_string(&bridge).unwrap();
        std::fs::write(&bridge_path, &bridge_text).unwrap();
        let bridge_sha = format!("{:x}", Sha256::digest(bridge_text.as_bytes()));
        let snap_sha = format!("{:x}", Sha256::digest(std::fs::read(&snap).unwrap()));
        let file = |present: bool| if present { json!({"status": "present", "size_bytes": 4096, "mtime_utc": "2026-10-06T00:00:00Z", "sha256": "e".repeat(64)}) } else { json!({"status": "absent"}) };
        let manifest = json!({
            "manifest_version": MANIFEST_VERSION, "protocol_version": VALIDATION_PROTOCOL, "source_head": HEAD, "runner_head": RUNNER_HEAD, "bridge_sha256": bridge_sha,
            "production_before": {"db": file(true), "wal": file(false), "shm": file(false)},
            "raw_copy_db_sha256": "f".repeat(64), "recovered_snapshot_sha256": snap_sha, "snapshot_created_at_utc": "2026-10-06T00:00:00Z",
        });
        let manifest_path = dir.join("manifest.json");
        std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap()).unwrap();
        let cfg = RunnerConfig {
            snapshot_db: snap,
            snapshot_manifest: manifest_path,
            bridge: bridge_path,
            output_dir: dir.join("out"),
            expected_head: HEAD.to_string(),
            runner_head: RUNNER_HEAD.to_string(),
            // 正式な期待値は常に 50 / 44 / 6（bridge も 44 / 6 を宣言する）。DB 側の実際の件数だけが変わる
            gt: GtExpectation { bridge_sha256: bridge_sha, population: 50, resolved: 44, deferred: 6 },
            pinned_head: HEAD.to_string(),
        };
        Fixture { dir, cfg }
    }

    fn fixture() -> Fixture {
        fixture_with(40, 4, 6, false)
    }

    fn rewrite_manifest(f: &Fixture, edit: impl FnOnce(&mut Value)) {
        let mut m: Value = serde_json::from_str(&std::fs::read_to_string(&f.cfg.snapshot_manifest).unwrap()).unwrap();
        edit(&mut m);
        std::fs::write(&f.cfg.snapshot_manifest, serde_json::to_string(&m).unwrap()).unwrap();
    }

    fn stops(r: Result<RunOutcome, Stop>, needle: &str) -> bool {
        matches!(r, Err(Stop(m)) if m.contains(needle))
    }

    fn go(f: &Fixture) -> Result<RunOutcome, Stop> {
        run(&f.cfg, "A1", "2026-10-06T00:00:00Z", "RUN1")
    }

    fn tree_files(root: &Path) -> Vec<String> {
        let mut out = Vec::new();
        fn walk(p: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(p).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() { walk(&p, out) } else { out.push(p.to_string_lossy().to_string()) }
            }
        }
        walk(root, &mut out);
        out.sort();
        out
    }

    #[test]
    fn a_converged_snapshot_passes_with_44_resolved_and_6_deferred() {
        let f = fixture();
        let o = go(&f).unwrap();
        assert_eq!(o.verdict, Verdict::Passed);
        let gt = &o.report["diagnostic_gt"];
        assert_eq!(gt["status"], "PASS");
        assert_eq!((gt["gt_universe"]["resolved"].as_u64(), gt["gt_universe"]["deferred"].as_u64()), (Some(44), Some(6)));
        assert_eq!(o.report["final_promotion"], json!({"status": "NOT_RUN", "reason": "HOLDOUT_SEALED_POLICY_NOT_FROZEN"}));
        // precision の分母は 44 ではなく AUTO の評価単位（ここでは 44 全部が AUTO だが、定義は別）
        assert_eq!(gt["profiles"]["rules-safe"]["auto_evaluable_count"], 44);
        let files = tree_files(&f.cfg.output_dir);
        assert_eq!(files.len(), 3);
        assert!(files.iter().any(|p| p.ends_with("snapshot_manifest.json")) && files.iter().any(|p| p.ends_with("diagnostic_report.json")) && files.iter().any(|p| p.ends_with("PR4_3V_RUN_REPORT.txt")));
        let text = std::fs::read_to_string(o.run_dir.join("PR4_3V_RUN_REPORT.txt")).unwrap();
        // 読み順: status が precision より先
        assert!(text.find("diagnostic_gt.status").unwrap() < text.find("reference values").unwrap());
        assert!(text.contains("VALIDATION: PASSED") && text.contains("NOT a promotion metric"));
        assert!(text.contains(&format!("evaluation_head: {HEAD}")) && text.contains(&format!("runner_head: {RUNNER_HEAD}")), "評価対象と runner の SHA を別々に記録する");
        assert_ne!(HEAD, RUNNER_HEAD);
    }

    #[test]
    fn a_gt_universe_of_43_or_45_is_blocked_and_precision_is_not_evaluated() {
        // 期待は 44 / 6 のまま、実際の DB 側の解決が 39 / 41（= 合計 43 / 45）
        for db_resolved in [39usize, 41] {
            let f = fixture_with(db_resolved, 4, 6, false);
            let o = go(&f).unwrap();
            assert!(matches!(&o.verdict, Verdict::Blocked(r) if r.iter().any(|x| x.contains("gt_universe_mismatch"))), "{db_resolved}");
            assert_eq!(o.report["diagnostic_gt"]["status"], "BLOCKED");
            assert_eq!(o.report["diagnostic_gt"]["profiles"], json!({}));
            let text = std::fs::read_to_string(o.run_dir.join("PR4_3V_RUN_REPORT.txt")).unwrap();
            assert!(text.contains("VALIDATION: BLOCKED") && !text.contains("reference values"));
        }
    }

    #[test]
    fn a_source_conflict_is_blocked() {
        let f = fixture_with(40, 4, 6, true);
        // bridge と DB の canonical GT が食い違うのは bridge-only の単位では起きないので、DB 側にも解決を足して衝突させる
        let conn = Connection::open(&f.cfg.snapshot_db).unwrap();
        let (task, work, _): (i64, i64, i64) = conn.query_row("SELECT t.id, t.work_id, t.run_id FROM metadata_review_tasks t WHERE json_extract(t.details_json,'$.state') = 'deferred' ORDER BY t.id LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        let run: i64 = conn.query_row("SELECT run_id FROM metadata_review_tasks WHERE id = ?1", [task], |r| r.get(0)).unwrap();
        conn.execute("INSERT INTO metadata_match_labels (work_id, run_id, review_task_id, label, tmdb_id, media_type, method, strength) VALUES (?1, ?2, ?3, 'tmdb', 77, 'movie', 'review_confirm', 'strong')", rusqlite::params![work, run, task]).unwrap();
        let label = conn.last_insert_rowid();
        conn.execute("UPDATE metadata_review_tasks SET resolved_at = '2026-10-06T00:00:00Z', resolution_label_id = ?2, details_json = '{\"review_protocol_version\":\"gt-review-1\",\"state\":\"resolved\",\"resolution_method\":\"review_confirm\"}' WHERE id = ?1", rusqlite::params![task, label]).unwrap();
        drop(conn);
        // snapshot を書き換えたので manifest の SHA を作り直す（preflight を通すため）
        let sha = format!("{:x}", Sha256::digest(std::fs::read(&f.cfg.snapshot_db).unwrap()));
        rewrite_manifest(&f, |m| m["recovered_snapshot_sha256"] = json!(sha));
        let o = go(&f).unwrap();
        assert!(matches!(&o.verdict, Verdict::Blocked(r) if r.iter().any(|x| x.contains("gt_source_conflict"))), "{:?}", o.verdict);
        assert_eq!(o.report["diagnostic_gt"]["profiles"], json!({}));
    }

    #[test]
    fn missing_environment_variables_stop() {
        let all: std::collections::HashMap<&str, &str> = ENV_VARS.iter().map(|k| (*k, "x")).collect();
        let with = |drop: Option<&str>, action: &str| RunnerConfig::from_env(&|k| {
            if Some(k) == drop { None } else if k == ENV_VARS[0] { Some(action.to_string()) } else { all.get(k).map(|v| v.to_string()) }
        });
        assert!(with(None, "validate").is_ok());
        for k in ENV_VARS {
            assert!(with(Some(k), "validate").is_err(), "{k}");
        }
        assert!(with(None, "other").is_err(), "ACTION が validate でなければ動かない");
        assert!(RunnerConfig::from_env(&|k| if k == ENV_VARS[4] { Some("  ".to_string()) } else { Some("x".to_string()) }).is_err());
        // production の DB のパスを受け取る入口が無い
        assert!(ENV_VARS.iter().all(|k| !k.to_lowercase().contains("production")));
    }

    #[test]
    fn preflight_stops_on_snapshot_sha_wal_shm_head_bridge_and_manifest_problems() {
        let f = fixture();
        assert!(preflight(&f.cfg).is_ok());
        // snapshot の SHA が manifest と違う
        let mut tampered = fixture();
        {
            use std::io::Write;
            std::fs::OpenOptions::new().append(true).open(&tampered.cfg.snapshot_db).unwrap().write_all(b"x").unwrap();
        }
        assert!(stops(go(&tampered), "SHA-256 が manifest"));
        tampered.cfg.snapshot_db = f.dir.join("nonexistent.db");
        assert!(stops(go(&tampered), "存在しません"));
        // 隣に -wal / -shm
        for suffix in ["-wal", "-shm"] {
            let g = fixture();
            let mut name = g.cfg.snapshot_db.as_os_str().to_os_string();
            name.push(suffix);
            std::fs::write(PathBuf::from(name), b"").unwrap();
            assert!(stops(go(&g), "-wal / -shm"), "{suffix}");
            assert!(!g.cfg.output_dir.exists(), "STOP のとき出力を作らない");
        }
        // HEAD
        let mut g = fixture();
        g.cfg.expected_head = "0".repeat(40);
        assert!(stops(go(&g), "expected HEAD"));
        let g = fixture();
        rewrite_manifest(&g, |m| m["source_head"] = json!("0".repeat(40)));
        assert!(stops(go(&g), "source HEAD"));
        // runner HEAD（manifest と違う / 40 桁でない）。評価対象の HEAD とは別に検査する
        let g = fixture();
        rewrite_manifest(&g, |m| m["runner_head"] = json!("2".repeat(40)));
        assert!(stops(go(&g), "runner HEAD"));
        let mut g = fixture();
        g.cfg.runner_head = "short".to_string();
        assert!(stops(go(&g), "runner HEAD"));
        // bridge（ファイルが違う / manifest の記載が違う）
        let g = fixture();
        std::fs::write(&g.cfg.bridge, b"{}").unwrap();
        assert!(stops(go(&g), "bridge の SHA-256"));
        let g = fixture();
        rewrite_manifest(&g, |m| m["bridge_sha256"] = json!("0".repeat(64)));
        assert!(stops(go(&g), "bridge SHA-256 が凍結値"));
        // manifest の不備
        for (edit, needle) in [
            (Box::new(|m: &mut Value| m["protocol_version"] = json!("x")) as Box<dyn Fn(&mut Value)>, "protocol"),
            (Box::new(|m: &mut Value| m["raw_copy_db_sha256"] = json!("abc")), "完全な SHA-256"),
            (Box::new(|m: &mut Value| { m.as_object_mut().unwrap().remove("production_before"); }), "production の実行前"),
            (Box::new(|m: &mut Value| m["production_before"]["wal"] = json!({"status": "created"})), "present / absent"),
            (Box::new(|m: &mut Value| m["production_before"]["db"] = json!({"status": "present", "size_bytes": 1})), "size / mtime / SHA-256"),
            (Box::new(|m: &mut Value| { m.as_object_mut().unwrap().remove("snapshot_created_at_utc"); }), "作成時刻"),
        ] {
            let g = fixture();
            rewrite_manifest(&g, |m| edit(m));
            assert!(stops(go(&g), needle), "{needle}");
        }
        // 実際の凍結 bridge の SHA と違う（本番の設定では、合成 bridge は通らない）
        let g = fixture();
        let mut prod = g.cfg.clone();
        prod.gt = GtExpectation::frozen();
        assert!(stops(run(&prod, "A1", "t", "R"), "凍結値"));
    }

    #[test]
    fn a_snapshot_inside_the_production_data_directory_is_refused() {
        let f = fixture();
        let inside = f.dir.join(PRODUCTION_DATA_DIR_NAME);
        std::fs::create_dir_all(&inside).unwrap();
        let moved = inside.join("snapshot.db");
        std::fs::copy(&f.cfg.snapshot_db, &moved).unwrap();
        let mut cfg = f.cfg.clone();
        cfg.snapshot_db = moved;
        assert!(stops(run(&cfg, "A1", "t", "R"), "production のアプリデータ置き場"));
    }

    #[test]
    fn only_the_a1_cohort_is_accepted() {
        let f = fixture();
        for label in ["A2", "A2_production_observed", "production", "", "a1 "] {
            assert!(stops(run(&f.cfg, label, "t", "R"), "A1 だけ"), "{label}");
        }
        assert!(go(&f).is_ok());
        assert!(!f.cfg.output_dir.join("R").exists());
    }

    #[test]
    fn the_snapshot_is_opened_read_only_and_cannot_be_written() {
        let f = fixture();
        let before = std::fs::read(&f.cfg.snapshot_db).unwrap();
        let conn = open_snapshot(&f.cfg.snapshot_db).unwrap();
        assert_eq!(conn.query_row("PRAGMA query_only", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        for sql in ["UPDATE metadata_match_runs SET latency_ms = 1", "DELETE FROM metadata_match_runs", "CREATE TABLE x(a)", "PRAGMA query_only = OFF; UPDATE metadata_match_runs SET latency_ms = 1"] {
            assert!(conn.execute_batch(sql).is_err(), "{sql}");
        }
        drop(conn);
        assert_eq!(std::fs::read(&f.cfg.snapshot_db).unwrap(), before, "snapshot は 1 byte も変わらない");
        // 隣に -wal / -shm を作らない
        assert!(!sidecar_exists(&f.cfg.snapshot_db, "-wal") && !sidecar_exists(&f.cfg.snapshot_db, "-shm"));
        go(&f).unwrap();
        assert_eq!(std::fs::read(&f.cfg.snapshot_db).unwrap(), before);
        assert!(!sidecar_exists(&f.cfg.snapshot_db, "-wal") && !sidecar_exists(&f.cfg.snapshot_db, "-shm"));
    }

    #[test]
    fn the_runner_source_has_no_writable_connection_and_no_production_path() {
        let source = include_str!("pr4_3v_runner.rs");
        let code = source.split("// ─── テスト").next().unwrap();
        let body: String = code.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        assert!(!body.contains("Connection::open(") && !body.contains("open_in_memory") && !body.contains("open_with_flags(&uri, OpenFlags::SQLITE_OPEN_READ_WRITE"));
        assert!(body.contains("SQLITE_OPEN_READ_ONLY") && body.contains("immutable=1") && body.contains("query_only"));
        for forbidden in ["INSERT ", "UPDATE ", "DELETE ", "DROP ", "ALTER ", "CREATE TABLE", "apply_migrations", "VACUUM", "wal_checkpoint"] {
            assert!(!body.contains(forbidden), "{forbidden}");
        }
        assert!(!body.contains("APPDATA") && !body.contains("cinemantis.db"), "production の DB のパスを組み立てない");
        assert!(!body.contains("std::process::Command") && !body.contains("reqwest") && !body.contains("TcpStream"), "通信・外部コマンドを使わない");
    }

    #[test]
    fn the_report_never_contains_identities_and_the_check_catches_them() {
        let f = fixture();
        let o = go(&f).unwrap();
        assert!(assert_no_identity(&o.report).is_ok());
        for text in std::fs::read_dir(&o.run_dir).unwrap().map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap().replace("exact_media_type_tmdb_id", "").replace("(tmdb_id, media_type)", "")) {
            for bad in ["task_id", "work_id", "tmdb_id", "sample_id", "c5c5c-redo"] {
                assert!(!text.contains(bad), "{bad}");
            }
        }
        for bad in [json!({"a": {"task_id": 1}}), json!({"b": [{"tmdb_id": 2}]}), json!({"x": {"Title": "t"}}), json!({"s": "c5c5c-redo-01"}), json!({"candidates": []})] {
            assert!(assert_no_identity(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn nothing_is_written_outside_the_output_directory_and_reruns_are_refused() {
        let f = fixture();
        let before = tree_files(&f.dir);
        let o = go(&f).unwrap();
        let after = tree_files(&f.dir);
        let new: Vec<&String> = after.iter().filter(|p| !before.contains(p)).collect();
        assert_eq!(new.len(), 3);
        assert!(new.iter().all(|p| p.starts_with(f.cfg.output_dir.to_string_lossy().as_ref())), "出力先の外に書いた: {new:?}");
        for p in &before {
            assert!(after.contains(p));
        }
        assert!(stops(go(&f), "既にあります"), "同じ run id には上書きしない");
        assert!(o.run_dir.ends_with("RUN1"));
        // 不正な run id
        assert!(stops(run(&f.cfg, "A1", "t", "../x"), "run id"));
        assert!(!f.dir.join("x").exists());
    }

    #[test]
    fn the_runner_is_manual_only_and_wired_only_in_test_builds() {
        let source = include_str!("pr4_3v_runner.rs");
        assert!(source.contains("#[ignore = \"manual:") && source.contains("CINEMANTIS_PR4_3V_ACTION"));
        let m = include_str!("mod.rs");
        assert!(m.contains("#[cfg(test)]\npub mod pr4_3v_runner;"), "test ビルドでだけコンパイルする");
    }

    #[test]
    fn run_ids_and_timestamps_are_well_formed() {
        let iso = utc_now_iso();
        assert!(iso.len() == 20 && iso.ends_with('Z') && &iso[10..11] == "T", "{iso}");
        let id = run_id_from(&iso);
        assert!(id.len() == 16 && id.ends_with('Z') && id.contains('T'), "{id}");
        assert_eq!(run_id_from("2026-10-06T01:02:03Z"), "20261006T010203Z");
    }
}
