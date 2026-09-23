//! Jev の質問 contract（PR3 C3A）。
//!
//! ここも**純粋な生成と検証だけ**を行う。DB・HTTP・環境変数・`match_once` には触れない。
//! contract の `jev_contracts` への登録は C4、実通信は C3B。
//!
//! # 設計
//!
//! - **決定的に判定できることはコードでやる。** 年の差・言語コードの一致・版の表記・
//!   episode 表記は `metadata_matcher` / `rules_v1` 側の仕事で、Jev には
//!   「与えた state だけを見て、この候補は同じ作品か」という狭い判断だけをさせる。
//! - 候補ごとの二値判断は **Noul**（yes である確率を 0〜1 で返す）。
//! - 最終選択は **Choice**（選択肢・各選択肢の確率・confidence を返す）。
//!   `best_match` 自身が confidence を返すので、**別の certainty 質問は作らない**。
//! - Noul の値も Choice の confidence も、CineMantis では「正解確率」として扱わない。
//!   当面は shadow 評価用のモデル出力として記録するだけ（Choice の confidence は
//!   TypeSafe が報告した selected label の confidence であって、CineMantis の正解確率ではない）。
//! - **Score は `jev-contract-1` では扱わない。** 生成も検証もしない。
//!   導入するときは contract の版を上げ、その時点の TypeSafe 仕様に合わせて実装する。
//!
//! # C3A と C3B の境界
//!
//! このモジュールの入口は [`parse_answers`] と [`validate_response`]。
//! HTTP・status・応答サイズ・top-level（`model` / `usage` / request id）・model mismatch は
//! C3B の担当で、C3B は `answers` の値だけをここへ渡す。
//!
//! # contract hash と実際の質問
//!
//! 候補数は 1〜3 で変わるので、実際の `best_match` の選択肢も毎回変わる。
//! そのたびに contract の hash が変わってはいけないので、
//! **hash の対象は「質問の生成規則」** であり、実際に生成した質問ではない。
//! 生成した質問は C4 で `metadata_match_jev_calls.questions_json` に保存する。

use crate::services::jev_state::{CandidateIdentity, JEV_STATE_SCHEMA_VERSION, MAX_CANDIDATES};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// contract の版。文面・生成規則・検証規則を変えたら上げる
pub const JEV_CONTRACT_VERSION: &str = "jev-contract-1";

/// 候補ごとの質問 ID（`c1_same_work` など）
pub const SAME_WORK_SUFFIX: &str = "_same_work";
/// 最終選択の質問 ID
pub const BEST_MATCH_ID: &str = "best_match";
/// 「どれでもない」の選択肢
pub const NONE_OPTION: &str = "NONE";

/// 確率の合計がこの範囲を超えてずれていたら不正とみなす
const PROBABILITY_SUM_TOLERANCE: f64 = 0.02;

/// Jev への指示。**state だけで判断し、自分の知識を使わない**ことを明記する
pub const INSTRUCTIONS: &str = "\
Judge only from the information supplied in this state. \
Do not use any outside knowledge about films, television, release dates, cast members, \
or these specific titles, and do not guess from what a title sounds like. \
The state contains local evidence taken from the file before any catalogue lookup, \
and a short list of catalogue candidates. \
If the supplied state does not contain enough information to decide, say so with the \
option that expresses uncertainty rather than picking a candidate. \
Never refer to a catalogue id that is not listed in the state, and never invent one.";

// ─── 質問 ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QuestionKind {
    /// yes である確率を 0〜1 で返す（wire では `"noul"`）
    Noul,
    /// 選択肢・各選択肢の確率・confidence を返す（wire では `"choice"`）
    Choice,
}

impl QuestionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            QuestionKind::Noul => "noul",
            QuestionKind::Choice => "choice",
        }
    }
}

/// Choice の選択肢1つ（キーと、その意味の説明）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Criterion {
    pub key: String,
    pub description: String,
}

/// 内部表現。実際に送る形は [`questions_to_json`] が作る
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuestionSpec {
    pub id: String,
    pub kind: QuestionKind,
    /// TypeSafe の `instructions` になる文面
    pub instructions: String,
    /// Choice のときの選択肢と説明
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<Criterion>,
}

impl QuestionSpec {
    fn noul(id: String, instructions: String) -> Self {
        QuestionSpec { id, kind: QuestionKind::Noul, instructions, criteria: Vec::new() }
    }

    fn choice(id: String, instructions: String, criteria: Vec<Criterion>) -> Self {
        QuestionSpec { id, kind: QuestionKind::Choice, instructions, criteria }
    }

    /// 選択肢のキー（検証に使う）
    pub fn criteria_keys(&self) -> Vec<String> {
        self.criteria.iter().map(|c| c.key.clone()).collect()
    }
}

/// 候補ごとの Noul の文面
fn same_work_prompt(cand_key: &str) -> String {
    format!(
        "Considering only the supplied state, is candidate {cand_key} the same work as the local file? \
         Weigh the local titles, the years, the media kind, the audio and subtitle languages, \
         and any edition markers that are present. \
         Treat a missing field as no evidence rather than as disagreement."
    )
}

/// 最終選択の文面
fn best_match_prompt() -> String {
    format!(
        "Which of the supplied candidates does the state support as the same work as the local file? \
         Each option below names one candidate by its key. \
         Choose {NONE_OPTION} if the state does not clearly support any of them."
    )
}

/// Choice の選択肢の説明。キーだけに意味を持たせず、本文でも対象を明記する
fn candidate_criterion(cand_key: &str) -> Criterion {
    Criterion {
        key: cand_key.to_string(),
        description: format!(
            "Candidate {cand_key} in the supplied state is the same work as the local file."
        ),
    }
}

/// 「どれでもない」の説明
fn none_criterion() -> Criterion {
    Criterion {
        key: NONE_OPTION.to_string(),
        description: "None of the supplied candidates is the same work as the local file, \
                      or the supplied state does not show clearly enough that any of them is."
            .to_string(),
    }
}

