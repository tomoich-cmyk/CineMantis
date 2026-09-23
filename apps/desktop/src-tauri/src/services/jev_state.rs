//! Jev へ送る state の組み立て（PR3 C2）。
//!
//! ここは**純粋な変換と検証だけ**を行う。DB も TMDB も TypeSafe も触らず、
//! 環境変数も読まない。HTTP 通信は C3 以降で別モジュールに作る。
//!
//! # 設計の要点
//!
//! - 送ってよい項目を **allowlist 型**として定義する。型にフィールドが無いことを
//!   第一防御にし、文字列の禁止パターン検査は二次防御（defense in depth）に留める。
//! - [`JevPayload`] とその子型は**完全所有型**（`String` / `Vec` / scalar / `Option` のみ）。
//!   lifetime parameter を持たず、`&PreMatchSnapshot` / `&LocalEvidence` / `&TmdbCandidate` /
//!   DB 接続 / パス / 遅延ローダを保持しない。
//!   これにより、materialize したあとで works や snapshot が変わっても payload は動かない
//!   （rules-safe の適用前に materialize しておけば target leakage が起きない）。
//! - 値が無い項目は**省略する**（`null` は出さない）。空の配列も省略する。

use serde::Serialize;

/// state の形式。allowlist か意味を変えたら上げる（023 の call 行に保存する）
pub const JEV_STATE_SCHEMA_VERSION: &str = "cm-jev-state-1";

// ─── 上限（TypeSafe の API 限界よりかなり小さく取る） ────────────────────────

/// 1回の判断に載せる候補の最大数
pub const MAX_CANDIDATES: usize = 3;
/// 送る出演者の人数
pub const MAX_CAST: usize = 10;
/// 版の表記の数
pub const MAX_EDITION_MARKERS: usize = 8;
/// 言語コードの数（音声・字幕それぞれ）
pub const MAX_LANGUAGES: usize = 8;
/// タイトル系の1つあたりの長さ
pub const MAX_TITLE_BYTES: usize = 512;
/// 人名1つあたりの長さ
pub const MAX_PERSON_BYTES: usize = 256;
/// 区分値（media_kind / country_type / evidence_class など）の長さ
pub const MAX_SHORT_FIELD_BYTES: usize = 64;
/// state 全体（JSON、UTF-8 バイト数）
pub const MAX_STATE_BYTES: usize = 32 * 1024;

/// `truncated_fields` に入れてよい固定パス（local 側）。
/// ユーザー入力が混ざらないよう、値はこの一覧と候補パターンだけに限る。
const TRUNCATABLE_LOCAL_FIELDS: [&str; 11] = [
    "local.derived_title",
    "local.embedded_title",
    "local.media_kind",
    "local.country_type",
    "local.part_hint",
    "local.evidence_class",
    "local.tag_provenance",
    "local.edition_markers",
    "local.audio_languages",
    "local.subtitle_languages",
    "local.cast",
];

/// 候補側の固定パス（`candidates.c1.title` など）
const TRUNCATABLE_CANDIDATE_SUFFIXES: [&str; 3] = ["title", "original_title", "original_language"];

/// `truncated_fields` の値がコード生成の固定パスであることを確かめる
fn is_known_field_path(path: &str) -> bool {
    if TRUNCATABLE_LOCAL_FIELDS.contains(&path) {
        return true;
    }
    let Some(rest) = path.strip_prefix("candidates.c") else {
        return false;
    };
    let Some((index, suffix)) = rest.split_once('.') else {
        return false;
    };
    index.parse::<usize>().is_ok_and(|n| n >= 1 && n <= MAX_CANDIDATES)
        && TRUNCATABLE_CANDIDATE_SUFFIXES.contains(&suffix)
}

// ─── 送り出す型（allowlist） ─────────────────────────────────────────────────

/// Jev へ送る state。materialize 後はこれだけで送信できる（再読込しない）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JevPayload {
    pub state_schema_version: String,
    pub local: JevLocalEvidence,
    pub candidates: Vec<JevCandidate>,
    /// 長さ・件数の上限で値を削った箇所（コードが生成する固定のフィールドパスだけ）。
    /// 削っていなければ省略する。Jev 側に「この項目は完全ではない」と伝えるために送る。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub truncated_fields: Vec<String>,
}

/// 照合前のローカル証拠。TMDB 由来の値は一切入らない
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct JevLocalEvidence {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derived_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedded_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename_year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedded_year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season_no: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_no: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_hint: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edition_markers: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub audio_languages: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subtitle_languages: Vec<String>,
    /// 構造化した人名だけ。タグの自由文（artist の生値）は送らない
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cast: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_provenance: Option<String>,
}

/// 候補1件。enrichment 前の基本項目だけ
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JevCandidate {
    /// c1 / c2 / c3。Jev はこの key でしか候補を指せない
    pub cand_key: String,
    pub tmdb_id: i64,
    /// "movie" | "tv"
    pub media_type: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
}

// ─── materialize の入力（これも allowlist） ──────────────────────────────────

/// materialize に渡すローカル証拠。
/// works / TMDB 由来の現在値（tmdb_id・imdb_id・適用後の title や year・poster など）を
/// 渡す口は**型として存在しない**。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LocalEvidenceInput {
    pub derived_title: Option<String>,
    pub embedded_title: Option<String>,
    pub filename_year: Option<i32>,
    pub embedded_year: Option<i32>,
    pub media_kind: Option<String>,
    pub country_type: Option<String>,
    pub season_no: Option<i32>,
    pub episode_no: Option<i32>,
    pub part_hint: Option<String>,
    pub edition_markers: Vec<String>,
    pub audio_languages: Vec<String>,
    pub subtitle_languages: Vec<String>,
    /// 構造化済みの人名（`ContainerTags::cast()` のような分割済みの値）
    pub cast: Vec<String>,
    pub evidence_class: Option<String>,
    pub tag_provenance: Option<String>,
}

/// materialize に渡す候補。cand_key は materialize 側で採番する
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateInput {
    pub tmdb_id: i64,
    pub media_type: String,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub original_language: Option<String>,
}

