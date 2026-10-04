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

use rusqlite::Connection;
use serde_json::{json, Map, Value};

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
    work_id: i64,
    run_id: Option<i64>,
    class: TaskClass,
    state: Option<String>,
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
            "SELECT work_id, run_id, sampling_json, details_json
               FROM metadata_review_tasks WHERE reason = 'audit_sample'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            let (work_id, run_id, sampling, details) = row?;
            let class = match sampling.as_deref() {
                Some(text) => classify_task(text),
                None => TaskClass::Unsupported,
            };
            tasks.push(TaskRow { work_id, run_id, class, state: task_state(details.as_deref()) });
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
    for run in &runs {
        match classify_run(run, cohort, &holdout_runs, &holdout_works, &unsupported_runs, &unsupported_works, &dev_by_run) {
            Some(reason) => *exclusions.get_mut(reason).expect("known reason") += 1,
            None => eligible.push(run.id),
        }
    }
    Ok(Selection { eligible, exclusions, total_source_rows: runs.len() as i64 })
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

// ─── レポート ────────────────────────────────────────────────────────────────

/// 通常の診断レポートを作る。**読み取り専用。** `generated_at` と `head` は呼び出し側が渡す（この関数は時刻も git も見ない）。
pub fn generate_report(
    conn: &Connection,
    cohort: DiagnosticCohort,
    generated_at: &str,
    head: &str,
) -> Result<Value, DiagnosticsError> {
    require_schema(conn)?;
    let _guard = QueryOnlyGuard::new(conn)?;

    let selection = select_runs(conn, cohort)?;
    let excluded_total: i64 = selection.exclusions.values().sum();
    debug_assert_eq!(selection.eligible.len() as i64 + excluded_total, selection.total_source_rows);

    // verdict は適格な run の分だけ読む（除外された run の verdict・候補・ラベルは読まない）
    let (per_run, rules_one_rows) = read_verdicts(conn, &selection.eligible)?;

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
        "diagnostic_gt": {"status": "NOT_IMPLEMENTED_PR4_1"},
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
        generate_report(conn, cohort, NOW, HEAD).unwrap()
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
    fn final_promotion_is_never_run_and_there_is_no_gt_or_precision_metric() {
        let conn = open_migrated();
        let run = dev_run(&conn, "AUTO");
        safe_v(&conn, run, Some((1, "movie")), "AUTO", Some("[]"));
        let r = dev(&conn);
        assert_eq!(r["final_promotion"], json!({"status": "NOT_RUN", "reason": "HOLDOUT_SEALED_POLICY_NOT_FROZEN"}));
        assert_eq!(r["diagnostic_gt"], json!({"status": "NOT_IMPLEMENTED_PR4_1"}));
        let text = r.to_string().to_lowercase();
        assert!(!text.contains("precision"), "precision の指標を作らない");
        assert!(!text.contains("recall"));
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
        let error = generate_report(&conn, DiagnosticCohort::DevelopmentAudit, NOW, HEAD).unwrap_err();
        assert!(matches!(error, DiagnosticsError::UnsupportedSchema(_)));
        // 列が足りない場合
        conn.execute_batch(
            "CREATE TABLE metadata_match_runs (id INTEGER PRIMARY KEY);
             CREATE TABLE metadata_match_verdicts (run_id INTEGER);
             CREATE TABLE metadata_review_tasks (id INTEGER PRIMARY KEY);",
        )
        .unwrap();
        assert!(matches!(
            generate_report(&conn, DiagnosticCohort::DevelopmentAudit, NOW, HEAD).unwrap_err(),
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
}
