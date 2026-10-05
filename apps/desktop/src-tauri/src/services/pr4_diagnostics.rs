//! PR4-1: 通常の診断集計（読み取り専用）。
//!
//! 契約は `docs/PR4_AGGREGATION_CONTRACT.md` / `docs/pr4_metric_contract.json`（PR4-0 で凍結）。
//! ここで出すのは、意味論が確定した次の指標だけ。
//!
//! - 理由コード別の件数と率
//! - REVIEW の件数と率 / AUTO の件数と coverage / abstention の件数と率
//! - rules-safe と rules-tags-shadow の比較（decision / top identity / AUTO の結果 / 理由集合）
//!
//! **作らないもの:** GT を使う正しさの指標（`diagnostic_conditional_auto_precision` を含む）、FINAL の昇格指標、
//! holdout の評価、昇格判断。レポートの `diagnostic_gt` は常に未実装、`final_promotion` は常に NOT_RUN。
//!
//! # 守っていること
//!
//! - **判定器の結論は `metadata_match_verdicts` が正。** `runs.decision` は適格性の判定（ERROR など）にだけ使い、
//!   判定器別の集計には使わない。
//! - **holdout は集計の前に除く。** 除外の境界は `metadata_review_tasks` の抽出記録（`sampling_json` の
//!   `purpose` / `cohort`）だけから導く。除外された run の verdict・候補・ラベルは**読まない**（件数だけ数える）。
//!   holdout の work の run は、別の run も含めて除く。抽出記録が読めない task の work も除く。
//! - **audit run は `review_tasks.run_id` で識別する。** run の名前や時刻から推測しない。
//! - **`mode='shadow'` の run は safe run の複製なので、source run として数えない。**
//! - **履歴の版は混ぜない。** 現行の版の組（rules-safe-2 / rules-tags-shadow-2）以外は別の除外理由で数える。
//!   rules-1 は凍結の前後で `matcher_version` が同じ文字列なので、このモジュールでは指標に使わず、件数だけ報告する。
//! - **何も黙って落とさない。** すべての run は「適格」か「ちょうど 1 つの除外理由」のどちらかになり、除外理由は
//!   0 件でも毎回すべて出す。比較できない組は `not_comparable` として理由別に数える。
//! - **書き込まない。** 集計の間は `PRAGMA query_only = ON`（終わると元に戻す）。行の追加・更新・削除、migration、
//!   backfill は無い。

// PR4-1 は backend の集計だけで、アプリ本体（UI / command）からはまだ呼ばない（呼び出し側は後続の gate）。
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use rusqlite::Connection;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::services::match_history::{POLICY_VERSION, SHADOW_VERSION};
use crate::services::metadata_matcher::reason;

/// レポートの版
pub const REPORT_VERSION: &str = "pr4-diagnostic-report-1";

/// cohort 境界の信頼性。`sampling_json` は DB では保護されていない（PR4-0 の棚卸し）ので、アプリケーション側の検証だけである旨を毎回残す
pub const COHORT_BOUNDARY_INTEGRITY: &str = "APPLICATION_VALIDATED_NOT_DB_ENFORCED";

const MATCHER_SAFE: &str = "rules-safe";
const MATCHER_SHADOW: &str = "rules-tags-shadow";

/// 現行より古い版として識別できる rules-safe の policy_version（af63b4d より前）
const HISTORICAL_POLICY_VERSIONS: [&str; 1] = ["rules-safe-1"];

/// verdict の一括取得で 1 回に渡す run ID の数（SQLite の変数上限に余裕を持たせる）
const CHUNK: usize = 400;

/// 理由コードの語彙（`metadata_matcher::reason` の定数そのもの。ここで別の解釈を作らない）
const KNOWN_REASONS: [&str; 14] = [
    reason::NO_CANDIDATES,
    reason::BELOW_THRESHOLD,
    reason::MULTIPLE_ABOVE_THRESHOLD,
    reason::TIED_TOP,
    reason::NO_YEAR_HINT,
    reason::CANDIDATE_YEAR_UNKNOWN,
    reason::YEAR_OUT_OF_RANGE,
    reason::EPISODE_MARKER_VS_MOVIE,
    reason::PART_MISMATCH,
    reason::EMBEDDED_YEAR_CONFLICT,
    reason::YEAR_SOURCES_DISAGREE,
    reason::ORIGINAL_LANGUAGE_MISMATCH,
    reason::EDITION_MARKER,
    reason::EPISODE_ID_VS_TV,
];

/// 除外理由。**毎回すべてを出す**（0 件でも）。順序は判定の優先順位。
pub const EXCLUSION_REASONS: [&str; 15] = [
    "sealed_holdout",
    "unsupported_sampling_metadata",
    "shadow_duplicate_run",
    "unsupported_run_mode",
    "replay_run",
    "run_error",
    "run_skipped",
    "unsupported_run_state",
    "audit_run_other_cohort",
    "production_run_other_cohort",
    "audit_task_not_valid",
    "unsupported_audit_task_state",
    "live_evidence_policy_ambiguous",
    "historical_version_ambiguous",
    "unsupported_profile_version",
];

/// 集計する cohort。**明示的に選ぶ。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticCohort {
    /// A1: development の audit_sample 課題に繋がる audit run
    DevelopmentAudit,
    /// A2: production の safe run（ラベルなし）。evidence_class='live' は holdout との重なりが未確定なので除く
    ProductionObserved,
}

impl DiagnosticCohort {
    pub fn as_str(self) -> &'static str {
        match self {
            DiagnosticCohort::DevelopmentAudit => "A1_development_audit",
            DiagnosticCohort::ProductionObserved => "A2_production_observed",
        }
    }

    fn selection_rule(self) -> &'static str {
        match self {
            DiagnosticCohort::DevelopmentAudit => {
                "metadata_match_runs の全行から除外理由を引いた残り。適格 = mode='safe'、trigger_kind<>'replay'、\
                 decision in (AUTO,REVIEW,UNRESOLVED)、sampling_json の purpose='development' かつ \
                 cohort in (reconstructed_clean,historical_audit_only) の audit_sample 課題の run_id に繋がる、\
                 課題の state in (ready,deferred,resolved)、policy_version が現行"
            }
            DiagnosticCohort::ProductionObserved => {
                "metadata_match_runs の全行から除外理由を引いた残り。適格 = mode='safe'、trigger_kind<>'replay'、\
                 decision in (AUTO,REVIEW,UNRESOLVED)、audit_sample 課題に繋がらない、evidence_class<>'live'、\
                 policy_version が現行"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticsError {
    /// 必要なテーブル / 列が無い
    UnsupportedSchema(String),
    Db(String),
}

impl std::fmt::Display for DiagnosticsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiagnosticsError::UnsupportedSchema(m) => write!(f, "unsupported schema: {m}"),
            DiagnosticsError::Db(m) => write!(f, "db error: {m}"),
        }
    }
}

impl From<rusqlite::Error> for DiagnosticsError {
    fn from(error: rusqlite::Error) -> Self {
        DiagnosticsError::Db(error.to_string())
    }
}

// ─── 読み取り専用の保証 ──────────────────────────────────────────────────────

/// 集計の間だけ `PRAGMA query_only = ON` にし、drop で元の値に戻す。
struct QueryOnlyGuard<'a> {
    conn: &'a Connection,
    previous: i64,
}

impl<'a> QueryOnlyGuard<'a> {
    fn new(conn: &'a Connection) -> Result<Self, DiagnosticsError> {
        let previous: i64 = conn.query_row("PRAGMA query_only", [], |row| row.get(0))?;
        conn.execute_batch("PRAGMA query_only = ON")?;
        Ok(QueryOnlyGuard { conn, previous })
    }
}

impl Drop for QueryOnlyGuard<'_> {
    fn drop(&mut self) {
        let _ = self.conn.execute_batch(if self.previous == 1 {
            "PRAGMA query_only = ON"
        } else {
            "PRAGMA query_only = OFF"
        });
    }
}

fn require_schema(conn: &Connection) -> Result<(), DiagnosticsError> {
    let required: [(&str, &[&str]); 3] = [
        (
            "metadata_match_runs",
            &["id", "work_id", "mode", "trigger_kind", "evidence_class", "policy_version", "decision"],
        ),
        (
            "metadata_match_verdicts",
            &["run_id", "matcher", "matcher_version", "tmdb_id", "media_type", "decision", "reasons_json"],
        ),
        (
            "metadata_review_tasks",
            &["id", "work_id", "run_id", "reason", "sampling_json", "details_json"],
        ),
    ];
    for (table, columns) in required {
        let mut stmt = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
        let present: HashSet<String> = stmt
            .query_map([table], |row| row.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        for column in columns {
            if !present.contains(*column) {
                return Err(DiagnosticsError::UnsupportedSchema(format!("{table}.{column} がありません")));
            }
        }
    }
    Ok(())
}

// ─── 選択（run の分類）───────────────────────────────────────────────────────

struct RunRow {
    id: i64,
    work_id: i64,
    mode: String,
    trigger_kind: String,
    evidence_class: String,
    policy_version: String,
    decision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskClass {
    Development,
    Holdout,
    Unsupported,
}

struct TaskRow {
    id: i64,
    work_id: i64,
    run_id: Option<i64>,
    class: TaskClass,
    state: Option<String>,
    /// PR4-3: GT の評価単位のキーと allowlist の検査に使う、抽出記録・解決記録のメタデータ
    sample_id: Option<String>,
    sampling_protocol: Option<String>,
    details: Option<Map<String, Value>>,
    resolved: bool,
    resolution_label_id: Option<i64>,
}

/// 抽出記録から task の種別を決める。**読めない・足りない・想定外は Unsupported**（holdout でないと言い切れない）。
fn classify_task(sampling_json: &str) -> TaskClass {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(sampling_json) else {
        return TaskClass::Unsupported;
    };
    let (Some(Value::String(purpose)), Some(Value::String(cohort))) = (map.get("purpose"), map.get("cohort")) else {
        return TaskClass::Unsupported;
    };
    // live は holdout 専用。live + development は禁止の組み合わせなので、迷わず holdout 側に倒す
    if purpose == "holdout" || cohort == "live" {
        return TaskClass::Holdout;
    }
    if purpose == "development" && (cohort == "reconstructed_clean" || cohort == "historical_audit_only") {
        return TaskClass::Development;
    }
    TaskClass::Unsupported
}

fn sampling_text_field(sampling_json: Option<&str>, key: &str) -> Option<String> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(sampling_json?) else {
        return None;
    };
    match map.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn details_map(details_json: Option<&str>) -> Option<Map<String, Value>> {
    match serde_json::from_str::<Value>(details_json?) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

fn task_state(details_json: Option<&str>) -> Option<String> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(details_json?) else {
        return None;
    };
    match map.get("state") {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

struct Selection {
    eligible: Vec<i64>,
    /// 除外理由ごとの件数（EXCLUSION_REASONS のすべてを 0 から持つ）
    exclusions: BTreeMap<&'static str, i64>,
    total_source_rows: i64,
    /// audit_sample の課題のメタデータ（種別の分類つき）。GT の組み立ては、ここで Development と分類された課題だけを対象にする
    tasks: Vec<TaskRow>,
    /// 除外された run の、除外理由（PR4-1 の分類をそのまま使う。第 2 の cohort 実装を作らない）
    run_exclusion: HashMap<i64, &'static str>,
    /// holdout / 未対応の task がある作品（GT の組み立てから除く）
    sealed_works: HashSet<i64>,
}

/// 1 つの run を「適格」または「ちょうど 1 つの除外理由」に分類する。
fn classify_run(
    run: &RunRow,
    cohort: DiagnosticCohort,
    holdout_runs: &HashSet<i64>,
    holdout_works: &HashSet<i64>,
    unsupported_runs: &HashSet<i64>,
    unsupported_works: &HashSet<i64>,
    dev_by_run: &HashMap<i64, &TaskRow>,
) -> Option<&'static str> {
    if holdout_runs.contains(&run.id) || holdout_works.contains(&run.work_id) {
        return Some("sealed_holdout");
    }
    if unsupported_runs.contains(&run.id) || unsupported_works.contains(&run.work_id) {
        return Some("unsupported_sampling_metadata");
    }
    if run.mode == "shadow" {
        return Some("shadow_duplicate_run");
    }
    if run.mode != "safe" {
        return Some("unsupported_run_mode");
    }
    if run.trigger_kind == "replay" {
        return Some("replay_run");
    }
    match run.decision.as_str() {
        "ERROR" => return Some("run_error"),
        "SKIPPED" => return Some("run_skipped"),
        "AUTO" | "REVIEW" | "UNRESOLVED" => {}
        _ => return Some("unsupported_run_state"),
    }
    let dev_task = dev_by_run.get(&run.id);
    match cohort {
        DiagnosticCohort::DevelopmentAudit => match dev_task {
            None => return Some("production_run_other_cohort"),
            Some(task) => match task.state.as_deref() {
                Some("ready") | Some("deferred") | Some("resolved") => {}
                Some("stale") | Some("generation_error") => return Some("audit_task_not_valid"),
                _ => return Some("unsupported_audit_task_state"),
            },
        },
        DiagnosticCohort::ProductionObserved => {
            if dev_task.is_some() {
                return Some("audit_run_other_cohort");
            }
            // 作品のファイルの出どころ分類。live は holdout の frame との重なりが未確定（PR4-0）
            if run.evidence_class == "live" {
                return Some("live_evidence_policy_ambiguous");
            }
        }
    }
    if run.policy_version != POLICY_VERSION {
        return Some(if HISTORICAL_POLICY_VERSIONS.contains(&run.policy_version.as_str()) {
            "historical_version_ambiguous"
        } else {
            "unsupported_profile_version"
        });
    }
    None
}

/// holdout の判定に使うのは runs / review_tasks の**メタデータだけ**。verdict・候補・ラベルは読まない。
fn select_runs(conn: &Connection, cohort: DiagnosticCohort) -> Result<Selection, DiagnosticsError> {
    let mut runs = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, work_id, mode, trigger_kind, evidence_class, policy_version, decision
               FROM metadata_match_runs ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RunRow {
                id: row.get(0)?,
                work_id: row.get(1)?,
                mode: row.get(2)?,
                trigger_kind: row.get(3)?,
                evidence_class: row.get(4)?,
                policy_version: row.get(5)?,
                decision: row.get(6)?,
            })
        })?;
        for row in rows {
            runs.push(row?);
        }
    }

    let mut tasks = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, work_id, run_id, sampling_json, details_json, resolved_at, resolution_label_id
               FROM metadata_review_tasks WHERE reason = 'audit_sample' ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<i64>>(6)?,
            ))
        })?;
        for row in rows {
            let (id, work_id, run_id, sampling, details, resolved_at, resolution_label_id) = row?;
            let class = match sampling.as_deref() {
                Some(text) => classify_task(text),
                None => TaskClass::Unsupported,
            };
            tasks.push(TaskRow {
                id,
                work_id,
                run_id,
                class,
                state: task_state(details.as_deref()),
                sample_id: sampling_text_field(sampling.as_deref(), "sample_id"),
                sampling_protocol: sampling_text_field(sampling.as_deref(), "protocol_version"),
                details: details_map(details.as_deref()),
                resolved: resolved_at.is_some(),
                resolution_label_id,
            });
        }
    }

    let mut holdout_runs = HashSet::new();
    let mut holdout_works = HashSet::new();
    let mut unsupported_runs = HashSet::new();
    let mut unsupported_works = HashSet::new();
    let mut dev_by_run: HashMap<i64, &TaskRow> = HashMap::new();
    for task in &tasks {
        match task.class {
            TaskClass::Holdout => {
                holdout_works.insert(task.work_id);
                if let Some(run_id) = task.run_id {
                    holdout_runs.insert(run_id);
                }
            }
            TaskClass::Unsupported => {
                unsupported_works.insert(task.work_id);
                if let Some(run_id) = task.run_id {
                    unsupported_runs.insert(run_id);
                }
            }
            TaskClass::Development => {
                if let Some(run_id) = task.run_id {
                    dev_by_run.insert(run_id, task);
                }
            }
        }
    }

    let mut exclusions: BTreeMap<&'static str, i64> = EXCLUSION_REASONS.iter().map(|r| (*r, 0)).collect();
    let mut eligible = Vec::new();
    let mut run_exclusion: HashMap<i64, &'static str> = HashMap::new();
    for run in &runs {
        match classify_run(run, cohort, &holdout_runs, &holdout_works, &unsupported_runs, &unsupported_works, &dev_by_run) {
            Some(reason) => {
                *exclusions.get_mut(reason).expect("known reason") += 1;
                run_exclusion.insert(run.id, reason);
            }
            None => eligible.push(run.id),
        }
    }
    let sealed_works: HashSet<i64> = holdout_works.union(&unsupported_works).copied().collect();
    drop(dev_by_run);
    Ok(Selection { eligible, exclusions, total_source_rows: runs.len() as i64, tasks, run_exclusion, sealed_works })
}

// ─── verdict の読み取り（適格な run だけ）─────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum Reasons {
    /// NULL / JSON でない / 文字列の配列でない
    Invalid,
    Set(Vec<String>),
}

#[derive(Debug, Clone)]
struct Verdict {
    decision: String,
    identity: Option<(i64, String)>,
    reasons: Reasons,
}

#[derive(Debug, Clone)]
enum VerdictState {
    Supported(Verdict),
    /// matcher_version が現行の版でない
    UnsupportedVersion,
}

#[derive(Default)]
struct PerRun {
    safe: Option<VerdictState>,
    shadow: Option<VerdictState>,
}

fn parse_reasons(text: Option<&str>) -> Reasons {
    let Some(text) = text else { return Reasons::Invalid };
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(items)) => {
            let mut out = Vec::new();
            for item in items {
                match item {
                    Value::String(s) => out.push(s),
                    _ => return Reasons::Invalid,
                }
            }
            Reasons::Set(out)
        }
        _ => Reasons::Invalid,
    }
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

