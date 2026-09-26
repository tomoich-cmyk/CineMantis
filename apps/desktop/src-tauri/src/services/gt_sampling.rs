//! ground truth 用の抽出（PR3 C5c.2）。
//!
//! 人が正解を付ける対象を選び、その集合を **DB に凍結**する。凍結した集合は
//! `metadata_review_tasks(reason='audit_sample')` そのものであり、あとから母集団が
//! 増減しても入れ替わらない。
//!
//! # なぜ「再抽出」を identity にしないか
//!
//! 同じ seed で同じ式を回しても、母集団に work が増えれば上位 N の集合は変わる
//! （新しい work の hash が小さければ入り込む）。したがって「アルゴリズムを再実行すれば
//! 永久に同じ sample になる」とは**言わない**。抽出は 1 回だけ行い、その結果を課題として
//! 書き出す。以後は同じ `sample_id` を指定しても、**既存の課題集合を読み直すだけ**。
//!
//! # 抽出に使ってよい属性
//!
//! Jev を動かす前に確定しているものだけ（cohort / media_kind / title provenance /
//! tags の有無 / file 数 / 現在の match_status）。モデルの出力や rules のスコアは
//! 母集団の条件にも層にも使わない。
//!
//! # 書くもの
//!
//! `metadata_review_tasks` の行だけ。`works` / `files` / `metadata_match_runs` には
//! 触らない。TMDB も TypeSafe も呼ばない。

use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::services::jev_contract::canonical_json;
use crate::services::match_history::{audit_details_json, ReviewReason, POLICY_VERSION};
use crate::services::prematch_snapshot::STATE_SCHEMA_VERSION;

/// 抽出手順の版。`sampling_json` に記録する
pub const PROTOCOL_VERSION: &str = "gt-protocol-1";

/// 順位づけの方式
pub const SELECTION_METHOD: &str = "sha256_rank_v1";

/// 抽出した課題の初期状態
pub const STATE_SELECTED: &str = "selected";

/// GT の用途。抽出時に決めて、あとから変えない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplePurpose {
    /// 閾値調整・失敗分析・UI 開発に使ってよい
    Development,
    /// 最終評価用。調整には一切使わない
    Holdout,
}

impl SamplePurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            SamplePurpose::Development => "development",
            SamplePurpose::Holdout => "holdout",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "development" => Some(SamplePurpose::Development),
            "holdout" => Some(SamplePurpose::Holdout),
            _ => None,
        }
    }
}

/// 抽出の依頼。
#[derive(Debug, Clone, PartialEq)]
pub struct SampleRequest {
    /// 抽出バッチの識別子。これが seed と run の batch_id を兼ねる
    pub sample_id: String,
    /// 対象の cohort（`live` / `reconstructed_clean` / `historical_audit_only`）
    pub cohort: String,
    pub purpose: SamplePurpose,
    /// 何件選ぶか
    pub target_count: usize,
    /// 抽出時のコード。40 桁の hex
    pub source_git_sha: String,
    pub state_schema_version: String,
    pub rules_policy_version: String,
}

/// 凍結された 1 件。
#[derive(Debug, Clone, PartialEq)]
pub struct SampleEntry {
    pub task_id: i64,
    pub work_id: i64,
    pub sample_rank: i64,
    pub state: String,
}

/// 凍結された sample 全体。
#[derive(Debug, Clone, PartialEq)]
pub struct SampleManifest {
    pub sample_id: String,
    pub cohort: String,
    pub purpose: SamplePurpose,
    pub frame_size: usize,
    pub frame_sha256: String,
    /// 凍結された抽出の前提。sample 内の全課題で一致していることを確認済み
    pub config: FrozenSampleConfig,
    pub entries: Vec<SampleEntry>,
    /// 既存の凍結集合を読み直したか（新規抽出なら false）
    pub resumed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GtSamplingError {
    InvalidRequest(String),
    /// 既存 sample と依頼内容が食い違う。既存を黙って変えない
    SampleConfigMismatch { field: &'static str },
    /// 母集団が足りない
    FrameTooSmall { frame_size: usize, requested: usize },
    /// 凍結済みの記録が壊れている（既定値で救わない）
    CorruptManifest { task_id: i64, reason: String },
    Db(String),
}

impl std::fmt::Display for GtSamplingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GtSamplingError::InvalidRequest(reason) => write!(f, "依頼が不正です: {reason}"),
            GtSamplingError::SampleConfigMismatch { field } => {
                write!(f, "既存 sample と設定が一致しません（{field}）")
            }
            GtSamplingError::FrameTooSmall { frame_size, requested } => write!(
                f,
                "母集団が {frame_size} 件しかありません（{requested} 件を要求）"
            ),
            GtSamplingError::CorruptManifest { task_id, reason } => {
                write!(f, "課題 {task_id} の記録が壊れています: {reason}")
            }
            GtSamplingError::Db(reason) => write!(f, "DB エラー: {reason}"),
        }
    }
}

impl std::error::Error for GtSamplingError {}

impl From<rusqlite::Error> for GtSamplingError {
    fn from(error: rusqlite::Error) -> Self {
        GtSamplingError::Db(error.to_string())
    }
}

// ─── 母集団 ──────────────────────────────────────────────────────────────────

/// 母集団の 1 件。層の値もここで確定する。
#[derive(Debug, Clone, PartialEq)]
pub struct FrameRow {
    pub work_id: i64,
    pub evidence_class: String,
    pub source_media_kind: String,
    pub title_provenance: String,
    pub tags_present: bool,
    pub part_count: i64,
    pub current_match_status: String,
}

impl FrameRow {
    fn strata(&self) -> Value {
        json!({
            "evidence_class": self.evidence_class,
            "source_media_kind": self.source_media_kind,
            "title_provenance": self.title_provenance,
            "tags_present": self.tags_present,
            "part_count": self.part_count,
            "current_match_status": self.current_match_status,
        })
    }

    /// `frame_sha256` の材料。work_id と層だけで、順序は呼び出し側が固定する
    fn fingerprint(&self) -> String {
        canonical_json(&json!({
            "work_id": self.work_id,
            "strata": self.strata(),
        }))
    }
}

/// `evidence_class_for()` と同じ規則を SQL で書いたもの。
///
/// files が無い / `original_file_name` が欠けている / `renamed_by_app` が付いている →
/// historical_audit_only。全 file が `original_captured` なら live、それ以外は
/// reconstructed_clean。
const COHORT_CTE: &str = "
WITH part AS (
    SELECT wp.work_id AS work_id, fi.*
      FROM work_parts wp JOIN files fi ON fi.id = wp.file_id
),
agg AS (
    SELECT w.id AS work_id,
           COUNT(p.id) AS n_files,
           SUM(CASE WHEN p.original_file_name IS NULL THEN 1 ELSE 0 END) AS missing_original,
           SUM(CASE WHEN p.renamed_by_app IS NOT NULL THEN 1 ELSE 0 END) AS renamed,
           SUM(CASE WHEN COALESCE(p.original_captured,0) = 1 THEN 1 ELSE 0 END) AS captured,
           -- tags_captured は「scan 時に取ったか、後から埋めたか」という出どころ。
           -- Jev が実際に使うのは container_tags_json の中身なので、層はそちらで決める
           SUM(CASE WHEN TRIM(COALESCE(p.container_tags_json,'')) NOT IN ('', '{}', 'null')
                    THEN 1 ELSE 0 END) AS tagged,
           SUM(CASE WHEN COALESCE(p.tags_captured,0) = 1 THEN 1 ELSE 0 END) AS tagged_live,
           SUM(CASE WHEN p.availability_status <> 'available' THEN 1 ELSE 0 END) AS unavailable,
           SUM(CASE WHEN TRIM(COALESCE(p.original_file_name,'')) <> '' THEN 1 ELSE 0 END)
               AS has_original_name
      FROM works w LEFT JOIN part p ON p.work_id = w.id
     GROUP BY w.id
),
cohort AS (
    SELECT work_id, n_files, tagged, tagged_live, unavailable, has_original_name,
           CASE
             WHEN n_files = 0 OR missing_original > 0 THEN 'historical_audit_only'
             WHEN renamed > 0 THEN 'historical_audit_only'
             WHEN captured = n_files THEN 'live'
             ELSE 'reconstructed_clean'
           END AS evidence_class
      FROM agg
)
";