/// cand_key と TMDB 候補の対応。応答の key から候補を引き直すのに使う
/// （Jev が返した TMDB ID は採用しない）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CandidateIdentity {
    pub cand_key: String,
    pub tmdb_id: i64,
    pub media_type: String,
}

/// materialize の結果
#[derive(Debug, Clone, PartialEq)]
pub struct MaterializedState {
    pub payload: JevPayload,
    /// cand_key ↔ TMDB 候補
    pub identity: Vec<CandidateIdentity>,
    /// 長さ上限で切り詰めた項目（監査用。payload には入れない）
    pub truncated_fields: Vec<String>,
}

impl MaterializedState {
    /// 送信に使う JSON。検証を通ってからでないと得られない
    pub fn to_validated_json(&self) -> Result<String, JevStateError> {
        validate_outbound_state(&self.payload, &[])
    }

    pub fn identity_for(&self, cand_key: &str) -> Option<&CandidateIdentity> {
        self.identity.iter().find(|i| i.cand_key == cand_key)
    }
}

// ─── エラー ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JevStateError {
    NoCandidates,
    TooManyCandidates { count: usize },
    InvalidMediaType { value: String },
    EmptyTitle { cand_key: String },
    DuplicateCandidate { tmdb_id: i64, media_type: String },
    BadCandidateKey { expected: String, found: String },
    TooManyCast { count: usize },
    FieldTooLong { field: String, bytes: usize, limit: usize },
    StateTooLarge { bytes: usize, limit: usize },
    Serialization(String),
    /// 区分値が state schema の値域から外れている
    InvalidCategoricalValue { field: String, value: String },
    /// 禁止パターン（パス・URL・秘密など）が値やキーに現れた
    ForbiddenContent { pattern: String, location: String },
}

impl std::fmt::Display for JevStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JevStateError::NoCandidates => write!(f, "候補が1件もありません"),
            JevStateError::TooManyCandidates { count } => {
                write!(f, "候補が多すぎます（{count} 件、上限 {MAX_CANDIDATES}）")
            }
            JevStateError::InvalidMediaType { value } => write!(f, "不正な media_type です: {value}"),
            JevStateError::EmptyTitle { cand_key } => write!(f, "候補 {cand_key} のタイトルが空です"),
            JevStateError::DuplicateCandidate { tmdb_id, media_type } => {
                write!(f, "同じ候補が重複しています: {media_type}/{tmdb_id}")
            }
            JevStateError::BadCandidateKey { expected, found } => {
                write!(f, "候補キーが連番ではありません（期待 {expected}、実際 {found}）")
            }
            JevStateError::TooManyCast { count } => {
                write!(f, "出演者が多すぎます（{count} 名、上限 {MAX_CAST}）")
            }
            JevStateError::FieldTooLong { field, bytes, limit } => {
                write!(f, "{field} が長すぎます（{bytes} バイト、上限 {limit}）")
            }
            JevStateError::StateTooLarge { bytes, limit } => {
                write!(f, "state が大きすぎます（{bytes} バイト、上限 {limit}）")
            }
            JevStateError::Serialization(message) => write!(f, "JSON にできません: {message}"),
            JevStateError::InvalidCategoricalValue { field, value } => {
                write!(f, "{field} に使えない値です: {value}")
            }
            JevStateError::ForbiddenContent { pattern, location } => {
                write!(f, "送ってはいけない内容が含まれています（{pattern} / {location}）")
            }
        }
    }
}

impl std::error::Error for JevStateError {}

// ─── materialize ─────────────────────────────────────────────────────────────