/// 適格な run の rules-safe / rules-tags-shadow の verdict だけを読む。
/// `UNIQUE (run_id, matcher)` なので、1 つの run・1 つの matcher につき最大 1 行（組み合わせの爆発は起きない）。
fn read_verdicts(conn: &Connection, run_ids: &[i64]) -> Result<(HashMap<i64, PerRun>, i64), DiagnosticsError> {
    let mut map: HashMap<i64, PerRun> = HashMap::new();
    let mut rules_one_rows = 0i64;
    for chunk in run_ids.chunks(CHUNK) {
        let sql = format!(
            "SELECT run_id, matcher, matcher_version, tmdb_id, media_type, decision, reasons_json
               FROM metadata_match_verdicts
              WHERE matcher IN ('{MATCHER_SAFE}','{MATCHER_SHADOW}') AND run_id IN ({})",
            placeholders(chunk.len())
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?;
        for row in rows {
            let (run_id, matcher, version, tmdb_id, media_type, decision, reasons) = row?;
            let expected = if matcher == MATCHER_SAFE { POLICY_VERSION } else { SHADOW_VERSION };
            let state = if version == expected {
                VerdictState::Supported(Verdict {
                    decision,
                    identity: match (tmdb_id, media_type) {
                        (Some(id), Some(mt)) => Some((id, mt)),
                        _ => None,
                    },
                    reasons: parse_reasons(reasons.as_deref()),
                })
            } else {
                VerdictState::UnsupportedVersion
            };
            let entry = map.entry(run_id).or_default();
            if matcher == MATCHER_SAFE {
                entry.safe = Some(state);
            } else {
                entry.shadow = Some(state);
            }
        }
        // rules-1 は指標に使わない。凍結の前後を version 文字列で区別できないので、件数だけ報告する
        let sql = format!(
            "SELECT COUNT(*) FROM metadata_match_verdicts WHERE matcher = 'rules-1' AND run_id IN ({})",
            placeholders(chunk.len())
        );
        let n: i64 = conn.query_row(&sql, rusqlite::params_from_iter(chunk.iter()), |row| row.get(0))?;
        rules_one_rows += n;
    }
    Ok((map, rules_one_rows))
}

// ─── 指標 ────────────────────────────────────────────────────────────────────

fn rate(count: i64, denominator: i64) -> Value {
    if denominator > 0 {
        json!(count as f64 / denominator as f64)
    } else {
        Value::Null
    }
}

fn supported<'a>(state: &'a Option<VerdictState>) -> Option<&'a Verdict> {
    match state {
        Some(VerdictState::Supported(v)) => Some(v),
        _ => None,
    }
}

/// 1 つの matcher の単独の指標（理由コード / REVIEW / AUTO / abstention）
fn matcher_metrics(matcher: &str, version: &str, eligible: &[i64], per_run: &HashMap<i64, PerRun>) -> Value {
    let pick = |p: &PerRun| -> Option<VerdictState> {
        if matcher == MATCHER_SAFE { p.safe.clone() } else { p.shadow.clone() }
    };
    let mut verdicts: Vec<Verdict> = Vec::new();
    let (mut missing, mut unsupported_version) = (0i64, 0i64);
    for run_id in eligible {
        match per_run.get(run_id).and_then(|p| pick(p)) {
            None => missing += 1,
            Some(VerdictState::UnsupportedVersion) => unsupported_version += 1,
            Some(VerdictState::Supported(v)) => verdicts.push(v),
        }
    }
    let denominator = verdicts.len() as i64;

    let count_decision = |d: &str| verdicts.iter().filter(|v| v.decision == d).count() as i64;
    let (auto, review, unresolved) = (count_decision("AUTO"), count_decision("REVIEW"), count_decision("UNRESOLVED"));
    let abstention = review + unresolved;

    // 理由コード（1 行に複数ありうるので、率の合計は 1 を超えうる）
    let mut counts: BTreeMap<&str, i64> = KNOWN_REASONS.iter().map(|c| (*c, 0)).collect();
    let (mut no_reason_auto, mut empty_non_auto, mut unknown_or_null) = (0i64, 0i64, 0i64);
    for v in &verdicts {
        match &v.reasons {
            Reasons::Invalid => unknown_or_null += 1,
            Reasons::Set(codes) if codes.is_empty() => {
                if v.decision == "AUTO" { no_reason_auto += 1 } else { empty_non_auto += 1 }
            }
            Reasons::Set(codes) => {
                let distinct: BTreeSet<&str> = codes.iter().map(|s| s.as_str()).collect();
                let mut has_unknown = false;
                for code in distinct {
                    match counts.get_mut(code) {
                        Some(n) => *n += 1,
                        None => has_unknown = true,
                    }
                }
                if has_unknown {
                    unknown_or_null += 1;
                }
            }
        }
    }
    let items: Vec<Value> = counts
        .iter()
        .map(|(code, n)| json!({"reason_code": code, "count": n, "rate": rate(*n, denominator)}))
        .collect();
    let buckets = json!([
        {"bucket": "NO_REASON_AUTO", "count": no_reason_auto, "rate": rate(no_reason_auto, denominator),
         "meaning": "理由が空配列で decision=AUTO"},
        {"bucket": "EMPTY_REASONS_NON_AUTO", "count": empty_non_auto, "rate": rate(empty_non_auto, denominator),
         "meaning": "理由が空配列なのに decision が AUTO でない（想定外の記録）"},
        {"bucket": "UNKNOWN_OR_NULL", "count": unknown_or_null, "rate": rate(unknown_or_null, denominator),
         "meaning": "reasons_json が NULL / 配列でない / 語彙に無いコードを含む"},
    ]);

    json!({
        "matcher": matcher,
        "matcher_version": version,
        "denominator": {
            "eligible_decision_count": denominator,
            "definition": "適格な run のうち、この matcher の verdict 行があり、matcher_version が現行の版のもの",
            "eligible_runs": eligible.len(),
            "missing_verdict_runs": missing,
            "unsupported_matcher_version_runs": unsupported_version,
        },
        "reason_codes": {
            "denominator": denominator,
            "rate_definition": "count / denominator。1 つの verdict が複数のコードを持ちうるので、率の合計は 1 を超えうる",
            "vocabulary": "metadata_matcher::reason（14 コード。0 件のコードも出す）",
            "items": items,
            "buckets": buckets,
        },
        "review": {
            "state": "verdicts.decision = 'REVIEW'",
            "count": review,
            "eligible_decision_count": denominator,
            "rate": rate(review, denominator),
        },
        "auto": {
            "state": "verdicts.decision = 'AUTO'",
            "count": auto,
            "eligible_decision_count": denominator,
            "coverage": rate(auto, denominator),
            "note": "正しさ（GT との照合）はここでは見ない",
        },
        "abstention": {
            "states": ["REVIEW", "UNRESOLVED"],
            "count": abstention,
            "unresolved_count": unresolved,
            "denominator": denominator,
            "rate": rate(abstention, denominator),
            "definition": "audit_run.rs の集計定義コメントどおり (REVIEW + UNRESOLVED) / 適格な decision 全件。REVIEW と同一視しない",
            "partition_check": {"auto_plus_abstention_equals_denominator": auto + abstention == denominator},
        },
    })
}

#[derive(Default)]
struct Dimension {
    comparable: i64,
    agreement: i64,
    disagreement: i64,
    not_comparable: BTreeMap<&'static str, i64>,
}

impl Dimension {
    fn new(reasons: &[&'static str]) -> Self {
        Dimension { not_comparable: reasons.iter().map(|r| (*r, 0)).collect(), ..Default::default() }
    }

    fn nc(&mut self, reason: &'static str) {
        *self.not_comparable.get_mut(reason).expect("known not-comparable reason") += 1;
    }

    fn compared(&mut self, agree: bool) {
        self.comparable += 1;
        if agree { self.agreement += 1 } else { self.disagreement += 1 }
    }

    fn to_json(&self, definition: &str, extra: Value) -> Value {
        let not_comparable: i64 = self.not_comparable.values().sum();
        let mut out = json!({
            "definition": definition,
            "comparable_count": self.comparable,
            "agreement_count": self.agreement,
            "disagreement_count": self.disagreement,
            "not_comparable_count": not_comparable,
            "not_comparable_by_reason": self.not_comparable,
            "agreement_rate": rate(self.agreement, self.comparable),
        });
        if let (Value::Object(base), Value::Object(more)) = (&mut out, extra) {
            base.extend(more);
        }
        out
    }
}

const NC_STRUCTURAL: [&str; 4] = [
    "missing_rules_safe_verdict",
    "missing_rules_tags_shadow_verdict",
    "missing_both_verdicts",
    "unsupported_matcher_version",
];

/// rules-safe と rules-tags-shadow の比較。**同じ run_id**（`UNIQUE (run_id, matcher)`）の 2 行だけを組にする。
/// 並び順や時刻で組を作らない。比較できない組は捨てずに理由別に数える。
fn profile_comparison(eligible: &[i64], per_run: &HashMap<i64, PerRun>) -> Value {
    let mut decision = Dimension::new(&NC_STRUCTURAL);
    let mut identity = Dimension::new(&[NC_STRUCTURAL[0], NC_STRUCTURAL[1], NC_STRUCTURAL[2], NC_STRUCTURAL[3], "one_side_no_identity", "both_no_identity"]);
    let mut auto_result = Dimension::new(&[NC_STRUCTURAL[0], NC_STRUCTURAL[1], NC_STRUCTURAL[2], NC_STRUCTURAL[3], "auto_without_identity"]);
    let mut reason_set = Dimension::new(&[NC_STRUCTURAL[0], NC_STRUCTURAL[1], NC_STRUCTURAL[2], NC_STRUCTURAL[3], "invalid_reason_representation"]);
    let mut auto_breakdown: BTreeMap<&str, i64> = [
        "both_auto_same_identity",
        "both_auto_different_identity",
        "rules_safe_auto_only",
        "rules_tags_shadow_auto_only",
        "neither_auto",
    ]
    .iter()
    .map(|k| (*k, 0))
    .collect();

    let empty = PerRun::default();
    for run_id in eligible {
        let p = per_run.get(run_id).unwrap_or(&empty);
        let structural: Option<&'static str> = match (&p.safe, &p.shadow) {
            (None, None) => Some("missing_both_verdicts"),
            (None, _) => Some("missing_rules_safe_verdict"),
            (_, None) => Some("missing_rules_tags_shadow_verdict"),
            (Some(VerdictState::UnsupportedVersion), _) | (_, Some(VerdictState::UnsupportedVersion)) => {
                Some("unsupported_matcher_version")
            }
            _ => None,
        };
        if let Some(reason) = structural {
            decision.nc(reason);
            identity.nc(reason);
            auto_result.nc(reason);
            reason_set.nc(reason);
            continue;
        }
        let (Some(s), Some(t)) = (supported(&p.safe), supported(&p.shadow)) else { continue };

        // A. decision
        decision.compared(s.decision == t.decision);

        // B. top identity（両側に identity がある場合だけ。候補集合が違うので、片側の欠落は不一致にしない）
        match (&s.identity, &t.identity) {
            (Some(a), Some(b)) => identity.compared(a == b),
            (None, None) => identity.nc("both_no_identity"),
            _ => identity.nc("one_side_no_identity"),
        }

        // C. AUTO の結果
        let (s_auto, t_auto) = (s.decision == "AUTO", t.decision == "AUTO");
        match (s_auto, t_auto) {
            (true, true) => match (&s.identity, &t.identity) {
                (Some(a), Some(b)) if a == b => {
                    *auto_breakdown.get_mut("both_auto_same_identity").unwrap() += 1;
                    auto_result.compared(true);
                }
                (Some(_), Some(_)) => {
                    *auto_breakdown.get_mut("both_auto_different_identity").unwrap() += 1;
                    auto_result.compared(false);
                }
                _ => auto_result.nc("auto_without_identity"),
            },
            (true, false) => {
                *auto_breakdown.get_mut("rules_safe_auto_only").unwrap() += 1;
                auto_result.compared(false);
            }
            (false, true) => {
                *auto_breakdown.get_mut("rules_tags_shadow_auto_only").unwrap() += 1;
                auto_result.compared(false);
            }
            (false, false) => {
                *auto_breakdown.get_mut("neither_auto").unwrap() += 1;
                auto_result.compared(true);
            }
        }

        // D. 理由集合（集合の等しさ。JSON の文字列としての一致ではない）
        match (&s.reasons, &t.reasons) {
            (Reasons::Set(a), Reasons::Set(b)) => {
                let (a, b): (BTreeSet<&String>, BTreeSet<&String>) = (a.iter().collect(), b.iter().collect());
                reason_set.compared(a == b);
            }
            _ => reason_set.nc("invalid_reason_representation"),
        }
    }

    json!({
        "pairing_key": "run_id（UNIQUE (run_id, matcher) の 2 行）。並び順・時刻では組まない",
        "input_note": "rules-safe は旧経路の候補だけ、rules-tags-shadow は統合候補を入力にする。候補集合が違うことを前提に読む",
        "note": "4 つの次元は別々の指標で、1 つの『一致率』にしない",
        "decision": decision.to_json("verdict.decision（AUTO / REVIEW / UNRESOLVED）が同じ", json!({})),
        "top_identity": identity.to_json(
            "両側の verdict が (tmdb_id, media_type) を持つときだけ比較する。片側または両側が identity を持たない組は not_comparable",
            json!({"deviation_note": "PR4-0 契約は『両方 NULL は一致として別掲』だが、PR4-1 の指示（両側に identity がある場合だけ比較）に従い、両方 NULL は not_comparable(both_no_identity) として数える"}),
        ),
        "auto_result": auto_result.to_json(
            "両側の decision が有効なときの AUTO の結果。一致 = both_auto_same_identity または neither_auto",
            json!({"breakdown": auto_breakdown}),
        ),
        "reason_set": reason_set.to_json("理由コードの集合（重複と順序を無視）が等しい。どちらかが配列でない組は not_comparable", json!({})),
    })
}

// ─── PR4-3: 診断用 GT と diagnostic_conditional_auto_precision ─────────────────
//
// 契約: `docs/PR4_DIAGNOSTIC_GT_CONTRACT.md`（PR4-2）/ `docs/PR4_DIAGNOSTIC_GT_BRIDGE_CONTRACT.md`（PR4-2A）。
//
// - GT の供給元は 2 つで、優先順位は無い: DB の信頼できる GT（PR4-2 の allowlist）と、凍結済みの外部 bridge（DB に無い正式な解決済み GT）。
//   同じ評価単位 `{sample_id, task_id, work_id}` を両方が指すとき、canonical GT が同じなら 1 件に畳み（`reviewed_run_id` が違っても結合し、両方を provenance に残す）、
//   canonical GT が食い違えば `gt_source_conflict`。`reviewed_run_id` は属性で、その差だけでは衝突にしない。
// - **cohort の選択と除外は PR4-1 の `select_runs` をそのまま使う**（第 2 の実装を作らない）。GT の組み立ては、`select_runs` が Development と
//   分類した課題の作品だけが対象で、holdout の task がある作品は組み立ての前に除かれる（ラベルは開発用の作品のものだけを読む）。
// - 機械側は PR4-1 と同じ `read_verdicts`（verdicts が正）。AUTO だけが precision の分母に入る。正しさは `(media_type, tmdb_id)` の完全一致だけで、
//   rank / top-3 / score / 題名 / Jev は参照しない。none の GT に AUTO が候補を選べば誤り。
// - 次のどれかがあれば `diagnostic_gt.status = BLOCKED` で、precision は出さない（分母を黙って減らして計算可能にしない）:
//   bridge の欠落 / SHA 不一致 / スキーマ不正 / 未準備、評価単位のキーの不一致、供給元の衝突、GT の単位数が正式な件数と違う、
//   未対応のラベルの意味、AUTO なのに選んだ identity が無い、cohort の境界の違反。
// - レポートには GT の中身（作品 ID・TMDB ID）を出さない。件数だけ。

pub const GT_CONTRACT_VERSION: &str = "pr4-gt-contract-1";
pub const BRIDGE_VERSION: &str = "pr4-diagnostic-gt-bridge-1";
/// 凍結済みの外部 bridge（`diagnostic_gt_bridge.json`）の SHA-256。パスは呼び出し側が渡し、ここでは固定しない。
pub const PINNED_BRIDGE_SHA256: &str = "0f31ea9678cb7e078b82970fca3d9f3beb1338640ee8b9cfe1fe7e623cb8559c";

/// 正式な開発用 sample（committed の `docs/C5C5_CLOSEOUT.md` と PR4-2 の契約が定める。task ID は使わない）
const GT_SAMPLE_ALLOWLIST: [&str; 5] = ["c5c5c-redo-01", "c5c5c-redo-02", "c5c5c-redo-03", "c5c5c-redo-04", "c5c5c-redo-05"];
const GT_REVIEW_PROTOCOL: &str = "gt-review-1";
const GT_SAMPLING_PROTOCOL: &str = "gt-protocol-1";
/// GT として許すのはこの 3 値だけ（prefix 一致は使わない）
const TRUSTED_METHODS: [&str; 3] = ["review_confirm", "review_pick_other", "review_none"];

const GT_EXCLUDED_KEYS: [&str; 8] = [
    "untrusted_manual_gt_source",
    "weak_label_not_gt",
    "unknown_label_semantics",
    "gt_sample_not_allowlisted",
    "gt_provenance_incomplete",
    "gt_superseded",
    "gt_conflict",
    "gt_multiple_units_per_work",
];

const BRIDGE_TOP_KEYS: [&str; 5] = ["bridge_version", "formal_study", "sources", "integrity", "records"];
const BRIDGE_STUDY_KEYS: [&str; 5] = ["study", "resolved", "deferred", "closeout_sha256", "f_closeout_sha256"];
const BRIDGE_SOURCE_KEYS: [&str; 3] = ["name", "path", "sha256"];
const BRIDGE_INTEGRITY_KEYS: [&str; 8] = [
    "off_db_records",
    "expected_final_resolved",
    "expected_final_deferred",
    "trusted_db_expected",
    "stage_counts",
    "holdout_scope",
    "production_db_modified",
    "holdout_read",
];
const BRIDGE_RECORD_KEYS: [&str; 10] = [
    "evaluation_unit",
    "reviewed_run_id",
    "semantic",
    "media_type",
    "tmdb_id",
    "source_stage",
    "source_artifact",
    "source_artifact_sha256",
    "source_protocol_version",
    "provenance_class",
];

/// GT の入力の設定。bridge のパスは呼び出し側が明示的に渡す（絶対パスを固定しない）。
#[derive(Debug, Clone)]
pub struct GtConfig {
    pub bridge_path: PathBuf,
    pub expected_bridge_sha256: String,
    /// 正式な母集団・解決・保留の件数（C5c.5 の凍結 closeout）
    pub expected_population: usize,
    pub expected_resolved: usize,
    pub expected_deferred: usize,
}