/// 抽出できる work を集める。
///
/// 除外するもの:
/// - cohort が違う
/// - 利用できない file を含む / source が online でない
/// - 既に `audit_sample` 課題を持つ（同じ sample_id を除く）
/// - 一度でも strong label が付いた（superseded も含む）
///
/// `current_match_status` は除外条件にせず、層として記録するだけにする。
pub fn build_frame(
    conn: &Connection,
    cohort: &str,
    sample_id: &str,
) -> Result<Vec<FrameRow>, GtSamplingError> {
    let sql = format!(
        "{COHORT_CTE}
SELECT c.work_id,
       c.evidence_class,
       -- PreMatchSnapshot と同じ規則: part_no, file_id 順に見て最初の movie / tv。
       -- 無ければ unknown（source の media_kind が未設定のときなど）
       COALESCE((SELECT s.media_kind FROM work_parts wp
                   JOIN files fi ON fi.id = wp.file_id
                   JOIN sources s ON s.id = fi.source_id
                  WHERE wp.work_id = c.work_id AND s.media_kind IN ('movie','tv')
                  ORDER BY wp.part_no, fi.id LIMIT 1), 'unknown')
         AS source_media_kind,
       CASE WHEN TRIM(COALESCE(w.title_guess,'')) <> '' THEN 'title_guess'
            WHEN c.has_original_name > 0 THEN 'original_file_name'
            ELSE 'none' END AS title_provenance,
       c.tagged AS tags_nonempty,
       c.n_files AS part_count,
       COALESCE(w.match_status,'unknown') AS current_match_status
  FROM cohort c JOIN works w ON w.id = c.work_id
 WHERE c.evidence_class = ?1
   AND c.n_files > 0
   AND c.unavailable = 0
   AND NOT EXISTS (SELECT 1 FROM work_parts wp
                     JOIN files fi ON fi.id = wp.file_id
                     JOIN sources s ON s.id = fi.source_id
                    WHERE wp.work_id = c.work_id AND s.status <> 'online')
   -- 他の sample で既に抽出済みの work は取らない（同じ sample_id の再開だけ許す）
   AND NOT EXISTS (SELECT 1 FROM metadata_review_tasks t
                    WHERE t.work_id = c.work_id AND t.reason = 'audit_sample'
                      AND COALESCE(json_extract(t.sampling_json,'$.sample_id'),'') <> ?2)
   -- 一度でも人が確定した work は母集団から外す（superseded も含む）
   AND NOT EXISTS (SELECT 1 FROM metadata_match_labels l
                    WHERE l.work_id = c.work_id AND l.strength = 'strong')
 ORDER BY c.work_id"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params![cohort, sample_id], |row| {
            Ok(FrameRow {
                work_id: row.get(0)?,
                evidence_class: row.get(1)?,
                source_media_kind: row.get(2)?,
                title_provenance: row.get(3)?,
                // ここでは仮置き。下で production と同じ parser を通して決める
                tags_present: row.get::<_, i64>(4)? > 0,
                part_count: row.get(5)?,
                current_match_status: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let usable = works_with_usable_tags(conn)?;
    let mut rows = rows;
    for row in &mut rows {
        row.tags_present = usable.contains(&row.work_id);
    }
    Ok(rows)
}

/// 「production が読めるタグを持つ work」の集合。
///
/// 条件は `container_tags_json` があり、production と同じ
/// [`ContainerTags::from_json`] で読めること。**中身の有無は問わない。**
/// `episode_id` や `comment` / `encoder` しか無いタグでも EmbeddedInputs や
/// tag_provenance には効くので、「match に役立つタグか」は別概念として扱い、
/// ここでは「読めるタグがあるか」だけを層にする。
/// 取得経路（`tags_captured`）とも無関係で、後から埋めたタグも数える。
fn works_with_usable_tags(conn: &Connection) -> Result<std::collections::HashSet<i64>, GtSamplingError> {
    let mut stmt = conn.prepare(
        "SELECT wp.work_id, fi.container_tags_json
           FROM work_parts wp JOIN files fi ON fi.id = wp.file_id
          WHERE fi.container_tags_json IS NOT NULL",
    )?;
    let mut usable = std::collections::HashSet::new();
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (work_id, raw) = row?;
        // 壊れた JSON だけが「タグ無し」。既定値で救わない
        if crate::services::container_tags::ContainerTags::from_json(&raw).is_some() {
            usable.insert(work_id);
        }
    }
    Ok(usable)
}

/// 母集団そのものの指紋。work_id 昇順に並べた (work_id, 層) から作る。
pub fn frame_sha256(frame: &[FrameRow]) -> String {
    let mut hasher = Sha256::new();
    for row in frame {
        hasher.update(row.fingerprint().as_bytes());
        hasher.update(b"\n");
    }
    hex(&hasher.finalize())
}

/// 順位づけの鍵。`SHA256(seed | work_id | protocol_version)`。
fn rank_key(seed: &str, work_id: i64, protocol_version: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    hasher.update(b"|");
    hasher.update(work_id.to_string().as_bytes());
    hasher.update(b"|");
    hasher.update(protocol_version.as_bytes());
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 母集団を並べ替えて上位 `count` 件を選ぶ。`SQLite RANDOM()` は使わない。
pub fn select_ranked(seed: &str, frame: &[FrameRow], count: usize) -> Vec<FrameRow> {
    let mut ordered: Vec<(String, &FrameRow)> = frame
        .iter()
        .map(|row| (rank_key(seed, row.work_id, PROTOCOL_VERSION), row))
        .collect();
    // 鍵の昇順。衝突したら work_id で安定させる
    ordered.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.work_id.cmp(&b.1.work_id)));
    ordered.into_iter().take(count).map(|(_, row)| row.clone()).collect()
}

// ─── 凍結と再開 ──────────────────────────────────────────────────────────────

/// sample を凍結する。既にあれば読み直す。
///
/// **同じ `sample_id` で 2 回目以降は母集団から選び直さない。** 既存の課題集合をそのまま
/// 返し、依頼内容と食い違っていればエラーにする（黙って作り替えない）。
pub fn freeze_sample(
    conn: &mut Connection,
    request: &SampleRequest,
) -> Result<SampleManifest, GtSamplingError> {
    validate(request)?;

    if let Some(manifest) = load_manifest(conn, &request.sample_id)? {
        verify_matches(&manifest, request)?;
        return Ok(SampleManifest { resumed: true, ..manifest });
    }

    // 母集団の読み取りから凍結まで 1 つの transaction で行う。
    // 途中で work の状態や他の sample が変わると、記録した frame と実際に凍結した
    // 集合が別の時点のものになってしまう
    let tx = conn.transaction()?;
    let frame = build_frame(&tx, &request.cohort, &request.sample_id)?;
    if frame.len() < request.target_count {
        tx.rollback()?;
        return Err(GtSamplingError::FrameTooSmall {
            frame_size: frame.len(),
            requested: request.target_count,
        });
    }
    let frame_size = frame.len();
    let digest = frame_sha256(&frame);
    let selected = select_ranked(&request.sample_id, &frame, request.target_count);

    let sampled_at = now_iso8601();
    let mut entries = Vec::with_capacity(selected.len());
    for (index, row) in selected.iter().enumerate() {
        let rank = (index + 1) as i64;
        let sampling_json = sampling_json(request, row, rank, frame_size, &digest, &sampled_at);
        tx.execute(
            "INSERT INTO metadata_review_tasks
               (work_id, run_id, reason, sampling_json, details_json)
             VALUES (?1, NULL, ?2, ?3, ?4)",
            rusqlite::params![
                row.work_id,
                ReviewReason::AuditSample.as_str(),
                sampling_json,
                audit_details_json(STATE_SELECTED, None),
            ],
        )?;
        entries.push(SampleEntry {
            task_id: tx.last_insert_rowid(),
            work_id: row.work_id,
            sample_rank: rank,
            state: STATE_SELECTED.to_string(),
        });
    }
    tx.commit()?;

    Ok(SampleManifest {
        sample_id: request.sample_id.clone(),
        cohort: request.cohort.clone(),
        purpose: request.purpose,
        frame_size,
        frame_sha256: digest.clone(),
        config: FrozenSampleConfig {
            protocol_version: PROTOCOL_VERSION.to_string(),
            sample_id: request.sample_id.clone(),
            sampled_at,
            source_git_sha: request.source_git_sha.clone(),
            cohort: request.cohort.clone(),
            purpose: request.purpose.as_str().to_string(),
            seed: request.sample_id.clone(),
            selection_method: SELECTION_METHOD.to_string(),
            frame_size: frame_size as i64,
            frame_sha256: digest,
            state_schema_version: request.state_schema_version.clone(),
            rules_policy_version: request.rules_policy_version.clone(),
            target_count: request.target_count as i64,
        },
        entries,
        resumed: false,
    })
}

fn validate(request: &SampleRequest) -> Result<(), GtSamplingError> {
    if request.sample_id.trim().is_empty() || request.sample_id.trim() != request.sample_id {
        return Err(GtSamplingError::InvalidRequest(
            "sample_id が空か、前後に空白があります".into(),
        ));
    }
    if request.sample_id.chars().count() > 128 {
        return Err(GtSamplingError::InvalidRequest("sample_id が長すぎます".into()));
    }
    if request.target_count == 0 {
        return Err(GtSamplingError::InvalidRequest("target_count が 0 です".into()));
    }
    if !matches!(
        request.cohort.as_str(),
        "live" | "reconstructed_clean" | "historical_audit_only"
    ) {
        return Err(GtSamplingError::InvalidRequest(format!(
            "cohort が不正です: {}",
            request.cohort
        )));
    }
    // 層と用途の組み合わせは表で固定する（C5c.1）。
    //
    //   reconstructed_clean + development  … 開発用。母集団が厚い
    //   historical_audit_only + development … 開発用。参考値まで
    //   live + holdout                      … 最終評価はこれだけ
    //
    // live を development に回さないのが要点で、いま手元にある live 28 件は
    // 最終評価のために未開封で残す。後埋めの証拠や改名後の履歴で
    // promotion の数字を作らないことと表裏の制約。
    if !matches!(
        (request.cohort.as_str(), request.purpose),
        ("reconstructed_clean", SamplePurpose::Development)
            | ("historical_audit_only", SamplePurpose::Development)
            | ("live", SamplePurpose::Holdout)
    ) {
        return Err(GtSamplingError::InvalidRequest(format!(
            "{} を {} に使うことはできません（live は holdout 専用、holdout は live 専用）",
            request.cohort,
            request.purpose.as_str()
        )));
    }
    // 抽出時のコードと、run に記録される版が食い違っていたら受け付けない
    if request.state_schema_version != STATE_SCHEMA_VERSION {
        return Err(GtSamplingError::InvalidRequest(format!(
            "state_schema_version は {STATE_SCHEMA_VERSION} です"
        )));
    }
    if request.rules_policy_version != POLICY_VERSION {
        return Err(GtSamplingError::InvalidRequest(format!(
            "rules_policy_version は {POLICY_VERSION} です"
        )));
    }
    if request.source_git_sha.len() != 40
        || !request.source_git_sha.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(GtSamplingError::InvalidRequest(
            "source_git_sha は 40 桁の hex です".into(),
        ));
    }
    Ok(())
}