/// 送信内容を確定させる。
///
/// **rules-safe が works を更新する前に呼ぶこと。** 戻り値は完全所有型なので、
/// このあと DB や snapshot が変わっても payload は変化しない。
///
/// 正規化の方針:
/// - 文字列は trim し、空なら無いものとして扱う
/// - 出演者は trim → 空除去 → 重複除去（入力順を維持）→ 先頭 [`MAX_CAST`] 名
/// - 長さ上限を超える個々の文字列は**文字境界で切り詰め**、`truncated_fields` に記録する
///   （長い題名の作品でも shadow 判定を捨てないため）
/// - 件数・重複・state 全体の大きさなど、**黙って送ると危ないもの**は切り詰めずにエラーにする
pub fn materialize(
    local: LocalEvidenceInput,
    candidates: Vec<CandidateInput>,
) -> Result<MaterializedState, JevStateError> {
    if candidates.is_empty() {
        return Err(JevStateError::NoCandidates);
    }
    if candidates.len() > MAX_CANDIDATES {
        return Err(JevStateError::TooManyCandidates { count: candidates.len() });
    }

    let mut truncated = Vec::new();

    let local_evidence = JevLocalEvidence {
        derived_title: clean_field(local.derived_title, "local.derived_title", MAX_TITLE_BYTES, &mut truncated),
        embedded_title: clean_field(local.embedded_title, "local.embedded_title", MAX_TITLE_BYTES, &mut truncated),
        filename_year: local.filename_year,
        embedded_year: local.embedded_year,
        media_kind: clean_field(local.media_kind, "local.media_kind", MAX_SHORT_FIELD_BYTES, &mut truncated),
        country_type: clean_field(local.country_type, "local.country_type", MAX_SHORT_FIELD_BYTES, &mut truncated),
        season_no: local.season_no,
        episode_no: local.episode_no,
        part_hint: clean_field(local.part_hint, "local.part_hint", MAX_SHORT_FIELD_BYTES, &mut truncated),
        edition_markers: clean_list(
            local.edition_markers,
            "local.edition_markers",
            MAX_EDITION_MARKERS,
            MAX_SHORT_FIELD_BYTES,
            &mut truncated,
        ),
        audio_languages: clean_list(
            local.audio_languages,
            "local.audio_languages",
            MAX_LANGUAGES,
            MAX_SHORT_FIELD_BYTES,
            &mut truncated,
        ),
        subtitle_languages: clean_list(
            local.subtitle_languages,
            "local.subtitle_languages",
            MAX_LANGUAGES,
            MAX_SHORT_FIELD_BYTES,
            &mut truncated,
        ),
        cast: clean_list(local.cast, "local.cast", MAX_CAST, MAX_PERSON_BYTES, &mut truncated),
        evidence_class: clean_field(
            local.evidence_class,
            "local.evidence_class",
            MAX_SHORT_FIELD_BYTES,
            &mut truncated,
        ),
        tag_provenance: clean_field(
            local.tag_provenance,
            "local.tag_provenance",
            MAX_SHORT_FIELD_BYTES,
            &mut truncated,
        ),
    };

    let mut built: Vec<JevCandidate> = Vec::with_capacity(candidates.len());
    let mut identity: Vec<CandidateIdentity> = Vec::with_capacity(candidates.len());
    for (index, candidate) in candidates.into_iter().enumerate() {
        let cand_key = format!("c{}", index + 1);
        if !MEDIA_TYPE_VALUES.contains(&candidate.media_type.as_str()) {
            return Err(JevStateError::InvalidMediaType { value: candidate.media_type });
        }
        if built
            .iter()
            .any(|c| c.tmdb_id == candidate.tmdb_id && c.media_type == candidate.media_type)
        {
            return Err(JevStateError::DuplicateCandidate {
                tmdb_id: candidate.tmdb_id,
                media_type: candidate.media_type,
            });
        }
        let title = clean_field(
            Some(candidate.title),
            &format!("candidates.{cand_key}.title"),
            MAX_TITLE_BYTES,
            &mut truncated,
        )
        .ok_or_else(|| JevStateError::EmptyTitle { cand_key: cand_key.clone() })?;

        let cand_key_for_fields = cand_key.clone();
        identity.push(CandidateIdentity {
            cand_key: cand_key.clone(),
            tmdb_id: candidate.tmdb_id,
            media_type: candidate.media_type.clone(),
        });
        built.push(JevCandidate {
            cand_key,
            tmdb_id: candidate.tmdb_id,
            media_type: candidate.media_type,
            title,
            original_title: clean_field(
                candidate.original_title,
                &format!("candidates.{cand_key_for_fields}.original_title"),
                MAX_TITLE_BYTES,
                &mut truncated,
            ),
            year: candidate.year,
            original_language: clean_field(
                candidate.original_language,
                &format!("candidates.{cand_key_for_fields}.original_language"),
                MAX_SHORT_FIELD_BYTES,
                &mut truncated,
            ),
        });
    }

    let payload = JevPayload {
        state_schema_version: JEV_STATE_SCHEMA_VERSION.to_string(),
        local: local_evidence,
        candidates: built,
        truncated_fields: truncated.clone(),
    };
    // 組み立てた時点で検証も通しておく（通らない payload を返さない）
    validate_outbound_state(&payload, &[])?;

    Ok(MaterializedState { payload, identity, truncated_fields: truncated })
}

fn clean_field(
    value: Option<String>,
    field: &str,
    limit: usize,
    truncated: &mut Vec<String>,
) -> Option<String> {
    let value = value?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(cut_to_limit(trimmed, field, limit, truncated))
}

fn clean_list(
    values: Vec<String>,
    field: &str,
    max_items: usize,
    limit: usize,
    truncated: &mut Vec<String>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value = cut_to_limit(trimmed, field, limit, truncated);
        if out.contains(&value) {
            continue;
        }
        if out.len() >= max_items {
            // 件数の上限で落とした。落とした事実を state にも残す
            mark_truncated(truncated, field);
            break;
        }
        out.push(value);
    }
    out
}

fn mark_truncated(truncated: &mut Vec<String>, field: &str) {
    debug_assert!(is_known_field_path(field), "固定のフィールドパスだけを記録する: {field}");
    if !truncated.iter().any(|f| f == field) {
        truncated.push(field.to_string());
    }
}

/// 文字境界を壊さずに上限まで切り詰める
fn cut_to_limit(value: &str, field: &str, limit: usize, truncated: &mut Vec<String>) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    mark_truncated(truncated, field);
    value[..end].trim_end().to_string()
}

// ─── 送信前の検証 ────────────────────────────────────────────────────────────

