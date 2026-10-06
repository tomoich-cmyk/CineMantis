//! PR4-PF0B: FINAL パイプラインの予行演習（**synthetic のみ・test ビルドだけ・製品には出ない**）。
//!
//! 目的は「この閾値で promotion できるか」を見ることではない。**最終評価のコードが、どんな policy を与えても、正しい分母・判定・失敗の扱いで動く**ことを、
//! holdout に触れずに確かめる（holdout が初めて FINAL のコード経路を通る、という技術的なリスクを消す）。実データでの性能は、holdout まで未知のままでよい。
//!
//! # 構造
//!
//! 入力（synthetic の評価単位 + 凍結 membership の commitment）→ membership の検査 → 機械出力の検査 → FINAL レポート（件数・率・信頼区間の下限）→ promotion 規則の判定。
//! 判定の結果は `Promote` / `NoPromotion` / `Inconclusive` / `TechnicalFailure`。
//!
//! # policy は外から注入する（既定値を持たない）
//!
//! [`PolicyConfig`] は `Default` を持たず、全項目を明示しなければ作れない。**このファイルの試験で使う数値は、すべて `TEST_ONLY_HYPOTHETICAL_POLICY`**
//! （出典 [`PolicyProvenance::TestOnlyHypothetical`]）で、レポートに「拘束力なし・FINAL policy の候補値として引用しない」と明記する。
//! 評価単位ごとの値（人間の GT の identity など）はレポートに出さない（件数と率だけ）。
//!
//! # 決めてあること（PR4-0 / PR4-2 の凍結契約と、監査の推奨）
//!
//! - FINAL precision = 正しい AUTO / すべての AUTO（候補集合の外に正解があるのに別の候補を AUTO した場合も誤り 1 件）。完全一致（`(media_type, tmdb_id)`）のみ。
//!   none の GT に AUTO が候補を選べば誤り。GT が未解決の単位の AUTO（機械 = AUTO・GT = defer / unresolved）の扱いは policy の明示項目（`UnevaluableAuto`）。
//!   **binding な FINAL policy は `BlockEvaluation` だけ**（評価全体を INCONCLUSIVE にする）。`Exclude`（分母から外す）/ `CountAsWrong`（誤りとして数える）は synthetic の試験用。**既定は無い。**
//! - precision だけで promotion しない。`n_AUTO`・AUTO coverage・abstention rate・誤 AUTO の件数・信頼区間の下限を必ず併記する。
//!   **判定条件は coverage、abstention rate は必須併記**（`coverage + abstention = 1` なので二重の閾値にしない）。
//! - 信頼区間は片側の exact Clopper–Pearson 下限。AUTO の件数が最低に届かなければ INCONCLUSIVE（閾値を緩めない）。coverage 未達は NO PROMOTION。
//! - 結果を見てから標本を足すことは禁止（コードでは強制できないので、PF0C の規則と policy freeze に入れる）。
//!
//! # 判定の優先順位（固定）
//!
//! 技術的な失敗（membership の不一致・機械出力の欠落・無効な AUTO の identity・実行の失敗）→ `TechnicalFailure`（件数も precision も出さない）/
//! AUTO の件数が最低に届かない → `Inconclusive`（他の条件も記録する）/ それ以外の条件（coverage・誤 AUTO の上限・abstention の上限・precision の下限）の未達 → `NoPromotion` /
//! すべて満たす → `Promote`。

use std::collections::BTreeSet;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const REPORT_VERSION: &str = "pr4-final-report-rehearsal-1";
pub const TEST_ONLY_LABEL: &str = "TEST_ONLY_HYPOTHETICAL_POLICY";
const MEMBERSHIP_DOMAIN: &str = "pr4-pf0a-membership-1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyProvenance {
    /// 試験だけに使う架空の policy。FINAL policy の候補値として引用しない
    TestOnlyHypothetical,
    /// policy freeze で凍結された policy（その SHA-256）。この予行演習では作らない
    Frozen { policy_sha256: String },
}

/// GT が未解決の単位の AUTO をどう扱うか。**明示しなければ policy を作れない。**
/// `Exclude` / `CountAsWrong` は synthetic の試験用（binding な FINAL policy では不可。`PolicyConfig::validate` が拒否する）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnevaluableAuto {
    /// 分母から外す（件数は必ず報告する）
    Exclude,
    /// 誤りとして数える（分母にも入れる）
    CountAsWrong,
    /// **binding な FINAL policy で使う唯一の値。** AUTO した単位の GT が確定できない時点で、promotion の根拠として不十分とし、評価全体を INCONCLUSIVE にする
    /// （Exclude は precision を上方に選べてしまい、CountAsWrong は「正誤不明」を誤答と定義してしまう）。表示用の暫定値は件数と併記する。
    BlockEvaluation,
}

/// promotion の policy。**`Default` を持たない。**
#[derive(Debug, Clone)]
///
/// **abstention rate は判定条件にしない（必須併記の指標）。** 判定の空間では `AUTO coverage + abstention rate = 1` なので、`minimum_auto_coverage` と
/// abstention の上限を別々に持つと、同じ条件を二重に持つことになる。判定条件は coverage、abstention は必ずレポートに併記する。
pub struct PolicyConfig {
    pub provenance: PolicyProvenance,
    pub minimum_auto_count: u64,
    pub minimum_auto_coverage: f64,
    /// 片側の信頼水準（例: 0.95）
    pub confidence_level: f64,
    pub minimum_precision_lower_bound: f64,
    pub maximum_wrong_auto: Option<u64>,
    pub unevaluable_auto: UnevaluableAuto,
}