/// 抽出の記録。**作成後に更新しない。**
fn sampling_json(
    request: &SampleRequest,
    row: &FrameRow,
    sample_rank: i64,
    frame_size: usize,
    frame_sha256: &str,
    sampled_at: &str,
) -> String {
    canonical_json(&json!({
        "protocol_version": PROTOCOL_VERSION,
        "sample_id": request.sample_id,
        "work_id": row.work_id,
        "sampled_at": sampled_at,
        "source_git_sha": request.source_git_sha,
        "cohort": request.cohort,
        "purpose": request.purpose.as_str(),
        "seed": request.sample_id,
        "selection_method": SELECTION_METHOD,
        "sample_rank": sample_rank,
        "target_count": request.target_count as i64,
        "frame_size": frame_size as i64,
        "frame_sha256": frame_sha256,
        "strata": row.strata(),
        "state_schema_version": request.state_schema_version,
        "rules_policy_version": request.rules_policy_version,
    }))
}

fn now_iso8601() -> String {
    // DB 側と同じ書式にそろえる
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let secs = (now / 1000) as i64;
    let millis = (now % 1000) as i64;
    let days = secs / 86_400;
    let rest = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// 1970-01-01 からの日数を暦日へ（Howard Hinnant の civil_from_days）
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// sample 全体で同じでなければならない抽出の前提。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenSampleConfig {
    pub protocol_version: String,
    pub sample_id: String,
    pub sampled_at: String,
    pub source_git_sha: String,
    pub cohort: String,
    pub purpose: String,
    pub seed: String,
    pub selection_method: String,
    pub frame_size: i64,
    pub frame_sha256: String,
    pub state_schema_version: String,
    pub rules_policy_version: String,
    /// 抽出時に何件選ぶと決めたか。
    /// 「いま残っている課題の数」ではないので、末尾の課題が消えたことも分かる
    pub target_count: i64,
}

/// 文字列を取り出す。欠けていたり型が違えばエラー（既定値で埋めない）。
fn required_str(value: &Value, key: &str, task_id: i64) -> Result<String, GtSamplingError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            GtSamplingError::CorruptManifest {
                task_id,
                reason: format!("{key} がありません"),
            }
        })
}

fn required_i64(value: &Value, key: &str, task_id: i64) -> Result<i64, GtSamplingError> {
    value.get(key).and_then(Value::as_i64).ok_or_else(|| GtSamplingError::CorruptManifest {
        task_id,
        reason: format!("{key} が整数ではありません"),
    })
}

fn frozen_config(sampling: &Value, task_id: i64) -> Result<FrozenSampleConfig, GtSamplingError> {
    Ok(FrozenSampleConfig {
        protocol_version: required_str(sampling, "protocol_version", task_id)?,
        sample_id: required_str(sampling, "sample_id", task_id)?,
        sampled_at: required_str(sampling, "sampled_at", task_id)?,
        source_git_sha: required_str(sampling, "source_git_sha", task_id)?,
        cohort: required_str(sampling, "cohort", task_id)?,
        purpose: required_str(sampling, "purpose", task_id)?,
        seed: required_str(sampling, "seed", task_id)?,
        selection_method: required_str(sampling, "selection_method", task_id)?,
        frame_size: required_i64(sampling, "frame_size", task_id)?,
        frame_sha256: required_str(sampling, "frame_sha256", task_id)?,
        state_schema_version: required_str(sampling, "state_schema_version", task_id)?,
        rules_policy_version: required_str(sampling, "rules_policy_version", task_id)?,
        target_count: required_i64(sampling, "target_count", task_id)?,
    })
}

/// 凍結済みの sample を読む。
///
/// **1 行目だけを信じない。** 同じ sample のすべての課題で抽出の前提が一致していること、
/// 順位が 1..N で重複なく連番であること、work が重複していないことまで確かめる。
/// JSON が壊れていたら既定値で救わずにエラーにする（監査の記録なので fail closed）。
pub fn load_manifest(
    conn: &Connection,
    sample_id: &str,
) -> Result<Option<SampleManifest>, GtSamplingError> {
    let mut stmt = conn.prepare(
        "SELECT id, work_id, sampling_json, details_json
           FROM metadata_review_tasks
          WHERE reason = 'audit_sample'
            AND json_extract(sampling_json,'$.sample_id') = ?1
          ORDER BY id",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![sample_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.is_empty() {
        return Ok(None);
    }

    let mut config: Option<FrozenSampleConfig> = None;
    let mut entries: Vec<SampleEntry> = Vec::with_capacity(rows.len());
    let mut seen_ranks: Vec<i64> = Vec::with_capacity(rows.len());
    let mut seen_works: Vec<i64> = Vec::with_capacity(rows.len());

    for (task_id, work_id, sampling_text, details_text) in &rows {
        let sampling: Value = serde_json::from_str(sampling_text).map_err(|error| {
            GtSamplingError::CorruptManifest {
                task_id: *task_id,
                reason: format!("sampling_json を読めません: {error}"),
            }
        })?;
        let frozen = frozen_config(&sampling, *task_id)?;
        match &config {
            None => config = Some(frozen),
            Some(first) if *first != frozen => {
                return Err(GtSamplingError::CorruptManifest {
                    task_id: *task_id,
                    reason: "sample 内で抽出の前提が食い違っています".to_string(),
                })
            }
            Some(_) => {}
        }

        // 課題の work と、抽出時に記録した work が同じであること。
        // 課題行だけ差し替えられた manifest を再開しない
        let recorded_work = required_i64(&sampling, "work_id", *task_id)?;
        if recorded_work != *work_id {
            return Err(GtSamplingError::CorruptManifest {
                task_id: *task_id,
                reason: format!("work が食い違っています（課題 {work_id} / 抽出 {recorded_work}）"),
            });
        }

        let rank = required_i64(&sampling, "sample_rank", *task_id)?;
        if seen_ranks.contains(&rank) {
            return Err(GtSamplingError::CorruptManifest {
                task_id: *task_id,
                reason: format!("sample_rank {rank} が重複しています"),
            });
        }
        if seen_works.contains(work_id) {
            return Err(GtSamplingError::CorruptManifest {
                task_id: *task_id,
                reason: format!("work {work_id} が重複しています"),
            });
        }
        seen_ranks.push(rank);
        seen_works.push(*work_id);

        // details_json は lifecycle。壊れていたら既定値に落とさずエラー
        let details_text = details_text.clone().ok_or_else(|| GtSamplingError::CorruptManifest {
            task_id: *task_id,
            reason: "details_json がありません".to_string(),
        })?;
        let details: Value =
            serde_json::from_str(&details_text).map_err(|error| GtSamplingError::CorruptManifest {
                task_id: *task_id,
                reason: format!("details_json を読めません: {error}"),
            })?;
        let state = required_str(&details, "state", *task_id)?;

        entries.push(SampleEntry { task_id: *task_id, work_id: *work_id, sample_rank: rank, state });
    }

    // 1..N の連番であること
    seen_ranks.sort_unstable();
    let expected: Vec<i64> = (1..=rows.len() as i64).collect();
    if seen_ranks != expected {
        return Err(GtSamplingError::CorruptManifest {
            task_id: rows[0].0,
            reason: "sample_rank が 1..N の連番ではありません".to_string(),
        });
    }

    let config = config.expect("1 件以上あるので必ず入る");
    // 末尾の課題が消えていても、rank の連番だけでは気づけない。
    // 抽出時に決めた件数と突き合わせる
    if config.target_count != rows.len() as i64 {
        return Err(GtSamplingError::CorruptManifest {
            task_id: rows[0].0,
            reason: format!(
                "課題が {} 件しかありません（抽出時は {} 件）",
                rows.len(),
                config.target_count
            ),
        });
    }
    let purpose = SamplePurpose::parse(&config.purpose).ok_or_else(|| {
        GtSamplingError::CorruptManifest {
            task_id: rows[0].0,
            reason: format!("purpose が不正です: {}", config.purpose),
        }
    })?;

    entries.sort_by_key(|entry| entry.sample_rank);
    Ok(Some(SampleManifest {
        sample_id: sample_id.to_string(),
        cohort: config.cohort.clone(),
        purpose,
        frame_size: config.frame_size as usize,
        frame_sha256: config.frame_sha256.clone(),
        config,
        entries,
        resumed: true,
    }))
}