/// 送信直前に呼ぶ検証。通れば送ってよい JSON を返す。
///
/// `forbidden_extra` には、呼び出し側だけが知っている文字列（API キー、ログイン名など）を
/// 渡せる。ここでは環境変数を読まない。
pub fn validate_outbound_state(
    payload: &JevPayload,
    forbidden_extra: &[&str],
) -> Result<String, JevStateError> {
    // 候補の件数と並び
    if payload.candidates.is_empty() {
        return Err(JevStateError::NoCandidates);
    }
    if payload.candidates.len() > MAX_CANDIDATES {
        return Err(JevStateError::TooManyCandidates { count: payload.candidates.len() });
    }
    let mut seen: Vec<(i64, &str)> = Vec::new();
    for (index, candidate) in payload.candidates.iter().enumerate() {
        let expected = format!("c{}", index + 1);
        if candidate.cand_key != expected {
            return Err(JevStateError::BadCandidateKey {
                expected,
                found: candidate.cand_key.clone(),
            });
        }
        if !MEDIA_TYPE_VALUES.contains(&candidate.media_type.as_str()) {
            return Err(JevStateError::InvalidMediaType {
                value: candidate.media_type.clone(),
            });
        }
        if candidate.title.trim().is_empty() {
            return Err(JevStateError::EmptyTitle { cand_key: candidate.cand_key.clone() });
        }
        if seen
            .iter()
            .any(|(id, media)| *id == candidate.tmdb_id && *media == candidate.media_type)
        {
            return Err(JevStateError::DuplicateCandidate {
                tmdb_id: candidate.tmdb_id,
                media_type: candidate.media_type.clone(),
            });
        }
        seen.push((candidate.tmdb_id, &candidate.media_type));
    }

    // 区分値の値域
    check_categorical("local.media_kind", &payload.local.media_kind, &MEDIA_KIND_VALUES)?;
    check_categorical("local.country_type", &payload.local.country_type, &COUNTRY_TYPE_VALUES)?;
    check_categorical(
        "local.evidence_class",
        &payload.local.evidence_class,
        &EVIDENCE_CLASS_VALUES,
    )?;
    check_categorical(
        "local.tag_provenance",
        &payload.local.tag_provenance,
        &TAG_PROVENANCE_VALUES,
    )?;

    // truncated_fields はコードが生成した固定パスだけ（ユーザー入力を混ぜない）
    for path in &payload.truncated_fields {
        if !is_known_field_path(path) {
            return Err(JevStateError::ForbiddenContent {
                pattern: "unknown truncated field path".to_string(),
                location: "truncated_fields".to_string(),
            });
        }
    }

    // 件数の上限
    if payload.local.cast.len() > MAX_CAST {
        return Err(JevStateError::TooManyCast { count: payload.local.cast.len() });
    }
    check_len("local.edition_markers", payload.local.edition_markers.len(), MAX_EDITION_MARKERS)?;
    check_len("local.audio_languages", payload.local.audio_languages.len(), MAX_LANGUAGES)?;
    check_len("local.subtitle_languages", payload.local.subtitle_languages.len(), MAX_LANGUAGES)?;

    // 文字列の長さ
    for (field, value, limit) in string_fields(payload) {
        if value.len() > limit {
            return Err(JevStateError::FieldTooLong {
                field,
                bytes: value.len(),
                limit,
            });
        }
    }

    // JSON 化と全体サイズ
    let json = serde_json::to_string(payload)
        .map_err(|e| JevStateError::Serialization(e.to_string()))?;
    if json.len() > MAX_STATE_BYTES {
        return Err(JevStateError::StateTooLarge {
            bytes: json.len(),
            limit: MAX_STATE_BYTES,
        });
    }

    // 二次防御: 値とキー名の禁止パターン
    scan_forbidden(payload, &json, forbidden_extra)?;
    Ok(json)
}

fn check_len(field: &str, count: usize, limit: usize) -> Result<(), JevStateError> {
    if count > limit {
        return Err(JevStateError::FieldTooLong {
            field: field.to_string(),
            bytes: count,
            limit,
        });
    }
    Ok(())
}

/// 検証対象の文字列（フィールド名, 値, 上限）
fn string_fields(payload: &JevPayload) -> Vec<(String, &str, usize)> {
    let mut fields: Vec<(String, &str, usize)> = Vec::new();
    let local = &payload.local;
    for (name, value, limit) in [
        ("local.derived_title", &local.derived_title, MAX_TITLE_BYTES),
        ("local.embedded_title", &local.embedded_title, MAX_TITLE_BYTES),
        ("local.media_kind", &local.media_kind, MAX_SHORT_FIELD_BYTES),
        ("local.country_type", &local.country_type, MAX_SHORT_FIELD_BYTES),
        ("local.part_hint", &local.part_hint, MAX_SHORT_FIELD_BYTES),
        ("local.evidence_class", &local.evidence_class, MAX_SHORT_FIELD_BYTES),
        ("local.tag_provenance", &local.tag_provenance, MAX_SHORT_FIELD_BYTES),
    ] {
        if let Some(value) = value {
            fields.push((name.to_string(), value.as_str(), limit));
        }
    }
    for value in &local.edition_markers {
        fields.push(("local.edition_markers".to_string(), value, MAX_SHORT_FIELD_BYTES));
    }
    for value in &local.audio_languages {
        fields.push(("local.audio_languages".to_string(), value, MAX_SHORT_FIELD_BYTES));
    }
    for value in &local.subtitle_languages {
        fields.push(("local.subtitle_languages".to_string(), value, MAX_SHORT_FIELD_BYTES));
    }
    for value in &local.cast {
        fields.push(("local.cast".to_string(), value, MAX_PERSON_BYTES));
    }
    for candidate in &payload.candidates {
        fields.push((format!("candidates.{}.title", candidate.cand_key), &candidate.title, MAX_TITLE_BYTES));
        if let Some(value) = &candidate.original_title {
            fields.push((
                format!("candidates.{}.original_title", candidate.cand_key),
                value,
                MAX_TITLE_BYTES,
            ));
        }
        if let Some(value) = &candidate.original_language {
            fields.push((
                format!("candidates.{}.original_language", candidate.cand_key),
                value,
                MAX_SHORT_FIELD_BYTES,
            ));
        }
        fields.push((
            format!("candidates.{}.media_type", candidate.cand_key),
            &candidate.media_type,
            MAX_SHORT_FIELD_BYTES,
        ));
        fields.push((
            format!("candidates.{}.cand_key", candidate.cand_key),
            &candidate.cand_key,
            MAX_SHORT_FIELD_BYTES,
        ));
    }
    fields.push((
        "state_schema_version".to_string(),
        &payload.state_schema_version,
        MAX_SHORT_FIELD_BYTES,
    ));
    fields
}

/// 区分値の値域（state schema の一部。変えるときは JEV_STATE_SCHEMA_VERSION を上げる）
const MEDIA_KIND_VALUES: [&str; 3] = ["movie", "tv", "unknown"];
const COUNTRY_TYPE_VALUES: [&str; 3] = ["domestic", "foreign", "unknown"];
const EVIDENCE_CLASS_VALUES: [&str; 3] = ["live", "reconstructed_clean", "historical_audit_only"];
const TAG_PROVENANCE_VALUES: [&str; 4] =
    ["provider_known", "pipeline_known", "unknown", "suspicious"];
const MEDIA_TYPE_VALUES: [&str; 2] = ["movie", "tv"];

fn check_categorical(
    field: &str,
    value: &Option<String>,
    allowed: &[&str],
) -> Result<(), JevStateError> {
    let Some(value) = value else {
        return Ok(());
    };
    if allowed.contains(&value.as_str()) {
        return Ok(());
    }
    Err(JevStateError::InvalidCategoricalValue {
        field: field.to_string(),
        value: value.clone(),
    })
}