impl PolicyConfig {
    /// 試験専用。数値は FINAL policy の候補値ではない
    #[allow(clippy::too_many_arguments)]
    pub fn test_only(
        minimum_auto_count: u64,
        minimum_auto_coverage: f64,
        confidence_level: f64,
        minimum_precision_lower_bound: f64,
        maximum_wrong_auto: Option<u64>,
        unevaluable_auto: UnevaluableAuto,
    ) -> Self {
        PolicyConfig {
            provenance: PolicyProvenance::TestOnlyHypothetical,
            minimum_auto_count,
            minimum_auto_coverage,
            confidence_level,
            minimum_precision_lower_bound,
            maximum_wrong_auto,
            unevaluable_auto,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if !(self.confidence_level > 0.0 && self.confidence_level < 1.0) {
            return Err("confidence_level が (0, 1) の外です".into());
        }
        for (name, v) in [("minimum_auto_coverage", self.minimum_auto_coverage), ("minimum_precision_lower_bound", self.minimum_precision_lower_bound)] {
            if !(0.0..=1.0).contains(&v) || v.is_nan() {
                return Err(format!("{name} が [0, 1] の外です"));
            }
        }
        if matches!(self.provenance, PolicyProvenance::Frozen { .. }) && self.unevaluable_auto != UnevaluableAuto::BlockEvaluation {
            return Err("binding な FINAL policy は unevaluable_auto = BlockEvaluation だけです（Exclude / CountAsWrong は試験用）".into());
        }
        if self.minimum_auto_count == 0 {
            return Err("minimum_auto_count が 0 です（AUTO が 0 件でも通る policy は作れない）".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gt {
    Positive(String, i64),
    /// 対応する TMDB 作品が存在しない（信頼できる none）
    NoMatch,
    /// GT が解決していない（defer など）
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Machine {
    /// AUTO。選んだ identity（無ければ無効な出力）
    Auto(Option<(String, i64)>),
    Review,
    Unresolved,
    /// 機械出力が無い（技術的な失敗）
    Missing,
}

#[derive(Debug, Clone)]
pub struct Unit {
    /// 不透明なキー（membership の commitment に使う。レポートには出さない）
    pub key: String,
    pub machine: Machine,
    pub gt: Gt,
}

#[derive(Debug, Clone)]
pub struct FinalInput {
    pub units: Vec<Unit>,
    /// 凍結された membership の commitment（`pr4-pf0a-membership-1` と同じ作り方）
    pub expected_membership_commitment: String,
    pub eligibility_rule_version: String,
    /// 実行の技術的な失敗（あれば TechnicalFailure）
    pub run_failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Promote,
    NoPromotion(Vec<String>),
    Inconclusive(Vec<String>),
    TechnicalFailure(Vec<String>),
}

pub fn membership_commitment(keys: &[String], version: &str) -> String {
    let mut sorted: Vec<&String> = keys.iter().collect();
    sorted.sort();
    let mut pre = format!("{MEMBERSHIP_DOMAIN}\n{version}\n");
    for k in sorted {
        pre.push_str(k);
        pre.push('\n');
    }
    format!("{:x}", Sha256::digest(pre.as_bytes()))
}

/// 片側の exact Clopper–Pearson 下限（`successes` / `n`、信頼水準 `confidence`）。
/// `P(X >= k | n, p) = 1 - confidence` を満たす p を二分法で求める。k = 0 のとき 0。
pub fn clopper_pearson_lower(successes: u64, n: u64, confidence: f64) -> Option<f64> {
    if n == 0 || successes > n || !(confidence > 0.0 && confidence < 1.0) {
        return None;
    }
    if successes == 0 {
        return Some(0.0);
    }
    let alpha = 1.0 - confidence;
    if successes == n {
        return Some(alpha.powf(1.0 / n as f64));
    }
    let ln_fact: Vec<f64> = std::iter::once(0.0).chain((1..=n).scan(0.0, |acc, i| { *acc += (i as f64).ln(); Some(*acc) })).collect();
    let tail = |p: f64| -> f64 {
        let (lp, lq) = (p.ln(), (1.0 - p).ln());
        (successes..=n).map(|i| (ln_fact[n as usize] - ln_fact[i as usize] - ln_fact[(n - i) as usize] + i as f64 * lp + (n - i) as f64 * lq).exp()).sum()
    };
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..200 {
        let mid = (lo + hi) / 2.0;
        if tail(mid) < alpha { lo = mid } else { hi = mid }
    }
    Some((lo + hi) / 2.0)
}

fn rate(num: u64, den: u64) -> Value {
    if den == 0 { Value::Null } else { json!(num as f64 / den as f64) }
}

/// FINAL のレポートと promotion の判定。**件数・率・信頼区間の下限だけ**（評価単位ごとの値は出さない）。
pub fn evaluate(input: &FinalInput, policy: &PolicyConfig) -> (Value, Decision) {
    let provenance = match &policy.provenance {
        PolicyProvenance::TestOnlyHypothetical => json!({"class": TEST_ONLY_LABEL, "binding": false,
            "note": "拘束力なし。この数値を FINAL policy の候補値として引用しない"}),
        PolicyProvenance::Frozen { policy_sha256 } => json!({"class": "FROZEN", "binding": true, "policy_sha256": policy_sha256}),
    };
    let head = |decision: &Decision, extra: Value| -> Value {
        let (label, reasons): (&str, &Vec<String>) = match decision {
            Decision::Promote => ("PROMOTE", &Vec::new()),
            Decision::NoPromotion(r) => ("NO_PROMOTION", r),
            Decision::Inconclusive(r) => ("INCONCLUSIVE", r),
            Decision::TechnicalFailure(r) => ("TECHNICAL_FAILURE", r),
        };
        let mut v = json!({"report_version": REPORT_VERSION, "policy_provenance": provenance, "decision": label, "decision_reasons": reasons});
        if let (Value::Object(base), Value::Object(more)) = (&mut v, extra) {
            base.extend(more);
        }
        v
    };

    // ── 技術的な失敗（件数も precision も出さない）──
    let mut technical: Vec<String> = Vec::new();
    if let Err(e) = policy.validate() {
        technical.push(format!("invalid_policy: {e}"));
    }
    if let Some(f) = &input.run_failure {
        technical.push(format!("run_failure: {f}"));
    }
    let keys: Vec<String> = input.units.iter().map(|u| u.key.clone()).collect();
    if keys.iter().collect::<BTreeSet<_>>().len() != keys.len() {
        technical.push("duplicate_unit_key".into());
    }
    if membership_commitment(&keys, &input.eligibility_rule_version) != input.expected_membership_commitment {
        technical.push("membership_mismatch".into());
    }
    if input.units.iter().any(|u| u.machine == Machine::Missing) {
        technical.push("machine_output_missing".into());
    }
    if input.units.iter().any(|u| matches!(u.machine, Machine::Auto(None))) {
        technical.push("invalid_auto_output".into());
    }
    if input.units.is_empty() {
        technical.push("empty_membership".into());
    }
    if !technical.is_empty() {
        let d = Decision::TechnicalFailure(technical);
        return (head(&d, json!({"metrics": null, "note": "技術的な失敗のとき、件数・precision を出さない（分母を黙って減らして計算可能にしない）"})), d);
    }

    // ── 件数 ──
    let n = input.units.len() as u64;
    let (mut auto, mut review, mut unresolved) = (0u64, 0u64, 0u64);
    let (mut evaluable_auto, mut correct, mut wrong_positive, mut wrong_none, mut unevaluable_auto) = (0u64, 0u64, 0u64, 0u64, 0u64);
    for u in &input.units {
        match &u.machine {
            Machine::Review => review += 1,
            Machine::Unresolved => unresolved += 1,
            Machine::Auto(Some(id)) => {
                auto += 1;
                match &u.gt {
                    Gt::Positive(mt, tm) if (mt, tm) == (&id.0, &id.1) => {
                        evaluable_auto += 1;
                        correct += 1;
                    }
                    Gt::Positive(..) => {
                        evaluable_auto += 1;
                        wrong_positive += 1;
                    }
                    Gt::NoMatch => {
                        evaluable_auto += 1;
                        wrong_none += 1;
                    }
                    Gt::Unresolved => unevaluable_auto += 1,
                }
            }
            Machine::Auto(None) | Machine::Missing => unreachable!("技術的な失敗として先に止めている"),
        }
    }
    let wrong = wrong_positive + wrong_none;
    // GT が未解決の AUTO の扱いは policy の明示項目
    let (denominator, wrong_total) = match policy.unevaluable_auto {
        // BlockEvaluation の表示用の暫定値は Exclude と同じ（binding な precision ではない。下で INCONCLUSIVE にする）
        UnevaluableAuto::Exclude | UnevaluableAuto::BlockEvaluation => (evaluable_auto, wrong),
        UnevaluableAuto::CountAsWrong => (evaluable_auto + unevaluable_auto, wrong + unevaluable_auto),
    };
    let precision = rate(correct, denominator);
    let lower = clopper_pearson_lower(correct, denominator, policy.confidence_level);
    let coverage = auto as f64 / n as f64;
    let abstention = (review + unresolved) as f64 / n as f64;

    // ── 条件（すべて評価して記録する）──
    let mut checks: Vec<(&str, bool)> = Vec::new();
    let enough_auto = denominator >= policy.minimum_auto_count;
    checks.push(("minimum_auto_count", enough_auto));
    checks.push(("minimum_auto_coverage", coverage >= policy.minimum_auto_coverage));
    if let Some(max) = policy.maximum_wrong_auto {
        checks.push(("maximum_wrong_auto", wrong_total <= max));
    }
    checks.push(("minimum_precision_lower_bound", lower.is_some_and(|l| denominator > 0 && l >= policy.minimum_precision_lower_bound)));
    // AUTO した単位の GT が確定できない → promotion の根拠として不十分（評価全体を INCONCLUSIVE にする）
    let blocked_by_gt = policy.unevaluable_auto == UnevaluableAuto::BlockEvaluation && unevaluable_auto > 0;
    if blocked_by_gt {
        checks.push(("auto_with_unresolved_gt", false));
    }
    let failed: Vec<String> = checks.iter().filter(|(_, ok)| !ok).map(|(name, _)| name.to_string()).collect();
    let decision = if failed.is_empty() {
        Decision::Promote
    } else if !enough_auto || blocked_by_gt {
        // AUTO の件数が足りない / AUTO の GT が確定できないとき、precision は判断できない。閾値は緩めず、結論は INCONCLUSIVE
        Decision::Inconclusive(failed)
    } else {
        Decision::NoPromotion(failed)
    };
    let metrics = json!({
        "n_units": n, "n_auto": auto, "n_review": review, "n_unresolved": unresolved,
        "n_auto_evaluable": evaluable_auto, "n_auto_gt_unresolved": unevaluable_auto, "unevaluable_auto_treatment": format!("{:?}", policy.unevaluable_auto),
        "precision_denominator": denominator, "n_correct_auto": correct, "n_wrong_auto": wrong_total,
        "n_wrong_auto_vs_positive_gt": wrong_positive, "n_wrong_auto_vs_none_gt": wrong_none,
        "precision_binding": !blocked_by_gt,
        "auto_precision_point": precision, "auto_precision_lower_bound": lower.filter(|_| denominator > 0),
        "confidence_level": policy.confidence_level, "bound_method": "one-sided exact Clopper-Pearson",
        "auto_coverage": coverage, "abstention_rate": abstention, "coverage_plus_abstention": coverage + abstention,
        "conditions": checks.iter().map(|(name, ok)| json!({"condition": name, "met": ok})).collect::<Vec<_>>(),
        "jointly_reported": ["n_auto", "auto_coverage", "abstention_rate", "n_wrong_auto", "auto_precision_lower_bound"],
    });
    (head(&decision, json!({"metrics": metrics})), decision)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VER: &str = "test-eligibility-version";

    fn pos(id: i64) -> Gt {
        Gt::Positive("movie".into(), id)
    }
    fn auto(id: i64) -> Machine {
        Machine::Auto(Some(("movie".into(), id)))
    }
    fn unit(i: usize, machine: Machine, gt: Gt) -> Unit {
        Unit { key: format!("work:{i}"), machine, gt }
    }
    fn input(units: Vec<Unit>) -> FinalInput {
        let keys: Vec<String> = units.iter().map(|u| u.key.clone()).collect();
        FinalInput { expected_membership_commitment: membership_commitment(&keys, VER), units, eligibility_rule_version: VER.into(), run_failure: None }
    }
    /// 試験専用の架空 policy（FINAL policy の候補値ではない）
    fn policy(min_auto: u64, min_cov: f64, lb: f64) -> PolicyConfig {
        PolicyConfig::test_only(min_auto, min_cov, 0.95, lb, None, UnevaluableAuto::Exclude)
    }
    /// n_auto 件が AUTO で全部正解、残りは REVIEW
    fn all_correct(n_auto: usize, total: usize) -> FinalInput {
        input((0..total).map(|i| if i < n_auto { unit(i, auto(i as i64), pos(i as i64)) } else { unit(i, Machine::Review, pos(i as i64)) }).collect())
    }
    fn decide(i: &FinalInput, p: &PolicyConfig) -> (Value, Decision) {
        evaluate(i, p)
    }

    // ── exact Clopper–Pearson ──
    #[test]
    fn the_clopper_pearson_bound_is_exact() {
        let lb = |k, n| clopper_pearson_lower(k, n, 0.95).unwrap();
        assert!((lb(25, 25) - 0.05f64.powf(1.0 / 25.0)).abs() < 1e-12 && (lb(25, 25) - 0.887).abs() < 0.001, "n=25 全正解で約 88.7%");
        assert!(lb(58, 58) < 0.95 && lb(59, 59) >= 0.95, "全正解で下限 95% 以上に要る件数は 59");
        assert!(lb(298, 298) < 0.99 && lb(299, 299) >= 0.99, "全正解で下限 99% 以上に要る件数は 299");
        assert!((lb(9, 10) - 0.6057).abs() < 0.001, "{}", lb(9, 10));
        assert_eq!(lb(0, 10), 0.0);
        assert!(clopper_pearson_lower(0, 0, 0.95).is_none() && clopper_pearson_lower(3, 2, 0.95).is_none() && clopper_pearson_lower(1, 2, 1.0).is_none());
        // 定義どおり: 下限 p で P(X >= k) = 1 - confidence
        for (k, n) in [(9u64, 10u64), (40, 50), (95, 100), (1, 30)] {
            let p = lb(k, n);
            let mut tail = 0.0;
            for i in k..=n {
                let c = (1..=n).map(|x| (x as f64).ln()).sum::<f64>() - (1..=i).map(|x| (x as f64).ln()).sum::<f64>() - (1..=(n - i)).map(|x| (x as f64).ln()).sum::<f64>();
                tail += (c + i as f64 * p.ln() + (n - i) as f64 * (1.0 - p).ln()).exp();
            }
            assert!((tail - 0.05).abs() < 1e-9, "{k}/{n}: {tail}");
        }
        // 単調性: 正解が増えれば下限は上がり、n が増えれば（全正解で）上がる
        assert!(lb(9, 10) < lb(10, 10) && lb(40, 50) < lb(45, 50) && lb(10, 10) < lb(30, 30));
    }

    // ── 判定（9 つの必須シナリオ）──
    #[test]
    fn precision_condition_pass_and_fail() {
        let pass = decide(&all_correct(60, 100), &policy(50, 0.5, 0.95));
        assert_eq!(pass.1, Decision::Promote);
        assert!(pass.0["metrics"]["auto_precision_lower_bound"].as_f64().unwrap() >= 0.95);
        // 同じ件数で誤りが増えると、点推定が高くても下限が条件を割る → NO PROMOTION
        let mut inp = all_correct(60, 100);
        for i in 0..4 {
            inp.units[i].gt = pos(9000 + i as i64);
        }
        let fail = decide(&inp, &policy(50, 0.5, 0.95));
        assert!(matches!(&fail.1, Decision::NoPromotion(r) if r == &vec!["minimum_precision_lower_bound".to_string()]), "{:?}", fail.1);
        assert!((fail.0["metrics"]["auto_precision_point"].as_f64().unwrap() - 56.0 / 60.0).abs() < 1e-12);
    }

    #[test]
    fn only_the_auto_count_is_short_gives_inconclusive_and_does_not_loosen_anything() {
        let (rep, d) = decide(&all_correct(30, 100), &policy(50, 0.2, 0.5));
        assert!(matches!(&d, Decision::Inconclusive(r) if r == &vec!["minimum_auto_count".to_string()]), "{d:?}");
        assert_eq!(rep["decision"], "INCONCLUSIVE");
        assert_eq!(rep["metrics"]["n_auto"], 30, "件数は併記する");
        // 閾値を緩めても結論は同じ（AUTO 件数の条件は緩められていない）。より厳しい下限と同時に未達でも INCONCLUSIVE が優先
        let (_, d2) = decide(&all_correct(30, 100), &policy(50, 0.2, 0.999));
        assert!(matches!(&d2, Decision::Inconclusive(r) if r.contains(&"minimum_auto_count".to_string()) && r.contains(&"minimum_precision_lower_bound".to_string())));
    }

    #[test]
    fn only_the_coverage_is_short_gives_no_promotion() {
        let (rep, d) = decide(&all_correct(60, 400), &policy(50, 0.5, 0.9));
        assert!(matches!(&d, Decision::NoPromotion(r) if r == &vec!["minimum_auto_coverage".to_string()]), "{d:?}");
        assert!((rep["metrics"]["auto_coverage"].as_f64().unwrap() - 0.15).abs() < 1e-12);
        assert!((rep["metrics"]["abstention_rate"].as_f64().unwrap() - 0.85).abs() < 1e-12);
    }

    #[test]
    fn zero_auto_is_inconclusive_with_null_precision() {
        let inp = input((0..20).map(|i| unit(i, if i % 2 == 0 { Machine::Review } else { Machine::Unresolved }, pos(i as i64))).collect());
        let (rep, d) = decide(&inp, &policy(1, 0.0, 0.0));
        assert!(matches!(&d, Decision::Inconclusive(_)), "{d:?}");
        assert!(rep["metrics"]["auto_precision_point"].is_null() && rep["metrics"]["auto_precision_lower_bound"].is_null());
        assert_eq!(rep["metrics"]["n_auto"], 0);
    }

    #[test]
    fn one_wrong_auto_is_counted_and_respects_a_wrong_auto_cap() {
        let mut inp = all_correct(60, 100);
        inp.units[0].gt = pos(9999);
        let strict = PolicyConfig::test_only(50, 0.5, 0.95, 0.5, Some(0), UnevaluableAuto::Exclude);
        let (rep, d) = decide(&inp, &strict);
        assert!(matches!(&d, Decision::NoPromotion(r) if r == &vec!["maximum_wrong_auto".to_string()]), "{d:?}");
        assert_eq!(rep["metrics"]["n_wrong_auto"], 1);
        let lenient = PolicyConfig::test_only(50, 0.5, 0.95, 0.5, Some(1), UnevaluableAuto::Exclude);
        assert_eq!(decide(&inp, &lenient).1, Decision::Promote);
    }

    #[test]
    fn an_auto_on_a_none_gt_is_wrong_and_stays_in_the_denominator() {
        let mut inp = all_correct(60, 100);
        inp.units[0].gt = Gt::NoMatch;
        inp.units[1].gt = Gt::NoMatch;
        let (rep, _) = decide(&inp, &policy(50, 0.5, 0.0));
        let m = &rep["metrics"];
        assert_eq!((m["precision_denominator"].as_u64(), m["n_correct_auto"].as_u64(), m["n_wrong_auto_vs_none_gt"].as_u64(), m["n_wrong_auto"].as_u64()), (Some(60), Some(58), Some(2), Some(2)));
    }

    #[test]
    fn review_and_unresolved_machine_states_are_outside_the_precision_denominator_but_inside_abstention() {
        let units: Vec<Unit> = (0..100)
            .map(|i| match i {
                0..=59 => unit(i, auto(i as i64), pos(i as i64)),
                60..=79 => unit(i, Machine::Review, pos(i as i64)),
                _ => unit(i, Machine::Unresolved, Gt::NoMatch),
            })
            .collect();
        let (rep, _) = decide(&input(units), &policy(50, 0.5, 0.5));
        let m = &rep["metrics"];
        assert_eq!((m["n_auto"].as_u64(), m["n_review"].as_u64(), m["n_unresolved"].as_u64(), m["precision_denominator"].as_u64()), (Some(60), Some(20), Some(20), Some(60)));
        assert!((m["abstention_rate"].as_f64().unwrap() - 0.4).abs() < 1e-12 && (m["auto_coverage"].as_f64().unwrap() - 0.6).abs() < 1e-12);
    }

    #[test]
    fn abstention_is_a_mandatory_report_not_a_second_threshold() {
        // 判定の空間では coverage + abstention = 1。abstention を別の閾値にしない（policy に項目が無い）
        for (n_auto, total) in [(60usize, 100usize), (0, 40), (100, 100), (7, 33)] {
            let inp = all_correct(n_auto, total);
            let (rep, _) = decide(&inp, &policy(1, 0.0, 0.0));
            let m = &rep["metrics"];
            assert!((m["coverage_plus_abstention"].as_f64().unwrap() - 1.0).abs() < 1e-12, "{n_auto}/{total}");
            assert!(m.get("abstention_rate").is_some(), "必ず併記する");
        }
        let src = include_str!("pr4_pf0b.rs");
        let code = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        assert!(!code.contains("maximum_abstention"), "abstention の上限は policy の項目にしない");
        // abstention が高くても、coverage の条件だけで判定される（同じ事実を二重に数えない）
        let (_, d) = decide(&all_correct(60, 100), &policy(50, 0.7, 0.5));
        assert!(matches!(&d, Decision::NoPromotion(r) if r == &vec!["minimum_auto_coverage".to_string()]), "{d:?}");
    }

    #[test]
    fn invalid_auto_output_and_missing_output_are_technical_failures_without_metrics() {
        let mut inp = all_correct(60, 100);
        inp.units[0].machine = Machine::Auto(None);
        let keys: Vec<String> = inp.units.iter().map(|u| u.key.clone()).collect();
        inp.expected_membership_commitment = membership_commitment(&keys, VER);
        let (rep, d) = decide(&inp, &policy(50, 0.5, 0.5));
        assert!(matches!(&d, Decision::TechnicalFailure(r) if r.contains(&"invalid_auto_output".to_string())), "{d:?}");
        assert!(rep["metrics"].is_null(), "分母を黙って減らして計算可能にしない");
        let mut inp2 = all_correct(60, 100);
        inp2.units[5].machine = Machine::Missing;
        assert!(matches!(&decide(&inp2, &policy(50, 0.5, 0.5)).1, Decision::TechnicalFailure(r) if r.contains(&"machine_output_missing".to_string())));
    }

    #[test]
    fn a_technical_failure_never_promotes_or_reports_a_precision() {
        let mut inp = all_correct(100, 100);
        inp.run_failure = Some("generator crashed".into());
        let (rep, d) = decide(&inp, &policy(1, 0.0, 0.0));
        assert!(matches!(&d, Decision::TechnicalFailure(r) if r[0].starts_with("run_failure")));
        assert_eq!(rep["decision"], "TECHNICAL_FAILURE");
        assert!(!rep.to_string().contains("auto_precision_point"));
    }

    // ── 追加の境界 ──
    #[test]
    fn membership_changes_after_the_commitment_are_detected() {
        let base = all_correct(60, 100);
        assert_eq!(decide(&base, &policy(50, 0.5, 0.5)).1, Decision::Promote);
        let mut added = base.clone();
        added.units.push(unit(500, auto(500), pos(500)));
        assert!(matches!(&decide(&added, &policy(1, 0.0, 0.0)).1, Decision::TechnicalFailure(r) if r.contains(&"membership_mismatch".to_string())), "結果を見てからの追加は検出される");
        let mut removed = base.clone();
        removed.units.pop();
        assert!(matches!(&decide(&removed, &policy(1, 0.0, 0.0)).1, Decision::TechnicalFailure(r) if r.contains(&"membership_mismatch".to_string())));
        let mut renamed = base.clone();
        renamed.units[0].key = "work:renamed".into();
        assert!(matches!(&decide(&renamed, &policy(1, 0.0, 0.0)).1, Decision::TechnicalFailure(_)));
        let mut version = base.clone();
        version.eligibility_rule_version = "other".into();
        assert!(matches!(&decide(&version, &policy(1, 0.0, 0.0)).1, Decision::TechnicalFailure(_)));
        // 順序には依存しない
        let mut shuffled = base.clone();
        shuffled.units.reverse();
        assert_eq!(decide(&shuffled, &policy(50, 0.5, 0.5)).1, Decision::Promote);
        let mut dup = base.clone();
        dup.units.push(dup.units[0].clone());
        assert!(matches!(&decide(&dup, &policy(1, 0.0, 0.0)).1, Decision::TechnicalFailure(r) if r.contains(&"duplicate_unit_key".to_string())));
        assert!(matches!(&decide(&FinalInput { units: vec![], expected_membership_commitment: membership_commitment(&[], VER), eligibility_rule_version: VER.into(), run_failure: None }, &policy(1, 0.0, 0.0)).1, Decision::TechnicalFailure(r) if r.contains(&"empty_membership".to_string())));
    }

    #[test]
    fn the_treatment_of_auto_without_resolved_gt_is_an_explicit_policy_choice() {
        let mut inp = all_correct(60, 100);
        for i in 0..10 {
            inp.units[i].gt = Gt::Unresolved;
        }
        let exclude = PolicyConfig::test_only(40, 0.5, 0.95, 0.5, None, UnevaluableAuto::Exclude);
        let wrong = PolicyConfig::test_only(40, 0.5, 0.95, 0.5, None, UnevaluableAuto::CountAsWrong);
        let (a, _) = decide(&inp, &exclude);
        let (b, _) = decide(&inp, &wrong);
        assert_eq!((a["metrics"]["precision_denominator"].as_u64(), a["metrics"]["n_correct_auto"].as_u64(), a["metrics"]["n_wrong_auto"].as_u64()), (Some(50), Some(50), Some(0)));
        assert_eq!((b["metrics"]["precision_denominator"].as_u64(), b["metrics"]["n_correct_auto"].as_u64(), b["metrics"]["n_wrong_auto"].as_u64()), (Some(60), Some(50), Some(10)));
        assert_eq!(a["metrics"]["n_auto_gt_unresolved"], 10, "どちらでも件数は報告する");
        assert!(b["metrics"]["auto_precision_lower_bound"].as_f64().unwrap() < a["metrics"]["auto_precision_lower_bound"].as_f64().unwrap());
        // binding な FINAL の扱い: AUTO した単位の GT が確定できなければ、評価全体が INCONCLUSIVE（他の条件を満たしていても promote しない）
        let block = PolicyConfig::test_only(40, 0.5, 0.95, 0.5, None, UnevaluableAuto::BlockEvaluation);
        let (c, d) = decide(&inp, &block);
        assert!(matches!(&d, Decision::Inconclusive(r) if r == &vec!["auto_with_unresolved_gt".to_string()]), "{d:?}");
        assert_eq!(c["decision"], "INCONCLUSIVE");
        assert_eq!(c["metrics"]["n_auto_gt_unresolved"], 10);
        assert_eq!(c["metrics"]["precision_binding"], false, "暫定値は binding ではない");
        // AUTO の GT が全部確定していれば、BlockEvaluation でも通常どおり判定される
        let (ok, d2) = decide(&all_correct(60, 100), &PolicyConfig::test_only(50, 0.5, 0.95, 0.5, None, UnevaluableAuto::BlockEvaluation));
        assert_eq!(d2, Decision::Promote);
        assert_eq!(ok["metrics"]["precision_binding"], true);
        // GT が未解決でも、機械が AUTO でない単位（REVIEW / UNRESOLVED）は影響しない
        let mut review_only = all_correct(60, 100);
        for u in review_only.units.iter_mut().skip(60) {
            u.gt = Gt::Unresolved;
        }
        assert_eq!(decide(&review_only, &PolicyConfig::test_only(50, 0.5, 0.95, 0.5, None, UnevaluableAuto::BlockEvaluation)).1, Decision::Promote);
        // AUTO 件数の不足と同時でも、結論は INCONCLUSIVE（理由は両方記録する）
        let (_, d3) = decide(&inp, &PolicyConfig::test_only(500, 0.5, 0.95, 0.5, None, UnevaluableAuto::BlockEvaluation));
        assert!(matches!(&d3, Decision::Inconclusive(r) if r.contains(&"minimum_auto_count".to_string()) && r.contains(&"auto_with_unresolved_gt".to_string())));
    }

    #[test]
    fn a_binding_policy_must_use_block_evaluation() {
        let inp = all_correct(60, 100);
        for treatment in [UnevaluableAuto::Exclude, UnevaluableAuto::CountAsWrong] {
            let frozen = PolicyConfig { provenance: PolicyProvenance::Frozen { policy_sha256: "a".repeat(64) }, unevaluable_auto: treatment, ..policy(50, 0.5, 0.5) };
            let (rep, d) = decide(&inp, &frozen);
            assert!(matches!(&d, Decision::TechnicalFailure(r) if r[0].starts_with("invalid_policy") && r[0].contains("BlockEvaluation")), "{treatment:?}: {d:?}");
            assert!(rep["metrics"].is_null());
        }
        let ok = PolicyConfig { provenance: PolicyProvenance::Frozen { policy_sha256: "a".repeat(64) }, unevaluable_auto: UnevaluableAuto::BlockEvaluation, ..policy(50, 0.5, 0.5) };
        assert_eq!(decide(&inp, &ok).1, Decision::Promote);
        // 試験用の policy は Exclude / CountAsWrong も使える（synthetic engine の試験）
        assert_eq!(decide(&inp, &policy(50, 0.5, 0.5)).1, Decision::Promote);
    }

    #[test]
    fn the_same_data_under_different_policies_changes_only_the_decision() {
        let inp = all_correct(60, 100);
        let strict = decide(&inp, &policy(50, 0.5, 0.999));
        let loose = decide(&inp, &policy(50, 0.5, 0.5));
        assert!(matches!(strict.1, Decision::NoPromotion(_)) && loose.1 == Decision::Promote);
        assert_eq!(strict.0["metrics"]["n_auto"], loose.0["metrics"]["n_auto"]);
        assert_eq!(strict.0["metrics"]["auto_precision_lower_bound"], loose.0["metrics"]["auto_precision_lower_bound"], "測る値は policy に依存しない");
        assert_eq!(decide(&inp, &policy(50, 0.5, 0.5)).0, loose.0, "決定的");
    }

    #[test]
    fn invalid_policies_are_technical_failures_and_there_is_no_default_policy() {
        let inp = all_correct(60, 100);
        for p in [policy(0, 0.5, 0.5), policy(50, 1.5, 0.5), policy(50, 0.5, -0.1), PolicyConfig::test_only(50, 0.5, 1.0, 0.5, None, UnevaluableAuto::Exclude),
                  PolicyConfig::test_only(50, f64::NAN, 0.95, 0.5, None, UnevaluableAuto::Exclude)] {
            assert!(matches!(&decide(&inp, &p).1, Decision::TechnicalFailure(r) if r[0].starts_with("invalid_policy")));
        }
        let src = include_str!("pr4_pf0b.rs");
        let code = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        assert!(!code.contains("impl Default for PolicyConfig") && !code.contains("derive(Default") && !code.contains("PolicyConfig::default"), "policy は既定値を持たない");
    }

    #[test]
    fn test_only_policies_are_labelled_and_never_binding() {
        let (rep, _) = decide(&all_correct(60, 100), &policy(50, 0.5, 0.5));
        assert_eq!(rep["policy_provenance"]["class"], TEST_ONLY_LABEL);
        assert_eq!(rep["policy_provenance"]["binding"], false);
        assert!(rep["policy_provenance"]["note"].as_str().unwrap().contains("候補値として引用しない"));
        assert_eq!(rep["report_version"], REPORT_VERSION);
        // 凍結 policy の表現は存在するが、この予行演習では作らない
        let frozen = PolicyConfig { provenance: PolicyProvenance::Frozen { policy_sha256: "a".repeat(64) }, ..policy(50, 0.5, 0.5) };
        let (rep2, _) = decide(&all_correct(60, 100), &frozen);
        assert_eq!(rep2["policy_provenance"]["class"], "FROZEN");
    }

    #[test]
    fn the_report_contains_counts_and_rates_only_no_unit_data() {
        let (rep, _) = decide(&all_correct(60, 100), &policy(50, 0.5, 0.5));
        let text = rep.to_string();
        for banned in ["work:", "tmdb", "movie", "\"key\"", "title"] {
            assert!(!text.contains(banned), "{banned}");
        }
        for joint in ["n_auto", "auto_coverage", "abstention_rate", "n_wrong_auto", "auto_precision_lower_bound"] {
            assert!(rep["metrics"].get(joint).is_some(), "{joint} を併記する");
        }
        assert_eq!(rep["metrics"]["bound_method"], "one-sided exact Clopper-Pearson");
    }

    #[test]
    fn the_rehearsal_has_no_database_network_or_filesystem_access() {
        let src = include_str!("pr4_pf0b.rs");
        let code = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        for banned in ["rusqlite", "std::fs", "std::net", "reqwest", "Command", "std::env", "metadata_match", "INSERT ", "SELECT "] {
            assert!(!code.contains(banned), "{banned}");
        }
        let m = include_str!("mod.rs");
        assert!(m.contains("#[cfg(test)]\npub mod pr4_pf0b;"), "test ビルドでだけコンパイルする");
    }

    #[test]
    fn a_full_rehearsal_over_every_scenario_runs_end_to_end() {
        // 1 つの holdout 形状（100 件）を、AUTO の正解 / 誤り / none / REVIEW / UNRESOLVED / GT 未解決 を混ぜて通す
        let units: Vec<Unit> = (0..100)
            .map(|i| match i {
                0..=39 => unit(i, auto(i as i64), pos(i as i64)),
                40..=42 => unit(i, auto(i as i64), pos(7000 + i as i64)),
                43..=44 => unit(i, auto(i as i64), Gt::NoMatch),
                45..=46 => unit(i, auto(i as i64), Gt::Unresolved),
                47..=69 => unit(i, Machine::Review, pos(i as i64)),
                _ => unit(i, Machine::Unresolved, Gt::NoMatch),
            })
            .collect();
        let inp = input(units);
        let policies = [
            ("loose", policy(10, 0.1, 0.5)),
            ("needs_more_auto", policy(100, 0.1, 0.5)),
            ("needs_more_coverage", policy(10, 0.9, 0.5)),
            ("needs_high_bound", policy(10, 0.1, 0.99)),
        ];
        let outcomes: Vec<(&str, String)> = policies.iter().map(|(n, p)| (*n, decide(&inp, p).0["decision"].as_str().unwrap().to_string())).collect();
        assert_eq!(outcomes, vec![("loose", "PROMOTE".to_string()), ("needs_more_auto", "INCONCLUSIVE".to_string()), ("needs_more_coverage", "NO_PROMOTION".to_string()), ("needs_high_bound", "NO_PROMOTION".to_string())]);
        let (rep, _) = decide(&inp, &policies[0].1);
        let m = &rep["metrics"];
        assert_eq!((m["n_auto"].as_u64(), m["n_wrong_auto_vs_positive_gt"].as_u64(), m["n_wrong_auto_vs_none_gt"].as_u64(), m["n_auto_gt_unresolved"].as_u64(), m["precision_denominator"].as_u64()), (Some(47), Some(3), Some(2), Some(2), Some(45)));
    }
}