impl GtConfig {
    /// 凍結した C5c.5 の値（母集団 50 / resolved 44 / deferred 6）と、凍結した bridge の SHA-256
    pub fn production(bridge_path: impl Into<PathBuf>) -> Self {
        GtConfig {
            bridge_path: bridge_path.into(),
            expected_bridge_sha256: PINNED_BRIDGE_SHA256.to_string(),
            expected_population: 50,
            expected_resolved: 44,
            expected_deferred: 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Gt {
    Positive(String, i64),
    NoMatch,
}

/// 評価単位のキー `{sample_id, task_id, work_id}`
type UnitKey = (String, i64, i64);

struct BridgeRec {
    key: UnitKey,
    run_id: i64,
    gt: Gt,
}

fn exact_keys(map: &Map<String, Value>, keys: &[&str]) -> bool {
    map.len() == keys.len() && keys.iter().all(|k| map.contains_key(*k))
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// bridge を読んで検証する。**SHA を確かめてからパースする。** 失敗は blocker のコードで返す。
fn load_bridge(cfg: &GtConfig) -> Result<(Vec<BridgeRec>, String), &'static str> {
    let bytes = std::fs::read(&cfg.bridge_path).map_err(|_| "bridge_missing")?;
    let sha = hex_sha256(&bytes);
    if sha != cfg.expected_bridge_sha256 {
        return Err("bridge_sha_mismatch");
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "bridge_schema_invalid")?;
    Ok((parse_bridge(&value, cfg)?, sha))
}

fn parse_bridge(value: &Value, cfg: &GtConfig) -> Result<Vec<BridgeRec>, &'static str> {
    const INVALID: &str = "bridge_schema_invalid";
    let top = value.as_object().ok_or(INVALID)?;
    if !exact_keys(top, &BRIDGE_TOP_KEYS) || top["bridge_version"] != BRIDGE_VERSION {
        return Err(INVALID);
    }
    let study = top["formal_study"].as_object().ok_or(INVALID)?;
    if !exact_keys(study, &BRIDGE_STUDY_KEYS) || study["study"] != "C5c.5" {
        return Err(INVALID);
    }
    for k in ["closeout_sha256", "f_closeout_sha256"] {
        if !study[k].as_str().is_some_and(is_hex64) {
            return Err(INVALID);
        }
    }
    let (Some(resolved), Some(deferred)) = (study["resolved"].as_u64(), study["deferred"].as_u64()) else {
        return Err(INVALID);
    };
    let sources = top["sources"].as_array().ok_or(INVALID)?;
    if sources.is_empty() {
        return Err(INVALID);
    }
    for s in sources {
        let s = s.as_object().ok_or(INVALID)?;
        if !exact_keys(s, &BRIDGE_SOURCE_KEYS) || !s["sha256"].as_str().is_some_and(is_hex64) || !s["name"].is_string() || !s["path"].is_string() {
            return Err(INVALID);
        }
    }
    let integrity = top["integrity"].as_object().ok_or(INVALID)?;
    if !exact_keys(integrity, &BRIDGE_INTEGRITY_KEYS) || !integrity["stage_counts"].is_object() || !integrity["holdout_scope"].is_string() {
        return Err(INVALID);
    }
    let records = top["records"].as_array().ok_or(INVALID)?;
    if integrity["off_db_records"].as_u64() != Some(records.len() as u64) {
        return Err(INVALID);
    }
    // 準備ができていない bridge（正式な件数が違う・DB 変更 / holdout 読み取りの宣言が偽でない）は使わない
    if resolved != cfg.expected_resolved as u64
        || deferred != cfg.expected_deferred as u64
        || integrity["expected_final_resolved"].as_u64() != Some(cfg.expected_resolved as u64)
        || integrity["expected_final_deferred"].as_u64() != Some(cfg.expected_deferred as u64)
        || integrity["production_db_modified"] != Value::Bool(false)
        || integrity["holdout_read"] != Value::Bool(false)
    {
        return Err("bridge_not_ready");
    }

    let mut out = Vec::new();
    let mut seen: HashSet<UnitKey> = HashSet::new();
    for rec in records {
        let rec = rec.as_object().ok_or(INVALID)?;
        if !exact_keys(rec, &BRIDGE_RECORD_KEYS) || rec["provenance_class"] != "FROZEN_OFF_DB_GT" {
            return Err(INVALID);
        }
        if !rec["source_artifact_sha256"].as_str().is_some_and(is_hex64)
            || !rec["source_stage"].is_string()
            || !rec["source_artifact"].is_string()
            || !rec["source_protocol_version"].is_string()
        {
            return Err(INVALID);
        }
        let unit = rec["evaluation_unit"].as_object().ok_or(INVALID)?;
        if !exact_keys(unit, &["sample_id", "task_id", "work_id"]) {
            return Err(INVALID);
        }
        let sample = unit["sample_id"].as_str().filter(|s| !s.is_empty()).ok_or(INVALID)?;
        let (Some(task_id), Some(work_id), Some(run_id)) = (unit["task_id"].as_i64(), unit["work_id"].as_i64(), rec["reviewed_run_id"].as_i64()) else {
            return Err(INVALID);
        };
        let gt = match rec["semantic"].as_str() {
            Some("POSITIVE") => match (rec["media_type"].as_str(), rec["tmdb_id"].as_i64()) {
                (Some(mt @ ("movie" | "tv")), Some(id)) if id > 0 => Gt::Positive(mt.to_string(), id),
                _ => return Err(INVALID),
            },
            Some("NONE") if rec["media_type"].is_null() && rec["tmdb_id"].is_null() => Gt::NoMatch,
            _ => return Err(INVALID),
        };
        // bridge は formal な開発 sample の GT だけ。それ以外は cohort の境界の違反
        if !GT_SAMPLE_ALLOWLIST.contains(&sample) {
            return Err("cohort_boundary_failure");
        }
        let key: UnitKey = (sample.to_string(), task_id, work_id);
        if !seen.insert(key.clone()) {
            return Err("duplicate_bridge_unit");
        }
        out.push(BridgeRec { key, run_id, gt });
    }
    Ok(out)
}

struct LabelRow {
    id: i64,
    work_id: i64,
    run_id: Option<i64>,
    review_task_id: Option<i64>,
    label: String,
    tmdb_id: Option<i64>,
    media_type: Option<String>,
    method: String,
    strength: String,
    superseded: bool,
}

fn gt_of_label(l: &LabelRow) -> Option<Gt> {
    match (l.label.as_str(), l.tmdb_id, l.media_type.as_deref()) {
        ("tmdb", Some(id), Some(mt @ ("movie" | "tv"))) if id > 0 => Some(Gt::Positive(mt.to_string(), id)),
        ("none", None, None) => Some(Gt::NoMatch),
        _ => None,
    }
}

/// 解決の記録（task 側）がラベルと整合していること（PR4-2 の allowlist の条件）
fn provenance_ok(l: &LabelRow, t: &TaskRow) -> bool {
    let Some(d) = t.details.as_ref() else { return false };
    let text = |k: &str| d.get(k).and_then(Value::as_str);
    t.run_id.is_some()
        && l.run_id == t.run_id
        && l.work_id == t.work_id
        && t.resolved
        && t.resolution_label_id == Some(l.id)
        && text("state") == Some("resolved")
        && text("review_protocol_version") == Some(GT_REVIEW_PROTOCOL)
        && text("resolution_method") == Some(l.method.as_str())
        && (l.method != "review_pick_other" || d.get("tmdb_verified") == Some(&Value::Bool(true)))
        && t.sampling_protocol.as_deref() == Some(GT_SAMPLING_PROTOCOL)
        && gt_of_label(l).is_some()
        && ((l.method == "review_none") == (l.label == "none"))
}

/// 1 つのラベルを「信頼できる（None）」か「除外理由」に分類する。**manual だから、最新だから、では信頼しない。**
fn classify_label(l: &LabelRow, task: Option<&TaskRow>) -> Option<&'static str> {
    if l.strength != "strong" || l.method == "lock" {
        return Some("weak_label_not_gt");
    }
    if l.method == "manual_apply" || l.method == "manual_direct_id" {
        return Some("untrusted_manual_gt_source");
    }
    if !TRUSTED_METHODS.contains(&l.method.as_str()) {
        return Some("unknown_label_semantics");
    }
    let Some(task) = task else { return Some("gt_provenance_incomplete") };
    if task.sample_id.as_deref().map_or(true, |s| !GT_SAMPLE_ALLOWLIST.contains(&s)) {
        return Some("gt_sample_not_allowlisted");
    }
    if !provenance_ok(l, task) {
        return Some("gt_provenance_incomplete");
    }
    if l.superseded {
        return Some("gt_superseded");
    }
    None
}

#[derive(Default)]
struct DbGt {
    /// 信頼できる解決済み GT（キー → (GT, 評価した run)）
    units: BTreeMap<UnitKey, (Gt, i64)>,
    /// formal な sample の開発用 task（母集団）
    population: BTreeSet<UnitKey>,
    /// 母集団のうち DB で未解決の task（state 別に数える）
    unresolved: BTreeMap<UnitKey, String>,
    excluded: BTreeMap<&'static str, i64>,
    blockers: BTreeSet<&'static str>,
    label_rows_read: i64,
    tasks_considered: i64,
}

fn require_label_schema(conn: &Connection) -> bool {
    let cols = [
        "id",
        "work_id",
        "run_id",
        "review_task_id",
        "label",
        "tmdb_id",
        "media_type",
        "method",
        "strength",
        "superseded_at",
    ];
    let Ok(mut stmt) = conn.prepare("SELECT name FROM pragma_table_info('metadata_match_labels')") else { return false };
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) else { return false };
    let present: HashSet<String> = rows.filter_map(Result::ok).collect();
    cols.iter().all(|c| present.contains(*c))
}

/// DB の信頼できる GT を組み立てる。対象は `select_runs` が Development と分類し、holdout / 未対応の task が無い作品の課題だけ。
fn db_gt(conn: &Connection, sel: &Selection) -> Result<DbGt, DiagnosticsError> {
    let mut out = DbGt { excluded: GT_EXCLUDED_KEYS.iter().map(|k| (*k, 0)).collect(), ..Default::default() };
    let tasks: Vec<&TaskRow> = sel.tasks.iter().filter(|t| t.class == TaskClass::Development && !sel.sealed_works.contains(&t.work_id)).collect();
    out.tasks_considered = tasks.len() as i64;
    if !require_label_schema(conn) {
        out.blockers.insert("unsupported_schema");
        return Ok(out);
    }
    let works: Vec<i64> = tasks.iter().map(|t| t.work_id).collect::<BTreeSet<_>>().into_iter().collect();
    let mut labels: Vec<LabelRow> = Vec::new();
    for chunk in works.chunks(CHUNK) {
        let sql = format!(
            "SELECT id, work_id, run_id, review_task_id, label, tmdb_id, media_type, method, strength, superseded_at
               FROM metadata_match_labels WHERE work_id IN ({}) ORDER BY id",
            placeholders(chunk.len())
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
            Ok(LabelRow {
                id: row.get(0)?,
                work_id: row.get(1)?,
                run_id: row.get(2)?,
                review_task_id: row.get(3)?,
                label: row.get(4)?,
                tmdb_id: row.get(5)?,
                media_type: row.get(6)?,
                method: row.get(7)?,
                strength: row.get(8)?,
                superseded: row.get::<_, Option<String>>(9)?.is_some(),
            })
        })?;
        for row in rows {
            labels.push(row?);
        }
    }
    out.label_rows_read = labels.len() as i64;

    let by_id: HashMap<i64, &TaskRow> = tasks.iter().map(|t| (t.id, *t)).collect();
    let mut bound: HashMap<i64, Vec<&LabelRow>> = HashMap::new();
    for l in &labels {
        match l.review_task_id.and_then(|id| by_id.get(&id)).filter(|t| t.work_id == l.work_id) {
            Some(task) => bound.entry(task.id).or_default().push(l),
            None => {
                if let Some(reason) = classify_label(l, None) {
                    *out.excluded.get_mut(reason).expect("known") += 1;
                }
            }
        }
    }

    for task in &tasks {
        let allowlisted = task.sample_id.as_deref().is_some_and(|s| GT_SAMPLE_ALLOWLIST.contains(&s));
        let key: Option<UnitKey> = match (&task.sample_id, allowlisted) {
            (Some(s), true) => Some((s.clone(), task.id, task.work_id)),
            _ => None,
        };
        if let Some(k) = &key {
            out.population.insert(k.clone());
        }
        let mut trusted: Vec<(&LabelRow, Gt)> = Vec::new();
        for &l in bound.get(&task.id).map(Vec::as_slice).unwrap_or(&[]) {
            match classify_label(l, Some(*task)) {
                None => trusted.push((l, gt_of_label(l).expect("checked by provenance_ok"))),
                Some(reason) => {
                    *out.excluded.get_mut(reason).expect("known") += 1;
                    // 監査課題に紐づくラベルの意味が未対応なら、黙って除かず GT 全体を止める
                    if reason == "unknown_label_semantics" {
                        out.blockers.insert("unsupported_label_semantics");
                    }
                }
            }
        }
        let Some(key) = key else { continue };
        if task.state.as_deref() != Some("resolved") {
            out.unresolved.insert(key, task.state.clone().unwrap_or_else(|| "unknown".to_string()));
            continue;
        }
        if trusted.is_empty() {
            continue;
        }
        let first = trusted[0].1.clone();
        if trusted.iter().any(|(_, g)| *g != first) {
            *out.excluded.get_mut("gt_conflict").expect("known") += 1;
            continue;
        }
        // 同じ評価単位の内容が同一の GT は 1 件に畳む
        out.units.insert(key, (first, trusted[0].0.run_id.expect("checked by provenance_ok")));
    }
    Ok(out)
}

/// GT の出典の記録。同じ評価単位を両方の供給元が指すときは、両方を残す（`reviewed_run_id` の値も含めて）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProvenanceRef {
    source: &'static str,
    /// その供給元が「人が見た run」として記録している run（属性。評価単位のキーの成分でも canonical GT でもない）
    reviewed_run_id: i64,
}

const SOURCE_DB: &str = "TRUSTED_DB_GT";
const SOURCE_BRIDGE: &str = "FROZEN_OFF_DB_GT";

struct Merged {
    gt: Gt,
    /// DB の task が指す run（DB に task が無い bridge だけの単位は None）。機械側の出力はこの run のものを使う
    db_run: Option<i64>,
    provenance: Vec<ProvenanceRef>,
}

struct MergeResult {
    units: BTreeMap<UnitKey, Merged>,
    db_only: i64,
    bridge_only: i64,
    merged_same_gt: i64,
    conflicts: i64,
    /// 同じ評価単位・同じ canonical GT で、`reviewed_run_id` の値だけが供給元の間で違う件数（結合は許す。値は provenance に残す）
    reviewed_run_id_differs: i64,
    population: BTreeSet<UnitKey>,
    blockers: BTreeSet<&'static str>,
}

/// 2 つの供給元を結合する。優先順位は無い（latest-wins なし）。
///
/// - 評価単位のキーは `{sample_id, task_id, work_id}`。canonical GT は `POSITIVE(media_type, tmdb_id)` / `NONE`。
/// - **同じキー・同じ canonical GT → 結合する**（`reviewed_run_id` が違っても結合を許し、両方の出典を provenance に残す）。
/// - **同じキー・違う canonical GT → `gt_source_conflict`**（`reviewed_run_id` が同じでも違っても）。
/// - `reviewed_run_id` は属性であって canonical GT ではない。その差だけで GT の衝突にはしない（PR4-2A のキーの定義に従う）。
/// - DB に task がある bridge の単位は、キーの全成分を DB と照合する。
fn merge_gt(db: &DbGt, bridge: &[BridgeRec], sel: &Selection) -> MergeResult {
    let mut res = MergeResult {
        units: BTreeMap::new(),
        db_only: 0,
        bridge_only: 0,
        merged_same_gt: 0,
        conflicts: 0,
        reviewed_run_id_differs: 0,
        population: db.population.clone(),
        blockers: BTreeSet::new(),
    };
    let all_tasks: HashMap<i64, &TaskRow> = sel.tasks.iter().map(|t| (t.id, t)).collect();
    let mut conflict_keys: BTreeSet<UnitKey> = BTreeSet::new();
    let mut bridge_keys: BTreeSet<UnitKey> = BTreeSet::new();
    for rec in bridge {
        res.population.insert(rec.key.clone());
        bridge_keys.insert(rec.key.clone());
        let (sample, task_id, work_id) = (&rec.key.0, rec.key.1, rec.key.2);
        let mut db_run: Option<i64> = None;
        if let Some(task) = all_tasks.get(&task_id) {
            if task.class != TaskClass::Development || sel.sealed_works.contains(&task.work_id) {
                res.blockers.insert("cohort_boundary_failure");
                continue;
            }
            if task.work_id != work_id || task.sample_id.as_deref() != Some(sample.as_str()) {
                res.blockers.insert("evaluation_key_mismatch");
                continue;
            }
            db_run = task.run_id;
        }
        let bridge_ref = ProvenanceRef { source: SOURCE_BRIDGE, reviewed_run_id: rec.run_id };
        match db.units.get(&rec.key) {
            Some((db_gt, db_label_run)) if *db_gt == rec.gt => {
                res.merged_same_gt += 1;
                if *db_label_run != rec.run_id {
                    res.reviewed_run_id_differs += 1;
                }
                res.units.insert(
                    rec.key.clone(),
                    Merged { gt: rec.gt.clone(), db_run, provenance: vec![ProvenanceRef { source: SOURCE_DB, reviewed_run_id: *db_label_run }, bridge_ref] },
                );
            }
            Some(_) => {
                res.blockers.insert("gt_source_conflict");
                res.conflicts += 1;
                conflict_keys.insert(rec.key.clone());
            }
            None => {
                res.bridge_only += 1;
                if db_run.is_some_and(|r| r != rec.run_id) {
                    res.reviewed_run_id_differs += 1;
                }
                res.units.insert(rec.key.clone(), Merged { gt: rec.gt.clone(), db_run, provenance: vec![bridge_ref] });
            }
        }
    }
    for (key, (gt, run)) in &db.units {
        if bridge_keys.contains(key) || conflict_keys.contains(key) {
            continue;
        }
        res.db_only += 1;
        res.units.insert(key.clone(), Merged { gt: gt.clone(), db_run: Some(*run), provenance: vec![ProvenanceRef { source: SOURCE_DB, reviewed_run_id: *run }] });
    }
    res
}