/// 候補キーから、実際に送る質問を作る。
/// `best_match` の選択肢は**送った候補キー + NONE** だけ。
pub fn build_questions(cand_keys: &[String]) -> Result<Vec<QuestionSpec>, JevContractError> {
    if cand_keys.is_empty() {
        return Err(JevContractError::NoCandidates);
    }
    if cand_keys.len() > MAX_CANDIDATES {
        return Err(JevContractError::TooManyCandidates { count: cand_keys.len() });
    }
    for (index, key) in cand_keys.iter().enumerate() {
        let expected = format!("c{}", index + 1);
        if *key != expected {
            return Err(JevContractError::UnknownCandidateKey { key: key.clone() });
        }
    }

    let mut questions: Vec<QuestionSpec> = cand_keys
        .iter()
        .map(|key| QuestionSpec::noul(format!("{key}{SAME_WORK_SUFFIX}"), same_work_prompt(key)))
        .collect();

    let mut criteria: Vec<Criterion> = cand_keys
        .iter()
        .map(|key| candidate_criterion(key))
        .collect();
    criteria.push(none_criterion());
    questions.push(QuestionSpec::choice(BEST_MATCH_ID.to_string(), best_match_prompt(), criteria));
    Ok(questions)
}

/// TypeSafe へ実際に送る `questions` オブジェクトを作る。
///
/// 形は **question id をキーにした map**。
/// ```text
/// {
///   "c1_same_work": { "type": "noul",   "instructions": "..." },
///   "best_match":   { "type": "choice", "instructions": "...",
///                     "criteria": { "c1": "...", "NONE": "..." } }
/// }
/// ```
/// C4 は内部表現ではなく、**ここが返した JSON そのもの**を
/// `metadata_match_jev_calls.questions_json` に保存する。
pub fn questions_to_json(questions: &[QuestionSpec]) -> Value {
    let mut wire = Map::new();
    for question in questions {
        let mut object = Map::new();
        object.insert("type".into(), Value::String(question.kind.as_str().to_string()));
        object.insert("instructions".into(), Value::String(question.instructions.clone()));
        if !question.criteria.is_empty() {
            let mut criteria = Map::new();
            for criterion in &question.criteria {
                criteria.insert(
                    criterion.key.clone(),
                    Value::String(criterion.description.clone()),
                );
            }
            object.insert("criteria".into(), Value::Object(criteria));
        }
        wire.insert(question.id.clone(), Value::Object(object));
    }
    Value::Object(wire)
}

// ─── contract ────────────────────────────────────────────────────────────────

/// 凍結する contract。hash の対象は「生成規則」であって、実際の質問ではない
#[derive(Debug, Clone, PartialEq)]
pub struct JevContract {
    pub contract_version: String,
    pub state_schema_version: String,
    pub instructions: String,
    /// 質問の生成規則（run ごとに変わる値は入れない）
    pub questions_template: Value,
    /// 判断の基準
    pub criteria: Value,
    /// 応答の検証規則
    pub validation_policy: Value,
    /// 上の5つの canonical JSON から作る SHA-256。**モデル名は含めない**
    pub canonical_sha256: String,
}

/// 質問の生成規則。実際の候補キーやタイトル、TMDB ID は入らない
fn questions_template() -> Value {
    json!({
        "wire_shape": "questions is an object keyed by question id; each value has type, instructions and (for choice) criteria",
        "candidate_key_pattern": "c{index}",
        "max_candidates": MAX_CANDIDATES,
        "per_candidate": [
            {
                "id_pattern": format!("{{cand_key}}{SAME_WORK_SUFFIX}"),
                "type": "noul",
                "instructions_template": same_work_prompt("{cand_key}")
            }
        ],
        "final": [
            {
                "id": BEST_MATCH_ID,
                "type": "choice",
                "instructions": best_match_prompt(),
                "criteria_rule": "present candidate keys in order, then NONE",
                "criterion_description_template": candidate_criterion("{cand_key}").description,
                "none_criterion_description": none_criterion().description
            }
        ]
    })
}

fn criteria() -> Value {
    json!({
        "judgement": "same work as the local file",
        "evidence_fields": [
            "local.derived_title", "local.embedded_title",
            "local.filename_year", "local.embedded_year",
            "local.media_kind", "local.country_type",
            "local.season_no", "local.episode_no", "local.part_hint",
            "local.edition_markers", "local.audio_languages", "local.subtitle_languages",
            "local.cast",
            "candidates[].title", "candidates[].original_title",
            "candidates[].year", "candidates[].media_type", "candidates[].original_language"
        ],
        "missing_field_policy": "treat as no evidence, not as disagreement",
        "outside_knowledge": "forbidden",
        "deterministic_checks_done_in_code": [
            "year tolerance", "original language comparison",
            "edition markers", "episode markers", "part numbers"
        ],
        "model_output_use": "shadow evaluation only; not treated as a probability of being correct"
    })
}

fn validation_policy() -> Value {
    json!({
        "answer_set": "must match the generated question set exactly",
        "missing_answer": "invalid",
        "unknown_answer": "invalid",
        "kind_mismatch": "invalid",
        "noul_value_range": [0.0, 1.0],
        "choice_label": "must be one of the options sent",
        "choice_probability_keys": "must match the options sent exactly",
        "probability_range": [0.0, 1.0],
        "probability_sum_tolerance": PROBABILITY_SUM_TOLERANCE,
        "confidence_range": [0.0, 1.0],
        "supported_question_types": ["noul", "choice"],
        "best_match_options": "candidate keys sent, plus NONE",
        "catalogue_id_from_model": "never accepted; resolved from the candidate identity table"
    })
}