/// JSON のキー名に現れてはいけない語（型の作り間違いを検知する）
const FORBIDDEN_KEYS: [&str; 12] = [
    "file_path",
    "original_file_name",
    "original_rel_path",
    "source_root",
    "provider_url",
    "provider_hint",
    "api_key",
    "apikey",
    "typesafe_api_key",
    "poster_path",
    "imdb_id",
    "overview",
];

fn scan_forbidden(
    payload: &JevPayload,
    json: &str,
    forbidden_extra: &[&str],
) -> Result<(), JevStateError> {
    let lower_json = json.to_lowercase();
    for key in FORBIDDEN_KEYS {
        if lower_json.contains(&format!("\"{key}\"")) {
            return Err(JevStateError::ForbiddenContent {
                pattern: key.to_string(),
                location: "json key".to_string(),
            });
        }
    }
    // 呼び出し側が渡した秘密（API キー・ログイン名など）。
    // **値そのものはエラーにも Debug にもログにも出さない。**
    // 空文字・空白だけの値は「何にでも一致する」ため無視する。
    for secret in forbidden_extra {
        if secret.trim().is_empty() {
            continue;
        }
        if json.contains(secret) {
            return Err(JevStateError::ForbiddenContent {
                pattern: "forbidden_extra value matched".to_string(),
                location: "state".to_string(),
            });
        }
    }
    // 値そのものを見る（JSON のエスケープに左右されないよう、値で検査する）
    for (field, value, _) in string_fields(payload) {
        if let Some(pattern) = path_like(value) {
            return Err(JevStateError::ForbiddenContent {
                pattern: pattern.to_string(),
                location: field,
            });
        }
    }
    Ok(())
}