/// `diagnostic_gt` セクションを作る。PR4-1 の cohort 選択（`selection`）と verdict（`per_run`）をそのまま使う。
fn diagnostic_gt_section(
    conn: &Connection,
    cohort: DiagnosticCohort,
    selection: &Selection,
    per_run: &HashMap<i64, PerRun>,
    cfg: Option<&GtConfig>,
) -> Result<Value, DiagnosticsError> {
    if cohort != DiagnosticCohort::DevelopmentAudit {
        return Ok(json!({"status": "NOT_APPLICABLE", "reason": "診断 GT の指標は A1 の開発用 audit cohort だけで定義される", "contract_version": GT_CONTRACT_VERSION}));
    }
    let blocked = |blockers: Vec<&str>, sha: Option<&str>| {
        json!({
            "status": "BLOCKED", "contract_version": GT_CONTRACT_VERSION, "bridge_version": BRIDGE_VERSION, "bridge_sha256": sha,
            "bridge_status": "BLOCKED", "blockers": blockers, "profiles": {}, "precision_computed": false,
            "correctness_definition": "exact_media_type_tmdb_id", "top3": "NOT_APPLICABLE",
            "cohort_boundary_integrity": COHORT_BOUNDARY_INTEGRITY,
        })
    };
    let Some(cfg) = cfg else { return Ok(blocked(vec!["bridge_missing"], None)) };
    let (bridge, bridge_sha) = match load_bridge(cfg) {
        Ok(ok) => ok,
        Err(code) => return Ok(blocked(vec![code], None)),
    };

    let db = db_gt(conn, selection)?;
    let mut blockers: BTreeSet<&'static str> = db.blockers.clone();
    let mut merged = merge_gt(&db, &bridge, selection);
    blockers.extend(merged.blockers.iter().copied());

    // 1 つの作品に評価単位が複数ある → 全て除外（PR4-2）。除外は件数に出し、黙って受け入れない
    let mut excluded = db.excluded.clone();
    let mut by_work: BTreeMap<i64, Vec<UnitKey>> = BTreeMap::new();
    for key in merged.units.keys() {
        by_work.entry(key.2).or_default().push(key.clone());
    }
    for keys in by_work.values().filter(|k| k.len() > 1) {
        *excluded.get_mut("gt_multiple_units_per_work").expect("known") += keys.len() as i64;
        for k in keys {
            merged.units.remove(k);
        }
    }

    // 母集団・解決・保留の整合（正式 closeout の会計）
    // 衝突・キーの不一致・cohort の違反など、より根本の blocker が既にあるときは、その結果として生じる件数のずれを重ねて報告しない
    let population = merged.population.len();
    let resolved = merged.units.len();
    let deferred = population.saturating_sub(resolved);
    if blockers.is_empty() && (population != cfg.expected_population || resolved != cfg.expected_resolved || deferred != cfg.expected_deferred) {
        blockers.insert("gt_universe_mismatch");
    }

    // DB で未解決だが bridge が解決している単位は、未解決として数えない
    let covered_by_bridge = db.unresolved.keys().filter(|k| merged.units.contains_key(*k)).count() as i64;
    let mut unresolved: BTreeMap<String, i64> = BTreeMap::new();
    for (key, state) in &db.unresolved {
        if !merged.units.contains_key(key) {
            *unresolved.entry(state.clone()).or_insert(0) += 1;
        }
    }

    // 機械側（blocker が無いときだけ計算する。分母を黙って減らして計算可能にしない）
    let eligible: HashSet<i64> = selection.eligible.iter().copied().collect();
    let mut profiles = Map::new();
    if blockers.is_empty() {
        for matcher in [MATCHER_SAFE, MATCHER_SHADOW] {
            let (mut evaluable, mut correct) = (0i64, 0i64);
            let mut skipped: BTreeMap<&str, i64> = ["machine_run_unavailable", "run_not_eligible", "machine_verdict_missing", "unsupported_matcher_version", "machine_not_auto"]
                .iter()
                .map(|k| (*k, 0))
                .collect();
            let mut run_not_eligible_by_reason: BTreeMap<&'static str, i64> = BTreeMap::new();
            for unit in merged.units.values() {
                let Some(run_id) = unit.db_run else {
                    *skipped.get_mut("machine_run_unavailable").unwrap() += 1;
                    continue;
                };
                if !eligible.contains(&run_id) {
                    *skipped.get_mut("run_not_eligible").unwrap() += 1;
                    if let Some(reason) = selection.run_exclusion.get(&run_id) {
                        *run_not_eligible_by_reason.entry(reason).or_insert(0) += 1;
                    }
                    continue;
                }
                let state = per_run.get(&run_id).and_then(|p| if matcher == MATCHER_SAFE { p.safe.as_ref() } else { p.shadow.as_ref() });
                match state {
                    None => *skipped.get_mut("machine_verdict_missing").unwrap() += 1,
                    Some(VerdictState::UnsupportedVersion) => *skipped.get_mut("unsupported_matcher_version").unwrap() += 1,
                    Some(VerdictState::Supported(v)) if v.decision != "AUTO" => *skipped.get_mut("machine_not_auto").unwrap() += 1,
                    Some(VerdictState::Supported(v)) => match &v.identity {
                        // AUTO なのに選んだ identity が無い → 正しさを推測せず GT 全体を止める
                        None => {
                            blockers.insert("invalid_auto_output");
                        }
                        Some((tmdb_id, media_type)) => {
                            evaluable += 1;
                            if matches!(&unit.gt, Gt::Positive(mt, id) if mt == media_type && id == tmdb_id) {
                                correct += 1;
                            }
                        }
                    },
                }
            }
            profiles.insert(
                matcher.to_string(),
                json!({
                    "matcher_version": if matcher == MATCHER_SAFE { POLICY_VERSION } else { SHADOW_VERSION },
                    "auto_evaluable_count": evaluable,
                    "auto_correct_count": correct,
                    "auto_incorrect_count": evaluable - correct,
                    "diagnostic_conditional_auto_precision": rate(correct, evaluable),
                    "not_in_denominator": skipped,
                    "run_not_eligible_by_reason": run_not_eligible_by_reason,
                }),
            );
        }
    }
    let pass = blockers.is_empty();
    if !pass {
        profiles.clear();
    }

    Ok(json!({
        "status": if pass { "PASS" } else { "BLOCKED" },
        "contract_version": GT_CONTRACT_VERSION,
        "bridge_version": BRIDGE_VERSION,
        "bridge_sha256": bridge_sha,
        "bridge_status": "READY",
        "blockers": blockers,
        "gt_universe": {
            "formal_population": population, "resolved": resolved, "deferred": deferred,
            "expected_population": cfg.expected_population, "expected_resolved": cfg.expected_resolved, "expected_deferred": cfg.expected_deferred,
            "db_only": merged.db_only, "bridge_only": merged.bridge_only, "merged_same_gt": merged.merged_same_gt,
            "source_conflicts": merged.conflicts, "reviewed_run_id_differs": merged.reviewed_run_id_differs,
            "provenance_refs": merged.units.values().map(|u| u.provenance.len()).sum::<usize>(), "db_unresolved_covered_by_bridge": covered_by_bridge,
        },
        "excluded_gt": excluded,
        "unresolved_gt_count": unresolved,
        "materialization": {
            "tasks_considered": db.tasks_considered, "label_rows_read": db.label_rows_read,
            "note": "ラベルは、select_runs が開発用と分類し、holdout / 未対応の task が無い作品の分だけを読む。GT の中身はレポートに出さない（件数だけ）",
        },
        "profiles": Value::Object(profiles),
        "precision_computed": pass,
        "denominator_definition": "matcher ごとに、適格な評価単位のうち、その matcher の verdict が AUTO で、信頼できる解決済みの GT がある単位。44 は GT の単位数の整合性の検査で、分母ではない",
        "correctness_definition": "exact_media_type_tmdb_id",
        "none_gt": "AUTO が候補を選んでいれば常に誤り（分母に入る）",
        "top3": "NOT_APPLICABLE",
        "cohort_boundary_integrity": COHORT_BOUNDARY_INTEGRITY,
    }))
}

// ─── レポート ────────────────────────────────────────────────────────────────

/// 通常の診断レポートを作る。**読み取り専用。** `generated_at` と `head` は呼び出し側が渡す（この関数は時刻も git も見ない）。
pub fn generate_report(
    conn: &Connection,
    cohort: DiagnosticCohort,
    generated_at: &str,
    head: &str,
    gt: Option<&GtConfig>,
) -> Result<Value, DiagnosticsError> {
    require_schema(conn)?;
    let _guard = QueryOnlyGuard::new(conn)?;

    let selection = select_runs(conn, cohort)?;
    let excluded_total: i64 = selection.exclusions.values().sum();
    debug_assert_eq!(selection.eligible.len() as i64 + excluded_total, selection.total_source_rows);

    // verdict は適格な run の分だけ読む（除外された run の verdict・候補・ラベルは読まない）
    let (per_run, rules_one_rows) = read_verdicts(conn, &selection.eligible)?;
    // PR4-3: 診断 GT。cohort の選択と verdict は上と同じもの（第 2 の実装を作らない）。gt が None なら bridge_missing で BLOCKED
    let diagnostic_gt = diagnostic_gt_section(conn, cohort, &selection, &per_run, gt)?;

    let safe = matcher_metrics(MATCHER_SAFE, POLICY_VERSION, &selection.eligible, &per_run);
    let shadow = matcher_metrics(MATCHER_SHADOW, SHADOW_VERSION, &selection.eligible, &per_run);
    let pick = |key: &str| -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(MATCHER_SAFE.to_string(), safe[key].clone());
        m.insert(MATCHER_SHADOW.to_string(), shadow[key].clone());
        m
    };
    let denominators: Map<String, Value> = {
        let mut m = Map::new();
        m.insert(MATCHER_SAFE.to_string(), safe["denominator"].clone());
        m.insert(MATCHER_SHADOW.to_string(), shadow["denominator"].clone());
        m
    };

    Ok(json!({
        "report_metadata": {
            "report_version": REPORT_VERSION,
            "generated_at": generated_at,
            "head": head,
            "cohort_type": cohort.as_str(),
            "source_selection": cohort.selection_rule(),
            "matcher_profiles": {MATCHER_SAFE: POLICY_VERSION, MATCHER_SHADOW: SHADOW_VERSION},
            "total_source_rows": selection.total_source_rows,
            "eligible_rows": selection.eligible.len(),
            "excluded_rows": excluded_total,
            "exclusions": selection.exclusions,
            "exclusion_precedence": EXCLUSION_REASONS,
            "excluded_holdout_count": selection.exclusions["sealed_holdout"],
            "cohort_boundary_integrity": COHORT_BOUNDARY_INTEGRITY,
            "cohort_boundary_note": "cohort（purpose / cohort）の境界は抽出記録（sampling_json）をアプリケーションが検証して判定している。DB のトリガーや制約で不変が保証されているわけではない（集計を無効にする注記ではない）",
            "holdout_policy": "holdout の run・同じ作品の他の run・抽出記録が読めない task の作品の run は、集計の前に除く。除外した run の verdict / 候補 / ラベルは読まない（件数だけ数える）",
            "matcher_denominators": denominators,
            "rules_1_version_boundary": {
                "status": "historical_version_boundary_unresolved",
                "rules_1_verdict_rows_among_eligible_runs": rules_one_rows,
                "used_in_metrics": false,
                "note": "rules-1 の matcher_version は凍結（91b533d）の前後で同じ文字列。凍結後の版と言い切れる永続された証拠が無いので、指標には使わず件数だけ報告する",
            },
            "limitations": [
                "rules-safe と rules-tags-shadow は入力の候補集合が違う",
                "A2 では evidence_class='live' を live_evidence_policy_ambiguous として除外している。policy freeze まで定義が変わりうる一時的な安全側の処理で、live を恒久的に A2 の対象外とする意味ではない",
                "履歴の版の境界は version 文字列でしか分からない。現行と異なる版は集計に混ぜず除外理由で数える",
            ],
        },
        "reason_codes": Value::Object(pick("reason_codes")),
        "review": Value::Object(pick("review")),
        "auto": Value::Object(pick("auto")),
        "abstention": Value::Object(pick("abstention")),
        "profile_comparison": profile_comparison(&selection.eligible, &per_run),
        "diagnostic_gt": diagnostic_gt,
        "final_promotion": {"status": "NOT_RUN", "reason": "HOLDOUT_SEALED_POLICY_NOT_FROZEN"},
    }))
}