/// 再開時、依頼内容が凍結済みの sample と同じか確かめる。
///
/// 抽出の前提（どのコードで・どの版で・どの規則で選んだか）が食い違ったまま
/// 再開すると、後から「何を測ったのか」が言えなくなる。**全項目を突き合わせる。**
fn verify_matches(
    manifest: &SampleManifest,
    request: &SampleRequest,
) -> Result<(), GtSamplingError> {
    let frozen = &manifest.config;
    let checks: [(&'static str, &str, &str); 8] = [
        ("protocol_version", frozen.protocol_version.as_str(), PROTOCOL_VERSION),
        ("sample_id", frozen.sample_id.as_str(), request.sample_id.as_str()),
        ("source_git_sha", frozen.source_git_sha.as_str(), request.source_git_sha.as_str()),
        ("cohort", frozen.cohort.as_str(), request.cohort.as_str()),
        ("purpose", frozen.purpose.as_str(), request.purpose.as_str()),
        ("seed", frozen.seed.as_str(), request.sample_id.as_str()),
        ("selection_method", frozen.selection_method.as_str(), SELECTION_METHOD),
        (
            "state_schema_version",
            frozen.state_schema_version.as_str(),
            request.state_schema_version.as_str(),
        ),
    ];
    for (field, frozen_value, expected) in checks {
        if frozen_value != expected {
            return Err(GtSamplingError::SampleConfigMismatch { field });
        }
    }
    if frozen.rules_policy_version != request.rules_policy_version {
        return Err(GtSamplingError::SampleConfigMismatch { field: "rules_policy_version" });
    }
    // 「いま残っている課題数」ではなく、凍結した件数と比べる
    if frozen.target_count != request.target_count as i64 {
        return Err(GtSamplingError::SampleConfigMismatch { field: "target_count" });
    }
    Ok(())
}

/// 課題 1 件の抽出記録（audit generator が cohort の照合に使う）。
pub fn task_sampling(conn: &Connection, task_id: i64) -> Result<Option<Value>, GtSamplingError> {
    let row: Option<String> = conn
        .query_row(
            "SELECT sampling_json FROM metadata_review_tasks
              WHERE id = ?1 AND reason = 'audit_sample'",
            rusqlite::params![task_id],
            |row| row.get(0),
        )
        .optional()?;
    match row {
        Some(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| GtSamplingError::Db(format!("sampling_json: {error}"))),
        None => Ok(None),
    }
}

/// 状態遷移の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStateChange {
    /// 期待どおりの状態だったので書き換えた
    Updated,
    /// 既に別の誰かが進めていたので、何も書かなかった
    Skipped,
}

/// `details_json` の状態だけを、**期待する現在状態を条件に**書き換える。
/// `sampling_json` は触らない。
///
/// 単純な UPDATE にしない理由は競合にある。generator を 2 本走らせると、
/// 片方が run を繋いで ready にした直後に、もう片方が自分の失敗を
/// generation_error として書き込み、繋がった run を持つ課題が
/// 「失敗」に見えてしまう。そこで
///
///   - `run_id IS NULL`（run が繋がった課題は状態を巻き戻さない）
///   - 現在の state が `expected` のどれかであること
///
/// を WHERE に入れ、負けた側は [`TaskStateChange::Skipped`] を受け取る。
/// 課題そのものが無い場合だけ [`GtSamplingError::CorruptManifest`]。
pub fn set_task_state(
    conn: &Connection,
    task_id: i64,
    expected: &[&str],
    state: &str,
    stale_reason: Option<&str>,
) -> Result<TaskStateChange, GtSamplingError> {
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM metadata_review_tasks WHERE id = ?1 AND reason = 'audit_sample'",
            rusqlite::params![task_id],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !exists {
        return Err(GtSamplingError::CorruptManifest {
            task_id,
            reason: "audit_sample 課題が見つかりません".to_string(),
        });
    }

    let placeholders = (0..expected.len())
        .map(|index| format!("?{}", index + 4))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "UPDATE metadata_review_tasks SET details_json = ?2
          WHERE id = ?1 AND reason = 'audit_sample'
            AND run_id IS NULL
            AND resolved_at IS NULL
            AND COALESCE(json_extract(details_json,'$.state'),'') IN ({placeholders})"
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![
        Box::new(task_id),
        Box::new(audit_details_json(state, stale_reason)),
        // ?3 は使わないが、期待状態の番号を 4 から始めて読みやすくする
        Box::new(0i64),
    ];
    for value in expected {
        params.push(Box::new(value.to_string()));
    }
    let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|value| value.as_ref()).collect();
    let changed = conn.execute(&sql, refs.as_slice())?;
    Ok(if changed == 1 { TaskStateChange::Updated } else { TaskStateChange::Skipped })
}