/// パス・URL らしさの判定。
/// 日本語タイトルによくある `/` や `／`（「OSLO / オスロ」など）は誤検知しない。
fn path_like(value: &str) -> Option<&'static str> {
    if value.contains('\\') {
        // Windows のパス区切り。作品名に入ることは無い
        return Some("backslash");
    }
    if value.contains("://") {
        return Some("url scheme");
    }
    let bytes = value.as_bytes();
    // ドライブレター（C:\ / C:/）
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/' {
        return Some("drive letter");
    }
    const UNIX_ROOTS: [&str; 6] = ["/home/", "/users/", "/mnt/", "/media/", "/var/", "/etc/"];
    let lower = value.to_lowercase();
    if UNIX_ROOTS.iter().any(|root| lower.starts_with(root)) {
        return Some("unix path");
    }
    if lower.starts_with("//") {
        return Some("unc path");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(tmdb_id: i64, title: &str) -> CandidateInput {
        CandidateInput {
            tmdb_id,
            media_type: "movie".to_string(),
            title: title.to_string(),
            original_title: None,
            year: Some(2019),
            original_language: Some("da".to_string()),
        }
    }

    fn full_local() -> LocalEvidenceInput {
        LocalEvidenceInput {
            derived_title: Some("THE GUILTY/ギルティ".to_string()),
            embedded_title: Some("THE GUILTY/ギルティ".to_string()),
            filename_year: None,
            embedded_year: Some(2019),
            media_kind: Some("unknown".to_string()),
            country_type: Some("foreign".to_string()),
            season_no: None,
            episode_no: None,
            part_hint: None,
            edition_markers: vec!["最終章".to_string()],
            audio_languages: vec!["dan".to_string()],
            subtitle_languages: vec!["jpn".to_string()],
            cast: vec!["ヤコブ・セーダーグレン".to_string(), "イェシカ・ディナウエ".to_string()],
            evidence_class: Some("live".to_string()),
            tag_provenance: Some("pipeline_known".to_string()),
        }
    }

    #[test]
    fn minimal_state_is_valid() {
        let state = materialize(LocalEvidenceInput::default(), vec![candidate(1, "Alien")]).unwrap();
        let json = state.to_validated_json().unwrap();
        // 値の無い項目は省略する（null を出さない）
        assert!(!json.contains("null"), "{json}");
        assert!(json.contains("\"state_schema_version\":\"cm-jev-state-1\""));
        assert!(json.contains("\"cand_key\":\"c1\""));
        assert_eq!(state.identity.len(), 1);
    }

    /// 期待 JSON を1件固定する
    #[test]
    fn full_state_matches_the_expected_json() {
        let state = materialize(full_local(), vec![candidate(475888, "THE GUILTY/ギルティ")]).unwrap();
        let json = state.to_validated_json().unwrap();
        let expected = concat!(
            r#"{"state_schema_version":"cm-jev-state-1","#,
            r#""local":{"derived_title":"THE GUILTY/ギルティ","embedded_title":"THE GUILTY/ギルティ","#,
            r#""embedded_year":2019,"media_kind":"unknown","country_type":"foreign","#,
            r#""edition_markers":["最終章"],"audio_languages":["dan"],"subtitle_languages":["jpn"],"#,
            r#""cast":["ヤコブ・セーダーグレン","イェシカ・ディナウエ"],"#,
            r#""evidence_class":"live","tag_provenance":"pipeline_known"},"#,
            r#""candidates":[{"cand_key":"c1","tmdb_id":475888,"media_type":"movie","#,
            r#""title":"THE GUILTY/ギルティ","year":2019,"original_language":"da"}]}"#
        );
        assert_eq!(json, expected);
    }

    #[test]
    fn three_candidates_are_allowed_and_four_are_rejected() {
        let three = materialize(
            LocalEvidenceInput::default(),
            vec![candidate(1, "A"), candidate(2, "B"), candidate(3, "C")],
        )
        .unwrap();
        assert_eq!(
            three.payload.candidates.iter().map(|c| c.cand_key.as_str()).collect::<Vec<_>>(),
            vec!["c1", "c2", "c3"]
        );

        let four = materialize(
            LocalEvidenceInput::default(),
            vec![candidate(1, "A"), candidate(2, "B"), candidate(3, "C"), candidate(4, "D")],
        );
        assert_eq!(four, Err(JevStateError::TooManyCandidates { count: 4 }));
        assert_eq!(
            materialize(LocalEvidenceInput::default(), vec![]),
            Err(JevStateError::NoCandidates)
        );
    }

    #[test]
    fn candidate_keys_must_be_sequential() {
        let mut state = materialize(
            LocalEvidenceInput::default(),
            vec![candidate(1, "A"), candidate(2, "B")],
        )
        .unwrap();
        state.payload.candidates[1].cand_key = "c9".to_string();
        assert_eq!(
            validate_outbound_state(&state.payload, &[]),
            Err(JevStateError::BadCandidateKey {
                expected: "c2".to_string(),
                found: "c9".to_string()
            })
        );
    }

    #[test]
    fn duplicate_candidates_are_rejected() {
        let duplicate = materialize(
            LocalEvidenceInput::default(),
            vec![candidate(7, "A"), candidate(7, "A の別表記")],
        );
        assert_eq!(
            duplicate,
            Err(JevStateError::DuplicateCandidate { tmdb_id: 7, media_type: "movie".to_string() })
        );

        // media_type が違えば別候補
        let mut tv = candidate(7, "A");
        tv.media_type = "tv".to_string();
        assert!(materialize(LocalEvidenceInput::default(), vec![candidate(7, "A"), tv]).is_ok());

        // 不正な media_type
        let mut bad = candidate(8, "B");
        bad.media_type = "anime".to_string();
        assert_eq!(
            materialize(LocalEvidenceInput::default(), vec![bad]),
            Err(JevStateError::InvalidMediaType { value: "anime".to_string() })
        );
    }

    #[test]
    fn cast_is_trimmed_deduplicated_and_capped() {
        let mut local = LocalEvidenceInput::default();
        local.cast = vec![
            "  ロビン・ウィリアムズ  ".to_string(),
            "".to_string(),
            "   ".to_string(),
            "ロビン・ウィリアムズ".to_string(),
        ];
        for i in 0..20 {
            local.cast.push(format!("俳優{i}"));
        }
        let state = materialize(local, vec![candidate(1, "A")]).unwrap();
        let cast = &state.payload.local.cast;
        assert_eq!(cast.len(), MAX_CAST);
        assert_eq!(cast[0], "ロビン・ウィリアムズ");
        assert_eq!(cast.iter().filter(|n| *n == "ロビン・ウィリアムズ").count(), 1);
        assert!(!cast.iter().any(|n| n.is_empty()));
        // 入力順を保つ
        assert_eq!(cast[1], "俳優0");

        // 11名以上の payload は検証で弾く
        let mut payload = state.payload.clone();
        payload.local.cast.push("溢れた人".to_string());
        assert_eq!(
            validate_outbound_state(&payload, &[]),
            Err(JevStateError::TooManyCast { count: 11 })
        );
    }

    #[test]
    fn oversized_strings_are_cut_at_char_boundaries() {
        let long_title = "あ".repeat(400); // 1200 バイト
        let mut local = LocalEvidenceInput::default();
        local.derived_title = Some(long_title.clone());
        local.cast = vec!["ん".repeat(200)];

        let state = materialize(local, vec![candidate(1, &long_title)]).unwrap();
        let title = state.payload.local.derived_title.as_ref().unwrap();
        assert!(title.len() <= MAX_TITLE_BYTES);
        assert!(title.chars().all(|c| c == 'あ'), "文字が壊れていない");
        assert!(state.payload.local.cast[0].len() <= MAX_PERSON_BYTES);
        assert!(state.truncated_fields.contains(&"local.derived_title".to_string()));
        assert!(state.truncated_fields.contains(&"local.cast".to_string()));
        // 切り詰めた結果は検証を通る
        state.to_validated_json().unwrap();
    }

    #[test]
    fn oversized_state_is_rejected_loudly() {
        let mut state =
            materialize(full_local(), vec![candidate(1, "A")]).unwrap();
        // 検証をすり抜けた巨大な値を直接入れて、サイズで弾かれることを確かめる
        state.payload.local.derived_title = Some("x".repeat(MAX_STATE_BYTES + 10));
        match validate_outbound_state(&state.payload, &[]) {
            Err(JevStateError::FieldTooLong { field, .. }) => {
                assert_eq!(field, "local.derived_title");
            }
            other => panic!("長さで弾かれること: {other:?}"),
        }

        // 個々の上限は満たすが全体が大きすぎる場合
        let mut payload = state.payload.clone();
        payload.local.derived_title = Some("x".repeat(MAX_TITLE_BYTES));
        payload.local.cast = (0..MAX_CAST).map(|_| "y".repeat(MAX_PERSON_BYTES)).collect();
        payload.candidates = vec![JevCandidate {
            cand_key: "c1".to_string(),
            tmdb_id: 1,
            media_type: "movie".to_string(),
            title: "z".repeat(MAX_TITLE_BYTES),
            original_title: Some("z".repeat(MAX_TITLE_BYTES)),
            year: None,
            original_language: None,
        }];
        // ここまでで約 4 KB。上限を小さく見積もらないよう、実際の判定だけ確認する
        assert!(validate_outbound_state(&payload, &[]).is_ok());
    }

    #[test]
    fn windows_paths_never_reach_the_payload() {
        let mut local = LocalEvidenceInput::default();
        local.derived_title = Some(r"C:\Users\tyama\Movies\Alien.mkv".to_string());
        let state = materialize(local, vec![candidate(1, "A")]);
        assert!(matches!(state, Err(JevStateError::ForbiddenContent { .. })), "{state:?}");
    }

    #[test]
    fn unix_and_unc_paths_never_reach_the_payload() {
        for path in [
            "/home/tyama/movies/alien.mkv",
            "/Users/tyama/Movies/alien.mkv",
            "//TYNAS/home/Movie/x.mp4",
        ] {
            let mut local = LocalEvidenceInput::default();
            local.derived_title = Some(path.to_string());
            assert!(
                matches!(
                    materialize(local, vec![candidate(1, "A")]),
                    Err(JevStateError::ForbiddenContent { .. })
                ),
                "{path} が通ってしまう"
            );
        }
    }

    #[test]
    fn provider_urls_and_secrets_are_rejected() {
        let mut local = LocalEvidenceInput::default();
        local.derived_title = Some("https://www.disneyplus.com/ja-jp/movies/x".to_string());
        assert!(matches!(
            materialize(local, vec![candidate(1, "A")]),
            Err(JevStateError::ForbiddenContent { .. })
        ));

        // 呼び出し側が知っている秘密（API キーなど）も弾ける
        let state = materialize(full_local(), vec![candidate(1, "A")]).unwrap();
        assert!(validate_outbound_state(&state.payload, &["cm-jev-state-1"]).is_err());
        assert!(validate_outbound_state(&state.payload, &["sk-secret-value"]).is_ok());
    }

    /// 送ってはいけない項目が JSON に「キーとしても値としても」現れない
    #[test]
    fn denied_fields_are_absent_from_the_payload() {
        let state = materialize(full_local(), vec![candidate(475888, "THE GUILTY/ギルティ")]).unwrap();
        let json = state.to_validated_json().unwrap().to_lowercase();
        for denied in [
            "file_path",
            "original_file_name",
            "original_rel_path",
            "source_root",
            "provider_url",
            "provider_hint",
            "comment",
            "description",
            "synopsis",
            "overview",
            "api_key",
            "typesafe",
            "imdb",
            "poster",
            "popularity",
            "vote_average",
            "vote_count",
            "raw_filename",
            "filename",
            "path",
            "u-next",
            "streamfab",
        ] {
            assert!(!json.contains(denied), "{denied} が payload に出ている: {json}");
        }
    }

    /// works の現在値（tmdb_id / imdb_id など）を渡す口が型として無いこと。
    /// 入力型のフィールドを列挙して、想定どおりであることを固定する。
    #[test]
    fn input_types_have_no_field_for_current_works_metadata() {
        // LocalEvidenceInput を「全フィールド明示」で構築する。
        // works 由来の項目を足したらここがコンパイルエラーになる。
        let LocalEvidenceInput {
            derived_title: _,
            embedded_title: _,
            filename_year: _,
            embedded_year: _,
            media_kind: _,
            country_type: _,
            season_no: _,
            episode_no: _,
            part_hint: _,
            edition_markers: _,
            audio_languages: _,
            subtitle_languages: _,
            cast: _,
            evidence_class: _,
            tag_provenance: _,
        } = LocalEvidenceInput::default();

        let CandidateInput {
            tmdb_id: _,
            media_type: _,
            title: _,
            original_title: _,
            year: _,
            original_language: _,
        } = candidate(1, "A");
    }

    /// materialize 後に元の入力を変えても payload は変わらない（完全所有型）
    #[test]
    fn payload_is_immutable_after_materialize() {
        let mut local = full_local();
        let candidates = vec![candidate(1, "Alien")];
        let state = materialize(local.clone(), candidates.clone()).unwrap();
        let before = state.to_validated_json().unwrap();

        // 元の入力を書き換える（apply 後に works / snapshot が変わる状況を模す）
        local.derived_title = Some("TMDB 由来のタイトル".to_string());
        local.embedded_year = Some(1999);
        local.cast.clear();
        let mut changed = candidates;
        changed[0].title = "書き換わった".to_string();

        let after = state.to_validated_json().unwrap();
        assert_eq!(before, after);
        assert!(after.contains("THE GUILTY/ギルティ"));
        assert!(!after.contains("TMDB 由来のタイトル"));
    }

    /// 日本語タイトルの `/` をパスと誤判定しない
    #[test]
    fn japanese_titles_with_slashes_are_allowed() {
        for title in [
            "OSLO / オスロ",
            "REC/レック2",
            "THE GUILTY／ギルティ",
            "ANNA／アナ",
            "7/11",
        ] {
            let mut local = LocalEvidenceInput::default();
            local.derived_title = Some(title.to_string());
            let state = materialize(local, vec![candidate(1, title)])
                .unwrap_or_else(|e| panic!("{title} が弾かれた: {e}"));
            assert!(state.to_validated_json().unwrap().contains(title));
        }
    }

    #[test]
    fn serialization_is_stable_across_validation() {
        let state = materialize(full_local(), vec![candidate(1, "A"), candidate(2, "B")]).unwrap();
        let first = validate_outbound_state(&state.payload, &[]).unwrap();
        let second = validate_outbound_state(&state.payload, &[]).unwrap();
        assert_eq!(first, second);

        // JSON として読み戻しても中身が同じ（serde_json::Value はキー順を並べ替えるので、
        // 文字列ではなく値として比べる）
        let parsed: serde_json::Value = serde_json::from_str(&first).unwrap();
        let round_tripped: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&parsed).unwrap()).unwrap();
        assert_eq!(parsed, round_tripped);
        assert_eq!(parsed["candidates"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["state_schema_version"], JEV_STATE_SCHEMA_VERSION);
    }

    #[test]
    fn candidate_identity_maps_keys_back_to_tmdb_candidates() {
        let mut tv = candidate(99, "TV");
        tv.media_type = "tv".to_string();
        let state =
            materialize(LocalEvidenceInput::default(), vec![candidate(1, "A"), tv]).unwrap();

        assert_eq!(
            state.identity,
            vec![
                CandidateIdentity {
                    cand_key: "c1".to_string(),
                    tmdb_id: 1,
                    media_type: "movie".to_string()
                },
                CandidateIdentity {
                    cand_key: "c2".to_string(),
                    tmdb_id: 99,
                    media_type: "tv".to_string()
                },
            ]
        );
        assert_eq!(state.identity_for("c2").unwrap().tmdb_id, 99);
        assert!(state.identity_for("c3").is_none());
    }

    // ─── truncation の記録 ───────────────────────────────────────────────────

    #[test]
    fn truncation_is_recorded_in_the_payload() {
        // タイトルの切り詰め
        let mut local = LocalEvidenceInput::default();
        local.derived_title = Some("あ".repeat(400));
        let state = materialize(local, vec![candidate(1, &"い".repeat(400))]).unwrap();
        assert!(state.payload.truncated_fields.contains(&"local.derived_title".to_string()));
        assert!(state.payload.truncated_fields.contains(&"candidates.c1.title".to_string()));
        let json = state.to_validated_json().unwrap();
        assert!(json.contains("\"truncated_fields\":[\"local.derived_title\",\"candidates.c1.title\"]"), "{json}");

        // 件数の cap も記録する
        let mut local = LocalEvidenceInput::default();
        local.cast = (0..11).map(|i| format!("俳優{i}")).collect();
        local.edition_markers = (0..9).map(|i| format!("版{i}")).collect();
        local.audio_languages = (0..9).map(|i| format!("l{i}")).collect();
        local.subtitle_languages = (0..9).map(|i| format!("s{i}")).collect();
        let state = materialize(local, vec![candidate(1, "A")]).unwrap();
        for field in [
            "local.cast",
            "local.edition_markers",
            "local.audio_languages",
            "local.subtitle_languages",
        ] {
            assert!(
                state.payload.truncated_fields.contains(&field.to_string()),
                "{field} が記録されていない: {:?}",
                state.payload.truncated_fields
            );
        }
        assert_eq!(state.payload.local.cast.len(), MAX_CAST);
        assert_eq!(state.payload.local.edition_markers.len(), MAX_EDITION_MARKERS);
    }

    #[test]
    fn payload_omits_truncated_fields_when_nothing_was_cut() {
        let state = materialize(full_local(), vec![candidate(1, "A")]).unwrap();
        assert!(state.payload.truncated_fields.is_empty());
        assert!(!state.to_validated_json().unwrap().contains("truncated_fields"));
    }

    /// truncated_fields にはコードが作る固定パスしか入らない
    #[test]
    fn truncated_fields_never_contain_user_input() {
        let mut local = LocalEvidenceInput::default();
        local.derived_title = Some("あ".repeat(400));
        local.cast = (0..11).map(|i| format!("秘密の名前{i}")).collect();
        let state = materialize(local, vec![candidate(1, "A")]).unwrap();
        for path in &state.payload.truncated_fields {
            assert!(is_known_field_path(path), "固定パスでない値: {path}");
            assert!(!path.contains("秘密"), "利用者の入力が混ざっている: {path}");
        }

        // 手で入れた任意の値は検証で弾く
        let mut payload = state.payload.clone();
        payload.truncated_fields.push("local.secret_note".to_string());
        assert!(matches!(
            validate_outbound_state(&payload, &[]),
            Err(JevStateError::ForbiddenContent { .. })
        ));
    }

    // ─── 区分値の値域 ────────────────────────────────────────────────────────

    #[test]
    fn categorical_fields_are_restricted_to_the_schema() {
        let cases: Vec<(&str, fn(&mut LocalEvidenceInput, &str), &str, &str)> = vec![
            ("local.media_kind", |l, v| l.media_kind = Some(v.to_string()), "movie", "anime"),
            ("local.country_type", |l, v| l.country_type = Some(v.to_string()), "domestic", "asian"),
            (
                "local.evidence_class",
                |l, v| l.evidence_class = Some(v.to_string()),
                "reconstructed_clean",
                "maybe_clean",
            ),
            (
                "local.tag_provenance",
                |l, v| l.tag_provenance = Some(v.to_string()),
                "suspicious",
                "trusted",
            ),
        ];
        for (field, setter, valid, invalid) in cases {
            let mut good = LocalEvidenceInput::default();
            setter(&mut good, valid);
            materialize(good, vec![candidate(1, "A")])
                .unwrap_or_else(|e| panic!("{field} の {valid} が通らない: {e}"));

            let mut bad = LocalEvidenceInput::default();
            setter(&mut bad, invalid);
            assert_eq!(
                materialize(bad, vec![candidate(1, "A")]),
                Err(JevStateError::InvalidCategoricalValue {
                    field: field.to_string(),
                    value: invalid.to_string()
                }),
                "{field}"
            );
        }

        // candidate.media_type は従来どおり movie / tv のみ
        let mut bad = candidate(1, "A");
        bad.media_type = "anime".to_string();
        assert_eq!(
            materialize(LocalEvidenceInput::default(), vec![bad]),
            Err(JevStateError::InvalidMediaType { value: "anime".to_string() })
        );
    }

    // ─── forbidden_extra ────────────────────────────────────────────────────

    #[test]
    fn forbidden_extra_never_leaks_the_secret_value() {
        const SECRET: &str = "ts-live-abcdef0123456789";
        let mut local = LocalEvidenceInput::default();
        local.derived_title = Some(format!("タイトル {SECRET}"));
        let state = materialize(local, vec![candidate(1, "A")]).unwrap();

        let error = validate_outbound_state(&state.payload, &[SECRET]).unwrap_err();
        assert!(matches!(error, JevStateError::ForbiddenContent { .. }));
        // 実値がエラー文にも Debug 出力にも出ない
        assert!(!error.to_string().contains(SECRET), "{error}");
        assert!(!format!("{error:?}").contains(SECRET), "{error:?}");
        assert_eq!(error.to_string(), "送ってはいけない内容が含まれています（forbidden_extra value matched / state）");

        // 空文字・空白だけの値で正常な payload を誤って弾かない
        let clean = materialize(full_local(), vec![candidate(1, "A")]).unwrap();
        assert!(validate_outbound_state(&clean.payload, &[]).is_ok());
        assert!(validate_outbound_state(&clean.payload, &["", "   ", "	"]).is_ok());
        // 実際に含まれていれば弾く
        assert!(validate_outbound_state(&clean.payload, &["ギルティ"]).is_err());
    }

    #[test]
    fn empty_titles_are_rejected() {
        let mut blank = candidate(1, "   ");
        blank.title = "   ".to_string();
        assert_eq!(
            materialize(LocalEvidenceInput::default(), vec![blank]),
            Err(JevStateError::EmptyTitle { cand_key: "c1".to_string() })
        );
    }
}