// ─── テスト ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::{insert_work, open_migrated};

    const HEAD: &str = "0000000000000000000000000000000000000000";
    const NOW: &str = "2026-10-05T00:00:00Z";

    fn run_row(conn: &Connection, mode: &str, trigger: &str, evidence: &str, policy: &str, decision: &str) -> (i64, i64) {
        let work_id = insert_work(conn, "w");
        conn.execute(
            "INSERT INTO metadata_match_runs
               (work_id, batch_id, trigger_kind, mode, evidence_class, state_schema_version,
                input_snapshot_json, search_queries_json, policy_version, policy_snapshot_json,
                governing_matcher, decision, decision_reasons_json, error_text)
             VALUES (?1, 'b', ?2, ?3, ?4, 'cm-prematch-2', '{}', '[]', ?5, '{}', 'rules-safe', ?6, '[]', ?7)",
            rusqlite::params![work_id, trigger, mode, evidence, policy, decision, if decision == "ERROR" { Some("e") } else { None }],
        )
        .unwrap();
        (work_id, conn.last_insert_rowid())
    }

    fn task(conn: &Connection, work_id: i64, run_id: Option<i64>, sampling: &str, state: Option<&str>) {
        let details = state.map(|s| format!("{{\"state\":\"{s}\"}}"));
        conn.execute(
            "INSERT INTO metadata_review_tasks (work_id, run_id, reason, sampling_json, details_json)
             VALUES (?1, ?2, 'audit_sample', ?3, ?4)",
            rusqlite::params![work_id, run_id, sampling, details],
        )
        .unwrap();
    }

    fn sampling(purpose: &str, cohort: &str) -> String {
        format!("{{\"purpose\":\"{purpose}\",\"cohort\":\"{cohort}\"}}")
    }

    /// 適格な development audit run（task 付き）
    fn dev_run(conn: &Connection, decision: &str) -> i64 {
        let (work, run) = run_row(conn, "safe", "batch", "reconstructed_clean", POLICY_VERSION, decision);
        task(conn, work, Some(run), &sampling("development", "reconstructed_clean"), Some("ready"));
        run
    }

    fn verdict(conn: &Connection, run: i64, matcher: &str, version: &str, id: Option<(i64, &str)>, decision: &str, reasons: Option<&str>) {
        conn.execute(
            "INSERT INTO metadata_match_verdicts (run_id, matcher, matcher_version, tmdb_id, media_type, decision, reasons_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![run, matcher, version, id.map(|x| x.0), id.map(|x| x.1.to_string()), decision, reasons],
        )
        .unwrap();
    }

    fn safe_v(conn: &Connection, run: i64, id: Option<(i64, &str)>, decision: &str, reasons: Option<&str>) {
        verdict(conn, run, "rules-safe", POLICY_VERSION, id, decision, reasons);
    }

    fn shadow_v(conn: &Connection, run: i64, id: Option<(i64, &str)>, decision: &str, reasons: Option<&str>) {
        verdict(conn, run, "rules-tags-shadow", SHADOW_VERSION, id, decision, reasons);
    }

    fn report(conn: &Connection, cohort: DiagnosticCohort) -> Value {
        generate_report(conn, cohort, NOW, HEAD, None).unwrap()
    }

    fn dev(conn: &Connection) -> Value {
        report(conn, DiagnosticCohort::DevelopmentAudit)
    }

    fn n(v: &Value) -> i64 {
        v.as_i64().unwrap_or_else(|| panic!("not an integer: {v}"))
    }

    fn reason_count(r: &Value, matcher: &str, code: &str) -> i64 {
        let items = r["reason_codes"][matcher]["items"].as_array().unwrap();
        n(&items.iter().find(|i| i["reason_code"] == code).unwrap()["count"])
    }

    fn bucket(r: &Value, matcher: &str, name: &str) -> i64 {
        let items = r["reason_codes"][matcher]["buckets"].as_array().unwrap();
        n(&items.iter().find(|i| i["bucket"] == name).unwrap()["count"])
    }

    fn dim<'a>(r: &'a Value, name: &str) -> &'a Value {
        &r["profile_comparison"][name]
    }

    // ─── 理由コード ──────────────────────────────────────────────────────

    #[test]
    fn reason_codes_cover_zero_one_multiple_unknown_and_null() {
        let conn = open_migrated();
        let r0 = dev_run(&conn, "AUTO");
        safe_v(&conn, r0, Some((1, "movie")), "AUTO", Some("[]"));
        let r1 = dev_run(&conn, "REVIEW");
        safe_v(&conn, r1, Some((2, "movie")), "REVIEW", Some("[\"NO_YEAR_HINT\"]"));
        let r2 = dev_run(&conn, "REVIEW");
        safe_v(&conn, r2, Some((3, "movie")), "REVIEW", Some("[\"TIED_TOP\",\"NO_YEAR_HINT\",\"BELOW_THRESHOLD\"]"));
        let r3 = dev_run(&conn, "REVIEW");
        safe_v(&conn, r3, Some((4, "movie")), "REVIEW", None);
        let r4 = dev_run(&conn, "REVIEW");
        safe_v(&conn, r4, Some((5, "movie")), "REVIEW", Some("[\"BRAND_NEW_CODE\",\"TIED_TOP\"]"));
        let r5 = dev_run(&conn, "REVIEW");
        safe_v(&conn, r5, Some((6, "movie")), "REVIEW", Some("[]"));
        let r6 = dev_run(&conn, "REVIEW");
        safe_v(&conn, r6, Some((7, "movie")), "REVIEW", Some("not json"));

        let r = dev(&conn);
        assert_eq!(n(&r["reason_codes"]["rules-safe"]["denominator"]), 7);
        assert_eq!(reason_count(&r, "rules-safe", "NO_YEAR_HINT"), 2);
        assert_eq!(reason_count(&r, "rules-safe", "TIED_TOP"), 2);
        assert_eq!(reason_count(&r, "rules-safe", "BELOW_THRESHOLD"), 1);
        assert_eq!(reason_count(&r, "rules-safe", "NO_CANDIDATES"), 0, "0 件のコードも出す");
        assert_eq!(r["reason_codes"]["rules-safe"]["items"].as_array().unwrap().len(), 14);
        assert_eq!(bucket(&r, "rules-safe", "NO_REASON_AUTO"), 1);
        assert_eq!(bucket(&r, "rules-safe", "EMPTY_REASONS_NON_AUTO"), 1, "AUTO でないのに空配列");
        assert_eq!(bucket(&r, "rules-safe", "UNKNOWN_OR_NULL"), 3, "NULL・JSON でない・未知コード");
        // 率は count / denominator
        let items = r["reason_codes"]["rules-safe"]["items"].as_array().unwrap();
        let tied = items.iter().find(|i| i["reason_code"] == "TIED_TOP").unwrap();
        assert!((tied["rate"].as_f64().unwrap() - 2.0 / 7.0).abs() < 1e-12);
    }

    #[test]
    fn reason_rates_may_sum_above_one_and_rules_one_notes_are_not_reasons() {
        let conn = open_migrated();
        for codes in ["[\"TIED_TOP\",\"NO_YEAR_HINT\"]", "[\"TIED_TOP\",\"NO_YEAR_HINT\"]", "[\"TIED_TOP\"]"] {
            let run = dev_run(&conn, "REVIEW");
            safe_v(&conn, run, Some((1, "movie")), "REVIEW", Some(codes));
            verdict(&conn, run, "rules-1", "rules-1", Some((1, "movie")), "AUTO", Some("[\"NOTE_FROM_FROZEN_SIDE\"]"));
        }
        let r = dev(&conn);
        let sum: f64 = r["reason_codes"]["rules-safe"]["items"].as_array().unwrap().iter().map(|i| i["rate"].as_f64().unwrap()).sum();
        assert!(sum > 1.0, "multi-label なので率の合計は 1 を超える: {sum}");
        assert_eq!(bucket(&r, "rules-safe", "UNKNOWN_OR_NULL"), 0, "rules-1 の notes は理由コードとして数えない");
        assert!(!r.to_string().contains("NOTE_FROM_FROZEN_SIDE"));
        assert_eq!(n(&r["report_metadata"]["rules_1_version_boundary"]["rules_1_verdict_rows_among_eligible_runs"]), 3);
        assert_eq!(r["report_metadata"]["rules_1_version_boundary"]["used_in_metrics"], false);
    }

    // ─── REVIEW / AUTO / abstention ──────────────────────────────────────

    #[test]
    fn review_auto_and_abstention_use_the_persisted_states_and_explicit_denominators() {
        let conn = open_migrated();
        for (decision, id) in [("AUTO", Some((1, "movie"))), ("AUTO", Some((2, "movie"))), ("REVIEW", Some((3, "movie"))), ("REVIEW", Some((4, "movie"))),
                               ("REVIEW", Some((5, "movie"))), ("UNRESOLVED", None)] {
            let run = dev_run(&conn, decision);
            safe_v(&conn, run, id, decision, Some(if decision == "AUTO" { "[]" } else { "[\"NO_YEAR_HINT\"]" }));
        }
        let r = dev(&conn);
        assert_eq!(n(&r["review"]["rules-safe"]["count"]), 3);
        assert_eq!(n(&r["review"]["rules-safe"]["eligible_decision_count"]), 6);
        assert!((r["review"]["rules-safe"]["rate"].as_f64().unwrap() - 0.5).abs() < 1e-12);
        assert_eq!(n(&r["auto"]["rules-safe"]["count"]), 2);
        assert!((r["auto"]["rules-safe"]["coverage"].as_f64().unwrap() - 2.0 / 6.0).abs() < 1e-12);
        assert_eq!(n(&r["abstention"]["rules-safe"]["count"]), 4, "REVIEW 3 + UNRESOLVED 1");
        assert_eq!(n(&r["abstention"]["rules-safe"]["unresolved_count"]), 1);
        assert_eq!(r["abstention"]["rules-safe"]["states"], json!(["REVIEW", "UNRESOLVED"]));
        assert!((r["abstention"]["rules-safe"]["rate"].as_f64().unwrap() - 4.0 / 6.0).abs() < 1e-12);
        assert_eq!(r["abstention"]["rules-safe"]["partition_check"]["auto_plus_abstention_equals_denominator"], true);
        // shadow の verdict は 1 件も無い: 分母は 0、率は null（0 除算しない）
        assert_eq!(n(&r["review"]["rules-tags-shadow"]["eligible_decision_count"]), 0);
        assert!(r["review"]["rules-tags-shadow"]["rate"].is_null());
        assert!(r["auto"]["rules-tags-shadow"]["coverage"].is_null());
        assert!(r["abstention"]["rules-tags-shadow"]["rate"].is_null());
        assert_eq!(n(&r["report_metadata"]["matcher_denominators"]["rules-tags-shadow"]["missing_verdict_runs"]), 6);
    }

    #[test]
    fn an_empty_database_reports_zero_denominators_without_dividing() {
        let conn = open_migrated();
        let r = dev(&conn);
        assert_eq!(n(&r["report_metadata"]["total_source_rows"]), 0);
        assert_eq!(n(&r["report_metadata"]["eligible_rows"]), 0);
        assert!(r["auto"]["rules-safe"]["coverage"].is_null());
        assert!(dim(&r, "decision")["agreement_rate"].is_null());
        assert_eq!(n(&dim(&r, "decision")["comparable_count"]), 0);
    }

    // ─── rules-safe vs rules-tags-shadow ─────────────────────────────────

    #[test]
    fn exact_agreement_is_counted_in_every_dimension() {
        let conn = open_migrated();
        let run = dev_run(&conn, "AUTO");
        safe_v(&conn, run, Some((10, "movie")), "AUTO", Some("[]"));
        shadow_v(&conn, run, Some((10, "movie")), "AUTO", Some("[]"));
        let r = dev(&conn);
        for d in ["decision", "top_identity", "auto_result", "reason_set"] {
            assert_eq!(n(&dim(&r, d)["comparable_count"]), 1, "{d}");
            assert_eq!(n(&dim(&r, d)["agreement_count"]), 1, "{d}");
            assert_eq!(n(&dim(&r, d)["disagreement_count"]), 0, "{d}");
            assert_eq!(n(&dim(&r, d)["not_comparable_count"]), 0, "{d}");
            assert!((dim(&r, d)["agreement_rate"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        }
        assert_eq!(n(&dim(&r, "auto_result")["breakdown"]["both_auto_same_identity"]), 1);
    }

    #[test]
    fn each_disagreement_type_is_counted_separately() {
        let conn = open_migrated();
        // 1) 両方 AUTO で identity が違う
        let a = dev_run(&conn, "AUTO");
        safe_v(&conn, a, Some((1, "movie")), "AUTO", Some("[]"));
        shadow_v(&conn, a, Some((2, "movie")), "AUTO", Some("[]"));
        // 2) rules-safe だけ AUTO
        let b = dev_run(&conn, "AUTO");
        safe_v(&conn, b, Some((3, "movie")), "AUTO", Some("[]"));
        shadow_v(&conn, b, Some((3, "movie")), "REVIEW", Some("[\"NO_YEAR_HINT\"]"));
        // 3) shadow だけ AUTO（identity は同じ）
        let c = dev_run(&conn, "REVIEW");
        safe_v(&conn, c, Some((4, "movie")), "REVIEW", Some("[\"NO_YEAR_HINT\"]"));
        shadow_v(&conn, c, Some((4, "movie")), "AUTO", Some("[]"));
        // 4) 両方 REVIEW・identity 違い・理由も違う
        let d = dev_run(&conn, "REVIEW");
        safe_v(&conn, d, Some((5, "movie")), "REVIEW", Some("[\"TIED_TOP\"]"));
        shadow_v(&conn, d, Some((6, "tv")), "REVIEW", Some("[\"NO_YEAR_HINT\"]"));
        // 5) どちらも AUTO でない・同じ
        let e = dev_run(&conn, "REVIEW");
        safe_v(&conn, e, Some((7, "movie")), "REVIEW", Some("[\"TIED_TOP\"]"));
        shadow_v(&conn, e, Some((7, "movie")), "REVIEW", Some("[\"TIED_TOP\"]"));

        let r = dev(&conn);
        let decision = dim(&r, "decision");
        assert_eq!((n(&decision["comparable_count"]), n(&decision["agreement_count"]), n(&decision["disagreement_count"])), (5, 3, 2));
        let identity = dim(&r, "top_identity");
        assert_eq!((n(&identity["agreement_count"]), n(&identity["disagreement_count"])), (3, 2), "b・c・e が一致、a・d が不一致");
        let auto = dim(&r, "auto_result");
        assert_eq!(n(&auto["breakdown"]["both_auto_different_identity"]), 1);
        assert_eq!(n(&auto["breakdown"]["rules_safe_auto_only"]), 1);
        assert_eq!(n(&auto["breakdown"]["rules_tags_shadow_auto_only"]), 1);
        assert_eq!(n(&auto["breakdown"]["neither_auto"]), 2);
        assert_eq!(n(&auto["breakdown"]["both_auto_same_identity"]), 0);
        assert_eq!((n(&auto["agreement_count"]), n(&auto["disagreement_count"])), (2, 3));
        let reasons = dim(&r, "reason_set");
        assert_eq!((n(&reasons["agreement_count"]), n(&reasons["disagreement_count"])), (2, 3), "a・e が一致");
    }

    #[test]
    fn missing_counterparts_are_not_comparable_and_are_counted_by_reason() {
        let conn = open_migrated();
        let only_safe = dev_run(&conn, "AUTO");
        safe_v(&conn, only_safe, Some((1, "movie")), "AUTO", Some("[]"));
        let only_shadow = dev_run(&conn, "AUTO");
        shadow_v(&conn, only_shadow, Some((2, "movie")), "AUTO", Some("[]"));
        let _neither = dev_run(&conn, "AUTO");
        let both = dev_run(&conn, "AUTO");
        safe_v(&conn, both, Some((3, "movie")), "AUTO", Some("[]"));
        shadow_v(&conn, both, Some((3, "movie")), "AUTO", Some("[]"));

        let r = dev(&conn);
        for d in ["decision", "top_identity", "auto_result", "reason_set"] {
            let x = dim(&r, d);
            assert_eq!(n(&x["comparable_count"]), 1, "{d}");
            assert_eq!(n(&x["not_comparable_count"]), 3, "{d}");
            assert_eq!(n(&x["not_comparable_by_reason"]["missing_rules_tags_shadow_verdict"]), 1, "{d}");
            assert_eq!(n(&x["not_comparable_by_reason"]["missing_rules_safe_verdict"]), 1, "{d}");
            assert_eq!(n(&x["not_comparable_by_reason"]["missing_both_verdicts"]), 1, "{d}");
        }
    }

    #[test]
    fn different_candidate_sets_do_not_turn_a_missing_identity_into_a_disagreement() {
        let conn = open_migrated();
        // rules-safe は候補なし（UNRESOLVED・identity なし）、rules-tags-shadow は統合候補で top がある
        let a = dev_run(&conn, "UNRESOLVED");
        safe_v(&conn, a, None, "UNRESOLVED", Some("[\"NO_CANDIDATES\"]"));
        shadow_v(&conn, a, Some((9, "movie")), "REVIEW", Some("[\"NO_YEAR_HINT\"]"));
        // 両方とも候補なし
        let b = dev_run(&conn, "UNRESOLVED");
        safe_v(&conn, b, None, "UNRESOLVED", Some("[\"NO_CANDIDATES\"]"));
        shadow_v(&conn, b, None, "UNRESOLVED", Some("[\"NO_CANDIDATES\"]"));

        let r = dev(&conn);
        let identity = dim(&r, "top_identity");
        assert_eq!(n(&identity["comparable_count"]), 0, "identity で比較できる組は無い");
        assert_eq!(n(&identity["disagreement_count"]), 0, "片側の欠落を不一致にしない");
        assert_eq!(n(&identity["not_comparable_by_reason"]["one_side_no_identity"]), 1);
        assert_eq!(n(&identity["not_comparable_by_reason"]["both_no_identity"]), 1);
        assert!(identity["agreement_rate"].is_null(), "comparable が 0 なら率は出さない");
        // decision は比較できる（2 件）
        assert_eq!((n(&dim(&r, "decision")["comparable_count"]), n(&dim(&r, "decision")["agreement_count"])), (2, 1));
    }

    #[test]
    fn reason_sets_ignore_order_and_duplicates_but_not_content() {
        let conn = open_migrated();
        let same = dev_run(&conn, "REVIEW");
        safe_v(&conn, same, Some((1, "movie")), "REVIEW", Some("[\"TIED_TOP\",\"NO_YEAR_HINT\"]"));
        shadow_v(&conn, same, Some((1, "movie")), "REVIEW", Some("[\"NO_YEAR_HINT\",\"TIED_TOP\",\"TIED_TOP\"]"));
        let different = dev_run(&conn, "REVIEW");
        safe_v(&conn, different, Some((2, "movie")), "REVIEW", Some("[\"TIED_TOP\"]"));
        shadow_v(&conn, different, Some((2, "movie")), "REVIEW", Some("[\"TIED_TOP\",\"NO_YEAR_HINT\"]"));
        let broken = dev_run(&conn, "REVIEW");
        safe_v(&conn, broken, Some((3, "movie")), "REVIEW", None);
        shadow_v(&conn, broken, Some((3, "movie")), "REVIEW", Some("[\"TIED_TOP\"]"));

        let r = dev(&conn);
        let x = dim(&r, "reason_set");
        assert_eq!((n(&x["comparable_count"]), n(&x["agreement_count"]), n(&x["disagreement_count"])), (2, 1, 1));
        assert_eq!(n(&x["not_comparable_by_reason"]["invalid_reason_representation"]), 1);
    }

    #[test]
    fn pairing_is_by_run_id_and_a_duplicate_verdict_row_is_impossible() {
        let conn = open_migrated();
        let run = dev_run(&conn, "AUTO");
        safe_v(&conn, run, Some((1, "movie")), "AUTO", Some("[]"));
        let duplicate = conn.execute(
            "INSERT INTO metadata_match_verdicts (run_id, matcher, matcher_version, decision) VALUES (?1, 'rules-safe', ?2, 'REVIEW')",
            rusqlite::params![run, POLICY_VERSION],
        );
        assert!(duplicate.is_err(), "UNIQUE (run_id, matcher) が組の爆発を防ぐ");
        // 別の run の shadow と、順序・時刻が近くても組まない
        let other = dev_run(&conn, "AUTO");
        shadow_v(&conn, other, Some((1, "movie")), "AUTO", Some("[]"));
        let r = dev(&conn);
        assert_eq!(n(&dim(&r, "decision")["comparable_count"]), 0);
        assert_eq!(n(&dim(&r, "decision")["not_comparable_count"]), 2);
    }

    #[test]
    fn a_verdict_with_an_unsupported_matcher_version_is_not_compared_or_counted() {
        let conn = open_migrated();
        let run = dev_run(&conn, "AUTO");
        safe_v(&conn, run, Some((1, "movie")), "AUTO", Some("[]"));
        verdict(&conn, run, "rules-tags-shadow", "rules-tags-shadow-1", Some((1, "movie")), "AUTO", Some("[]"));
        let r = dev(&conn);
        assert_eq!(n(&r["auto"]["rules-tags-shadow"]["eligible_decision_count"]), 0);
        assert_eq!(n(&r["report_metadata"]["matcher_denominators"]["rules-tags-shadow"]["unsupported_matcher_version_runs"]), 1);
        assert_eq!(n(&dim(&r, "decision")["not_comparable_by_reason"]["unsupported_matcher_version"]), 1);
        assert_eq!(n(&r["auto"]["rules-safe"]["count"]), 1);
    }

    // ─── cohort の選択と除外 ─────────────────────────────────────────────

    fn exclusions(r: &Value) -> BTreeMap<String, i64> {
        r["report_metadata"]["exclusions"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), n(v))).collect()
    }

    fn assert_partition(r: &Value) {
        let m = &r["report_metadata"];
        let total: i64 = exclusions(r).values().sum();
        assert_eq!(n(&m["excluded_rows"]), total);
        assert_eq!(n(&m["eligible_rows"]) + n(&m["excluded_rows"]), n(&m["total_source_rows"]), "どの run も適格か除外のどちらか");
        for reason in EXCLUSION_REASONS {
            assert!(m["exclusions"].get(reason).is_some(), "{reason} は 0 件でも出す");
        }
    }

    #[test]
    fn a_holdout_row_never_reaches_aggregation_and_its_content_is_not_exposed() {
        let conn = open_migrated();
        let ok = dev_run(&conn, "AUTO");
        safe_v(&conn, ok, Some((1, "movie")), "AUTO", Some("[]"));
        shadow_v(&conn, ok, Some((1, "movie")), "AUTO", Some("[]"));

        // holdout の audit run（live + holdout）。verdict に目印を入れておく
        let (hw, hr) = run_row(&conn, "safe", "batch", "live", POLICY_VERSION, "REVIEW");
        task(&conn, hw, Some(hr), &sampling("holdout", "live"), Some("resolved"));
        safe_v(&conn, hr, Some((987654, "movie")), "REVIEW", Some("[\"SENTINEL_HOLDOUT_REASON\"]"));
        shadow_v(&conn, hr, Some((987654, "movie")), "AUTO", Some("[]"));
        conn.execute(
            "INSERT INTO metadata_match_labels (work_id, run_id, label, tmdb_id, media_type, method, strength)
             VALUES (?1, ?2, 'tmdb', 987654, 'movie', 'review_confirm', 'strong')",
            rusqlite::params![hw, hr],
        )
        .unwrap();
        // 同じ作品の別の run（production）も除く
        let (_, sibling) = {
            conn.execute(
                "INSERT INTO metadata_match_runs (work_id, trigger_kind, mode, evidence_class, state_schema_version, input_snapshot_json,
                   search_queries_json, policy_version, policy_snapshot_json, governing_matcher, decision, decision_reasons_json)
                 VALUES (?1, 'single', 'safe', 'reconstructed_clean', 'cm-prematch-2', '{}', '[]', ?2, '{}', 'rules-safe', 'REVIEW', '[]')",
                rusqlite::params![hw, POLICY_VERSION],
            )
            .unwrap();
            (hw, conn.last_insert_rowid())
        };
        safe_v(&conn, sibling, Some((987654, "movie")), "REVIEW", Some("[\"SENTINEL_HOLDOUT_REASON\"]"));
        // live + development という禁止の組み合わせも holdout 側に倒す
        let (lw, lr) = run_row(&conn, "safe", "batch", "live", POLICY_VERSION, "AUTO");
        task(&conn, lw, Some(lr), &sampling("development", "live"), Some("ready"));
        safe_v(&conn, lr, Some((987654, "movie")), "AUTO", Some("[]"));

        for cohort in [DiagnosticCohort::DevelopmentAudit, DiagnosticCohort::ProductionObserved] {
            let r = report(&conn, cohort);
            assert_eq!(n(&r["report_metadata"]["excluded_holdout_count"]), 3, "holdout の run 2 + live+development の run 1");
            assert_eq!(exclusions(&r)["sealed_holdout"], 3);
            let text = r.to_string();
            assert!(!text.contains("SENTINEL_HOLDOUT_REASON") && !text.contains("987654"), "holdout の中身がレポートに出ている");
            assert_partition(&r);
        }
        let r = dev(&conn);
        assert_eq!(n(&r["report_metadata"]["eligible_rows"]), 1);
        assert_eq!(n(&r["review"]["rules-safe"]["eligible_decision_count"]), 1, "holdout の REVIEW は数えない");
        assert_eq!(n(&r["auto"]["rules-safe"]["count"]), 1);
        assert_eq!(n(&dim(&r, "decision")["comparable_count"]), 1);
    }

    #[test]
    fn the_report_does_not_read_labels_at_all() {
        let conn = open_migrated();
        let run = dev_run(&conn, "AUTO");
        safe_v(&conn, run, Some((1, "movie")), "AUTO", Some("[]"));
        conn.execute_batch("PRAGMA foreign_keys = OFF; DROP TABLE metadata_match_labels;").unwrap();
        let r = dev(&conn);
        assert_eq!(n(&r["auto"]["rules-safe"]["count"]), 1, "labels テーブルが無くても動く = 読んでいない");
    }

    #[test]
    fn shadow_mode_runs_are_not_counted_as_source_runs() {
        let conn = open_migrated();
        let safe_run = dev_run(&conn, "REVIEW");
        safe_v(&conn, safe_run, Some((1, "movie")), "REVIEW", Some("[\"TIED_TOP\"]"));
        shadow_v(&conn, safe_run, Some((1, "movie")), "REVIEW", Some("[\"TIED_TOP\"]"));
        // create_jev_shadow_run と同じ形: mode='shadow' の複製（verdict も写している）
        let (_, sibling) = run_row(&conn, "shadow", "batch", "reconstructed_clean", POLICY_VERSION, "REVIEW");
        safe_v(&conn, sibling, Some((1, "movie")), "REVIEW", Some("[\"TIED_TOP\"]"));
        shadow_v(&conn, sibling, Some((1, "movie")), "REVIEW", Some("[\"TIED_TOP\"]"));
        for cohort in [DiagnosticCohort::DevelopmentAudit, DiagnosticCohort::ProductionObserved] {
            let r = report(&conn, cohort);
            assert_eq!(exclusions(&r)["shadow_duplicate_run"], 1);
            assert_partition(&r);
        }
        let r = dev(&conn);
        assert_eq!(n(&r["review"]["rules-safe"]["count"]), 1, "複製を数えると 2 になる");
        assert_eq!(n(&dim(&r, "decision")["comparable_count"]), 1);
    }

    #[test]
    fn audit_and_production_runs_are_separated_by_the_review_task_link() {
        let conn = open_migrated();
        let audit = dev_run(&conn, "AUTO");
        safe_v(&conn, audit, Some((1, "movie")), "AUTO", Some("[]"));
        let (_, production) = run_row(&conn, "safe", "single", "reconstructed_clean", POLICY_VERSION, "REVIEW");
        safe_v(&conn, production, Some((2, "movie")), "REVIEW", Some("[\"TIED_TOP\"]"));

        let a1 = dev(&conn);
        assert_eq!(n(&a1["report_metadata"]["eligible_rows"]), 1);
        assert_eq!(exclusions(&a1)["production_run_other_cohort"], 1);
        assert_eq!(n(&a1["auto"]["rules-safe"]["count"]), 1);
        assert_partition(&a1);

        let a2 = report(&conn, DiagnosticCohort::ProductionObserved);
        assert_eq!(n(&a2["report_metadata"]["eligible_rows"]), 1);
        assert_eq!(exclusions(&a2)["audit_run_other_cohort"], 1);
        assert_eq!(n(&a2["review"]["rules-safe"]["count"]), 1);
        assert_eq!(a2["report_metadata"]["cohort_type"], "A2_production_observed");
        assert_partition(&a2);
    }

    #[test]
    fn production_live_rows_are_excluded_until_the_holdout_overlap_is_defined() {
        let conn = open_migrated();
        let (_, live) = run_row(&conn, "safe", "single", "live", POLICY_VERSION, "AUTO");
        safe_v(&conn, live, Some((1, "movie")), "AUTO", Some("[]"));
        let (_, clean) = run_row(&conn, "safe", "single", "reconstructed_clean", POLICY_VERSION, "AUTO");
        safe_v(&conn, clean, Some((2, "movie")), "AUTO", Some("[]"));
        let r = report(&conn, DiagnosticCohort::ProductionObserved);
        assert_eq!(exclusions(&r)["live_evidence_policy_ambiguous"], 1);
        assert_eq!(n(&r["report_metadata"]["eligible_rows"]), 1);
        assert_partition(&r);
    }

    #[test]
    fn ambiguous_historical_and_unsupported_versions_are_bucketed_not_merged() {
        let conn = open_migrated();
        let ok = dev_run(&conn, "AUTO");
        safe_v(&conn, ok, Some((1, "movie")), "AUTO", Some("[]"));
        for (policy, expected) in [("rules-safe-1", "historical_version_ambiguous"), ("rules-safe-9", "unsupported_profile_version")] {
            let (w, run) = run_row(&conn, "safe", "batch", "reconstructed_clean", policy, "AUTO");
            task(&conn, w, Some(run), &sampling("development", "reconstructed_clean"), Some("ready"));
            verdict(&conn, run, "rules-safe", policy, Some((5, "movie")), "AUTO", Some("[]"));
            let r = dev(&conn);
            assert_eq!(exclusions(&r)[expected], 1, "{policy}");
            assert_eq!(n(&r["auto"]["rules-safe"]["count"]), 1, "古い版を現行として数えない");
            assert_partition(&r);
            conn.execute("DELETE FROM metadata_review_tasks WHERE run_id = ?1", [run]).unwrap();
            conn.execute("DELETE FROM metadata_match_runs WHERE id = ?1", [run]).unwrap();
        }
    }

    #[test]
    fn missing_or_unreadable_cohort_metadata_is_never_treated_as_development() {
        let conn = open_migrated();
        let ok = dev_run(&conn, "AUTO");
        safe_v(&conn, ok, Some((1, "movie")), "AUTO", Some("[]"));
        for bad in ["{}", "not json", "{\"purpose\":\"development\"}", "{\"purpose\":\"other\",\"cohort\":\"reconstructed_clean\"}", "[1,2]"] {
            let (w, run) = run_row(&conn, "safe", "batch", "reconstructed_clean", POLICY_VERSION, "AUTO");
            task(&conn, w, Some(run), bad, Some("ready"));
            safe_v(&conn, run, Some((9, "movie")), "AUTO", Some("[]"));
        }
        // run に繋がらない task（run_id が NULL）でも、その作品の run は除く
        let (w, run) = run_row(&conn, "safe", "single", "reconstructed_clean", POLICY_VERSION, "AUTO");
        task(&conn, w, None, "{}", None);
        safe_v(&conn, run, Some((9, "movie")), "AUTO", Some("[]"));
        let r = dev(&conn);
        assert_eq!(exclusions(&r)["unsupported_sampling_metadata"], 6);
        assert_eq!(n(&r["auto"]["rules-safe"]["count"]), 1);
        assert_partition(&r);
    }

    #[test]
    fn audit_task_states_decide_whether_the_run_is_usable() {
        let conn = open_migrated();
        let mut ids = Vec::new();
        for state in [Some("ready"), Some("deferred"), Some("resolved"), Some("stale"), Some("generation_error"), Some("weird"), None] {
            let (w, run) = run_row(&conn, "safe", "batch", "reconstructed_clean", POLICY_VERSION, "AUTO");
            task(&conn, w, Some(run), &sampling("development", "historical_audit_only"), state);
            safe_v(&conn, run, Some((1, "movie")), "AUTO", Some("[]"));
            ids.push(run);
        }
        let r = dev(&conn);
        assert_eq!(n(&r["report_metadata"]["eligible_rows"]), 3, "ready / deferred / resolved");
        assert_eq!(exclusions(&r)["audit_task_not_valid"], 2);
        assert_eq!(exclusions(&r)["unsupported_audit_task_state"], 2);
        assert_partition(&r);
    }

    #[test]
    fn replay_error_skipped_and_other_modes_are_excluded_with_their_own_reasons() {
        let conn = open_migrated();
        let ok = dev_run(&conn, "AUTO");
        safe_v(&conn, ok, Some((1, "movie")), "AUTO", Some("[]"));
        for (mode, trigger, decision) in [("safe", "replay", "REVIEW"), ("safe", "batch", "ERROR"), ("safe", "batch", "SKIPPED"), ("gated", "batch", "REVIEW")] {
            let (w, run) = run_row(&conn, mode, trigger, "reconstructed_clean", POLICY_VERSION, decision);
            task(&conn, w, Some(run), &sampling("development", "reconstructed_clean"), Some("ready"));
            safe_v(&conn, run, Some((1, "movie")), "REVIEW", Some("[\"TIED_TOP\"]"));
        }
        let r = dev(&conn);
        let e = exclusions(&r);
        assert_eq!((e["replay_run"], e["run_error"], e["run_skipped"], e["unsupported_run_mode"]), (1, 1, 1, 1));
        assert_eq!(n(&r["review"]["rules-safe"]["count"]), 0, "除外した run の REVIEW を数えない");
        assert_partition(&r);
    }

    // ─── レポートの安全性 ────────────────────────────────────────────────

    #[test]
    fn final_promotion_is_never_run_and_gt_is_blocked_without_a_bridge() {
        let conn = open_migrated();
        let run = dev_run(&conn, "AUTO");
        safe_v(&conn, run, Some((1, "movie")), "AUTO", Some("[]"));
        let r = dev(&conn);
        assert_eq!(r["final_promotion"], json!({"status": "NOT_RUN", "reason": "HOLDOUT_SEALED_POLICY_NOT_FROZEN"}));
        // bridge が渡されなければ、診断 GT は BLOCKED（precision は出さない）。PR4-1 の NOT_IMPLEMENTED_PR4_1 は置き換わった
        assert_eq!(r["diagnostic_gt"]["status"], "BLOCKED");
        assert_eq!(r["diagnostic_gt"]["blockers"], json!(["bridge_missing"]));
        assert_eq!(r["diagnostic_gt"]["profiles"], json!({}));
        assert_eq!(r["diagnostic_gt"]["precision_computed"], false);
        assert!(!r.to_string().contains("diagnostic_conditional_auto_precision"), "BLOCKED のとき precision の値を出さない");
        assert!(!r.to_string().to_lowercase().contains("recall"));
        assert_eq!(r["report_metadata"]["report_version"], REPORT_VERSION);
        assert_eq!(r["report_metadata"]["rules_1_version_boundary"]["status"], "historical_version_boundary_unresolved");
        // 昇格に見える値が development のデータから埋まらない
        let o = r.as_object().unwrap();
        for key in ["report_metadata", "reason_codes", "review", "auto", "abstention", "profile_comparison", "diagnostic_gt", "final_promotion"] {
            assert!(o.contains_key(key), "{key}");
        }
    }

    fn fingerprint(conn: &Connection) -> String {
        let mut parts = Vec::new();
        for table in ["works", "metadata_match_runs", "metadata_match_candidates", "metadata_match_verdicts", "metadata_review_tasks",
                      "metadata_match_labels", "metadata_match_rejections", "metadata_match_candidate_scores"] {
            let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).unwrap();
            let sum: String = conn
                .query_row(&format!("SELECT COALESCE(group_concat(rowid || ':' || length(CAST({table}.rowid AS TEXT)), ','), '') FROM {table}"), [], |row| row.get(0))
                .unwrap();
            parts.push(format!("{table}={count}/{sum}"));
        }
        let verdicts: String = conn
            .query_row("SELECT COALESCE(group_concat(run_id || matcher || matcher_version || decision || COALESCE(reasons_json,''), '|'), '') FROM metadata_match_verdicts", [], |r| r.get(0))
            .unwrap();
        let runs: String = conn
            .query_row("SELECT COALESCE(group_concat(id || mode || decision || policy_version, '|'), '') FROM metadata_match_runs", [], |r| r.get(0))
            .unwrap();
        parts.push(verdicts);
        parts.push(runs);
        parts.join("#")
    }

    #[test]
    fn every_report_states_that_the_cohort_boundary_is_not_enforced_by_the_database() {
        let conn = open_migrated();
        for cohort in [DiagnosticCohort::DevelopmentAudit, DiagnosticCohort::ProductionObserved] {
            let r = report(&conn, cohort);
            assert_eq!(r["report_metadata"]["cohort_boundary_integrity"], "APPLICATION_VALIDATED_NOT_DB_ENFORCED");
            assert!(r["report_metadata"]["cohort_boundary_note"].as_str().unwrap().contains("DB のトリガーや制約"));
        }
    }

    #[test]
    fn the_aggregation_writes_nothing_and_restores_the_connection_state() {
        let conn = open_migrated();
        let run = dev_run(&conn, "AUTO");
        safe_v(&conn, run, Some((1, "movie")), "AUTO", Some("[]"));
        shadow_v(&conn, run, Some((1, "movie")), "AUTO", Some("[]"));
        let before = fingerprint(&conn);
        let query_only_before: i64 = conn.query_row("PRAGMA query_only", [], |r| r.get(0)).unwrap();
        let first = dev(&conn);
        let second = dev(&conn);
        assert_eq!(first, second, "同じ入力から同じレポート（決定的）");
        assert_eq!(fingerprint(&conn), before, "集計が DB を変えた");
        let query_only_after: i64 = conn.query_row("PRAGMA query_only", [], |r| r.get(0)).unwrap();
        assert_eq!(query_only_before, query_only_after);
        // 集計の後は通常どおり書ける
        conn.execute("UPDATE metadata_match_runs SET latency_ms = 1", []).unwrap();
    }

    #[test]
    fn query_only_blocks_writes_while_the_guard_is_alive() {
        let conn = open_migrated();
        let (_, run) = run_row(&conn, "safe", "batch", "reconstructed_clean", POLICY_VERSION, "AUTO");
        {
            let _guard = QueryOnlyGuard::new(&conn).unwrap();
            let write = conn.execute("UPDATE metadata_match_runs SET latency_ms = 5 WHERE id = ?1", [run]);
            assert!(write.is_err(), "query_only の間は書けない");
            let write = conn.execute("DELETE FROM metadata_match_runs WHERE id = ?1", [run]);
            assert!(write.is_err());
        }
        conn.execute("UPDATE metadata_match_runs SET latency_ms = 5 WHERE id = ?1", [run]).unwrap();
    }

    #[test]
    fn a_database_without_the_required_schema_is_reported_not_guessed() {
        let conn = Connection::open_in_memory().unwrap();
        let error = generate_report(&conn, DiagnosticCohort::DevelopmentAudit, NOW, HEAD, None).unwrap_err();
        assert!(matches!(error, DiagnosticsError::UnsupportedSchema(_)));
        // 列が足りない場合
        conn.execute_batch(
            "CREATE TABLE metadata_match_runs (id INTEGER PRIMARY KEY);
             CREATE TABLE metadata_match_verdicts (run_id INTEGER);
             CREATE TABLE metadata_review_tasks (id INTEGER PRIMARY KEY);",
        )
        .unwrap();
        assert!(matches!(
            generate_report(&conn, DiagnosticCohort::DevelopmentAudit, NOW, HEAD, None).unwrap_err(),
            DiagnosticsError::UnsupportedSchema(_)
        ));
    }

    #[test]
    fn many_runs_are_read_in_chunks_without_losing_rows() {
        let conn = open_migrated();
        let total = CHUNK * 2 + 37;
        for i in 0..total {
            let run = dev_run(&conn, "AUTO");
            safe_v(&conn, run, Some((i as i64 + 1, "movie")), "AUTO", Some("[]"));
        }
        let r = dev(&conn);
        assert_eq!(n(&r["auto"]["rules-safe"]["count"]), total as i64);
        assert_eq!(n(&r["report_metadata"]["eligible_rows"]), total as i64);
    }

    #[test]
    fn the_module_has_no_write_statements() {
        let source = include_str!("pr4_diagnostics.rs");
        let code = source.split("// ─── テスト").next().unwrap();
        for forbidden in ["INSERT ", "UPDATE ", "DELETE ", "DROP ", "ALTER ", "CREATE TABLE", "REPLACE INTO"] {
            assert!(!code.contains(forbidden), "{forbidden} がある");
        }
    }

    // ═══ PR4-3: 診断用 GT と diagnostic_conditional_auto_precision ═══════════════

    use std::sync::atomic::{AtomicUsize, Ordering};

    const S1: &str = "c5c5c-redo-01";

    fn gt_sampling(sample: &str) -> String {
        format!("{{\"purpose\":\"development\",\"cohort\":\"reconstructed_clean\",\"sample_id\":\"{sample}\",\"protocol_version\":\"gt-protocol-1\"}}")
    }

    #[derive(Clone, Copy)]
    struct GtUnit {
        task: i64,
        work: i64,
        run: i64,
    }

    /// formal な sample の開発用 task（run つき）。`resolution` = (label, tmdb_id, media_type, method) があれば、GT レビューと同じ形で解決済みにする
    fn gt_task(conn: &Connection, sample: &str, run_decision: &str, resolution: Option<(&str, Option<i64>, Option<&str>, &str)>) -> GtUnit {
        let (work, run) = run_row(conn, "safe", "batch", "reconstructed_clean", POLICY_VERSION, run_decision);
        conn.execute(
            "INSERT INTO metadata_review_tasks (work_id, run_id, reason, sampling_json, details_json)
             VALUES (?1, ?2, 'audit_sample', ?3, '{\"review_protocol_version\":\"gt-review-1\",\"state\":\"ready\"}')",
            rusqlite::params![work, run, gt_sampling(sample)],
        )
        .unwrap();
        let task = conn.last_insert_rowid();
        if let Some((label, tmdb, media, method)) = resolution {
            conn.execute(
                "INSERT INTO metadata_match_labels (work_id, run_id, review_task_id, label, tmdb_id, media_type, method, strength)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'strong')",
                rusqlite::params![work, run, task, label, tmdb, media, method],
            )
            .unwrap();
            let label_id = conn.last_insert_rowid();
            let verified = if method == "review_pick_other" { ",\"tmdb_verified\":true" } else { "" };
            conn.execute(
                "UPDATE metadata_review_tasks SET resolved_at = '2026-10-05T00:00:00Z', resolution_label_id = ?2, details_json = ?3 WHERE id = ?1",
                rusqlite::params![task, label_id, format!("{{\"review_protocol_version\":\"gt-review-1\",\"state\":\"resolved\",\"resolution_method\":\"{method}\"{verified}}}")],
            )
            .unwrap();
        }
        GtUnit { task, work, run }
    }

    fn positive(conn: &Connection, id: i64) -> GtUnit {
        gt_task(conn, S1, "AUTO", Some(("tmdb", Some(id), Some("movie"), "review_confirm")))
    }

    fn deferred_unit(conn: &Connection) -> GtUnit {
        let u = gt_task(conn, S1, "AUTO", None);
        conn.execute("UPDATE metadata_review_tasks SET details_json = '{\"review_protocol_version\":\"gt-review-1\",\"state\":\"deferred\"}' WHERE id = ?1", [u.task]).unwrap();
        u
    }

    static BRIDGE_SEQ: AtomicUsize = AtomicUsize::new(0);

    /// (sample, task, work, run, media_type, tmdb_id)
    type BR<'a> = (&'a str, i64, i64, i64, &'a str, i64);

    fn bridge_value(records: &[BR], resolved: u64, deferred: u64) -> Value {
        let recs: Vec<Value> = records
            .iter()
            .map(|(sample, task, work, run, mt, id)| {
                json!({"evaluation_unit": {"sample_id": sample, "task_id": task, "work_id": work}, "reviewed_run_id": run, "semantic": "POSITIVE",
                       "media_type": mt, "tmdb_id": id, "source_stage": "D Phase 2", "source_artifact": "x", "source_artifact_sha256": "a".repeat(64),
                       "source_protocol_version": "p", "provenance_class": "FROZEN_OFF_DB_GT"})
            })
            .collect();
        json!({
            "bridge_version": BRIDGE_VERSION,
            "formal_study": {"study": "C5c.5", "resolved": resolved, "deferred": deferred, "closeout_sha256": "b".repeat(64), "f_closeout_sha256": "c".repeat(64)},
            "sources": [{"name": "n", "path": "p", "sha256": "d".repeat(64)}],
            "integrity": {"off_db_records": records.len(), "expected_final_resolved": resolved, "expected_final_deferred": deferred, "trusted_db_expected": 0,
                          "stage_counts": {}, "holdout_scope": "x", "production_db_modified": false, "holdout_read": false},
            "records": recs,
        })
    }

    fn write_bridge(value: &Value) -> (PathBuf, String) {
        let path = std::env::temp_dir().join(format!("pr4_bridge_{}_{}.json", std::process::id(), BRIDGE_SEQ.fetch_add(1, Ordering::SeqCst)));
        let text = serde_json::to_string(value).unwrap();
        std::fs::write(&path, &text).unwrap();
        (path, hex_sha256(text.as_bytes()))
    }

    fn cfg_for(records: &[BR], population: usize, resolved: usize) -> GtConfig {
        let deferred = population - resolved;
        let (path, sha) = write_bridge(&bridge_value(records, resolved as u64, deferred as u64));
        GtConfig { bridge_path: path, expected_bridge_sha256: sha, expected_population: population, expected_resolved: resolved, expected_deferred: deferred }
    }

    fn gt_report(conn: &Connection, cfg: &GtConfig) -> Value {
        generate_report(conn, DiagnosticCohort::DevelopmentAudit, NOW, HEAD, Some(cfg)).unwrap()
    }

    fn blockers(r: &Value) -> Vec<String> {
        r["diagnostic_gt"]["blockers"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
    }

    fn profile<'a>(r: &'a Value, matcher: &str) -> &'a Value {
        &r["diagnostic_gt"]["profiles"][matcher]
    }

    #[test]
    fn auto_correctness_is_exact_identity_and_none_makes_any_auto_incorrect() {
        let conn = open_migrated();
        let u1 = positive(&conn, 424241);
        safe_v(&conn, u1.run, Some((424241, "movie")), "AUTO", Some("[]"));
        shadow_v(&conn, u1.run, Some((424241, "movie")), "AUTO", Some("[]"));
        let u2 = positive(&conn, 424242);
        safe_v(&conn, u2.run, Some((424243, "movie")), "AUTO", Some("[]"));          // 別の identity → 誤り
        shadow_v(&conn, u2.run, Some((424242, "movie")), "AUTO", Some("[]"));
        let u3 = gt_task(&conn, S1, "AUTO", Some(("none", None, None, "review_none")));
        safe_v(&conn, u3.run, Some((424244, "movie")), "AUTO", Some("[]"));          // none に AUTO → 誤り
        shadow_v(&conn, u3.run, Some((424244, "movie")), "REVIEW", Some("[\"TIED_TOP\"]"));
        let r = gt_report(&conn, &cfg_for(&[], 3, 3));
        let gt = &r["diagnostic_gt"];
        assert_eq!(gt["status"], "PASS", "{}", gt["blockers"]);
        assert_eq!(gt["precision_computed"], true);
        let safe = profile(&r, "rules-safe");
        assert_eq!((n(&safe["auto_evaluable_count"]), n(&safe["auto_correct_count"]), n(&safe["auto_incorrect_count"])), (3, 1, 2));
        assert!((safe["diagnostic_conditional_auto_precision"].as_f64().unwrap() - 1.0 / 3.0).abs() < 1e-12);
        // matcher は別々に計算する（shadow の分母は REVIEW を除いた 2 件で、どちらも正しい）
        let shadow = profile(&r, "rules-tags-shadow");
        assert_eq!((n(&shadow["auto_evaluable_count"]), n(&shadow["auto_correct_count"])), (2, 2));
        assert!((shadow["diagnostic_conditional_auto_precision"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        assert_eq!(n(&shadow["not_in_denominator"]["machine_not_auto"]), 1);
        assert_eq!(gt["correctness_definition"], "exact_media_type_tmdb_id");
        assert_eq!(gt["top3"], "NOT_APPLICABLE");
        assert_eq!(n(&gt["gt_universe"]["db_only"]), 3);
        let text = r.to_string();
        assert!(!text.contains("42424"), "GT の identity をレポートに出さない（件数だけ）");
    }

    #[test]
    fn review_and_unresolved_machine_states_and_missing_gt_stay_out_of_the_denominator() {
        let conn = open_migrated();
        let review = positive(&conn, 1001);
        safe_v(&conn, review.run, Some((1001, "movie")), "REVIEW", Some("[\"TIED_TOP\"]"));
        let unresolved = positive(&conn, 1002);
        safe_v(&conn, unresolved.run, None, "UNRESOLVED", Some("[\"NO_CANDIDATES\"]"));
        let deferred = deferred_unit(&conn);            // GT なし（defer）。AUTO でも分母に入らない
        safe_v(&conn, deferred.run, Some((1003, "movie")), "AUTO", Some("[]"));
        let r = gt_report(&conn, &cfg_for(&[], 3, 2));
        let gt = &r["diagnostic_gt"];
        assert_eq!(gt["status"], "PASS", "{}", gt["blockers"]);
        let safe = profile(&r, "rules-safe");
        assert_eq!(n(&safe["auto_evaluable_count"]), 0);
        assert!(safe["diagnostic_conditional_auto_precision"].is_null(), "分母 0 は null");
        assert_eq!(n(&safe["not_in_denominator"]["machine_not_auto"]), 2);
        assert_eq!(n(&gt["unresolved_gt_count"]["deferred"]), 1, "defer は未解決として数える");
        // PR4-1 の coverage / REVIEW / abstention には残る
        assert_eq!(n(&r["review"]["rules-safe"]["count"]), 1);
        assert_eq!(n(&r["auto"]["rules-safe"]["count"]), 1);
        assert_eq!(n(&r["abstention"]["rules-safe"]["count"]), 2);
    }

    #[test]
    fn db_only_bridge_only_and_identical_gt_are_merged_without_priority() {
        let conn = open_migrated();
        let db_only = positive(&conn, 2001);
        safe_v(&conn, db_only.run, Some((2001, "movie")), "AUTO", Some("[]"));
        let same = positive(&conn, 2002);
        safe_v(&conn, same.run, Some((2002, "movie")), "AUTO", Some("[]"));
        // bridge だけが解決している単位（DB では deferred の task）
        let bridge_only = deferred_unit(&conn);
        safe_v(&conn, bridge_only.run, Some((2003, "movie")), "AUTO", Some("[]"));
        let records: Vec<BR> = vec![
            (S1, same.task, same.work, same.run, "movie", 2002),
            (S1, bridge_only.task, bridge_only.work, bridge_only.run, "movie", 2003),
        ];
        let r = gt_report(&conn, &cfg_for(&records, 3, 3));
        let gt = &r["diagnostic_gt"];
        assert_eq!(gt["status"], "PASS", "{}", gt["blockers"]);
        let u = &gt["gt_universe"];
        assert_eq!((n(&u["db_only"]), n(&u["bridge_only"]), n(&u["merged_same_gt"])), (1, 1, 1));
        assert_eq!(n(&u["db_unresolved_covered_by_bridge"]), 1, "bridge が解決した単位は未解決として数えない");
        assert!(gt["unresolved_gt_count"].as_object().unwrap().is_empty());
        let safe = profile(&r, "rules-safe");
        assert_eq!((n(&safe["auto_evaluable_count"]), n(&safe["auto_correct_count"])), (3, 3));
    }

    #[test]
    fn a_bridge_only_unit_without_a_db_task_is_in_the_universe_but_not_in_the_denominator() {
        let conn = open_migrated();
        let db = positive(&conn, 3001);
        safe_v(&conn, db.run, Some((3001, "movie")), "AUTO", Some("[]"));
        let records: Vec<BR> = vec![(S1, 9_000_001, 9_000_002, 9_000_003, "movie", 3002)];
        let r = gt_report(&conn, &cfg_for(&records, 2, 2));
        assert_eq!(r["diagnostic_gt"]["status"], "PASS", "{}", r["diagnostic_gt"]["blockers"]);
        let safe = profile(&r, "rules-safe");
        assert_eq!(n(&safe["auto_evaluable_count"]), 1);
        assert_eq!(n(&safe["not_in_denominator"]["machine_run_unavailable"]), 1);
    }

    #[test]
    fn a_db_and_bridge_conflict_blocks_the_metric_and_never_computes_precision() {
        let conn = open_migrated();
        let u = positive(&conn, 4001);
        safe_v(&conn, u.run, Some((4001, "movie")), "AUTO", Some("[]"));
        let records: Vec<BR> = vec![(S1, u.task, u.work, u.run, "movie", 4002)];
        let r = gt_report(&conn, &cfg_for(&records, 1, 1));
        let gt = &r["diagnostic_gt"];
        assert_eq!(gt["status"], "BLOCKED");
        assert_eq!(blockers(&r), vec!["gt_source_conflict"]);
        assert_eq!(gt["profiles"], json!({}));
        assert_eq!(gt["precision_computed"], false);
        assert!(!r.to_string().contains("diagnostic_conditional_auto_precision"));
        // canonical GT が違えば、reviewed_run_id が同じでも違っても衝突
        let records: Vec<BR> = vec![(S1, u.task, u.work, u.run + 999, "movie", 4002)];
        assert_eq!(blockers(&gt_report(&conn, &cfg_for(&records, 1, 1))), vec!["gt_source_conflict"]);
        // PR4-1 の通常のセクションは BLOCKED でも出る
        assert_eq!(n(&r["report_metadata"]["eligible_rows"]), 1);
        assert_eq!(n(&r["auto"]["rules-safe"]["count"]), 1);
    }

    #[test]
    fn the_same_key_and_gt_with_a_different_reviewed_run_id_is_merged_and_both_refs_are_kept() {
        let conn = open_migrated();
        let u = positive(&conn, 4101);
        safe_v(&conn, u.run, Some((4101, "movie")), "AUTO", Some("[]"));
        // reviewed_run_id は属性。評価単位のキー・canonical GT が同じなら、値が違っても結合する
        let records: Vec<BR> = vec![(S1, u.task, u.work, u.run + 999, "movie", 4101)];
        let r = gt_report(&conn, &cfg_for(&records, 1, 1));
        let gt = &r["diagnostic_gt"];
        assert_eq!(gt["status"], "PASS", "{}", gt["blockers"]);
        let universe = &gt["gt_universe"];
        assert_eq!((n(&universe["merged_same_gt"]), n(&universe["db_only"]), n(&universe["bridge_only"]), n(&universe["source_conflicts"])), (1, 0, 0, 0));
        assert_eq!(n(&universe["reviewed_run_id_differs"]), 1, "run の違いは数える（衝突にはしない）");
        assert_eq!(n(&universe["provenance_refs"]), 2, "両方の出典を残す");
        // 機械側の出力は DB の task が指す run のものを使う（評価できる）
        let safe = profile(&r, "rules-safe");
        assert_eq!((n(&safe["auto_evaluable_count"]), n(&safe["auto_correct_count"])), (1, 1));
        // 内部の provenance: 両方の供給元と、それぞれの reviewed_run_id の値が残っている
        let selection = select_runs(&conn, DiagnosticCohort::DevelopmentAudit).unwrap();
        let db = db_gt(&conn, &selection).unwrap();
        let bridge = vec![BridgeRec { key: (S1.to_string(), u.task, u.work), run_id: u.run + 999, gt: Gt::Positive("movie".to_string(), 4101) }];
        let merged = merge_gt(&db, &bridge, &selection);
        assert!(merged.blockers.is_empty() && merged.conflicts == 0);
        let unit = &merged.units[&(S1.to_string(), u.task, u.work)];
        assert_eq!(unit.provenance, vec![ProvenanceRef { source: "TRUSTED_DB_GT", reviewed_run_id: u.run }, ProvenanceRef { source: "FROZEN_OFF_DB_GT", reviewed_run_id: u.run + 999 }]);
        assert_eq!(unit.db_run, Some(u.run));
        // run が同じなら、差は 0 件
        let same_run: Vec<BR> = vec![(S1, u.task, u.work, u.run, "movie", 4101)];
        let r2 = gt_report(&conn, &cfg_for(&same_run, 1, 1));
        assert_eq!(r2["diagnostic_gt"]["status"], "PASS");
        assert_eq!(n(&r2["diagnostic_gt"]["gt_universe"]["reviewed_run_id_differs"]), 0);
        // bridge だけが解決している単位（DB では deferred）でも、run の違いは衝突にならない
        let d = deferred_unit(&conn);
        safe_v(&conn, d.run, Some((4102, "movie")), "AUTO", Some("[]"));
        let records: Vec<BR> = vec![(S1, u.task, u.work, u.run, "movie", 4101), (S1, d.task, d.work, d.run + 999, "movie", 4102)];
        let r3 = gt_report(&conn, &cfg_for(&records, 2, 2));
        assert_eq!(r3["diagnostic_gt"]["status"], "PASS", "{}", r3["diagnostic_gt"]["blockers"]);
        assert_eq!(n(&r3["diagnostic_gt"]["gt_universe"]["bridge_only"]), 1);
        assert_eq!(n(&r3["diagnostic_gt"]["gt_universe"]["reviewed_run_id_differs"]), 1);
    }

    #[test]
    fn the_same_key_with_a_different_canonical_gt_is_a_conflict_whatever_the_reviewed_run_id() {
        let conn = open_migrated();
        let u = positive(&conn, 4201);
        safe_v(&conn, u.run, Some((4201, "movie")), "AUTO", Some("[]"));
        for run in [u.run, u.run + 999] {
            let records: Vec<BR> = vec![(S1, u.task, u.work, run, "movie", 4202)];
            let r = gt_report(&conn, &cfg_for(&records, 1, 1));
            assert_eq!(r["diagnostic_gt"]["status"], "BLOCKED", "run {run}");
            assert_eq!(blockers(&r), vec!["gt_source_conflict"], "run {run}");
            assert_eq!(r["diagnostic_gt"]["profiles"], json!({}), "衝突のとき precision を出さない");
            assert_eq!(n(&r["diagnostic_gt"]["gt_universe"]["source_conflicts"]), 1);
        }
        // media_type だけが違う（identity の一部）場合も衝突
        let records: Vec<BR> = vec![(S1, u.task, u.work, u.run, "tv", 4201)];
        assert_eq!(blockers(&gt_report(&conn, &cfg_for(&records, 1, 1))), vec!["gt_source_conflict"]);
    }

    #[test]
    fn the_bridge_is_verified_before_use() {
        let conn = open_migrated();
        let u = positive(&conn, 5001);
        safe_v(&conn, u.run, Some((5001, "movie")), "AUTO", Some("[]"));
        let good = cfg_for(&[], 1, 1);
        assert_eq!(gt_report(&conn, &good)["diagnostic_gt"]["status"], "PASS");
        // SHA の不一致（ファイルは正しくても anchor が違う）
        let mut bad_sha = good.clone();
        bad_sha.expected_bridge_sha256 = "0".repeat(64);
        assert_eq!(blockers(&gt_report(&conn, &bad_sha)), vec!["bridge_sha_mismatch"]);
        // ファイルが無い
        let mut missing = good.clone();
        missing.bridge_path = std::env::temp_dir().join("pr4_bridge_does_not_exist.json");
        assert_eq!(blockers(&gt_report(&conn, &missing)), vec!["bridge_missing"]);
        // 渡されない
        assert_eq!(blockers(&generate_report(&conn, DiagnosticCohort::DevelopmentAudit, NOW, HEAD, None).unwrap()), vec!["bridge_missing"]);
        // SHA が合っていても、スキーマが不正（title などの余分なキー / 余分な top-level / 欠落）は使わない
        let mut extra_title = bridge_value(&[(S1, 1, 2, 3, "movie", 4)], 1, 0);
        extra_title["records"][0]["title"] = json!("x");
        let mut extra_top = bridge_value(&[], 1, 0);
        extra_top["note"] = json!("x");
        let mut no_unit_part = bridge_value(&[(S1, 1, 2, 3, "movie", 4)], 1, 0);
        no_unit_part["records"][0]["evaluation_unit"].as_object_mut().unwrap().remove("work_id");
        let mut bad_semantic = bridge_value(&[(S1, 1, 2, 3, "movie", 4)], 1, 0);
        bad_semantic["records"][0]["semantic"] = json!("DEFER");
        for (name, v) in [("extra_title", extra_title), ("extra_top", extra_top), ("no_unit_part", no_unit_part), ("bad_semantic", bad_semantic)] {
            let (path, sha) = write_bridge(&v);
            let c = GtConfig { bridge_path: path, expected_bridge_sha256: sha, expected_population: 1, expected_resolved: 1, expected_deferred: 0 };
            assert_eq!(blockers(&gt_report(&conn, &c)), vec!["bridge_schema_invalid"], "{name}");
        }
        // 準備ができていない bridge（件数が違う）/ formal な sample 群の外 / 重複した単位
        let (path, sha) = write_bridge(&bridge_value(&[], 45, 5));
        let c = GtConfig { bridge_path: path, expected_bridge_sha256: sha, expected_population: 1, expected_resolved: 1, expected_deferred: 0 };
        assert_eq!(blockers(&gt_report(&conn, &c)), vec!["bridge_not_ready"]);
        let (path, sha) = write_bridge(&bridge_value(&[("c5c5-dev-expand-01", 1, 2, 3, "movie", 4)], 1, 0));
        let c = GtConfig { bridge_path: path, expected_bridge_sha256: sha, expected_population: 1, expected_resolved: 1, expected_deferred: 0 };
        assert_eq!(blockers(&gt_report(&conn, &c)), vec!["cohort_boundary_failure"]);
        let (path, sha) = write_bridge(&bridge_value(&[(S1, 1, 2, 3, "movie", 4), (S1, 1, 2, 3, "movie", 5)], 2, 0));
        let c = GtConfig { bridge_path: path, expected_bridge_sha256: sha, expected_population: 2, expected_resolved: 2, expected_deferred: 0 };
        assert_eq!(blockers(&gt_report(&conn, &c)), vec!["duplicate_bridge_unit"]);
    }

    #[test]
    fn a_gt_universe_that_does_not_match_the_closeout_blocks_the_metric() {
        let conn = open_migrated();
        for i in 0..4 {
            let u = positive(&conn, 6000 + i);
            safe_v(&conn, u.run, Some((6000 + i, "movie")), "AUTO", Some("[]"));
        }
        // 4 件の GT に対し、期待が 5 件（足りない = 43 に相当）/ 3 件（多い = 45 に相当）/ 母集団が違う
        for (population, resolved) in [(5usize, 5usize), (3, 3), (6, 4), (4, 4)] {
            let r = gt_report(&conn, &cfg_for(&[], population, resolved));
            if (population, resolved) == (4, 4) {
                assert_eq!(r["diagnostic_gt"]["status"], "PASS");
            } else {
                assert_eq!(r["diagnostic_gt"]["status"], "BLOCKED", "{population}/{resolved}");
                assert_eq!(blockers(&r), vec!["gt_universe_mismatch"], "{population}/{resolved}");
                assert_eq!(r["diagnostic_gt"]["profiles"], json!({}), "分母を黙って減らして計算可能にしない");
            }
        }
        // 44 は GT の単位数の整合性の検査で、precision の分母ではない（AUTO の件数とは別）
        let r = gt_report(&conn, &cfg_for(&[], 4, 4));
        assert_eq!(n(&r["diagnostic_gt"]["gt_universe"]["resolved"]), 4);
        assert!(r["diagnostic_gt"]["denominator_definition"].as_str().unwrap().contains("分母ではない"));
    }

    #[test]
    fn an_auto_verdict_without_a_selected_identity_blocks_the_metric() {
        let conn = open_migrated();
        let u = positive(&conn, 7001);
        safe_v(&conn, u.run, None, "AUTO", Some("[]"));             // AUTO なのに identity が無い（想定外の記録）
        let r = gt_report(&conn, &cfg_for(&[], 1, 1));
        assert_eq!(r["diagnostic_gt"]["status"], "BLOCKED");
        assert_eq!(blockers(&r), vec!["invalid_auto_output"]);
        assert_eq!(r["diagnostic_gt"]["profiles"], json!({}));
    }

    #[test]
    fn evaluation_key_mismatches_and_cohort_boundary_failures_block() {
        let conn = open_migrated();
        let u = positive(&conn, 8001);
        safe_v(&conn, u.run, Some((8001, "movie")), "AUTO", Some("[]"));
        // task_id は DB と同じだが work_id が違う / sample_id が違う
        let wrong_work: Vec<BR> = vec![(S1, u.task, u.work + 1, u.run, "movie", 8001)];
        assert_eq!(blockers(&gt_report(&conn, &cfg_for(&wrong_work, 1, 1))), vec!["evaluation_key_mismatch"]);
        let wrong_sample: Vec<BR> = vec![("c5c5c-redo-02", u.task, u.work, u.run, "movie", 8001)];
        assert_eq!(blockers(&gt_report(&conn, &cfg_for(&wrong_sample, 1, 1))), vec!["evaluation_key_mismatch"]);
        // bridge の単位が holdout の task を指す
        let (hw, hr) = run_row(&conn, "safe", "batch", "live", POLICY_VERSION, "AUTO");
        task(&conn, hw, Some(hr), &sampling("holdout", "live"), Some("resolved"));
        let hold_task: i64 = conn.query_row("SELECT id FROM metadata_review_tasks WHERE work_id = ?1", [hw], |row| row.get(0)).unwrap();
        let sealed: Vec<BR> = vec![(S1, hold_task, hw, hr, "movie", 8002)];
        assert_eq!(blockers(&gt_report(&conn, &cfg_for(&sealed, 2, 2))), vec!["cohort_boundary_failure"]);
    }

    #[test]
    fn a_holdout_fixture_never_reaches_gt_materialization() {
        let conn = open_migrated();
        let a = positive(&conn, 9001);
        safe_v(&conn, a.run, Some((9001, "movie")), "AUTO", Some("[]"));
        let b = positive(&conn, 9002);
        safe_v(&conn, b.run, Some((9002, "movie")), "AUTO", Some("[]"));
        // holdout の audit run と、その作品のラベル（確定済み）。中身は目印つき
        let (hw, hr) = run_row(&conn, "safe", "batch", "live", POLICY_VERSION, "AUTO");
        task(&conn, hw, Some(hr), &sampling("holdout", "live"), Some("resolved"));
        safe_v(&conn, hr, Some((987654, "movie")), "AUTO", Some("[\"SENTINEL_HOLDOUT_REASON\"]"));
        conn.execute(
            "INSERT INTO metadata_match_labels (work_id, run_id, label, tmdb_id, media_type, method, strength) VALUES (?1, ?2, 'tmdb', 987654, 'movie', 'review_confirm', 'strong')",
            rusqlite::params![hw, hr],
        )
        .unwrap();
        // 抽出記録が読めない task の作品も、GT の組み立てから除く
        let (uw, ur) = run_row(&conn, "safe", "batch", "reconstructed_clean", POLICY_VERSION, "AUTO");
        task(&conn, uw, Some(ur), "{}", Some("resolved"));
        conn.execute(
            "INSERT INTO metadata_match_labels (work_id, run_id, label, tmdb_id, media_type, method, strength) VALUES (?1, ?2, 'tmdb', 987655, 'movie', 'review_confirm', 'strong')",
            rusqlite::params![uw, ur],
        )
        .unwrap();
        let r = gt_report(&conn, &cfg_for(&[], 2, 2));
        let gt = &r["diagnostic_gt"];
        assert_eq!(gt["status"], "PASS", "{}", gt["blockers"]);
        assert_eq!(n(&gt["materialization"]["label_rows_read"]), 2, "読んだラベルは開発用の作品の 2 件だけ（holdout / 未対応の作品のラベルは読まない）");
        assert_eq!(n(&gt["materialization"]["tasks_considered"]), 2);
        assert_eq!(n(&r["report_metadata"]["excluded_holdout_count"]), 1, "holdout は件数だけ");
        let text = r.to_string();
        assert!(!text.contains("987654") && !text.contains("987655") && !text.contains("SENTINEL_HOLDOUT_REASON"));
        assert_eq!(n(&profile(&r, "rules-safe")["auto_evaluable_count"]), 2);
    }

    #[test]
    fn untrusted_weak_superseded_and_unlisted_labels_are_excluded_and_counted() {
        let conn = open_migrated();
        // 1) 信頼できるラベルが後で上書きされた
        let sup = positive(&conn, 100);
        conn.execute("UPDATE metadata_match_labels SET superseded_at = '2026-10-05T01:00:00Z' WHERE work_id = ?1", [sup.work]).unwrap();
        // 2) allowlist にない sample（provisional）の review ラベル
        let prov = gt_task(&conn, "c5c5-dev-expand-01", "AUTO", Some(("tmdb", Some(101), Some("movie"), "review_confirm")));
        let _ = prov;
        // 3) production の manual ラベル（作品に付いているが audit task に紐づかない）
        let man = deferred_unit(&conn);
        conn.execute("INSERT INTO metadata_match_labels (work_id, run_id, label, tmdb_id, media_type, method, strength) VALUES (?1, ?2, 'tmdb', 102, 'movie', 'manual_apply', 'strong')", rusqlite::params![man.work, man.run]).unwrap();
        // 4) weak な lock
        let lock = deferred_unit(&conn);
        conn.execute("INSERT INTO metadata_match_labels (work_id, run_id, label, tmdb_id, media_type, method, strength) VALUES (?1, ?2, 'tmdb', 103, 'movie', 'lock', 'weak')", rusqlite::params![lock.work, lock.run]).unwrap();
        // 5) 解決の記録の protocol 版が合わない review ラベル（provenance が不完全）
        let broken = positive(&conn, 104);
        conn.execute("UPDATE metadata_review_tasks SET details_json = replace(details_json, 'gt-review-1', 'gt-review-0') WHERE id = ?1", [broken.task]).unwrap();
        let r = gt_report(&conn, &cfg_for(&[], 5, 5));
        let ex = &r["diagnostic_gt"]["excluded_gt"];
        assert_eq!(n(&ex["gt_superseded"]), 1);
        assert_eq!(n(&ex["gt_sample_not_allowlisted"]), 1);
        assert_eq!(n(&ex["untrusted_manual_gt_source"]), 1);
        assert_eq!(n(&ex["weak_label_not_gt"]), 1);
        assert_eq!(n(&ex["gt_provenance_incomplete"]), 1);
        for key in ["untrusted_manual_gt_source", "gt_superseded", "gt_conflict", "gt_multiple_units_per_work", "unknown_label_semantics"] {
            assert!(ex.get(key).is_some(), "{key} は 0 件でも出す");
        }
        // 信頼できる単位が足りないので、整合性の検査で BLOCKED（黙って受け入れない）
        assert_eq!(r["diagnostic_gt"]["status"], "BLOCKED");
        assert!(blockers(&r).contains(&"gt_universe_mismatch".to_string()));
    }

    #[test]
    fn a_label_with_unsupported_semantics_on_an_audit_task_blocks() {
        let conn = open_migrated();
        let u = positive(&conn, 200);
        safe_v(&conn, u.run, Some((200, "movie")), "AUTO", Some("[]"));
        // CHECK を一時的に外して、未対応の method（意味が確かめられない値）を作る
        conn.execute_batch("PRAGMA ignore_check_constraints = ON").unwrap();
        conn.execute("UPDATE metadata_match_labels SET method = 'review_future_method' WHERE work_id = ?1", [u.work]).unwrap();
        conn.execute_batch("PRAGMA ignore_check_constraints = OFF").unwrap();
        let r = gt_report(&conn, &cfg_for(&[], 1, 1));
        assert_eq!(r["diagnostic_gt"]["status"], "BLOCKED");
        assert!(blockers(&r).contains(&"unsupported_label_semantics".to_string()));
        assert_eq!(n(&r["diagnostic_gt"]["excluded_gt"]["unknown_label_semantics"]), 1);
    }

    #[test]
    fn multiple_units_for_one_work_are_all_excluded() {
        let conn = open_migrated();
        let a = positive(&conn, 300);
        safe_v(&conn, a.run, Some((300, "movie")), "AUTO", Some("[]"));
        // bridge が、同じ作品の別の task を指す
        let records: Vec<BR> = vec![(S1, 9_100_001, a.work, 9_100_002, "movie", 301)];
        let r = gt_report(&conn, &cfg_for(&records, 2, 2));
        assert_eq!(n(&r["diagnostic_gt"]["excluded_gt"]["gt_multiple_units_per_work"]), 2);
        assert_eq!(r["diagnostic_gt"]["status"], "BLOCKED");
        assert!(blockers(&r).contains(&"gt_universe_mismatch".to_string()));
    }

    #[test]
    fn a_unit_whose_run_is_not_eligible_is_counted_not_silently_dropped() {
        let conn = open_migrated();
        let ok = positive(&conn, 400);
        safe_v(&conn, ok.run, Some((400, "movie")), "AUTO", Some("[]"));
        let old = positive(&conn, 401);
        conn.execute("UPDATE metadata_match_runs SET policy_version = 'rules-safe-1' WHERE id = ?1", [old.run]).unwrap();
        verdict(&conn, old.run, "rules-safe", "rules-safe-1", Some((401, "movie")), "AUTO", Some("[]"));
        let r = gt_report(&conn, &cfg_for(&[], 2, 2));
        assert_eq!(r["diagnostic_gt"]["status"], "PASS", "{}", r["diagnostic_gt"]["blockers"]);
        let safe = profile(&r, "rules-safe");
        assert_eq!(n(&safe["auto_evaluable_count"]), 1);
        assert_eq!(n(&safe["not_in_denominator"]["run_not_eligible"]), 1);
        assert_eq!(n(&safe["run_not_eligible_by_reason"]["historical_version_ambiguous"]), 1);
    }

    #[test]
    fn the_ordinary_pr4_1_sections_are_unchanged_by_the_gt_section() {
        let conn = open_migrated();
        for i in 0..3 {
            let u = positive(&conn, 500 + i);
            safe_v(&conn, u.run, Some((500 + i, "movie")), if i == 0 { "AUTO" } else { "REVIEW" }, Some(if i == 0 { "[]" } else { "[\"TIED_TOP\"]" }));
            shadow_v(&conn, u.run, Some((500 + i, "movie")), "AUTO", Some("[]"));
        }
        let plain = generate_report(&conn, DiagnosticCohort::DevelopmentAudit, NOW, HEAD, None).unwrap();
        let with_gt = gt_report(&conn, &cfg_for(&[], 3, 3));
        for key in ["report_metadata", "reason_codes", "review", "auto", "abstention", "profile_comparison", "final_promotion"] {
            assert_eq!(plain[key], with_gt[key], "{key} は GT の有無で変わらない");
        }
        assert_ne!(plain["diagnostic_gt"], with_gt["diagnostic_gt"]);
        assert_eq!(with_gt["final_promotion"], json!({"status": "NOT_RUN", "reason": "HOLDOUT_SEALED_POLICY_NOT_FROZEN"}), "昇格には使えない");
        assert_eq!(with_gt["report_metadata"]["cohort_boundary_integrity"], "APPLICATION_VALIDATED_NOT_DB_ENFORCED");
        assert_eq!(with_gt["diagnostic_gt"]["cohort_boundary_integrity"], "APPLICATION_VALIDATED_NOT_DB_ENFORCED");
    }

    #[test]
    fn the_gt_metric_is_only_defined_for_the_development_audit_cohort() {
        let conn = open_migrated();
        let u = positive(&conn, 600);
        safe_v(&conn, u.run, Some((600, "movie")), "AUTO", Some("[]"));
        let r = generate_report(&conn, DiagnosticCohort::ProductionObserved, NOW, HEAD, Some(&cfg_for(&[], 1, 1))).unwrap();
        assert_eq!(r["diagnostic_gt"]["status"], "NOT_APPLICABLE");
        assert!(!r.to_string().contains("diagnostic_conditional_auto_precision"));
    }

    #[test]
    fn rank_top3_score_and_candidates_are_never_consulted_and_the_bridge_path_is_not_hard_coded() {
        let source = include_str!("pr4_diagnostics.rs");
        let code = source.split("// ─── テスト").next().unwrap();
        for forbidden in ["rules_rank", "metadata_match_candidates", "candidate_scores", "jev_calls", "search_rank"] {
            assert!(!code.contains(forbidden), "{forbidden} を参照している");
        }
        assert!(!code.contains("CineMantis-audit") && !code.contains("C:\\\\Users") && !code.contains("pr4\\\\gt_bridge"), "bridge の絶対パスを固定しない");
        let production = GtConfig::production("x");
        assert_eq!(production.expected_bridge_sha256, "0f31ea9678cb7e078b82970fca3d9f3beb1338640ee8b9cfe1fe7e623cb8559c");
        assert_eq!((production.expected_population, production.expected_resolved, production.expected_deferred), (50, 44, 6));
    }

    #[test]
    fn the_real_frozen_bridge_is_accepted_when_its_path_is_provided() {
        // 凍結した外部 bridge の実ファイルを、パスが環境変数で渡されたときだけ検証する（未設定なら何もしない）。本番 DB は使わない
        let Ok(path) = std::env::var("CM_PR4_BRIDGE_PATH") else { return };
        let cfg = GtConfig::production(path);
        let (records, sha) = load_bridge(&cfg).expect("凍結した bridge は検証を通る");
        assert_eq!(sha, PINNED_BRIDGE_SHA256);
        assert_eq!(records.len(), 4);
        assert!(records.iter().all(|r| GT_SAMPLE_ALLOWLIST.contains(&r.key.0.as_str()) && matches!(r.gt, Gt::Positive(..))));
    }
}