/// 課題の現在の様子（競合に負けた側が読み直すために使う）
pub fn task_progress(
    conn: &Connection,
    task_id: i64,
) -> Result<Option<(Option<i64>, String)>, GtSamplingError> {
    let row = conn
        .query_row(
            "SELECT run_id, COALESCE(json_extract(details_json,'$.state'),'')
               FROM metadata_review_tasks WHERE id = ?1 AND reason = 'audit_sample'",
            rusqlite::params![task_id],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    Ok(row)
}

/// canonical JSON を作るための Map ヘルパ（テストからも使う）
pub(crate) fn json_object(pairs: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

// ─── テスト ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::scan::insert_scanned_work;
    use crate::db::test_support::*;

    const SHA: &str = "3946af66200053309c45fd45bc57743fd9345b7c";

    fn request(sample_id: &str, count: usize, purpose: SamplePurpose) -> SampleRequest {
        SampleRequest {
            sample_id: sample_id.to_string(),
            cohort: "reconstructed_clean".to_string(),
            purpose,
            target_count: count,
            source_git_sha: SHA.to_string(),
            state_schema_version: "cm-prematch-2".to_string(),
            rules_policy_version: "rules-safe-2".to_string(),
        }
    }

    /// reconstructed_clean（後埋め）の work を 1 件作る
    fn insert_reconstructed(conn: &Connection, index: usize) -> i64 {
        let root = r"D:\Import";
        let source_id = conn
            .query_row(
                "SELECT id FROM sources WHERE root_path = ?1",
                rusqlite::params![root],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or_else(|_| insert_source(conn, root));
        let title = format!("作品 {index}");
        let file_name = format!("work-{index}.mkv");
        let work_id = insert_scanned_work(conn, "movie", "unknown", &title, "unknown").unwrap();
        // 020 以前の行と同じ形で入れ、backfill に original_* を埋めさせる
        // （original_captured = 0 → reconstructed_clean）
        conn.execute(
            "INSERT INTO files (source_id, file_path, file_name, extension, container)
             VALUES (?1, ?2, ?3, 'mkv', 'mkv')",
            rusqlite::params![source_id, format!(r"D:\Import\{file_name}"), file_name],
        )
        .unwrap();
        let file_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO work_parts (work_id, file_id) VALUES (?1, ?2)",
            rusqlite::params![work_id, file_id],
        )
        .unwrap();
        crate::db::backfill_prematch_inputs(conn).unwrap();
        // title_guess は使わない（実 DB の reconstructed_clean と同じ）
        conn.execute(
            "UPDATE works SET title_guess = NULL WHERE id = ?1",
            rusqlite::params![work_id],
        )
        .unwrap();
        work_id
    }

    fn seed_db(count: usize) -> (Connection, Vec<i64>) {
        let conn = open_migrated();
        let works = (1..=count).map(|index| insert_reconstructed(&conn, index)).collect();
        (conn, works)
    }

    /// 課題に繋ぐためだけの最小の run（FK を満たす）
    fn insert_dummy_run(conn: &Connection, work_id: i64) -> i64 {
        conn.execute(
            "INSERT INTO metadata_match_runs
                 (work_id, trigger_kind, mode, evidence_class, decision,
                  state_schema_version, policy_version, input_snapshot_json,
                  search_queries_json, policy_snapshot_json, governing_matcher,
                  decision_reasons_json, error_text)
             VALUES (?1,'batch','safe','reconstructed_clean','ERROR',?2,?3,
                     '{}','[]','{}','rules-safe','[]','test')",
            rusqlite::params![work_id, STATE_SCHEMA_VERSION, POLICY_VERSION],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn work_ids(manifest: &SampleManifest) -> Vec<i64> {
        manifest.entries.iter().map(|entry| entry.work_id).collect()
    }

    // ─── 抽出の決定性 ────────────────────────────────────────────────────

    #[test]
    fn the_same_frame_and_seed_give_the_same_order() {
        let (conn, _) = seed_db(20);
        let frame = build_frame(&conn, "reconstructed_clean", "sample-a").unwrap();
        assert_eq!(frame.len(), 20);

        let first = select_ranked("sample-a", &frame, 5);
        let second = select_ranked("sample-a", &frame, 5);
        assert_eq!(first, second, "同じ seed で順位が変わる");

        // 母集団の並びを変えても結果は同じ（順位は hash だけで決まる）
        let mut shuffled = frame.clone();
        shuffled.reverse();
        assert_eq!(select_ranked("sample-a", &shuffled, 5), first);
    }

    #[test]
    fn a_different_seed_gives_a_different_order() {
        let (conn, _) = seed_db(30);
        let frame = build_frame(&conn, "reconstructed_clean", "x").unwrap();
        let a = select_ranked("sample-a", &frame, 10);
        let b = select_ranked("sample-b", &frame, 10);
        assert_ne!(a, b, "seed を変えても同じ並びになっている");
        // どちらも決定的
        assert_eq!(select_ranked("sample-b", &frame, 10), b);
    }

    #[test]
    fn the_frame_fingerprint_is_stable_and_sensitive() {
        let (conn, works) = seed_db(5);
        let frame = build_frame(&conn, "reconstructed_clean", "x").unwrap();
        let digest = frame_sha256(&frame);
        assert_eq!(digest.len(), 64);
        assert_eq!(frame_sha256(&frame), digest, "同じ母集団で指紋が変わる");

        // 層が変われば指紋も変わる
        conn.execute(
            "UPDATE works SET match_status = 'pending' WHERE id = ?1",
            rusqlite::params![works[0]],
        )
        .unwrap();
        let changed = build_frame(&conn, "reconstructed_clean", "x").unwrap();
        assert_ne!(frame_sha256(&changed), digest);
    }

    // ─── 凍結と再開 ──────────────────────────────────────────────────────

    #[test]
    fn a_frozen_sample_does_not_change_when_the_frame_grows() {
        let (mut conn, _) = seed_db(20);
        let manifest = freeze_sample(&mut conn, &request("sample-a", 5, SamplePurpose::Development))
            .unwrap();
        assert!(!manifest.resumed);
        assert_eq!(manifest.entries.len(), 5);
        assert_eq!(manifest.frame_size, 20);
        let selected = work_ids(&manifest);

        // 母集団を増やす（新しい work の hash が小さければ、再抽出なら入れ替わり得る）
        for index in 21..=60 {
            insert_reconstructed(&conn, index);
        }

        let resumed = freeze_sample(&mut conn, &request("sample-a", 5, SamplePurpose::Development))
            .unwrap();
        assert!(resumed.resumed, "再抽出している");
        assert_eq!(work_ids(&resumed), selected, "凍結した集合が入れ替わっている");
        assert_eq!(resumed.frame_size, 20, "抽出時の母集団サイズを保つ");
        assert_eq!(resumed.frame_sha256, manifest.frame_sha256);

        // 課題は増えていない
        let tasks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata_review_tasks WHERE reason = 'audit_sample'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tasks, 5);
    }

    #[test]
    fn resuming_with_a_different_config_is_rejected() {
        let (mut conn, _) = seed_db(20);
        freeze_sample(&mut conn, &request("sample-a", 5, SamplePurpose::Development)).unwrap();

        // 件数違い
        assert_eq!(
            freeze_sample(&mut conn, &request("sample-a", 6, SamplePurpose::Development))
                .unwrap_err(),
            GtSamplingError::SampleConfigMismatch { field: "target_count" }
        );
        // purpose 違い。
        // 表で cohort と purpose を固定したので、正当な request では
        // 「同じ cohort で purpose だけ違う」再開は作れない。
        // そこで凍結側の記録を書き換え、突き合わせが効くことを確かめる。
        conn.execute(
            "UPDATE metadata_review_tasks
                SET sampling_json = json_set(sampling_json,'$.purpose','holdout')
              WHERE reason = 'audit_sample'
                AND json_extract(sampling_json,'$.sample_id') = 'sample-a'",
            [],
        )
        .unwrap();
        assert_eq!(
            freeze_sample(&mut conn, &request("sample-a", 5, SamplePurpose::Development))
                .unwrap_err(),
            GtSamplingError::SampleConfigMismatch { field: "purpose" }
        );
        conn.execute(
            "UPDATE metadata_review_tasks
                SET sampling_json = json_set(sampling_json,'$.purpose','development')
              WHERE reason = 'audit_sample'
                AND json_extract(sampling_json,'$.sample_id') = 'sample-a'",
            [],
        )
        .unwrap();
        // cohort 違い（表で許される組み合わせの中で入れ替える）
        let mut other = request("sample-a", 5, SamplePurpose::Development);
        other.cohort = "historical_audit_only".to_string();
        assert_eq!(
            freeze_sample(&mut conn, &other).unwrap_err(),
            GtSamplingError::SampleConfigMismatch { field: "cohort" }
        );

        let tasks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata_review_tasks
                  WHERE reason = 'audit_sample'
                    AND json_extract(sampling_json,'$.sample_id') = 'sample-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tasks, 5, "拒否したのに課題が増減している");
    }

    /// 別の sample で抽出済みの work は、次の sample の母集団から外れる
    #[test]
    fn samples_do_not_overlap() {
        let (mut conn, _) = seed_db(20);
        let first = freeze_sample(&mut conn, &request("sample-a", 5, SamplePurpose::Development))
            .unwrap();
        let second = freeze_sample(&mut conn, &request("sample-b", 5, SamplePurpose::Development))
            .unwrap();

        let first_ids = work_ids(&first);
        for work_id in work_ids(&second) {
            assert!(!first_ids.contains(&work_id), "work {work_id} が両方に入っている");
        }
        assert_eq!(second.frame_size, 15, "既に抽出済みの 5 件が母集団から外れていない");
    }

    /// live を development に回さない（手元の 28 件を最終評価まで未開封で残す）
    #[test]
    fn live_cannot_be_used_for_development() {
        let (mut conn, _) = seed_db(4);
        for index in 1..=3 {
            insert_live(&conn, index);
        }
        let mut bad = request("sample-dev-live", 1, SamplePurpose::Development);
        bad.cohort = "live".to_string();
        assert!(matches!(
            freeze_sample(&mut conn, &bad),
            Err(GtSamplingError::InvalidRequest(_))
        ));
        let tasks: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_review_tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tasks, 0, "live を開封してしまっている");
    }

    /// 層と用途の組み合わせは表のとおり（許可 3 通りだけ）
    #[test]
    fn the_cohort_purpose_matrix_is_fixed() {
        let allowed = [
            ("reconstructed_clean", SamplePurpose::Development),
            ("historical_audit_only", SamplePurpose::Development),
            ("live", SamplePurpose::Holdout),
        ];
        let rejected = [
            ("live", SamplePurpose::Development),
            ("reconstructed_clean", SamplePurpose::Holdout),
            ("historical_audit_only", SamplePurpose::Holdout),
        ];
        for (cohort, purpose) in allowed {
            let mut ok = request("sample-x", 1, purpose);
            ok.cohort = cohort.to_string();
            assert!(validate(&ok).is_ok(), "{cohort} + {} が弾かれた", purpose.as_str());
        }
        for (cohort, purpose) in rejected {
            let mut bad = request("sample-x", 1, purpose);
            bad.cohort = cohort.to_string();
            assert!(
                matches!(validate(&bad), Err(GtSamplingError::InvalidRequest(_))),
                "{cohort} + {} が通ってしまう",
                purpose.as_str()
            );
        }
    }

    /// final holdout に使えるのは live だけ
    #[test]
    fn only_live_may_be_used_as_a_holdout() {
        let (mut conn, _) = seed_db(6);
        for cohort in ["reconstructed_clean", "historical_audit_only"] {
            let mut bad = request("sample-hold", 1, SamplePurpose::Holdout);
            bad.cohort = cohort.to_string();
            assert!(
                matches!(
                    freeze_sample(&mut conn, &bad),
                    Err(GtSamplingError::InvalidRequest(_))
                ),
                "{cohort} を holdout に使えてしまう"
            );
        }
        // development なら両方使える（母集団があれば）
        let mut dev = request("sample-dev", 1, SamplePurpose::Development);
        dev.cohort = "reconstructed_clean".to_string();
        assert!(freeze_sample(&mut conn, &dev).is_ok());
    }

    #[test]
    fn works_with_a_historical_strong_label_are_excluded() {
        let (mut conn, works) = seed_db(10);
        // superseded であっても除外する
        conn.execute(
            "INSERT INTO metadata_match_labels
               (work_id, label, tmdb_id, media_type, method, strength, superseded_at)
             VALUES (?1, 'tmdb', 1, 'movie', 'manual_apply', 'strong', '2026-01-01T00:00:00Z')",
            rusqlite::params![works[0]],
        )
        .unwrap();
        // 弱い証拠は除外しない
        conn.execute(
            "INSERT INTO metadata_match_labels
               (work_id, label, tmdb_id, media_type, method, strength)
             VALUES (?1, 'tmdb', 2, 'movie', 'lock', 'weak')",
            rusqlite::params![works[1]],
        )
        .unwrap();

        let frame = build_frame(&conn, "reconstructed_clean", "x").unwrap();
        let ids: Vec<i64> = frame.iter().map(|row| row.work_id).collect();
        assert!(!ids.contains(&works[0]), "strong label の work が残っている");
        assert!(ids.contains(&works[1]), "weak label の work まで外している");
        assert_eq!(frame.len(), 9);

        let manifest =
            freeze_sample(&mut conn, &request("sample-a", 9, SamplePurpose::Development)).unwrap();
        assert!(!work_ids(&manifest).contains(&works[0]));
    }

    #[test]
    fn a_cohort_mismatch_keeps_works_out_of_the_frame() {
        let (conn, works) = seed_db(5);
        // scan 時の名前がそのまま残っている work（original_captured=1）を 1 件足す
        let live_work = insert_live(&conn, 100);
        // reconstructed_clean のうち 1 件を historical_audit_only にする
        conn.execute(
            "UPDATE files SET renamed_by_app = 'nas_sort'
              WHERE id IN (SELECT file_id FROM work_parts WHERE work_id = ?1)",
            rusqlite::params![works[1]],
        )
        .unwrap();

        let frame = build_frame(&conn, "reconstructed_clean", "x").unwrap();
        let ids: Vec<i64> = frame.iter().map(|row| row.work_id).collect();
        assert!(!ids.contains(&live_work), "live が混ざっている");
        assert!(!ids.contains(&works[1]), "historical_audit_only が混ざっている");
        assert_eq!(frame.len(), 4);

        assert_eq!(build_frame(&conn, "live", "x").unwrap().len(), 1);
        assert_eq!(build_frame(&conn, "historical_audit_only", "x").unwrap().len(), 1);
    }

    /// scan 経路そのままの work（original_captured = 1 → live）
    fn insert_live(conn: &Connection, index: usize) -> i64 {
        let root = r"D:\Import";
        let source_id = conn
            .query_row(
                "SELECT id FROM sources WHERE root_path = ?1",
                rusqlite::params![root],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or_else(|_| insert_source(conn, root));
        let file_name = format!("live-{index}.mkv");
        let work_id =
            insert_scanned_work(conn, "movie", "unknown", &format!("live {index}"), "unknown")
                .unwrap();
        conn.execute(
            "INSERT INTO files (source_id, file_path, file_name, extension, container,
                                original_file_name, original_rel_path, original_captured)
             VALUES (?1, ?2, ?3, 'mkv', 'mkv', ?3, ?3, 1)",
            rusqlite::params![source_id, format!(r"D:\Import\{file_name}"), file_name],
        )
        .unwrap();
        let file_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO work_parts (work_id, file_id) VALUES (?1, ?2)",
            rusqlite::params![work_id, file_id],
        )
        .unwrap();
        work_id
    }

    #[test]
    fn sampling_json_records_the_protocol() {
        let (mut conn, _) = seed_db(8);
        let manifest =
            freeze_sample(&mut conn, &request("sample-a", 3, SamplePurpose::Development)).unwrap();

        let sampling = task_sampling(&conn, manifest.entries[0].task_id).unwrap().unwrap();
        assert_eq!(sampling["protocol_version"], PROTOCOL_VERSION);
        assert_eq!(sampling["sample_id"], "sample-a");
        assert_eq!(sampling["seed"], "sample-a");
        assert_eq!(sampling["selection_method"], SELECTION_METHOD);
        assert_eq!(sampling["cohort"], "reconstructed_clean");
        assert_eq!(sampling["purpose"], "development");
        assert_eq!(sampling["source_git_sha"], SHA);
        assert_eq!(sampling["sample_rank"], 1);
        assert_eq!(sampling["frame_size"], 8);
        assert_eq!(sampling["frame_sha256"], manifest.frame_sha256);
        assert_eq!(sampling["state_schema_version"], "cm-prematch-2");
        assert_eq!(sampling["rules_policy_version"], "rules-safe-2");
        assert_eq!(sampling["strata"]["evidence_class"], "reconstructed_clean");
        assert_eq!(sampling["strata"]["title_provenance"], "original_file_name");
        assert_eq!(sampling["strata"]["tags_present"], false);
        assert_eq!(sampling["strata"]["part_count"], 1);

        // モデルの出力に由来する値を層に入れていない
        let text = sampling.to_string();
        for forbidden in ["jev", "confidence", "noul", "rules_score", "rank_score"] {
            assert!(!text.contains(forbidden), "{forbidden} が sampling_json にある");
        }

        // 初期状態
        let state: String = conn
            .query_row(
                "SELECT json_extract(details_json,'$.state') FROM metadata_review_tasks
                  WHERE id = ?1",
                rusqlite::params![manifest.entries[0].task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, STATE_SELECTED);
    }

    #[test]
    fn bad_requests_are_rejected() {
        let (mut conn, _) = seed_db(4);
        for bad in [
            request("", 2, SamplePurpose::Development),
            request(" sample ", 2, SamplePurpose::Development),
            request("sample-a", 0, SamplePurpose::Development),
        ] {
            assert!(matches!(
                freeze_sample(&mut conn, &bad),
                Err(GtSamplingError::InvalidRequest(_))
            ));
        }
        let mut bad_sha = request("sample-a", 2, SamplePurpose::Development);
        bad_sha.source_git_sha = "not-a-sha".to_string();
        assert!(matches!(
            freeze_sample(&mut conn, &bad_sha),
            Err(GtSamplingError::InvalidRequest(_))
        ));
        let mut bad_cohort = request("sample-a", 2, SamplePurpose::Development);
        bad_cohort.cohort = "everything".to_string();
        assert!(matches!(
            freeze_sample(&mut conn, &bad_cohort),
            Err(GtSamplingError::InvalidRequest(_))
        ));

        // 母集団が足りない
        assert!(matches!(
            freeze_sample(&mut conn, &request("sample-a", 99, SamplePurpose::Development)),
            Err(GtSamplingError::FrameTooSmall { .. })
        ));

        let tasks: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_review_tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tasks, 0);
    }

    /// 途中で書き込みが失敗したら、部分的な manifest を残さない。
    ///
    /// production 側に失敗の入口は作らない。テスト DB にだけ trigger を置き、
    /// 2 件目の挿入で ABORT させる。
    #[test]
    fn a_failed_insert_leaves_no_partial_manifest() {
        let (mut conn, _) = seed_db(20);
        conn.execute_batch(
            "CREATE TRIGGER test_abort_second_audit_task
             BEFORE INSERT ON metadata_review_tasks
             WHEN NEW.reason = 'audit_sample'
              AND json_extract(NEW.sampling_json,'$.sample_rank') = 2
             BEGIN SELECT RAISE(ABORT, 'test abort'); END;",
        )
        .unwrap();

        assert!(
            freeze_sample(&mut conn, &request("sample-a", 5, SamplePurpose::Development)).is_err(),
            "失敗したはずが成功している"
        );

        let tasks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata_review_tasks WHERE reason = 'audit_sample'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tasks, 0, "1 件目だけ残った半端な manifest がある");
        assert!(load_manifest(&conn, "sample-a").unwrap().is_none());
    }

    // ─── 件数の凍結 ──────────────────────────────────────────────────────

    /// 末尾の課題が消えたら、manifest 単体で気づける。
    /// rank の連番だけでは「最初から少なかった」と区別できない
    #[test]
    fn a_missing_trailing_task_is_detected() {
        let (mut conn, _) = seed_db(20);
        let manifest =
            freeze_sample(&mut conn, &request("sample-a", 5, SamplePurpose::Development)).unwrap();
        assert_eq!(manifest.config.target_count, 5, "抽出時の件数を凍結していない");

        // 5 件目（末尾）を消す
        conn.execute(
            "DELETE FROM metadata_review_tasks WHERE id = ?1",
            rusqlite::params![manifest.entries[4].task_id],
        )
        .unwrap();

        let error = load_manifest(&conn, "sample-a").unwrap_err();
        assert!(
            matches!(error, GtSamplingError::CorruptManifest { .. }),
            "末尾の消失を見逃している: {error:?}"
        );
        // 残り 4 件で rank は 1..4 の連番なので、件数を凍結していなければ通ってしまう
    }

    /// 再開の突き合わせは、残っている課題数ではなく凍結した件数で行う
    #[test]
    fn the_frozen_target_count_is_what_resume_compares() {
        let (mut conn, _) = seed_db(20);
        freeze_sample(&mut conn, &request("sample-a", 5, SamplePurpose::Development)).unwrap();
        let resumed =
            freeze_sample(&mut conn, &request("sample-a", 5, SamplePurpose::Development)).unwrap();
        assert!(resumed.resumed);
        assert_eq!(resumed.config.target_count, 5);
        assert_eq!(
            freeze_sample(&mut conn, &request("sample-a", 4, SamplePurpose::Development))
                .unwrap_err(),
            GtSamplingError::SampleConfigMismatch { field: "target_count" }
        );
    }

    // ─── 再開時の前提の突き合わせ ────────────────────────────────────────

    #[test]
    fn resuming_with_different_immutable_config_is_rejected() {
        let (mut conn, _) = seed_db(20);
        freeze_sample(&mut conn, &request("sample-a", 5, SamplePurpose::Development)).unwrap();

        // 抽出したコードが違う
        let mut other_sha = request("sample-a", 5, SamplePurpose::Development);
        other_sha.source_git_sha = "0".repeat(40);
        assert_eq!(
            freeze_sample(&mut conn, &other_sha).unwrap_err(),
            GtSamplingError::SampleConfigMismatch { field: "source_git_sha" }
        );

        // 版が違うものは validate で弾く（そもそも依頼として受けない）
        let mut other_state = request("sample-a", 5, SamplePurpose::Development);
        other_state.state_schema_version = "cm-prematch-1".to_string();
        assert!(matches!(
            freeze_sample(&mut conn, &other_state),
            Err(GtSamplingError::InvalidRequest(_))
        ));
        let mut other_policy = request("sample-a", 5, SamplePurpose::Development);
        other_policy.rules_policy_version = "rules-safe-1".to_string();
        assert!(matches!(
            freeze_sample(&mut conn, &other_policy),
            Err(GtSamplingError::InvalidRequest(_))
        ));

        // 凍結済みの記録側が別の版だった場合は mismatch として弾く
        conn.execute(
            "UPDATE metadata_review_tasks
                SET sampling_json = json_set(sampling_json,'$.rules_policy_version','rules-safe-1')
              WHERE reason = 'audit_sample'",
            [],
        )
        .unwrap();
        assert_eq!(
            freeze_sample(&mut conn, &request("sample-a", 5, SamplePurpose::Development))
                .unwrap_err(),
            GtSamplingError::SampleConfigMismatch { field: "rules_policy_version" }
        );
    }

    // ─── manifest の健全性 ───────────────────────────────────────────────

    #[test]
    fn a_corrupted_manifest_row_is_rejected() {
        let (mut conn, _) = seed_db(10);
        let manifest =
            freeze_sample(&mut conn, &request("sample-a", 3, SamplePurpose::Development)).unwrap();
        let task_id = manifest.entries[1].task_id;

        // ① sample 内で前提が食い違う
        conn.execute(
            "UPDATE metadata_review_tasks
                SET sampling_json = json_set(sampling_json,'$.frame_sha256','deadbeef')
              WHERE id = ?1",
            rusqlite::params![task_id],
        )
        .unwrap();
        assert!(matches!(
            load_manifest(&conn, "sample-a"),
            Err(GtSamplingError::CorruptManifest { .. })
        ));

        // ② 必須項目の欠落
        conn.execute(
            "UPDATE metadata_review_tasks
                SET sampling_json = json_remove(
                      json_set(sampling_json,'$.frame_sha256',
                               (SELECT json_extract(sampling_json,'$.frame_sha256')
                                  FROM metadata_review_tasks WHERE id = ?2)),
                      '$.source_git_sha')
              WHERE id = ?1",
            rusqlite::params![task_id, manifest.entries[0].task_id],
        )
        .unwrap();
        assert!(matches!(
            load_manifest(&conn, "sample-a"),
            Err(GtSamplingError::CorruptManifest { .. })
        ));
    }

    /// 課題の work だけ差し替えられた manifest は再開しない
    #[test]
    fn a_task_pointing_at_another_work_is_rejected() {
        let (mut conn, works) = seed_db(10);
        let manifest =
            freeze_sample(&mut conn, &request("sample-a", 3, SamplePurpose::Development)).unwrap();
        let victim = manifest.entries[0].task_id;
        let selected = work_ids(&manifest);
        let other = works.iter().find(|work_id| !selected.contains(work_id)).copied().unwrap();
        conn.execute(
            "UPDATE metadata_review_tasks SET work_id = ?2 WHERE id = ?1",
            rusqlite::params![victim, other],
        )
        .unwrap();

        assert!(matches!(
            load_manifest(&conn, "sample-a"),
            Err(GtSamplingError::CorruptManifest { .. })
        ));
    }

    #[test]
    fn duplicate_or_missing_ranks_are_rejected() {
        let (mut conn, _) = seed_db(10);
        let manifest =
            freeze_sample(&mut conn, &request("sample-a", 3, SamplePurpose::Development)).unwrap();

        // 順位の重複
        conn.execute(
            "UPDATE metadata_review_tasks
                SET sampling_json = json_set(sampling_json,'$.sample_rank',1)
              WHERE id = ?1",
            rusqlite::params![manifest.entries[2].task_id],
        )
        .unwrap();
        assert!(matches!(
            load_manifest(&conn, "sample-a"),
            Err(GtSamplingError::CorruptManifest { .. })
        ));

        // 連番の欠け
        conn.execute(
            "UPDATE metadata_review_tasks
                SET sampling_json = json_set(sampling_json,'$.sample_rank',9)
              WHERE id = ?1",
            rusqlite::params![manifest.entries[2].task_id],
        )
        .unwrap();
        assert!(matches!(
            load_manifest(&conn, "sample-a"),
            Err(GtSamplingError::CorruptManifest { .. })
        ));
    }

    #[test]
    fn a_corrupted_details_json_is_not_silently_defaulted() {
        let (mut conn, _) = seed_db(10);
        let manifest =
            freeze_sample(&mut conn, &request("sample-a", 2, SamplePurpose::Development)).unwrap();
        conn.execute(
            "UPDATE metadata_review_tasks SET details_json = '{ broken' WHERE id = ?1",
            rusqlite::params![manifest.entries[0].task_id],
        )
        .unwrap();
        assert!(matches!(
            load_manifest(&conn, "sample-a"),
            Err(GtSamplingError::CorruptManifest { .. })
        ));

        // state が無いのも既定値で救わない
        conn.execute(
            "UPDATE metadata_review_tasks SET details_json = '{}' WHERE id = ?1",
            rusqlite::params![manifest.entries[0].task_id],
        )
        .unwrap();
        assert!(matches!(
            load_manifest(&conn, "sample-a"),
            Err(GtSamplingError::CorruptManifest { .. })
        ));
    }

    #[test]
    fn updating_a_missing_task_state_is_an_error() {
        let (conn, _) = seed_db(2);
        assert!(matches!(
            set_task_state(&conn, 999_999, &[STATE_SELECTED], "ready", None),
            Err(GtSamplingError::CorruptManifest { .. })
        ));
    }

    /// 期待した状態でなければ書き換えない（勝った側の結果を塗り潰さない）
    #[test]
    fn a_state_change_needs_the_expected_current_state() {
        let (mut conn, _) = seed_db(10);
        let manifest =
            freeze_sample(&mut conn, &request("sample-a", 2, SamplePurpose::Development)).unwrap();
        let task_id = manifest.entries[0].task_id;

        assert_eq!(
            set_task_state(&conn, task_id, &[STATE_SELECTED], "stale", Some("x")).unwrap(),
            TaskStateChange::Updated
        );
        // もう selected ではないので、同じ遷移は通らない
        assert_eq!(
            set_task_state(&conn, task_id, &[STATE_SELECTED], "generation_error", None).unwrap(),
            TaskStateChange::Skipped
        );
        let (_, state) = task_progress(&conn, task_id).unwrap().unwrap();
        assert_eq!(state, "stale", "負けた側が状態を上書きしている");
    }

    /// run が繋がった課題は、状態を巻き戻せない
    #[test]
    fn a_task_with_a_run_is_never_moved_back() {
        let (mut conn, _) = seed_db(10);
        let manifest =
            freeze_sample(&mut conn, &request("sample-a", 2, SamplePurpose::Development)).unwrap();
        let task_id = manifest.entries[0].task_id;
        let run_id = insert_dummy_run(&conn, manifest.entries[0].work_id);
        conn.execute(
            "UPDATE metadata_review_tasks
                SET run_id = ?2, details_json = json_object('state','ready')
              WHERE id = ?1",
            rusqlite::params![task_id, run_id],
        )
        .unwrap();

        assert_eq!(
            set_task_state(&conn, task_id, &["ready", STATE_SELECTED], "generation_error", None)
                .unwrap(),
            TaskStateChange::Skipped
        );
        let (linked, state) = task_progress(&conn, task_id).unwrap().unwrap();
        assert_eq!(linked, Some(run_id));
        assert_eq!(state, "ready");
    }

    // ─── 層の意味 ────────────────────────────────────────────────────────

    /// tags_present は「production の parser で読めるタグがあるか」。
    ///
    /// 中身が match に役立つかは別の話で、ここでは層として扱わない。
    /// episode_id だけ、comment / encoder だけ、といったタグでも
    /// EmbeddedInputs や tag_provenance には効くので true にする。
    /// 取得経路（tags_captured）とも無関係。
    #[test]
    fn tags_present_means_the_production_parser_can_read_them() {
        let (conn, works) = seed_db(8);
        let set_tags = |work_id: i64, json: Option<&str>| {
            conn.execute(
                "UPDATE files SET container_tags_json = ?2, tags_captured = 0
                  WHERE id IN (SELECT file_id FROM work_parts WHERE work_id = ?1)",
                rusqlite::params![work_id, json],
            )
            .unwrap();
        };
        let filled = {
            let mut tags = crate::services::container_tags::ContainerTags::empty();
            tags.title = Some("タイトル".to_string());
            tags.to_json()
        };
        // (1) 題名のある正当なタグ
        set_tags(works[0], Some(&filled));
        // (2) 読めるが空
        set_tags(works[1], Some(r#"{"v":1}"#));
        // (3) episode_id だけ
        set_tags(works[2], Some(r#"{"v":1,"episode_id":"S01E02"}"#));
        // (4) comment / encoder だけ
        set_tags(works[3], Some(r#"{"v":1,"comment":"rip","encoder":"x264"}"#));
        // (5) 壊れた JSON
        set_tags(works[4], Some("{ broken"));
        // (6) そもそも無い（works[5] は NULL のまま）
        // (7) tags_captured = 0 でも読めれば true（上の set_tags が常に 0 にしている）
        conn.execute(
            "UPDATE files SET tags_captured = 1
              WHERE id IN (SELECT file_id FROM work_parts WHERE work_id = ?1)",
            rusqlite::params![works[0]],
        )
        .unwrap();

        let frame = build_frame(&conn, "reconstructed_clean", "x").unwrap();
        let present =
            |work_id: i64| frame.iter().find(|row| row.work_id == work_id).unwrap().tags_present;
        assert!(present(works[0]), "題名のあるタグを数えていない");
        assert!(present(works[1]), "読めるが空のタグを落としている");
        assert!(present(works[2]), "episode_id だけのタグを落としている");
        assert!(present(works[3]), "comment / encoder だけのタグを落としている");
        assert!(!present(works[4]), "壊れた JSON をタグとして数えている");
        assert!(!present(works[5]));

        // 後から埋めたタグ（tags_captured = 0）も同じく true
        set_tags(works[6], Some(&filled));
        let frame = build_frame(&conn, "reconstructed_clean", "x").unwrap();
        let backfilled = frame.iter().find(|row| row.work_id == works[6]).unwrap();
        assert!(backfilled.tags_present, "後から埋めたタグを無視している");
        let captured: i64 = conn
            .query_row(
                "SELECT MAX(fi.tags_captured) FROM work_parts wp
                   JOIN files fi ON fi.id = wp.file_id WHERE wp.work_id = ?1",
                rusqlite::params![works[6]],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(captured, 0, "provenance 側は別管理のまま");
    }

    /// media_kind は part 順で最初の movie / tv（PreMatchSnapshot と同じ）
    #[test]
    fn the_media_kind_follows_the_snapshot_semantics() {
        let conn = open_migrated();
        let unknown_source = insert_source(&conn, r"D:\Unknown");
        conn.execute(
            "INSERT INTO sources (name, root_path, source_type, media_kind)
             VALUES ('movie-src', ?1, 'local', 'movie')",
            rusqlite::params![r"D:\Movies"],
        )
        .unwrap();
        let movie_source = conn.last_insert_rowid();

        let work_id = insert_scanned_work(&conn, "movie", "unknown", "2 枚組", "unknown").unwrap();
        // part_no 1 が unknown、part_no 2 が movie
        for (part_no, source_id, name) in
            [(1, unknown_source, "disc1.mkv"), (2, movie_source, "disc2.mkv")]
        {
            conn.execute(
                "INSERT INTO files (source_id, file_path, file_name, extension, container)
                 VALUES (?1, ?2, ?3, 'mkv', 'mkv')",
                rusqlite::params![source_id, format!(r"D:\x\{name}"), name],
            )
            .unwrap();
            let file_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO work_parts (work_id, file_id, part_no) VALUES (?1, ?2, ?3)",
                rusqlite::params![work_id, file_id, part_no],
            )
            .unwrap();
        }
        crate::db::backfill_prematch_inputs(&conn).unwrap();
        conn.execute(
            "UPDATE works SET title_guess = NULL WHERE id = ?1",
            rusqlite::params![work_id],
        )
        .unwrap();

        let frame = build_frame(&conn, "reconstructed_clean", "x").unwrap();
        let row = frame.iter().find(|row| row.work_id == work_id).unwrap();
        assert_eq!(row.source_media_kind, "movie", "最初の movie/tv を拾えていない");
        assert_eq!(row.part_count, 2);

        // snapshot 側と同じ結論になること
        let snapshot =
            crate::services::prematch_snapshot::PreMatchSnapshot::capture(&conn, work_id).unwrap();
        assert_eq!(row.source_media_kind, snapshot.media_kind);
    }

    /// 抽出は review 課題しか書かない
    #[test]
    fn sampling_touches_nothing_but_review_tasks() {
        let (mut conn, _) = seed_db(10);
        let before = crate::services::audit_run::tests_support::production_fingerprint(&conn);
        freeze_sample(&mut conn, &request("sample-a", 4, SamplePurpose::Development)).unwrap();
        assert_eq!(
            crate::services::audit_run::tests_support::production_fingerprint(&conn),
            before
        );
        let runs: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_match_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(runs, 0, "抽出で run を作っている");
    }
}