/// 初版の contract
pub fn contract_v1() -> JevContract {
    let instructions = INSTRUCTIONS.to_string();
    let questions_template = questions_template();
    let criteria = criteria();
    let validation_policy = validation_policy();
    let canonical_sha256 = contract_hash(
        JEV_STATE_SCHEMA_VERSION,
        &instructions,
        &questions_template,
        &criteria,
        &validation_policy,
    );
    JevContract {
        contract_version: JEV_CONTRACT_VERSION.to_string(),
        state_schema_version: JEV_STATE_SCHEMA_VERSION.to_string(),
        instructions,
        questions_template,
        criteria,
        validation_policy,
        canonical_sha256,
    }
}

/// contract の hash。**モデル名も、run ごとの値も含めない**
pub fn contract_hash(
    state_schema_version: &str,
    instructions: &str,
    questions_template: &Value,
    criteria: &Value,
    validation_policy: &Value,
) -> String {
    let document = json!({
        "state_schema_version": state_schema_version,
        "instructions": instructions,
        "questions_template": questions_template,
        "criteria": criteria,
        "validation_policy": validation_policy,
    });
    sha256_hex(canonical_json(&document).as_bytes())
}

/// JSON を canonical 化する。
/// object のキーを再帰的にソートし、**配列の順序は保つ**。整形の違いで hash を変えないため。
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            out.push('{');
            for (index, (key, value)) in sorted.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String((*key).clone()).to_string());
                out.push(':');
                write_canonical(value, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

// ─── 応答 ────────────────────────────────────────────────────────────────────

/// 応答1件。公式 wire をそのまま写したもの。
///
/// ```json
/// {"type":"noul","noul":0.73}
/// {"type":"choice","choice":"c1","confidence":0.8,"probabilities":{"c1":0.8,"NONE":0.2}}
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum RawAnswer {
    /// wire の `noul`（yes である確率）
    Noul { value: f64 },
    /// wire の `choice`（選んだ選択肢）。`confidence` は必須
    Choice {
        label: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
}

impl RawAnswer {
    fn kind(&self) -> QuestionKind {
        match self {
            RawAnswer::Noul { .. } => QuestionKind::Noul,
            RawAnswer::Choice { .. } => QuestionKind::Choice,
        }
    }
}

/// `answers` オブジェクトを読む。**C3A の入口はここ。**
///
/// C3B は HTTP・status・応答サイズ・top-level（`model` / `usage`）・model mismatch を扱い、
/// `answers` の値だけをここへ渡す（別形式へ読み替えない）。
/// 公式の map 形式以外（配列など）は fail closed で拒否する。
pub fn parse_answers(answers: &Value) -> Result<BTreeMap<String, RawAnswer>, JevContractError> {
    let Value::Object(map) = answers else {
        return Err(JevContractError::Parse(
            "answers は question id をキーにしたオブジェクトである必要があります".to_string(),
        ));
    };
    let mut parsed = BTreeMap::new();
    for (id, answer) in map {
        parsed.insert(id.clone(), parse_answer(id, answer)?);
    }
    Ok(parsed)
}

fn parse_answer(id: &str, value: &Value) -> Result<RawAnswer, JevContractError> {
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "noul" => {
            // 公式は `noul`。旧 `value` は受け付けない（fail closed）
            let noul = value
                .get("noul")
                .and_then(Value::as_f64)
                .ok_or_else(|| JevContractError::Parse(format!("{id}: noul がありません")))?;
            Ok(RawAnswer::Noul { value: noul })
        }
        "choice" => {
            // 公式は `choice`。旧 `label` は受け付けない
            let label = value
                .get("choice")
                .and_then(Value::as_str)
                .ok_or_else(|| JevContractError::Parse(format!("{id}: choice がありません")))?
                .to_string();
            // 公式の ChoiceResponse では confidence は必須
            let confidence = value
                .get("confidence")
                .and_then(Value::as_f64)
                .ok_or_else(|| JevContractError::Parse(format!("{id}: confidence がありません")))?;
            // probabilities は必須。entry を1つも捨てない（捨てると後段の
            // exact-key 検証をすり抜ける）。数値でない値が1つでもあれば即 Parse error。
            let raw = value.get("probabilities").ok_or_else(|| {
                JevContractError::Parse(format!("{id}: probabilities がありません"))
            })?;
            let raw = raw.as_object().ok_or_else(|| {
                JevContractError::Parse(format!("{id}: probabilities がオブジェクトではありません"))
            })?;
            let mut probabilities = BTreeMap::new();
            for (key, value) in raw {
                let value = value.as_f64().ok_or_else(|| {
                    JevContractError::Parse(format!("{id}: probabilities.{key} が数値ではありません"))
                })?;
                probabilities.insert(key.clone(), value);
            }
            Ok(RawAnswer::Choice { label, probabilities, confidence })
        }
        other => Err(JevContractError::Parse(format!("{id}: 未知の回答種別 {other}"))),
    }
}

/// 検証を通した回答
#[derive(Debug, Clone, PartialEq)]
pub struct JevAnswers {
    /// 候補ごとの「同じ作品である確率」（Noul の値。正解確率としては扱わない）
    pub same_work: Vec<CandidateProbability>,
    /// 選ばれた候補。`None` は NONE（該当なし）
    pub best_match: Option<CandidateIdentity>,
    /// `best_match` の選択肢ごとの確率
    pub best_match_probabilities: BTreeMap<String, f64>,
    /// TypeSafe が報告した selected label の confidence。CineMantis の正解確率ではない
    pub best_match_confidence: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateProbability {
    pub cand_key: String,
    pub tmdb_id: i64,
    pub media_type: String,
    /// 0〜1
    pub same_work: f64,
}

/// 生成した質問と応答を突き合わせて検証する。
///
/// - 質問集合と回答集合は**完全一致**でなければならない（欠落・余分・種別違いは不正）
/// - `best_match` は送った候補キーか NONE だけ
/// - **TMDB ID は応答から受け取らない**。選ばれた key を `identity` から引き直す
pub fn validate_response(
    questions: &[QuestionSpec],
    identity: &[CandidateIdentity],
    answers: &BTreeMap<String, RawAnswer>,
) -> Result<JevAnswers, JevContractError> {
    // 質問集合との完全一致
    for question in questions {
        if !answers.contains_key(&question.id) {
            return Err(JevContractError::MissingAnswer { question_id: question.id.clone() });
        }
    }
    for id in answers.keys() {
        if !questions.iter().any(|q| q.id == *id) {
            return Err(JevContractError::UnknownAnswer { question_id: id.clone() });
        }
    }

    let mut same_work = Vec::new();
    let mut best_match_key: Option<String> = None;
    let mut best_match_probabilities = BTreeMap::new();
    let mut best_match_confidence = 0.0;

    for question in questions {
        let answer = &answers[&question.id];
        if answer.kind() != question.kind {
            return Err(JevContractError::KindMismatch {
                question_id: question.id.clone(),
                expected: question.kind.as_str().to_string(),
                found: answer.kind().as_str().to_string(),
            });
        }
        match answer {
            RawAnswer::Noul { value } => {
                check_unit_range(&question.id, *value)?;
                let cand_key = question
                    .id
                    .strip_suffix(SAME_WORK_SUFFIX)
                    .ok_or_else(|| JevContractError::UnknownAnswer {
                        question_id: question.id.clone(),
                    })?;
                let found = identity
                    .iter()
                    .find(|i| i.cand_key == cand_key)
                    .ok_or_else(|| JevContractError::UnknownCandidateKey {
                        key: cand_key.to_string(),
                    })?;
                same_work.push(CandidateProbability {
                    cand_key: found.cand_key.clone(),
                    tmdb_id: found.tmdb_id,
                    media_type: found.media_type.clone(),
                    same_work: *value,
                });
            }
            RawAnswer::Choice { label, probabilities, confidence } => {
                let keys = question.criteria_keys();
                if !keys.contains(label) {
                    return Err(JevContractError::UnknownChoiceLabel {
                        question_id: question.id.clone(),
                        label: label.clone(),
                    });
                }
                check_probability_map(&question.id, probabilities, &keys)?;
                check_unit_range(&question.id, *confidence)?;
                if question.id == BEST_MATCH_ID {
                    best_match_key = Some(label.clone());
                    best_match_probabilities = probabilities.clone();
                    best_match_confidence = *confidence;
                }
            }
        }
    }

    let best_match_key = best_match_key.ok_or_else(|| JevContractError::MissingAnswer {
        question_id: BEST_MATCH_ID.to_string(),
    })?;
    // 選ばれた key から候補を引き直す（応答の ID は使わない）
    let best_match = if best_match_key == NONE_OPTION {
        None
    } else {
        Some(
            identity
                .iter()
                .find(|i| i.cand_key == best_match_key)
                .cloned()
                .ok_or(JevContractError::UnknownCandidateKey { key: best_match_key })?,
        )
    };

    Ok(JevAnswers { same_work, best_match, best_match_probabilities, best_match_confidence })
}

fn check_unit_range(question_id: &str, value: f64) -> Result<(), JevContractError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(JevContractError::ValueOutOfRange {
            question_id: question_id.to_string(),
            value,
        });
    }
    Ok(())
}

fn check_probability_map(
    question_id: &str,
    probabilities: &BTreeMap<String, f64>,
    expected_keys: &[String],
) -> Result<(), JevContractError> {
    if probabilities.is_empty() {
        // 確率分布が無い応答は、どの選択肢をどれだけ支持したか分からない
        return Err(JevContractError::ProbabilityKeysMismatch {
            question_id: question_id.to_string(),
        });
    }
    if probabilities.len() != expected_keys.len()
        || !expected_keys.iter().all(|key| probabilities.contains_key(key))
    {
        return Err(JevContractError::ProbabilityKeysMismatch {
            question_id: question_id.to_string(),
        });
    }
    let mut sum = 0.0;
    for value in probabilities.values() {
        check_unit_range(question_id, *value)?;
        sum += value;
    }
    if (sum - 1.0).abs() > PROBABILITY_SUM_TOLERANCE {
        return Err(JevContractError::ProbabilitySum {
            question_id: question_id.to_string(),
            sum,
        });
    }
    Ok(())
}

// ─── エラー ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum JevContractError {
    NoCandidates,
    TooManyCandidates { count: usize },
    UnknownCandidateKey { key: String },
    MissingAnswer { question_id: String },
    UnknownAnswer { question_id: String },
    KindMismatch { question_id: String, expected: String, found: String },
    ValueOutOfRange { question_id: String, value: f64 },
    UnknownChoiceLabel { question_id: String, label: String },
    ProbabilityKeysMismatch { question_id: String },
    ProbabilitySum { question_id: String, sum: f64 },
    Parse(String),
}

impl std::fmt::Display for JevContractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JevContractError::NoCandidates => write!(f, "候補が1件もありません"),
            JevContractError::TooManyCandidates { count } => {
                write!(f, "候補が多すぎます（{count} 件、上限 {MAX_CANDIDATES}）")
            }
            JevContractError::UnknownCandidateKey { key } => {
                write!(f, "送っていない候補キーです: {key}")
            }
            JevContractError::MissingAnswer { question_id } => {
                write!(f, "回答がありません: {question_id}")
            }
            JevContractError::UnknownAnswer { question_id } => {
                write!(f, "聞いていない質問への回答です: {question_id}")
            }
            JevContractError::KindMismatch { question_id, expected, found } => {
                write!(f, "{question_id} の回答種別が違います（期待 {expected}、実際 {found}）")
            }
            JevContractError::ValueOutOfRange { question_id, value } => {
                write!(f, "{question_id} の値が範囲外です: {value}")
            }
            JevContractError::UnknownChoiceLabel { question_id, label } => {
                write!(f, "{question_id} に無い選択肢です: {label}")
            }
            JevContractError::ProbabilityKeysMismatch { question_id } => {
                write!(f, "{question_id} の確率分布が選択肢と一致しません")
            }
            JevContractError::ProbabilitySum { question_id, sum } => {
                write!(f, "{question_id} の確率の合計がずれています: {sum}")
            }
            JevContractError::Parse(message) => write!(f, "応答を読めません: {message}"),
        }
    }
}

impl std::error::Error for JevContractError {}

// ─── SHA-256 ─────────────────────────────────────────────────────────────────
//
// contract の指紋（監査識別子）に使う。秘密保護の用途ではないが、
// 既知の実装を使うため sha2 crate に任せる。

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(count: usize) -> Vec<String> {
        (1..=count).map(|index| format!("c{index}")).collect()
    }

    fn identity(count: usize) -> Vec<CandidateIdentity> {
        (1..=count)
            .map(|index| CandidateIdentity {
                cand_key: format!("c{index}"),
                tmdb_id: 100 + index as i64,
                media_type: "movie".to_string(),
            })
            .collect()
    }

    /// 公式 wire の answers オブジェクトを組み立てる
    fn official_answers(count: usize, choice: &str) -> Value {
        let mut answers = Map::new();
        for key in keys(count) {
            answers.insert(
                format!("{key}{SAME_WORK_SUFFIX}"),
                json!({"type": "noul", "noul": 0.8}),
            );
        }
        let mut options = keys(count);
        options.push(NONE_OPTION.to_string());
        let share = 1.0 / options.len() as f64;
        let probabilities: Map<String, Value> = options
            .iter()
            .map(|option| (option.clone(), json!(share)))
            .collect();
        answers.insert(
            BEST_MATCH_ID.to_string(),
            json!({
                "type": "choice",
                "choice": choice,
                "confidence": 0.7,
                "probabilities": Value::Object(probabilities)
            }),
        );
        Value::Object(answers)
    }

    fn parsed(count: usize, choice: &str) -> BTreeMap<String, RawAnswer> {
        parse_answers(&official_answers(count, choice)).unwrap()
    }

    // ─── canonical JSON と hash ──────────────────────────────────────────────

    #[test]
    fn canonical_json_sorts_keys_and_keeps_array_order() {
        let a = json!({"b": 1, "a": {"d": [3, 1, 2], "c": true}});
        let b = json!({"a": {"c": true, "d": [3, 1, 2]}, "b": 1});
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert_eq!(canonical_json(&a), r#"{"a":{"c":true,"d":[3,1,2]},"b":1}"#);
        assert_ne!(canonical_json(&json!([1, 2])), canonical_json(&json!([2, 1])));
    }

    /// contract hash の golden。文面・生成規則・基準・検証規則を変えたらここが落ちる。
    ///
    /// 経緯: SHA-256 を自前実装から `sha2` crate へ移した時点では、同じ内容に対して
    /// hash は変わらなかった（実装と canonical 化がどちらも正しかった）。
    /// その後 wire 形式（questions object 化・criteria の説明・best_match の文面）を
    /// 変えたため、**内容の変更として** golden を更新している。
    /// C3A.1 の応答 wire 修正では contract 文書を変えていない。ただし初版を確定する際に
    /// `score_rubric` を落として `supported_question_types` を入れたので、その分だけ
    /// golden を更新している（jev-contract-1 は未登録・未使用のため版は据え置き）。
    #[test]
    fn contract_hash_is_stable() {
        let contract = contract_v1();
        assert_eq!(contract.contract_version, "jev-contract-1");
        assert_eq!(contract.state_schema_version, JEV_STATE_SCHEMA_VERSION);
        assert_eq!(
            contract.canonical_sha256,
            "d585fb9a49d39f77b45730a3763a35cb2a0cf85d36a4753a2e860f7be5765df3",
            "contract の内容が変わった。意図した変更なら contract_version を上げて golden を更新する"
        );
    }

    #[test]
    fn contract_hash_ignores_formatting_and_model() {
        let contract = contract_v1();
        let reordered = contract_hash(
            &contract.state_schema_version,
            &contract.instructions,
            &serde_json::from_str::<Value>(
                &serde_json::to_string_pretty(&contract.questions_template).unwrap(),
            )
            .unwrap(),
            &contract.criteria,
            &contract.validation_policy,
        );
        assert_eq!(contract.canonical_sha256, reordered);

        let document = canonical_json(&json!({
            "instructions": contract.instructions,
            "questions_template": contract.questions_template,
            "criteria": contract.criteria,
            "validation_policy": contract.validation_policy,
        }));
        assert!(!document.contains("jev-1."));
        assert!(!document.to_lowercase().contains("model_name"));
    }

    #[test]
    fn contract_hash_does_not_depend_on_run_values() {
        let before = contract_v1().canonical_sha256;
        let _ = build_questions(&keys(1)).unwrap();
        let _ = build_questions(&keys(3)).unwrap();
        assert_eq!(contract_v1().canonical_sha256, before);

        let template = canonical_json(&contract_v1().questions_template);
        assert!(!template.contains("\"c1\""), "実際の候補キーが template に入っている");
        assert!(template.contains("{cand_key}"), "生成規則として残す");
    }

    // ─── 質問生成 ────────────────────────────────────────────────────────────

    #[test]
    fn questions_are_generated_for_one_two_and_three_candidates() {
        for count in 1..=MAX_CANDIDATES {
            let questions = build_questions(&keys(count)).unwrap();
            assert_eq!(questions.len(), count + 1);

            for (index, question) in questions.iter().take(count).enumerate() {
                let cand_key = format!("c{}", index + 1);
                assert_eq!(question.id, format!("{cand_key}_same_work"));
                assert_eq!(question.kind, QuestionKind::Noul);
                assert!(question.criteria.is_empty());
                assert!(question.instructions.contains(&format!("candidate {cand_key}")));
            }

            let best = questions.last().unwrap();
            assert_eq!(best.id, BEST_MATCH_ID);
            assert_eq!(best.kind, QuestionKind::Choice);
            let mut expected = keys(count);
            expected.push(NONE_OPTION.to_string());
            assert_eq!(best.criteria_keys(), expected, "選択肢は送った候補 + NONE だけ");
            let none = best.criteria.last().unwrap();
            assert_eq!(none.key, NONE_OPTION);
            assert!(
                none.description.contains("None of the supplied candidates is the same work"),
                "NONE の意味を説明する: {}",
                none.description
            );

            // certainty 質問は作らない / Score は v1 で扱わない
            assert!(!questions.iter().any(|q| q.id.contains("certainty")));
            let wire = canonical_json(&questions_to_json(&questions));
            assert!(!wire.contains("score"), "Score は v1 で生成しない");
            assert!(!wire.contains("rubric"));
        }
    }

    #[test]
    fn question_generation_rejects_bad_candidate_sets() {
        assert_eq!(build_questions(&[]), Err(JevContractError::NoCandidates));
        assert_eq!(
            build_questions(&keys(4)),
            Err(JevContractError::TooManyCandidates { count: 4 })
        );
        assert_eq!(
            build_questions(&["c2".to_string()]),
            Err(JevContractError::UnknownCandidateKey { key: "c2".to_string() })
        );
    }

    #[test]
    fn questions_are_serialized_as_the_typesafe_wire_object() {
        for count in 1..=MAX_CANDIDATES {
            let questions = build_questions(&keys(count)).unwrap();
            let wire = questions_to_json(&questions);
            let object = wire.as_object().expect("questions はオブジェクト");
            assert_eq!(object.len(), count + 1);

            for key in keys(count) {
                let question = &object[&format!("{key}_same_work")];
                assert_eq!(question["type"], "noul");
                assert!(question["instructions"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("candidate {key}")));
                assert!(question.get("criteria").is_none(), "noul に criteria は付けない");
                assert!(question.get("kind").is_none(), "wire のキーは type");
                assert!(question.get("prompt").is_none(), "wire のキーは instructions");
            }

            let best = &object[BEST_MATCH_ID];
            assert_eq!(best["type"], "choice");
            let criteria = best["criteria"].as_object().unwrap();
            assert_eq!(criteria.len(), count + 1);
            for key in keys(count) {
                assert!(criteria[&key].as_str().unwrap().contains(&format!("Candidate {key}")));
            }
            assert!(criteria[NONE_OPTION]
                .as_str()
                .unwrap()
                .contains("None of the supplied candidates"));
        }
    }

    /// K=2 の wire JSON を丸ごと固定する（C4 が保存するのはこの形）
    #[test]
    fn wire_questions_match_the_expected_json() {
        let wire = questions_to_json(&build_questions(&keys(2)).unwrap());
        let actual = serde_json::to_string(&wire).unwrap();
        let expected = "{\"best_match\":{\"criteria\":{\"NONE\":\"None of the supplied candidates is the same work as the local file, or the supplied state does not show clearly enough that any of them is.\",\"c1\":\"Candidate c1 in the supplied state is the same work as the local file.\",\"c2\":\"Candidate c2 in the supplied state is the same work as the local file.\"},\"instructions\":\"Which of the supplied candidates does the state support as the same work as the local file? Each option below names one candidate by its key. Choose NONE if the state does not clearly support any of them.\",\"type\":\"choice\"},\"c1_same_work\":{\"instructions\":\"Considering only the supplied state, is candidate c1 the same work as the local file? Weigh the local titles, the years, the media kind, the audio and subtitle languages, and any edition markers that are present. Treat a missing field as no evidence rather than as disagreement.\",\"type\":\"noul\"},\"c2_same_work\":{\"instructions\":\"Considering only the supplied state, is candidate c2 the same work as the local file? Weigh the local titles, the years, the media kind, the audio and subtitle languages, and any edition markers that are present. Treat a missing field as no evidence rather than as disagreement.\",\"type\":\"noul\"}}";
        assert_eq!(actual, expected, "wire JSON が変わった");
    }

    #[test]
    fn instructions_forbid_outside_knowledge() {
        let instructions = contract_v1().instructions;
        assert!(instructions.contains("Judge only from the information supplied in this state"));
        assert!(instructions.contains("Do not use any outside knowledge"));
        assert!(instructions.contains("never invent one"));
    }

    // ─── 公式 wire の応答 ────────────────────────────────────────────────────

    /// Noul は公式の `noul` から読む
    #[test]
    fn noul_official_wire_is_accepted() {
        let answers = parse_answers(&json!({
            "c1_same_work": {"type": "noul", "noul": 0.91},
            "best_match": {"type": "choice", "choice": "c1", "confidence": 0.87,
                           "probabilities": {"c1": 0.87, "NONE": 0.13}}
        }))
        .unwrap();
        assert_eq!(answers["c1_same_work"], RawAnswer::Noul { value: 0.91 });

        let validated =
            validate_response(&build_questions(&keys(1)).unwrap(), &identity(1), &answers).unwrap();
        assert_eq!(validated.same_work[0].same_work, 0.91);
        assert_eq!(validated.same_work[0].tmdb_id, 101);
    }

    /// Choice は公式の `choice` と必須の `confidence` から読む
    #[test]
    fn choice_official_wire_is_accepted() {
        let answers = parse_answers(&json!({
            "c1_same_work": {"type": "noul", "noul": 0.91},
            "best_match": {"type": "choice", "choice": "c1", "confidence": 0.87,
                           "probabilities": {"c1": 0.87, "NONE": 0.13}}
        }))
        .unwrap();
        assert_eq!(
            answers[BEST_MATCH_ID],
            RawAnswer::Choice {
                label: "c1".to_string(),
                probabilities: BTreeMap::from([
                    ("c1".to_string(), 0.87),
                    ("NONE".to_string(), 0.13)
                ]),
                confidence: 0.87,
            }
        );

        let validated =
            validate_response(&build_questions(&keys(1)).unwrap(), &identity(1), &answers).unwrap();
        assert_eq!(validated.best_match.unwrap().tmdb_id, 101);
        assert_eq!(validated.best_match_confidence, 0.87);
    }

    /// 旧フィールド名（`value` / `label`）は受け付けない
    #[test]
    fn legacy_field_names_are_rejected() {
        // Noul の value
        assert!(matches!(
            parse_answers(&json!({"c1_same_work": {"type": "noul", "value": 0.91}})),
            Err(JevContractError::Parse(_))
        ));
        // Noul の欠落
        assert!(matches!(
            parse_answers(&json!({"c1_same_work": {"type": "noul"}})),
            Err(JevContractError::Parse(_))
        ));
        // Choice の label
        assert!(matches!(
            parse_answers(&json!({"best_match": {"type": "choice", "label": "c1",
                                                 "confidence": 0.8,
                                                 "probabilities": {"c1": 1.0}}})),
            Err(JevContractError::Parse(_))
        ));
    }

    /// Choice の confidence は必須
    #[test]
    fn choice_without_confidence_is_rejected() {
        assert!(matches!(
            parse_answers(&json!({"best_match": {"type": "choice", "choice": "c1",
                                                 "probabilities": {"c1": 1.0}}})),
            Err(JevContractError::Parse(_))
        ));
        // 数値でない confidence
        assert!(matches!(
            parse_answers(&json!({"best_match": {"type": "choice", "choice": "c1",
                                                 "confidence": "high",
                                                 "probabilities": {"c1": 1.0}}})),
            Err(JevContractError::Parse(_))
        ));
        // 範囲外は検証で落ちる
        let answers = parse_answers(&json!({
            "c1_same_work": {"type": "noul", "noul": 0.5},
            "best_match": {"type": "choice", "choice": "c1", "confidence": 1.4,
                           "probabilities": {"c1": 0.9, "NONE": 0.1}}
        }))
        .unwrap();
        assert!(matches!(
            validate_response(&build_questions(&keys(1)).unwrap(), &identity(1), &answers),
            Err(JevContractError::ValueOutOfRange { .. })
        ));
    }

    /// Choice の probabilities は entry を1つも捨てずに読む（C3A.2 の回帰テスト）
    #[test]
    fn choice_probabilities_are_parsed_fail_closed() {
        let choice = |probabilities: Value| {
            let mut answer = Map::new();
            answer.insert("type".into(), json!("choice"));
            answer.insert("choice".into(), json!("c1"));
            answer.insert("confidence".into(), json!(0.8));
            if !probabilities.is_null() {
                answer.insert("probabilities".into(), probabilities);
            }
            parse_answers(&json!({ BEST_MATCH_ID: Value::Object(answer) }))
        };

        // 1. 正常
        let ok = choice(json!({"c1": 0.8, "NONE": 0.2})).unwrap();
        assert_eq!(
            ok[BEST_MATCH_ID],
            RawAnswer::Choice {
                label: "c1".to_string(),
                probabilities: BTreeMap::from([
                    ("c1".to_string(), 0.8),
                    ("NONE".to_string(), 0.2)
                ]),
                confidence: 0.8,
            }
        );

        // 2. 欠落
        assert!(matches!(choice(Value::Null), Err(JevContractError::Parse(_))));

        // 3. object 以外
        for not_object in [json!([0.8, 0.2]), json!(0.8), json!("0.8")] {
            assert!(
                matches!(choice(not_object.clone()), Err(JevContractError::Parse(_))),
                "{not_object} が通ってしまう"
            );
        }

        // 4. 想定キーの値が文字列
        assert!(matches!(
            choice(json!({"c1": "0.8", "NONE": 0.2})),
            Err(JevContractError::Parse(_))
        ));

        // 5. 余分なキー（値は数値）→ parse は通るが exact-key 検証で落ちる
        let extra_numeric = choice(json!({"c1": 0.8, "NONE": 0.2, "unexpected": 0.0})).unwrap();
        assert_eq!(
            extra_numeric[BEST_MATCH_ID],
            RawAnswer::Choice {
                label: "c1".to_string(),
                probabilities: BTreeMap::from([
                    ("c1".to_string(), 0.8),
                    ("NONE".to_string(), 0.2),
                    ("unexpected".to_string(), 0.0),
                ]),
                confidence: 0.8,
            },
            "余分なキーを黙って捨てない"
        );
        let questions = build_questions(&keys(1)).unwrap();
        let mut answers = extra_numeric;
        answers.insert("c1_same_work".to_string(), RawAnswer::Noul { value: 0.8 });
        assert!(matches!(
            validate_response(&questions, &identity(1), &answers),
            Err(JevContractError::ProbabilityKeysMismatch { .. })
        ));

        // 6. 余分なキーの値が文字列 → parse 段階で落とす（黙って消して素通りさせない）
        assert!(
            matches!(
                choice(json!({"c1": 0.8, "NONE": 0.2, "unexpected": "bad"})),
                Err(JevContractError::Parse(_))
            ),
            "数値でない probability entry が黙って消えている"
        );

        // 7. 想定キーだけ・すべて数値 → 検証まで通る
        let mut good = choice(json!({"c1": 0.8, "NONE": 0.2})).unwrap();
        good.insert("c1_same_work".to_string(), RawAnswer::Noul { value: 0.8 });
        let validated = validate_response(&questions, &identity(1), &good).unwrap();
        assert_eq!(validated.best_match.unwrap().tmdb_id, 101);
        assert_eq!(validated.best_match_probabilities.len(), 2);
    }

    /// 公式の answers map 以外は受け付けない
    #[test]
    fn only_the_official_answers_map_is_accepted() {
        // 配列形式（未確認の wire）
        assert!(matches!(
            parse_answers(&json!([
                {"id": "c1_same_work", "type": "noul", "noul": 0.9}
            ])),
            Err(JevContractError::Parse(_))
        ));
        assert!(matches!(parse_answers(&json!(5)), Err(JevContractError::Parse(_))));
        assert!(matches!(parse_answers(&json!("x")), Err(JevContractError::Parse(_))));

        // 未知の種別
        assert!(matches!(
            parse_answers(&json!({"x": {"type": "magic"}})),
            Err(JevContractError::Parse(_))
        ));
        // Score は v1 では受け付けない
        assert!(matches!(
            parse_answers(&json!({"x": {"type": "score", "score": 3, "confidence": 0.5}})),
            Err(JevContractError::Parse(_))
        ));
    }

    // ─── 応答の検証 ──────────────────────────────────────────────────────────

    #[test]
    fn valid_response_is_accepted_and_ids_come_from_identity() {
        let answers = parsed(2, "c2");
        let questions = build_questions(&keys(2)).unwrap();
        let validated = validate_response(&questions, &identity(2), &answers).unwrap();

        assert_eq!(validated.same_work.len(), 2);
        assert_eq!(validated.same_work[0].cand_key, "c1");
        assert_eq!(validated.same_work[0].tmdb_id, 101);
        assert_eq!(validated.same_work[0].same_work, 0.8);

        let best = validated.best_match.expect("c2 が選ばれる");
        assert_eq!(best.cand_key, "c2");
        assert_eq!(best.tmdb_id, 102, "TMDB ID は identity から引く");
        assert_eq!(validated.best_match_confidence, 0.7);
    }

    #[test]
    fn none_is_accepted_as_an_answer() {
        let questions = build_questions(&keys(3)).unwrap();
        let validated =
            validate_response(&questions, &identity(3), &parsed(3, NONE_OPTION)).unwrap();
        assert!(validated.best_match.is_none());
        assert_eq!(validated.same_work.len(), 3);
    }

    #[test]
    fn missing_or_extra_answers_are_invalid() {
        let questions = build_questions(&keys(2)).unwrap();

        let mut missing = parsed(2, "c1");
        missing.remove("c2_same_work");
        assert_eq!(
            validate_response(&questions, &identity(2), &missing),
            Err(JevContractError::MissingAnswer { question_id: "c2_same_work".to_string() })
        );

        let mut extra = parsed(2, "c1");
        extra.insert("c3_same_work".to_string(), RawAnswer::Noul { value: 0.5 });
        assert_eq!(
            validate_response(&questions, &identity(2), &extra),
            Err(JevContractError::UnknownAnswer { question_id: "c3_same_work".to_string() })
        );
    }

    #[test]
    fn kind_mismatch_is_invalid() {
        let questions = build_questions(&keys(1)).unwrap();
        let mut answers = parsed(1, "c1");
        answers.insert(
            "c1_same_work".to_string(),
            RawAnswer::Choice {
                label: "c1".to_string(),
                probabilities: BTreeMap::new(),
                confidence: 0.5,
            },
        );
        assert_eq!(
            validate_response(&questions, &identity(1), &answers),
            Err(JevContractError::KindMismatch {
                question_id: "c1_same_work".to_string(),
                expected: "noul".to_string(),
                found: "choice".to_string(),
            })
        );
    }

    #[test]
    fn noul_values_outside_zero_to_one_are_invalid() {
        let questions = build_questions(&keys(1)).unwrap();
        for value in [-0.1, 1.5, f64::NAN, f64::INFINITY] {
            let mut answers = parsed(1, "c1");
            answers.insert("c1_same_work".to_string(), RawAnswer::Noul { value });
            assert!(
                matches!(
                    validate_response(&questions, &identity(1), &answers),
                    Err(JevContractError::ValueOutOfRange { .. })
                ),
                "{value} が通ってしまう"
            );
        }
    }

    #[test]
    fn choice_label_and_probabilities_are_checked() {
        let questions = build_questions(&keys(2)).unwrap();

        // 送っていない選択肢
        let mut unknown_label = parsed(2, "c1");
        if let Some(RawAnswer::Choice { label, .. }) = unknown_label.get_mut(BEST_MATCH_ID) {
            *label = "c3".to_string();
        }
        assert!(matches!(
            validate_response(&questions, &identity(2), &unknown_label),
            Err(JevContractError::UnknownChoiceLabel { .. })
        ));

        // 確率のキーが選択肢と違う
        let mut bad_keys = parsed(2, "c1");
        if let Some(RawAnswer::Choice { probabilities, .. }) = bad_keys.get_mut(BEST_MATCH_ID) {
            probabilities.remove(NONE_OPTION);
        }
        assert!(matches!(
            validate_response(&questions, &identity(2), &bad_keys),
            Err(JevContractError::ProbabilityKeysMismatch { .. })
        ));

        // 確率の合計が大きくずれる
        let mut bad_sum = parsed(2, "c1");
        if let Some(RawAnswer::Choice { probabilities, .. }) = bad_sum.get_mut(BEST_MATCH_ID) {
            for value in probabilities.values_mut() {
                *value = 0.1;
            }
        }
        assert!(matches!(
            validate_response(&questions, &identity(2), &bad_sum),
            Err(JevContractError::ProbabilitySum { .. })
        ));

        // 確率が範囲外
        let mut bad_probability = parsed(2, "c1");
        if let Some(RawAnswer::Choice { probabilities, .. }) =
            bad_probability.get_mut(BEST_MATCH_ID)
        {
            if let Some(value) = probabilities.get_mut("c1") {
                *value = f64::NAN;
            }
        }
        assert!(matches!(
            validate_response(&questions, &identity(2), &bad_probability),
            Err(JevContractError::ValueOutOfRange { .. })
        ));
    }

    /// Jev が返した catalogue id は採用しない
    #[test]
    fn catalogue_ids_in_the_response_are_ignored() {
        let answers = parse_answers(&json!({
            "c1_same_work": {"type": "noul", "noul": 0.9},
            "best_match": {"type": "choice", "choice": "c1", "tmdb_id": 999999,
                           "confidence": 0.9,
                           "probabilities": {"c1": 0.9, "NONE": 0.1}}
        }))
        .unwrap();
        let questions = build_questions(&keys(1)).unwrap();
        let validated = validate_response(&questions, &identity(1), &answers).unwrap();
        assert_eq!(validated.best_match.unwrap().tmdb_id, 101);
    }
}
