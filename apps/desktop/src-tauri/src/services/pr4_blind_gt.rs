//! PR4-PF0G1: H2 FINAL 用 blind Human GT reviewer の **core**（ledger / lease / CAS / resume /
//! correction / opaque review_unit_id / completion audit）。契約: `docs/PR4_PF0G0_BLIND_GT_CONTRACT.md`
//!
//! - UI も packager も TMDB 通信も持たない（PF0G2 以降。PF0F1 の schema freeze 待ち）。
//! - 機械の出力（decision / score / rank / source / Jev …）を表す型を **このモジュールは持たない**。
//!   reviewer artifact の型（[`ReviewUnit`]）は whitelist 済みの射影だけで、`deny_unknown_fields`。
//! - production DB・`rusqlite`・`metadata_match_*` を参照しない（ソース走査テストで固定）。
//! - 状態は **ledger（追記専用・hash chain）からだけ**再構築する。メモリの状態を信じない。
//! - 時刻は呼び出し側が渡す（`YYYY-MM-DDTHH:MM:SSZ`）。テストが決定的になる。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const REVIEW_PROTOCOL_VERSION: &str = "gt-review-1";
pub const UNIT_SCHEMA: &str = "pr4-blind-review-unit-2";
pub const MANIFEST_SCHEMA: &str = "pr4-blind-review-manifest-1";
pub const EVENT_SCHEMA: &str = "pr4-blind-review-event-1";
const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const LEDGER_FILE: &str = "events.jsonl";
const LOCK_FILE: &str = "review.lock";
pub const LEASE_SECONDS: i64 = 15 * 60;
pub const SESSION_MAX_UNITS: u32 = 50;
pub const SESSION_MAX_SECONDS: i64 = 90 * 60;
const MAX_TMDB_ID: i64 = 9_007_199_254_740_992; // 2^53
const MAX_TEXT: usize = 2000;

pub const DEFER_REASONS: [&str; 5] = [
    "insufficient_evidence",
    "multiple_equally_valid_targets",
    "candidate_ambiguity_tmdb",
    "need_external_lookup",
    "other_with_note",
];
pub const CORRECTION_REASONS: [&str; 4] = ["mis_click", "misread_evidence", "new_evidence_in_tmdb", "other_with_note"];

// ─── errors ──────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum BlindError {
    /// 整合性の破れなど。operator の判断なしに進めない
    Stop(String),
    Invalid(String),
    Locked,
    TornTail,
    Tampered(String),
    MainAppRunning,
    SessionLimit(String),
    Sealed,
    Poisoned,
    /// CAS 不一致（副作用ゼロ）
    Conflict,
    UnitAlreadyResolved(String),
    LeaseInvalid(String),
    UnitNotAvailable(String),
    TargetIsCandidate,
    TargetNotVerified(String),
    NoneNeedsSearch,
    AuditFailed(Vec<String>),
    /// init と seal の pin が一致しない（不一致の pin 名）
    PinMismatch(Vec<String>),
}

impl std::fmt::Display for BlindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for BlindError {}

type R<T> = Result<T, BlindError>;

fn stop<T>(m: impl Into<String>) -> R<T> {
    Err(BlindError::Stop(m.into()))
}
fn invalid<T>(m: impl Into<String>) -> R<T> {
    Err(BlindError::Invalid(m.into()))
}

// ─── hash / canonical JSON / time ────────────────────────────────────────────────────────────

pub fn hex_sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn tool_sha256() -> String {
    hex_sha(include_str!("pr4_blind_gt.rs").as_bytes())
}

/// `pr4-canonical-json-1`（PF0E と同じ規則: キーはバイト順・空白なし・整数のみ・null は保持）
pub fn canon(v: &Value) -> R<String> {
    Ok(match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                n.to_string()
            } else {
                return stop("canonical JSON に浮動小数は使えません");
            }
        }
        Value::String(s) => serde_json::to_string(s).map_err(|e| BlindError::Stop(e.to_string()))?,
        Value::Array(a) => format!("[{}]", a.iter().map(canon).collect::<R<Vec<_>>>()?.join(",")),
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            let mut parts = Vec::with_capacity(keys.len());
            for k in keys {
                parts.push(format!(
                    "{}:{}",
                    serde_json::to_string(k).map_err(|e| BlindError::Stop(e.to_string()))?,
                    canon(&o[k])?
                ));
            }
            format!("{{{}}}", parts.join(","))
        }
    })
}

fn sha_of(v: &Value) -> R<String> {
    Ok(hex_sha(canon(v)?.as_bytes()))
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_time(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() == 20
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && b[19] == b'Z'
        && t.bytes().enumerate().all(|(i, c)| matches!(i, 4 | 7 | 10 | 13 | 16 | 19) || c.is_ascii_digit())
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn epoch(t: &str) -> R<i64> {
    if !valid_time(t) {
        return invalid(format!("時刻の形式が不正です: {t}"));
    }
    let n = |a: usize, b: usize| t[a..b].parse::<i64>().unwrap_or(-1);
    let (y, mo, d, h, mi, s) = (n(0, 4), n(5, 7), n(8, 10), n(11, 13), n(14, 16), n(17, 19));
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 59 {
        return invalid(format!("時刻の値が不正です: {t}"));
    }
    Ok(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + s)
}

pub fn fmt_epoch(e: i64) -> String {
    let days = e.div_euclid(86400);
    let rem = e.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

// ─── opaque review_unit_id / 提示順 / QC 抽出 ───────────────────────────────────────────────

fn base32_lower(bytes: &[u8]) -> String {
    const A: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for &b in bytes {
        buf = (buf << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(A[((buf >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(A[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// unit key = `work:<base10 integer>`。先頭ゼロ・符号・空白は禁止（1 以上）。reviewer には見せない
pub fn validate_unit_key(key: &str) -> R<()> {
    let ok = key.strip_prefix("work:").map(|n| !n.is_empty() && n.len() <= 18 && n.bytes().all(|c| c.is_ascii_digit()) && !n.starts_with('0')).unwrap_or(false);
    if ok {
        Ok(())
    } else {
        invalid("unit key は work:<base10 整数>（先頭ゼロ・符号・空白禁止）")
    }
}

fn hex_decode(h: &str) -> R<Vec<u8>> {
    if !is_hex64(h) {
        return invalid("64 桁 hex が必要です");
    }
    Ok((0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0)).collect())
}

/// HMAC-SHA256（RFC 2104）。key は 64 byte 以下
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

pub const CANDIDATE_TOKEN_DOMAIN: &str = "pr4-pf0g-candidate-token-1";

/// `ct_` + 20 文字。`HMAC-SHA256(build_secret, domain \n review_unit_id \n media_type \n tmdb_id)`。
/// **packager だけが計算できる**（reviewer 側に build_secret は無い）。順位・index を含意しない
pub fn derive_candidate_token(build_secret_hex: &str, review_unit_id: &str, media_type: &str, tmdb_id: i64) -> R<String> {
    let key = hex_decode(build_secret_hex)?;
    if !valid_unit_id(review_unit_id) {
        return invalid("review_unit_id が不正です");
    }
    check_target(tmdb_id, media_type)?;
    let mac = hmac_sha256(&key, format!("{CANDIDATE_TOKEN_DOMAIN}\n{review_unit_id}\n{media_type}\n{tmdb_id}").as_bytes());
    Ok(format!("ct_{}", &base32_lower(&mac[..13])[..20]))
}

/// pick_other が表示候補と重なるかを reviewer 側で判定するための salted digest（token とは別の値で、対応は付かない）
pub fn overlap_digest(unit_salt: &str, media_type: &str, tmdb_id: i64) -> R<String> {
    if !is_hex64(unit_salt) {
        return invalid("unit_salt が不正です");
    }
    check_target(tmdb_id, media_type)?;
    Ok(hex_sha(format!("{unit_salt}|overlap|{media_type}|{tmdb_id}").as_bytes()))
}

/// `ru_` + 26 文字（128 bit）。`build_secret` を知らない限り work / ledger キーと結び付かない
pub fn derive_review_unit_id(build_secret_hex: &str, evaluation_unit_key: &str) -> R<String> {
    if !is_hex64(build_secret_hex) {
        return invalid("build_secret は 64 桁 hex");
    }
    validate_unit_key(evaluation_unit_key)?;
    let d = Sha256::digest(format!("{build_secret_hex}|unit-id|{evaluation_unit_key}").as_bytes());
    Ok(format!("ru_{}", &base32_lower(&d[..16])[..26]))
}

/// 提示順。入力は secret と key だけ（rank / score / decision は入力にならない）。返すのは position（0 起点）
pub fn assign_positions(build_secret_hex: &str, keys: &[String]) -> R<Vec<(String, usize)>> {
    if !is_hex64(build_secret_hex) {
        return invalid("build_secret は 64 桁 hex");
    }
    for k in keys {
        validate_unit_key(k)?;
    }
    let mut v: Vec<(String, &String)> = keys
        .iter()
        .map(|k| (hex_sha(format!("{build_secret_hex}|order|{k}").as_bytes()), k))
        .collect();
    v.sort();
    Ok(v.into_iter().enumerate().map(|(i, (_, k))| (k.clone(), i)).collect())
}

/// QC repeat の抽出。seed は review 開始前に freeze する。k = ceil(n × pct / 100)
pub fn qc_select(seed_hex: &str, unit_ids: &[String], pct: u32) -> R<Vec<String>> {
    if !is_hex64(seed_hex) {
        return invalid("qc seed は 64 桁 hex");
    }
    if pct == 0 || pct > 100 {
        return invalid("qc pct は 1..=100");
    }
    let mut v: Vec<(String, &String)> =
        unit_ids.iter().map(|k| (hex_sha(format!("{seed_hex}|qc|{k}").as_bytes()), k)).collect();
    v.sort();
    let k = (unit_ids.len() * pct as usize).div_ceil(100);
    let mut out: Vec<String> = v.into_iter().take(k).map(|(_, id)| id.clone()).collect();
    out.sort();
    Ok(out)
}

fn valid_unit_id(id: &str) -> bool {
    id.len() == 29 && id.starts_with("ru_") && id[3..].bytes().all(|c| matches!(c, b'a'..=b'z' | b'2'..=b'7'))
}

// ─── reviewer artifact（whitelist 射影のみ）─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceFile {
    pub part_no: i64,
    pub original_file_name: String,
    pub original_rel_path: String,
    pub container_tags_json: String,
    pub source_media_kind: String,
}

/// `display_title` は持たない（契約 Q3: 見せない）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub files: Vec<EvidenceFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    /// 不透明な token（`HMAC-SHA256(build_secret, domain + review_unit_id + media_type + tmdb_id)`）。順位・index を含意しない。
    /// token → TMDB identity の対応表は reviewer package に置かない（seal 後の unblind map だけ）
    pub candidate_token: String,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub original_language: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewUnit {
    pub schema_version: String,
    pub review_unit_id: String,
    pub position: i64,
    /// この unit だけの salt（build_secret ではない）。`overlap_digests` の計算にだけ使う
    pub unit_salt: String,
    /// 表示候補の `overlap_digest` の昇順一覧。token との対応は付かない（順序は無関係）。reviewer 側の pick_other 重複判定用
    pub overlap_digests: Vec<String>,
    pub evidence: Evidence,
    pub candidates: Vec<Candidate>,
    pub unit_sha256: String,
}

impl ReviewUnit {
    pub fn compute_sha(&self) -> R<String> {
        let mut v = serde_json::to_value(self).map_err(|e| BlindError::Stop(e.to_string()))?;
        v.as_object_mut().map(|o| o.remove("unit_sha256"));
        sha_of(&v)
    }

    /// unit_sha256 を埋める（テスト・PF0G2 の packager 用）
    pub fn sealed(mut self) -> R<Self> {
        self.unit_sha256 = self.compute_sha()?;
        Ok(self)
    }

    pub fn parse(text: &str) -> R<Self> {
        let unit: ReviewUnit = serde_json::from_str(text).map_err(|e| BlindError::Invalid(format!("unit を読めません: {e}")))?;
        unit.validate()?;
        Ok(unit)
    }

    pub fn validate(&self) -> R<()> {
        if self.schema_version != UNIT_SCHEMA {
            return invalid("unit.schema_version が違います");
        }
        if !valid_unit_id(&self.review_unit_id) {
            return invalid("review_unit_id の形式が不正です");
        }
        if self.position < 0 {
            return invalid("position が負です");
        }
        if self.evidence.files.is_empty() {
            return invalid("evidence.files が空です");
        }
        if !is_hex64(&self.unit_salt) {
            return invalid("unit_salt が不正です");
        }
        let mut digests = BTreeSet::new();
        if self.overlap_digests.len() != self.candidates.len()
            || !self.overlap_digests.iter().all(|d| is_hex64(d) && digests.insert(d.clone()))
            || !self.overlap_digests.windows(2).all(|w| w[0] < w[1])
        {
            return invalid("overlap_digests が不正です（件数・形式・昇順・重複）");
        }
        let mut tokens = BTreeSet::new();
        for c in &self.candidates {
            if !valid_token(&c.candidate_token) || !tokens.insert(c.candidate_token.clone()) {
                return invalid("candidate_token が不正または重複しています");
            }
        }
        if !scan_forbidden_keys(&serde_json::to_value(self).map_err(|e| BlindError::Stop(e.to_string()))?).is_empty() {
            return stop("unit に禁止キーがあります");
        }
        if self.compute_sha()? != self.unit_sha256 {
            return invalid("unit_sha256 が一致しません");
        }
        Ok(())
    }

    fn find(&self, candidate_token: &str) -> Option<&Candidate> {
        self.candidates.iter().find(|c| c.candidate_token == candidate_token)
    }
    /// (media_type, tmdb_id) が候補と重なるか。identity の一覧ではなく salted digest の集合で判定する
    fn has_target(&self, tmdb_id: i64, media_type: &str) -> bool {
        overlap_digest(&self.unit_salt, media_type, tmdb_id).map(|d| self.overlap_digests.contains(&d)).unwrap_or(false)
    }
}

pub fn derive_unit_salt(build_secret_hex: &str, evaluation_unit_key: &str) -> R<String> {
    if !is_hex64(build_secret_hex) {
        return invalid("build_secret は 64 桁 hex");
    }
    Ok(hex_sha(format!("{build_secret_hex}|unit-salt|{evaluation_unit_key}").as_bytes()))
}

fn valid_token(t: &str) -> bool {
    t.len() == 23 && t.starts_with("ct_") && t[3..].bytes().all(|c| matches!(c, b'a'..=b'z' | b'2'..=b'7'))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: String,
    pub build_id: String,
    pub unit_count: i64,
    pub order_commitment_sha256: String,
    pub artifact_sha256: String,
    pub machine_freeze_commitment_sha256: String,
    pub pf0e_ledger_head_sha256: String,
    pub pf0e_record_count: i64,
    pub packager_sha256: String,
    pub built_at_utc: String,
}

pub fn artifact_sha256(units: &BTreeMap<String, ReviewUnit>) -> String {
    let mut s = String::new();
    for (id, u) in units {
        s.push_str(&format!("{id}:{}\n", u.unit_sha256));
    }
    hex_sha(s.as_bytes())
}

pub fn order_commitment(units: &BTreeMap<String, ReviewUnit>) -> String {
    let mut v: Vec<(&i64, &String)> = units.iter().map(|(id, u)| (&u.position, id)).collect();
    v.sort();
    hex_sha(v.iter().map(|(_, id)| id.as_str()).collect::<Vec<_>>().join("\n").as_bytes())
}

#[derive(Debug, Clone)]
pub struct Artifact {
    pub manifest: Manifest,
    pub manifest_sha256: String,
    pub units: BTreeMap<String, ReviewUnit>,
}

impl Artifact {
    pub fn from_parts(manifest: Manifest, units: Vec<ReviewUnit>) -> R<Self> {
        if manifest.schema_version != MANIFEST_SCHEMA {
            return invalid("manifest.schema_version が違います");
        }
        for h in [
            &manifest.order_commitment_sha256,
            &manifest.artifact_sha256,
            &manifest.machine_freeze_commitment_sha256,
            &manifest.pf0e_ledger_head_sha256,
            &manifest.packager_sha256,
        ] {
            if !is_hex64(h) {
                return invalid("manifest の hash が不正です");
            }
        }
        if !valid_time(&manifest.built_at_utc) {
            return invalid("manifest.built_at_utc が不正です");
        }
        let mut map = BTreeMap::new();
        for u in units {
            u.validate()?;
            if map.insert(u.review_unit_id.clone(), u).is_some() {
                return invalid("review_unit_id が重複しています");
            }
        }
        if manifest.unit_count < 1 || manifest.unit_count as usize != map.len() {
            return invalid("manifest.unit_count と unit 数が一致しません");
        }
        let mut positions: Vec<i64> = map.values().map(|u| u.position).collect();
        positions.sort();
        if positions != (0..map.len() as i64).collect::<Vec<_>>() {
            return invalid("position が 0..n の順列ではありません");
        }
        if order_commitment(&map) != manifest.order_commitment_sha256 {
            return invalid("order_commitment が一致しません");
        }
        if artifact_sha256(&map) != manifest.artifact_sha256 {
            return invalid("artifact_sha256 が一致しません");
        }
        let manifest_sha256 = sha_of(&serde_json::to_value(&manifest).map_err(|e| BlindError::Stop(e.to_string()))?)?;
        Ok(Artifact { manifest, manifest_sha256, units: map })
    }

    /// `manifest.json` と `units/<id>.json`。欠け・余分・名前違いは起動拒否（既定値で救わない）
    pub fn load(dir: &Path) -> R<Self> {
        check_reviewer_package(dir)?;
        let rd = |p: PathBuf| std::fs::read_to_string(&p).map_err(|e| BlindError::Invalid(format!("{} を読めません: {e}", p.display())));
        let manifest: Manifest = serde_json::from_str(&rd(dir.join("manifest.json"))?)
            .map_err(|e| BlindError::Invalid(format!("manifest を読めません: {e}")))?;
        let mut units = Vec::new();
        let entries = std::fs::read_dir(dir.join("units")).map_err(|e| BlindError::Invalid(e.to_string()))?;
        for entry in entries {
            let entry = entry.map_err(|e| BlindError::Invalid(e.to_string()))?;
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(id) = name.strip_suffix(".json") else {
                return invalid(format!("units に余分なファイル: {name}"));
            };
            if !valid_unit_id(id) {
                return invalid(format!("unit のファイル名が不正: {name}"));
            }
            let unit = ReviewUnit::parse(&rd(entry.path())?)?;
            if unit.review_unit_id != id {
                return invalid("unit のファイル名と review_unit_id が違います");
            }
            units.push(unit);
        }
        Artifact::from_parts(manifest, units)
    }
}

fn check_target(tmdb_id: i64, media_type: &str) -> R<()> {
    if !(1..=MAX_TMDB_ID).contains(&tmdb_id) {
        return invalid("tmdb_id が不正です");
    }
    if !matches!(media_type, "movie" | "tv") {
        return invalid("media_type が不正です");
    }
    Ok(())
}

/// 漏洩の走査（キーと値）。テスト・A8 用。`source_media_kind` は raw 証拠の正規キーなので除外
pub fn scan_forbidden(v: &Value) -> Vec<String> {
    scan_impl(v, true)
}

/// キーだけの走査（作品名などの自由文を誤検出しない）。unit の validate が使う
pub fn scan_forbidden_keys(v: &Value) -> Vec<String> {
    scan_impl(v, false)
}

fn scan_impl(v: &Value, values: bool) -> Vec<String> {
    const KEYS: [&str; 22] = [
        "decision", "verdict", "score", "rank", "rules", "jev", "matcher", "source", "promotion", "precision",
        "coverage", "abstention", "workid", "runid", "evaluationunitkey", "filepath", "firstseenat", "candidatecount",
        "buildsecret", "unblind", "candidatemapping", "tmdbid",
    ];
    let mut out = Vec::new();
    fn walk(v: &Value, path: &str, out: &mut Vec<String>, values: bool) {
        match v {
            Value::Object(o) => {
                for (k, x) in o {
                    let norm: String = k.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
                    if k != "source_media_kind" && KEYS.iter().any(|f| norm.contains(f)) {
                        out.push(format!("key:{path}/{k}"));
                    }
                    walk(x, &format!("{path}/{k}"), out, values);
                }
            }
            Value::Array(a) => a.iter().enumerate().for_each(|(i, x)| walk(x, &format!("{path}/{i}"), out, values)),
            Value::String(s) if values => {
                let t = s.trim().to_uppercase();
                if matches!(t.as_str(), "AUTO" | "REVIEW" | "UNRESOLVED") || s.starts_with("work:") {
                    out.push(format!("value:{path}"));
                }
            }
            _ => {}
        }
    }
    walk(v, "", &mut out, values);
    out
}

// ─── 入力（提出）──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct TmdbVerification {
    pub requested_id: i64,
    pub requested_media_type: String,
    pub http_status: i64,
    pub response_id: i64,
    pub response_media_type: String,
    pub response_body_sha256: String,
    pub title: String,
    pub year: Option<String>,
    pub fetched_at: String,
    pub verifier_impl: String,
}

impl TmdbVerification {
    fn to_value(&self) -> Value {
        json!({"requested_id": self.requested_id, "requested_media_type": self.requested_media_type,
               "http_status": self.http_status, "response_id": self.response_id,
               "response_media_type": self.response_media_type, "response_body_sha256": self.response_body_sha256,
               "title": self.title, "year": self.year, "fetched_at": self.fetched_at, "verifier_impl": self.verifier_impl})
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Confirm { candidate_token: String },
    PickOther { tmdb_id: i64, media_type: String, verification: Option<TmdbVerification> },
    None,
    Defer { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Submit {
    pub unit: String,
    pub submission_id: String,
    pub lease_id: String,
    pub prev_event_sha: String,
    pub decision: Decision,
    pub note: Option<String>,
    /// correction のときだけ
    pub correction_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SubmitOutcome {
    Recorded { seq: u64 },
    /// 同じ submission_id・同内容の再送（event は増えない）
    Duplicate { seq: u64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LeaseGrant {
    pub lease_id: String,
    pub unit: String,
    pub purpose: String,
    /// この値を提出時の `prev_event_sha` に使う
    pub prev_event_sha: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    Unit { unit: String, purpose: &'static str },
    Done,
}

pub trait MainAppProbe {
    fn main_app_running(&self) -> bool;
}
pub struct NoMainApp;
impl MainAppProbe for NoMainApp {
    fn main_app_running(&self) -> bool {
        false
    }
}

// ─── 状態（ledger からの再構築）─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Status {
    Unopened,
    Deferred,
    Resolved,
    DeferFinal,
}

#[derive(Debug, Clone, PartialEq)]
struct Effective {
    decision: String,
    candidate_token: Option<String>,
    tmdb_id: Option<i64>,
    media_type: Option<String>,
    has_verification: bool,
    search_count: usize,
    reviewer_id: String,
}

#[derive(Debug, Clone)]
struct LeaseState {
    lease_id: String,
    session_id: String,
    purpose: String,
    started: String,
    expires_at: String,
    searches: usize,
}

#[derive(Debug, Clone)]
struct UnitState {
    status: Status,
    pass: u8,
    last: String,
    effective: Option<Effective>,
    lease: Option<LeaseState>,
    corrections: u32,
    qc: Option<(bool, String)>,
}

#[derive(Debug, Clone)]
struct SessionState {
    reviewer_id: String,
    started: String,
    ended: bool,
    units: u32,
}

#[derive(Debug, Clone, Default)]
struct State {
    count: u64,
    head: String,
    commitments: Vec<String>,
    last_ts: String,
    inited: bool,
    manifest_sha: String,
    artifact_sha: String,
    anchored_at: String,
    pins: Option<Pins>,
    review_seal: Option<String>,
    tool_sha: Option<String>,
    qc_plan: Option<Vec<String>>,
    first_session_ts: Option<String>,
    units: BTreeMap<String, UnitStateWrap>,
    sessions: BTreeMap<String, SessionState>,
    submissions: HashMap<String, (String, u64)>,
    duplicate_submission_ids: u32,
    poisoned: bool,
    sealed: bool,
    anomalies: u32,
}

/// `Default` を使うための薄い包み
#[derive(Debug, Clone)]
struct UnitStateWrap(UnitState);

fn content_sha(s: &Submit) -> R<String> {
    let (kind, cref, tid, mt, reason) = match &s.decision {
        Decision::Confirm { candidate_token } => ("confirm", Some(candidate_token.clone()), None, None, None),
        Decision::PickOther { tmdb_id, media_type, .. } => ("pick_other", None, Some(*tmdb_id), Some(media_type.clone()), None),
        Decision::None => ("none", None, None, None, None),
        Decision::Defer { reason } => ("defer", None, None, None, Some(reason.clone())),
    };
    sha_of(&json!({"unit": s.unit, "decision": kind, "candidate_token": cref, "tmdb_id": tid, "media_type": mt,
                    "defer_reason": reason, "note": s.note, "correction_reason": s.correction_reason}))
}

// ─── apply: 1 event を state に適用（open 時の replay と新規 append で同じ関数）────────────

fn s_of<'a>(e: &'a Value, k: &str) -> R<&'a str> {
    e.get(k).and_then(Value::as_str).ok_or_else(|| BlindError::Tampered(format!("event.{k} がありません")))
}

fn ustate<'a>(st: &'a mut State, unit: &str) -> R<&'a mut UnitState> {
    st.units.get_mut(unit).map(|w| &mut w.0).ok_or_else(|| BlindError::Tampered("存在しない unit です".to_string()))
}

fn check_note(note: Option<&str>) -> R<()> {
    if let Some(n) = note {
        if n.chars().count() > MAX_TEXT || n.chars().any(|c| c.is_control() && c != '\n') {
            return invalid("note が不正です");
        }
    }
    Ok(())
}

fn apply(st: &mut State, e: &Value, art: &Artifact) -> R<()> {
    let ty = s_of(e, "type")?.to_string();
    let ts = s_of(e, "ts")?.to_string();
    let t = epoch(&ts)?;
    if st.count > 0 && t < epoch(&st.last_ts)? {
        return stop("時刻が巻き戻っています");
    }
    if st.poisoned && ty != "integrity_anomaly" {
        return Err(BlindError::Poisoned);
    }
    if st.sealed {
        return Err(BlindError::Sealed);
    }
    let commitment = s_of(e, "event_commitment")?.to_string();
    if ty != "init" && !st.inited {
        return stop("init より前の event です");
    }

    match ty.as_str() {
        "init" => {
            if st.inited || st.count != 0 {
                return stop("init が重複しています");
            }
            if s_of(e, "manifest_sha256")? != art.manifest_sha256 || s_of(e, "artifact_sha256")? != art.manifest.artifact_sha256 {
                return stop("init が artifact と一致しません");
            }
            if e.get("unit_count").and_then(Value::as_i64) != Some(art.manifest.unit_count) {
                return stop("init.unit_count が一致しません");
            }
            let pins = Pins::from_event(e)?;
            if !valid_time(s_of(e, "anchored_at")?) {
                return stop("anchored_at が不正です");
            }
            st.pins = Some(pins);
            st.inited = true;
            st.manifest_sha = art.manifest_sha256.clone();
            st.artifact_sha = art.manifest.artifact_sha256.clone();
            st.anchored_at = s_of(e, "anchored_at")?.to_string();
            for (id, u) in &art.units {
                st.units.insert(
                    id.clone(),
                    UnitStateWrap(UnitState {
                        status: Status::Unopened,
                        pass: 1,
                        last: hex_sha(format!("unit0|{}", u.unit_sha256).as_bytes()),
                        effective: None,
                        lease: None,
                        corrections: 0,
                        qc: None,
                    }),
                );
            }
        }
        "qc_plan" => {
            if st.qc_plan.is_some() || st.first_session_ts.is_some() {
                return stop("qc_plan は review 開始前に 1 回だけ");
            }
            let ids: Vec<String> = art.units.keys().cloned().collect();
            let pct = e.get("pct").and_then(Value::as_u64).unwrap_or(0) as u32;
            let sel = qc_select(s_of(e, "seed_hex")?, &ids, pct)?;
            let recorded: Vec<String> = e
                .get("selected")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            if sel != recorded {
                return stop("qc 抽出が seed から再現できません");
            }
            st.qc_plan = Some(sel);
        }
        "session_start" => {
            let sid = s_of(e, "session_id")?.to_string();
            if st.qc_plan.is_none() {
                return stop("qc_plan の前に session は開始できません");
            }
            if e.get("attestation") != Some(&json!(true)) {
                return invalid("attestation が必要です");
            }
            if s_of(e, "artifact_sha256")? != st.artifact_sha {
                return stop("artifact_sha256 が違います");
            }
            let tool = s_of(e, "tool_sha256")?.to_string();
            match &st.tool_sha {
                Some(prev) if *prev != tool => return stop("tool_sha256 が前の session と違います"),
                _ => st.tool_sha = Some(tool),
            }
            if st.sessions.values().any(|s| !s.ended) {
                return stop("終了していない session があります");
            }
            if t <= epoch(&st.anchored_at)? {
                return stop("machine freeze の anchor より前に review は始められません");
            }
            if st.first_session_ts.is_none() {
                st.first_session_ts = Some(ts.clone());
            }
            st.sessions.insert(
                sid,
                SessionState { reviewer_id: s_of(e, "reviewer_id")?.to_string(), started: ts.clone(), ended: false, units: 0 },
            );
        }
        "session_end" => {
            let s = st.sessions.get_mut(s_of(e, "session_id")?).ok_or_else(|| BlindError::Tampered("session がありません".into()))?;
            if s.ended {
                return stop("session が二重に終了しています");
            }
            s.ended = true;
        }
        "lease" => {
            let (sid, unit, purpose) = (s_of(e, "session_id")?.to_string(), s_of(e, "unit")?.to_string(), s_of(e, "purpose")?.to_string());
            let lease_id = s_of(e, "lease_id")?.to_string();
            let expires = s_of(e, "expires_at")?.to_string();
            if epoch(&expires)? != t + LEASE_SECONDS {
                return stop("lease の期限が規定と違います");
            }
            let any_unopened = st.units.values().any(|u| u.0.status == Status::Unopened);
            let any_deferred = st.units.values().any(|u| u.0.status == Status::Deferred);
            let qc_plan = st.qc_plan.clone().unwrap_or_default();
            {
                let sess = st.sessions.get(&sid).ok_or_else(|| BlindError::Tampered("session がありません".into()))?;
                if sess.ended {
                    return stop("終了した session の lease");
                }
                if sess.units >= SESSION_MAX_UNITS {
                    return Err(BlindError::SessionLimit("1 session 50 unit まで".into()));
                }
                if t - epoch(&sess.started)? > SESSION_MAX_SECONDS {
                    return Err(BlindError::SessionLimit("1 session 90 分まで".into()));
                }
            }
            if st.units.values().any(|u| u.0.lease.as_ref().map(|l| l.session_id == sid).unwrap_or(false)) {
                return Err(BlindError::LeaseInvalid("session は同時に 1 つの lease まで".into()));
            }
            let u = ustate(st, &unit)?;
            if u.lease.is_some() {
                return Err(BlindError::UnitNotAvailable("別の lease が有効です".into()));
            }
            match (purpose.as_str(), &u.status) {
                ("review", Status::Unopened) => {}
                ("review", Status::Deferred) => {
                    if any_unopened {
                        return Err(BlindError::UnitNotAvailable("保留は本流が終わってから".into()));
                    }
                    u.pass = 2;
                }
                ("correction", Status::Resolved | Status::DeferFinal) => {}
                ("qc", Status::Resolved | Status::DeferFinal) => {
                    if any_unopened || any_deferred {
                        return Err(BlindError::UnitNotAvailable("QC は本流が終わってから".into()));
                    }
                    if !qc_plan.contains(&unit) || u.qc.is_some() {
                        return Err(BlindError::UnitNotAvailable("QC の対象ではない、または実施済み".into()));
                    }
                }
                _ => return Err(BlindError::UnitNotAvailable(format!("{purpose} は今この unit に使えません"))),
            }
            if e.get("unit_prev").and_then(Value::as_str) != Some(u.last.as_str()) {
                return stop("lease の unit_prev が一致しません");
            }
            u.lease = Some(LeaseState { lease_id, session_id: sid.clone(), purpose, started: ts.clone(), expires_at: expires, searches: 0 });
            u.last = commitment.clone();
            st.sessions.get_mut(&sid).map(|s| s.units += 1);
        }
        "lease_expired" | "lease_release" => {
            let unit = s_of(e, "unit")?.to_string();
            let lease_id = s_of(e, "lease_id")?.to_string();
            let u = ustate(st, &unit)?;
            let l = u.lease.clone().ok_or_else(|| BlindError::Tampered("lease がありません".into()))?;
            if l.lease_id != lease_id {
                return stop("lease_id が違います");
            }
            if ty == "lease_expired" && t < epoch(&l.expires_at)? {
                return stop("期限前に lease_expired が記録されています");
            }
            u.lease = None;
            u.last = commitment.clone();
        }
        "search" => {
            let unit = s_of(e, "unit")?.to_string();
            let u = ustate(st, &unit)?;
            let l = u.lease.as_mut().ok_or_else(|| BlindError::LeaseInvalid("lease がありません".into()))?;
            if l.lease_id != s_of(e, "lease_id")? || t >= epoch(&l.expires_at)? {
                return Err(BlindError::LeaseInvalid("lease が違う、または期限切れ".into()));
            }
            if s_of(e, "query")?.trim().is_empty() || !matches!(s_of(e, "media_type")?, "movie" | "tv" | "multi") {
                return invalid("search が不正です");
            }
            if e.get("result_count").and_then(Value::as_i64).map(|n| n < 0).unwrap_or(true) {
                return invalid("result_count が不正です");
            }
            l.searches += 1;
            u.last = commitment.clone();
        }
        "decision" | "correction" | "qc_decision" | "defer" | "defer_final" => apply_outcome(st, e, art, &ty, t, &ts, &commitment)?,
        "integrity_anomaly" => {
            st.poisoned = true;
            st.anomalies += 1;
        }
        "seal" => {
            let failures = audit(st, art).failures();
            if !failures.is_empty() {
                return Err(BlindError::AuditFailed(failures));
            }
            let init_pins = st.pins.clone().ok_or_else(|| BlindError::Tampered("init の pin がありません".into()))?;
            let mismatch = init_pins.diff(&Pins::from_event(e)?);
            if !mismatch.is_empty() {
                return Err(BlindError::PinMismatch(mismatch));
            }
            st.sealed = true;
            st.review_seal = Some(commitment.clone());
        }
        other => return stop(format!("未知の event: {other}")),
    }
    st.count += 1;
    st.head = commitment.clone();
    st.commitments.push(commitment);
    st.last_ts = ts;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_outcome(st: &mut State, e: &Value, art: &Artifact, ty: &str, t: i64, _ts: &str, commitment: &str) -> R<()> {
    let unit_id = s_of(e, "unit")?.to_string();
    let unit = art.units.get(&unit_id).ok_or_else(|| BlindError::Tampered("artifact に無い unit".into()))?;
    let sub_id = s_of(e, "submission_id")?.to_string();
    if st.submissions.contains_key(&sub_id) {
        st.duplicate_submission_ids += 1;
        return stop("submission_id が重複しています");
    }
    check_note(e.get("note").and_then(Value::as_str))?;
    let reviewer = st
        .sessions
        .get(s_of(e, "session_id")?)
        .map(|s| s.reviewer_id.clone())
        .ok_or_else(|| BlindError::Tampered("session がありません".into()))?;
    let count = st.count;
    let u = ustate(st, &unit_id)?;
    let l = u.lease.clone().ok_or_else(|| BlindError::LeaseInvalid("lease がありません".into()))?;
    if l.lease_id != s_of(e, "lease_id")? || l.session_id != s_of(e, "session_id")? {
        return Err(BlindError::LeaseInvalid("lease が違います".into()));
    }
    if t >= epoch(&l.expires_at)? {
        return Err(BlindError::LeaseInvalid("lease が期限切れです".into()));
    }
    if e.get("unit_prev").and_then(Value::as_str) != Some(u.last.as_str()) {
        return Err(BlindError::Conflict);
    }
    let expect_purpose = match ty {
        "decision" | "defer" | "defer_final" => "review",
        "correction" => "correction",
        _ => "qc",
    };
    if l.purpose != expect_purpose {
        return Err(BlindError::LeaseInvalid("lease の用途が違います".into()));
    }

    if ty == "defer" || ty == "defer_final" {
        let reason = s_of(e, "defer_reason")?;
        if !DEFER_REASONS.contains(&reason) {
            return invalid("defer_reason が不正です");
        }
        if reason == "other_with_note" && e.get("note").and_then(Value::as_str).map(str::trim).unwrap_or("").is_empty() {
            return invalid("other_with_note には note が必要です");
        }
        // 1 巡目の defer は deferred、2 巡目の defer は defer_final
        match (ty, u.pass) {
            ("defer", 1) => u.status = Status::Deferred,
            ("defer_final", 2) => u.status = Status::DeferFinal,
            _ => return invalid("defer の巡が違います"),
        }
        u.effective = Some(Effective {
            decision: "defer".into(),
            candidate_token: None,
            tmdb_id: None,
            media_type: None,
            has_verification: false,
            search_count: l.searches,
            reviewer_id: reviewer,
        });
        u.lease = None;
        u.last = commitment.to_string();
        st.submissions.insert(sub_id, (s_of(e, "content_sha256")?.to_string(), count));
        return Ok(());
    }

    let decision = s_of(e, "decision")?;
    let searches = e.get("search_count").and_then(Value::as_u64).unwrap_or(u64::MAX) as usize;
    if searches != l.searches {
        return stop("search_log が lease の記録と一致しません");
    }
    let eff = match decision {
        "confirm" => {
            let c = unit.find(s_of(e, "candidate_token")?).ok_or_else(|| BlindError::Invalid("candidate_token が unit にありません".into()))?;
            // confirm の event は identity（tmdb_id / media_type / index）を持たない。対応表は unblind map だけ
            if e.get("tmdb_id").is_some() || e.get("media_type").is_some() || e.get("candidate_ref").is_some() {
                return stop("confirm の event に identity が付いています");
            }
            Effective { decision: "confirm".into(), candidate_token: Some(c.candidate_token.clone()), tmdb_id: None, media_type: None, has_verification: false, search_count: l.searches, reviewer_id: reviewer.clone() }
        }
        "pick_other" => {
            let tid = e.get("tmdb_id").and_then(Value::as_i64).unwrap_or(0);
            let mt = s_of(e, "media_type")?.to_string();
            check_target(tid, &mt)?;
            if unit.has_target(tid, &mt) {
                return Err(BlindError::TargetIsCandidate);
            }
            let v = e.get("tmdb_verification").filter(|v| v.is_object()).ok_or_else(|| BlindError::TargetNotVerified("検証記録がありません".into()))?;
            let ok = v.get("http_status").and_then(Value::as_i64) == Some(200)
                && v.get("response_id").and_then(Value::as_i64) == Some(tid)
                && v.get("requested_id").and_then(Value::as_i64) == Some(tid)
                && v.get("response_media_type").and_then(Value::as_str) == Some(mt.as_str())
                && v.get("requested_media_type").and_then(Value::as_str) == Some(mt.as_str())
                && v.get("response_body_sha256").and_then(Value::as_str).map(is_hex64).unwrap_or(false)
                && v.get("fetched_at").and_then(Value::as_str).map(valid_time).unwrap_or(false);
            if !ok {
                return Err(BlindError::TargetNotVerified("検証記録が要求と一致しません".into()));
            }
            Effective { decision: "pick_other".into(), candidate_token: None, tmdb_id: Some(tid), media_type: Some(mt), has_verification: true, search_count: l.searches, reviewer_id: reviewer.clone() }
        }
        "none" => {
            if l.searches < 1 {
                return Err(BlindError::NoneNeedsSearch);
            }
            if !e.get("tmdb_id").map(Value::is_null).unwrap_or(false) || !e.get("media_type").map(Value::is_null).unwrap_or(false) {
                return stop("none に identity が付いています");
            }
            Effective { decision: "none".into(), candidate_token: None, tmdb_id: None, media_type: None, has_verification: false, search_count: l.searches, reviewer_id: reviewer.clone() }
        }
        _ => return invalid("decision が不正です"),
    };

    match ty {
        "decision" => {
            if !matches!(u.status, Status::Unopened | Status::Deferred) {
                return Err(BlindError::UnitAlreadyResolved(unit_id));
            }
            u.status = Status::Resolved;
            u.effective = Some(eff);
        }
        "correction" => {
            let reason = s_of(e, "correction_reason")?;
            if !CORRECTION_REASONS.contains(&reason) {
                return invalid("correction_reason が不正です");
            }
            if reason == "other_with_note" && e.get("note").and_then(Value::as_str).map(str::trim).unwrap_or("").is_empty() {
                return invalid("other_with_note には note が必要です");
            }
            if !matches!(u.status, Status::Resolved | Status::DeferFinal) {
                return invalid("訂正できる状態ではありません");
            }
            u.status = Status::Resolved;
            u.effective = Some(eff);
            u.corrections += 1;
        }
        _ => {
            // qc_decision: effective を変えない。一致したかだけを記録する
            let prev = u.effective.clone().ok_or_else(|| BlindError::Tampered("QC 対象に decision がありません".into()))?;
            let agree = prev.decision == eff.decision && prev.candidate_token == eff.candidate_token && prev.tmdb_id == eff.tmdb_id && prev.media_type == eff.media_type;
            u.qc = Some((agree, reviewer));
        }
    }
    u.lease = None;
    u.last = commitment.to_string();
    st.submissions.insert(sub_id, (s_of(e, "content_sha256")?.to_string(), count));
    Ok(())
}

// ─── completion audit ────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct AuditReport {
    /// (id, passed, detail)
    pub checks: Vec<(String, bool, String)>,
    pub unit_count: usize,
    pub confirm: usize,
    pub pick_other: usize,
    pub none: usize,
    pub defer_final: usize,
    pub corrections: u32,
    pub qc_total: usize,
    pub qc_done: usize,
    pub qc_agree: usize,
    /// QC を行った reviewer が元の reviewer と **全件で別人**のときだけ true
    pub independent_second_review: bool,
    pub sessions: usize,
}

impl AuditReport {
    pub fn failures(&self) -> Vec<String> {
        self.checks.iter().filter(|c| !c.1).map(|c| format!("{}: {}", c.0, c.2)).collect()
    }
    pub fn passed(&self) -> bool {
        self.failures().is_empty()
    }
    pub fn to_value(&self) -> Value {
        json!({"checks": self.checks.iter().map(|c| json!({"id": c.0, "pass": c.1, "detail": c.2})).collect::<Vec<_>>(),
               "unit_count": self.unit_count, "confirm": self.confirm, "pick_other": self.pick_other, "none": self.none,
               "defer_final": self.defer_final, "corrections": self.corrections, "qc_total": self.qc_total,
               "qc_done": self.qc_done, "qc_agree": self.qc_agree,
               "independent_second_review": self.independent_second_review, "sessions": self.sessions})
    }
}

fn audit(st: &State, art: &Artifact) -> AuditReport {
    let mut checks: Vec<(String, bool, String)> = Vec::new();
    let mut add = |id: &str, ok: bool, d: String| checks.push((id.to_string(), ok, d));
    let n = art.units.len();
    let (mut confirm, mut pick_other, mut none, mut dfinal) = (0, 0, 0, 0);
    let (mut non_terminal, mut bad_decision, mut none_without_search) = (0, 0, 0);
    let (mut corrections, mut qc_done, mut qc_agree, mut leased) = (0, 0, 0, 0);
    let mut all_independent = true;
    for (id, w) in &st.units {
        let u = &w.0;
        corrections += u.corrections;
        if u.lease.is_some() {
            leased += 1;
        }
        if let (Some((agree, qc_reviewer)), Some(eff)) = (&u.qc, &u.effective) {
            qc_done += 1;
            if *agree {
                qc_agree += 1;
            }
            if *qc_reviewer == eff.reviewer_id {
                all_independent = false;
            }
        }
        match (&u.status, &u.effective) {
            (Status::Resolved, Some(eff)) => {
                match eff.decision.as_str() {
                    "confirm" => {
                        confirm += 1;
                        let ok = art.units.get(id).map(|x| eff.candidate_token.as_deref().map(|t| x.find(t).is_some()).unwrap_or(false)).unwrap_or(false)
                            && eff.tmdb_id.is_none()
                            && eff.media_type.is_none();
                        if !ok {
                            bad_decision += 1;
                        }
                    }
                    "pick_other" => {
                        pick_other += 1;
                        let bad = !eff.has_verification
                            || art.units.get(id).map(|x| x.has_target(eff.tmdb_id.unwrap_or(0), eff.media_type.as_deref().unwrap_or(""))).unwrap_or(true);
                        if bad {
                            bad_decision += 1;
                        }
                    }
                    "none" => {
                        none += 1;
                        if eff.search_count < 1 {
                            none_without_search += 1;
                        }
                        if eff.tmdb_id.is_some() || eff.media_type.is_some() {
                            bad_decision += 1;
                        }
                    }
                    _ => bad_decision += 1,
                }
            }
            (Status::DeferFinal, Some(_)) => dfinal += 1,
            _ => non_terminal += 1,
        }
    }
    let qc_total = st.qc_plan.as_ref().map(Vec::len).unwrap_or(0);
    add("A1_chain", st.count > 0 && st.commitments.len() as u64 == st.count, format!("events={}", st.count));
    add("A2_terminal_count", non_terminal == 0 && (confirm + pick_other + none + dfinal) as i64 == art.manifest.unit_count && n as i64 == art.manifest.unit_count, format!("terminal={} of {}", confirm + pick_other + none + dfinal, n));
    add("A3_no_open_state", non_terminal == 0 && leased == 0 && st.sessions.values().all(|s| s.ended), format!("non_terminal={non_terminal} leased={leased}"));
    add("A4_decision_integrity", bad_decision == 0, format!("bad={bad_decision}"));
    add("A5_none_has_search", none_without_search == 0, format!("bad={none_without_search}"));
    add("A6_constant_artifact_tool", st.tool_sha.is_some() && st.artifact_sha == art.manifest.artifact_sha256, "artifact/tool sha constant (enforced at session_start)".into());
    add("A7_no_duplicate_no_anomaly", st.duplicate_submission_ids == 0 && st.anomalies == 0 && !st.poisoned, format!("anomalies={}", st.anomalies));
    add(
        "A9_anchor_precedes_review",
        match (&st.first_session_ts, epoch(&st.anchored_at)) {
            (Some(f), Ok(a)) => epoch(f).map(|x| x > a).unwrap_or(false),
            _ => false,
        },
        format!("anchored_at={}", st.anchored_at),
    );
    add("A11_qc_complete", qc_total > 0 && qc_done == qc_total, format!("qc {qc_done}/{qc_total}"));
    AuditReport {
        checks,
        unit_count: n,
        confirm,
        pick_other,
        none,
        defer_final: dfinal,
        corrections,
        qc_total,
        qc_done,
        qc_agree,
        independent_second_review: qc_total > 0 && qc_done == qc_total && all_independent,
        sessions: st.sessions.len(),
    }
}

// ─── Store（ファイル I/O・lock・checkpoint）──────────────────────────────────────────────

pub struct Store {
    dir: PathBuf,
    cp_dir: PathBuf,
    art: Artifact,
    st: State,
    probe: Box<dyn MainAppProbe>,
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.dir.join(LOCK_FILE));
    }
}

fn read_lines(path: &Path) -> R<Vec<Value>> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return stop(e.to_string()),
    };
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if *bytes.last().unwrap() != b'\n' {
        return Err(BlindError::TornTail);
    }
    let text = String::from_utf8(bytes).map_err(|_| BlindError::Tampered("UTF-8 ではありません".into()))?;
    text.lines().map(|l| serde_json::from_str::<Value>(l).map_err(|_| BlindError::Tampered("ledger の行が壊れています".into()))).collect()
}

fn with_commitment(mut e: Value, seq: u64, prev: &str) -> R<Value> {
    e["schema_version"] = json!(EVENT_SCHEMA);
    e["seq"] = json!(seq);
    e["prev_commitment"] = json!(prev);
    let c = hex_sha(format!("{}\n{}", canon(&e)?, prev).as_bytes());
    e["event_commitment"] = json!(c);
    Ok(e)
}

impl Store {
    /// 新規 ledger（空であること）。`anchor` は独立した audit anchor chain に先に固定した commitment
    pub fn create(
        dir: &Path,
        cp_dir: &Path,
        art: Artifact,
        pins: &Pins,
        anchored_at: &str,
        now: &str,
        probe: Box<dyn MainAppProbe>,
    ) -> R<Self> {
        std::fs::create_dir_all(dir).map_err(|e| BlindError::Stop(e.to_string()))?;
        std::fs::create_dir_all(cp_dir).map_err(|e| BlindError::Stop(e.to_string()))?;
        if dir.join(LEDGER_FILE).exists() {
            return stop("ledger は空ではありません");
        }
        pins.validate()?;
        Self::lock(dir)?;
        let mut s = Store { dir: dir.to_path_buf(), cp_dir: cp_dir.to_path_buf(), art, st: State::default(), probe };
        s.st = State::default();
        let init = json!({"type": "init", "ts": now, "manifest_sha256": s.art.manifest_sha256, "artifact_sha256": s.art.manifest.artifact_sha256,
            "unit_count": s.art.manifest.unit_count, "pins": pins.to_value(), "anchored_at": anchored_at,
            "review_protocol_version": REVIEW_PROTOCOL_VERSION, "core_tool_sha256": tool_sha256()});
        s.commit(init)?;
        Ok(s)
    }

    fn lock(dir: &Path) -> R<()> {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join(LOCK_FILE)) {
            Ok(mut f) => {
                let _ = writeln!(f, "pid={}", std::process::id());
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(BlindError::Locked),
            Err(e) => stop(e.to_string()),
        }
    }

    /// 既存 ledger を開き、chain 全体・checkpoint・意味の遷移を replay で検証する（resume）
    pub fn open(dir: &Path, cp_dir: &Path, art: Artifact, probe: Box<dyn MainAppProbe>) -> R<Self> {
        Self::lock(dir)?;
        let mut s = Store { dir: dir.to_path_buf(), cp_dir: cp_dir.to_path_buf(), art, st: State::default(), probe };
        // 失敗時は Drop が lock を外す
        let events = read_lines(&dir.join(LEDGER_FILE))?;
        if events.is_empty() {
            return stop("ledger が空です");
        }
        let mut prev = GENESIS.to_string();
        let mut st = State::default();
        for (i, e) in events.iter().enumerate() {
            let mut body = e.clone();
            let recorded = body.as_object_mut().and_then(|o| o.remove("event_commitment")).and_then(|v| v.as_str().map(str::to_string));
            let expect = hex_sha(format!("{}\n{}", canon(&body)?, prev).as_bytes());
            if recorded.as_deref() != Some(expect.as_str()) || e.get("seq").and_then(Value::as_u64) != Some(i as u64) || e.get("prev_commitment").and_then(Value::as_str) != Some(prev.as_str()) {
                return Err(BlindError::Tampered(format!("event {i} の commitment が合いません")));
            }
            apply(&mut st, e, &s.art)?;
            prev = expect;
        }
        s.st = st;
        s.verify_checkpoints()?;
        Ok(s)
    }

    fn commit(&mut self, body: Value) -> R<u64> {
        if self.st.sealed {
            return Err(BlindError::Sealed);
        }
        let seq = self.st.count;
        let prev = if seq == 0 { GENESIS.to_string() } else { self.st.head.clone() };
        let e = with_commitment(body, seq, &prev)?;
        let mut next = self.st.clone();
        apply(&mut next, &e, &self.art)?;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join(LEDGER_FILE)).map_err(|e| BlindError::Stop(e.to_string()))?;
        f.write_all(format!("{}\n", canon(&e)?).as_bytes()).map_err(|e| BlindError::Stop(e.to_string()))?;
        f.sync_all().map_err(|e| BlindError::Stop(e.to_string()))?;
        self.st = next;
        Ok(seq)
    }

    // ── checkpoint chain（ledger 末尾削除の検出）──

    pub fn write_checkpoint(&mut self) -> R<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.cp_dir)
            .map_err(|e| BlindError::Stop(e.to_string()))?
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
            .collect();
        names.sort();
        let prev = match names.last() {
            Some(n) => {
                let v: Value = serde_json::from_str(&std::fs::read_to_string(self.cp_dir.join(n)).map_err(|e| BlindError::Stop(e.to_string()))?).map_err(|e| BlindError::Stop(e.to_string()))?;
                v["cp_sha256"].as_str().unwrap_or(GENESIS).to_string()
            }
            None => GENESIS.to_string(),
        };
        let mut v = json!({"n": names.len(), "event_count": self.st.count, "ledger_head_sha256": self.st.head, "prev_cp_sha256": prev});
        v["cp_sha256"] = json!(sha_of(&v)?);
        std::fs::write(self.cp_dir.join(format!("cp_{:06}.json", names.len())), canon(&v)?).map_err(|e| BlindError::Stop(e.to_string()))?;
        Ok(v["cp_sha256"].as_str().unwrap().to_string())
    }

    fn verify_checkpoints(&self) -> R<()> {
        let mut names: Vec<String> = std::fs::read_dir(&self.cp_dir)
            .map_err(|e| BlindError::Stop(e.to_string()))?
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
            .collect();
        names.sort();
        let mut prev = GENESIS.to_string();
        for (i, n) in names.iter().enumerate() {
            let mut v: Value = serde_json::from_str(&std::fs::read_to_string(self.cp_dir.join(n)).map_err(|e| BlindError::Stop(e.to_string()))?)
                .map_err(|_| BlindError::Tampered("checkpoint が壊れています".into()))?;
            let recorded = v.as_object_mut().and_then(|o| o.remove("cp_sha256")).and_then(|x| x.as_str().map(str::to_string));
            if n != &format!("cp_{i:06}.json") || recorded.as_deref() != Some(sha_of(&v)?.as_str()) || v["prev_cp_sha256"] != json!(prev) {
                return Err(BlindError::Tampered("checkpoint chain が合いません".into()));
            }
            let count = v["event_count"].as_u64().unwrap_or(u64::MAX);
            if count == 0 || count > self.st.count || self.st.commitments[count as usize - 1] != v["ledger_head_sha256"].as_str().unwrap_or("") {
                return Err(BlindError::Tampered("ledger の末尾が削除された、または checkpoint と食い違います".into()));
            }
            prev = recorded.unwrap();
        }
        Ok(())
    }

    // ── 参照 ──

    pub fn event_count(&self) -> u64 {
        self.st.count
    }
    pub fn head(&self) -> &str {
        &self.st.head
    }
    pub fn sealed(&self) -> bool {
        self.st.sealed
    }
    /// seal 前は unblind map を開けない（PF0G2 のサーバはこれを見る）
    pub fn unblind_allowed(&self) -> bool {
        self.st.sealed
    }
    pub fn unit(&self, id: &str) -> Option<&ReviewUnit> {
        self.art.units.get(id)
    }
    pub fn progress(&self) -> (usize, usize, usize) {
        let (mut done, mut deferred, mut total) = (0, 0, 0);
        for w in self.st.units.values() {
            total += 1;
            match w.0.status {
                Status::Resolved | Status::DeferFinal => done += 1,
                Status::Deferred => deferred += 1,
                Status::Unopened => {}
            }
        }
        (done, total - done, deferred)
    }
    #[allow(dead_code)]
    pub fn unit_last(&self, unit: &str) -> Option<String> {
        self.st.units.get(unit).map(|w| w.0.last.clone())
    }
    pub fn unit_corrections(&self, unit: &str) -> u32 {
        self.st.units.get(unit).map(|w| w.0.corrections).unwrap_or(0)
    }
    pub fn completion_audit(&self) -> AuditReport {
        audit(&self.st, &self.art)
    }

    fn guard(&self) -> R<()> {
        if self.st.sealed {
            return Err(BlindError::Sealed);
        }
        if self.st.poisoned {
            return Err(BlindError::Poisoned);
        }
        if self.probe.main_app_running() {
            return Err(BlindError::MainAppRunning);
        }
        Ok(())
    }

    // ── commands ──

    /// review 開始前に 1 回。seed はここで freeze される
    pub fn freeze_qc_plan(&mut self, seed_hex: &str, pct: u32, now: &str) -> R<()> {
        let ids: Vec<String> = self.art.units.keys().cloned().collect();
        let sel = qc_select(seed_hex, &ids, pct)?;
        self.commit(json!({"type": "qc_plan", "ts": now, "seed_hex": seed_hex, "pct": pct, "selected": sel, "selection_sha256": hex_sha(sel.join("\n").as_bytes())}))?;
        Ok(())
    }

    /// 前回の crash で残った session / lease を閉じる（resume）
    pub fn recover(&mut self, now: &str) -> R<()> {
        let dangling: Vec<(String, String, String)> = self
            .st
            .units
            .iter()
            .filter_map(|(id, w)| w.0.lease.as_ref().map(|l| (id.clone(), l.lease_id.clone(), l.expires_at.clone())))
            .collect();
        for (unit, lease_id, _) in dangling {
            // 期限前でも crash 回収は release として記録する
            self.commit(json!({"type": "lease_release", "ts": now, "unit": unit, "lease_id": lease_id, "reason": "crash_recovered"}))?;
        }
        let open: Vec<String> = self.st.sessions.iter().filter(|(_, s)| !s.ended).map(|(k, _)| k.clone()).collect();
        for sid in open {
            self.commit(json!({"type": "session_end", "ts": now, "session_id": sid, "reason": "crash_recovered"}))?;
        }
        Ok(())
    }

    pub fn start_session(&mut self, reviewer_id: &str, attestation: bool, ui_protocol_version: &str, now: &str) -> R<String> {
        self.guard()?;
        if !attestation {
            return invalid("attestation が必要です");
        }
        if reviewer_id.trim().is_empty() || reviewer_id.len() > 64 {
            return invalid("reviewer_id が不正です");
        }
        self.recover(now)?;
        let sid = format!("S{}", self.st.count);
        self.commit(json!({"type": "session_start", "ts": now, "session_id": sid, "reviewer_id": reviewer_id, "attestation": true,
            "artifact_sha256": self.st.artifact_sha, "tool_sha256": tool_sha256(), "ui_protocol_version": ui_protocol_version,
            "review_protocol_version": REVIEW_PROTOCOL_VERSION}))?;
        Ok(sid)
    }

    pub fn end_session(&mut self, session_id: &str, now: &str) -> R<()> {
        let leased: Vec<(String, String)> = self
            .st
            .units
            .iter()
            .filter_map(|(id, w)| w.0.lease.as_ref().filter(|l| l.session_id == session_id).map(|l| (id.clone(), l.lease_id.clone())))
            .collect();
        for (unit, lease_id) in leased {
            self.commit(json!({"type": "lease_release", "ts": now, "unit": unit, "lease_id": lease_id, "reason": "session_end"}))?;
        }
        self.commit(json!({"type": "session_end", "ts": now, "session_id": session_id, "reason": "normal"}))?;
        self.write_checkpoint()?;
        Ok(())
    }

    fn expire_leases(&mut self, now: &str) -> R<()> {
        let t = epoch(now)?;
        let expired: Vec<(String, String)> = self
            .st
            .units
            .iter()
            .filter_map(|(id, w)| w.0.lease.as_ref().filter(|l| epoch(&l.expires_at).map(|x| t >= x).unwrap_or(false)).map(|l| (id.clone(), l.lease_id.clone())))
            .collect();
        for (unit, lease_id) in expired {
            self.commit(json!({"type": "lease_expired", "ts": now, "unit": unit, "lease_id": lease_id}))?;
        }
        Ok(())
    }

    /// 次に出す unit。本流（position 順）→ 保留（本流が尽きてから）→ QC
    pub fn next(&self) -> Next {
        let by_pos = |pred: &dyn Fn(&UnitState, &str) -> bool| -> Option<String> {
            self.art.units.iter().filter(|(id, _)| self.st.units.get(*id).map(|w| pred(&w.0, id)).unwrap_or(false)).min_by_key(|(_, u)| u.position).map(|(id, _)| id.clone())
        };
        if let Some(u) = by_pos(&|s, _| s.status == Status::Unopened && s.lease.is_none()) {
            return Next::Unit { unit: u, purpose: "review" };
        }
        let any_unopened = self.st.units.values().any(|w| w.0.status == Status::Unopened);
        if !any_unopened {
            if let Some(u) = by_pos(&|s, _| s.status == Status::Deferred && s.lease.is_none()) {
                return Next::Unit { unit: u, purpose: "review" };
            }
            let any_deferred = self.st.units.values().any(|w| w.0.status == Status::Deferred);
            if !any_deferred {
                let plan = self.st.qc_plan.clone().unwrap_or_default();
                if let Some(u) = by_pos(&|s, id| plan.iter().any(|p| p.as_str() == id) && matches!(s.status, Status::Resolved | Status::DeferFinal) && s.qc.is_none() && s.lease.is_none()) {
                    return Next::Unit { unit: u, purpose: "qc" };
                }
            }
        }
        Next::Done
    }

    pub fn acquire_lease(&mut self, session_id: &str, unit: &str, purpose: &str, now: &str) -> R<LeaseGrant> {
        self.guard()?;
        if !self.st.units.contains_key(unit) {
            return invalid("unit がありません");
        }
        self.expire_leases(now)?;
        let unit_prev = self.st.units[unit].0.last.clone();
        let expires = fmt_epoch(epoch(now)? + LEASE_SECONDS);
        let lease_id = format!("L{}", self.st.count);
        self.commit(json!({"type": "lease", "ts": now, "session_id": session_id, "unit": unit, "lease_id": lease_id, "purpose": purpose,
            "expires_at": expires, "unit_prev": unit_prev}))?;
        Ok(LeaseGrant { lease_id, unit: unit.to_string(), purpose: purpose.to_string(), prev_event_sha: self.st.units[unit].0.last.clone(), expires_at: expires })
    }

    pub fn record_search(&mut self, unit: &str, lease_id: &str, query: &str, media_type: &str, result_count: i64, now: &str) -> R<String> {
        self.guard()?;
        self.commit(json!({"type": "search", "ts": now, "unit": unit, "lease_id": lease_id, "query": query, "media_type": media_type, "result_count": result_count}))?;
        Ok(self.st.units.get(unit).map(|w| w.0.last.clone()).unwrap_or_default())
    }

    /// 提出。review（decision / defer）・correction・qc は lease の用途で決まる
    pub fn submit(&mut self, s: Submit, now: &str) -> R<SubmitOutcome> {
        self.guard()?;
        let content = content_sha(&s)?;
        // ① 二重提出
        if let Some((prev_content, seq)) = self.st.submissions.get(&s.submission_id).cloned() {
            if prev_content == content {
                return Ok(SubmitOutcome::Duplicate { seq });
            }
            self.commit(json!({"type": "integrity_anomaly", "ts": now, "reason": "submission_id reused with different content", "submission_id": s.submission_id}))?;
            return Err(BlindError::Poisoned);
        }
        let Some(w) = self.st.units.get(&s.unit) else { return invalid("unit がありません") };
        let u = &w.0;
        let purpose = u.lease.as_ref().map(|l| l.purpose.clone());
        // ② 解決済み unit への通常提出（lease が無い／用途が review でない）
        if matches!(u.status, Status::Resolved | Status::DeferFinal) && purpose.as_deref().map(|p| p == "review").unwrap_or(true) && s.correction_reason.is_none() {
            return Err(BlindError::UnitAlreadyResolved(s.unit.clone()));
        }
        // ③ lease
        let Some(l) = u.lease.clone() else { return Err(BlindError::LeaseInvalid("lease がありません".into())) };
        if l.lease_id != s.lease_id {
            return Err(BlindError::LeaseInvalid("lease_id が違います".into()));
        }
        if epoch(now)? >= epoch(&l.expires_at)? {
            return Err(BlindError::LeaseInvalid("lease が期限切れです".into()));
        }
        // ④ CAS（副作用ゼロ）
        if s.prev_event_sha != u.last {
            return Err(BlindError::Conflict);
        }
        check_note(s.note.as_deref())?;
        if l.purpose == "correction" && s.correction_reason.is_none() {
            return invalid("correction には reason が必要です");
        }
        let unit_def = self.art.units.get(&s.unit).ok_or_else(|| BlindError::Invalid("unit がありません".into()))?;
        let mut ev = json!({"ts": now, "unit": s.unit, "lease_id": s.lease_id, "session_id": l.session_id, "submission_id": s.submission_id,
            "unit_prev": s.prev_event_sha, "content_sha256": content, "note": s.note, "correction_reason": s.correction_reason,
            "artifact_sha256": self.st.artifact_sha, "unit_sha256": unit_def.unit_sha256, "tool_sha256": tool_sha256(),
            "review_protocol_version": REVIEW_PROTOCOL_VERSION, "shown_at": l.started,
            "active_ms": (epoch(now)? - epoch(&l.started)?) * 1000, "search_count": l.searches});
        let ty = match (&s.decision, l.purpose.as_str()) {
            (Decision::Defer { reason }, "review") => {
                ev["defer_reason"] = json!(reason);
                if u.pass == 2 { "defer_final" } else { "defer" }
            }
            (Decision::Defer { .. }, _) => return invalid("defer は review の用途だけ"),
            (d, purpose) => {
                match d {
                    Decision::Confirm { candidate_token } => {
                        unit_def.find(candidate_token).ok_or_else(|| BlindError::Invalid("candidate_token が unit にありません".into()))?;
                        ev["decision"] = json!("confirm");
                        ev["candidate_token"] = json!(candidate_token);
                        // identity（tmdb_id / media_type）は event に書かない
                    }
                    Decision::PickOther { tmdb_id, media_type, verification } => {
                        check_target(*tmdb_id, media_type)?;
                        if unit_def.has_target(*tmdb_id, media_type) {
                            return Err(BlindError::TargetIsCandidate);
                        }
                        let v = verification.as_ref().ok_or_else(|| BlindError::TargetNotVerified("検証されていません".into()))?;
                        ev["decision"] = json!("pick_other");
                        ev["candidate_token"] = Value::Null;
                        ev["tmdb_id"] = json!(tmdb_id);
                        ev["media_type"] = json!(media_type);
                        ev["tmdb_verification"] = v.to_value();
                    }
                    Decision::None => {
                        if l.searches < 1 {
                            return Err(BlindError::NoneNeedsSearch);
                        }
                        ev["decision"] = json!("none");
                        ev["candidate_token"] = Value::Null;
                        ev["tmdb_id"] = Value::Null;
                        ev["media_type"] = Value::Null;
                    }
                    Decision::Defer { .. } => unreachable!(),
                }
                match purpose {
                    "review" => "decision",
                    "correction" => "correction",
                    _ => "qc_decision",
                }
            }
        };
        ev["type"] = json!(ty);
        let seq = self.commit(ev)?;
        Ok(SubmitOutcome::Recorded { seq })
    }

    pub fn release_lease(&mut self, unit: &str, lease_id: &str, now: &str) -> R<()> {
        self.commit(json!({"type": "lease_release", "ts": now, "unit": unit, "lease_id": lease_id, "reason": "released"}))?;
        Ok(())
    }

    /// seal。completion audit が全て通り、**init の pin と完全一致**するときだけ。
    /// 戻り値の `review_seal_sha256`（= seal event の commitment）を外部 audit anchor に固定する
    pub fn seal(&mut self, pins: &Pins, now: &str) -> R<(AuditReport, String)> {
        self.guard()?;
        pins.validate()?;
        let init = self.st.pins.clone().ok_or_else(|| BlindError::Stop("init の pin がありません".into()))?;
        let mismatch = init.diff(pins);
        if !mismatch.is_empty() {
            return Err(BlindError::PinMismatch(mismatch));
        }
        let report = audit(&self.st, &self.art);
        if !report.passed() {
            return Err(BlindError::AuditFailed(report.failures()));
        }
        let report_sha = sha_of(&report.to_value())?;
        self.commit(json!({"type": "seal", "ts": now, "pins": pins.to_value(), "audit_report_sha256": report_sha,
            "ledger_head_before": self.st.head, "unit_count": report.unit_count}))?;
        Ok((report, self.st.review_seal.clone().unwrap_or_default()))
    }

    pub fn review_seal_sha256(&self) -> Option<&str> {
        self.st.review_seal.as_deref()
    }

    // ── post-seal errata（sealed GT は書き換えない）──

    fn errata_path(&self) -> PathBuf {
        self.dir.join("errata.jsonl")
    }

    fn read_errata(&self) -> R<Vec<Value>> {
        let events = read_lines(&self.errata_path())?;
        let seal = self.st.review_seal.clone().unwrap_or_default();
        let mut prev = GENESIS.to_string();
        for (i, e) in events.iter().enumerate() {
            let mut body = e.clone();
            let recorded = body.as_object_mut().and_then(|o| o.remove("errata_commitment")).and_then(|v| v.as_str().map(str::to_string));
            let expect = hex_sha(format!("{}\n{}", canon(&body)?, prev).as_bytes());
            if recorded.as_deref() != Some(expect.as_str())
                || e.get("seq").and_then(Value::as_u64) != Some(i as u64)
                || e.get("review_seal_sha256").and_then(Value::as_str) != Some(seal.as_str())
            {
                return Err(BlindError::Tampered("errata の chain が合いません".into()));
            }
            prev = expect;
        }
        Ok(events)
    }

    /// seal 後に GT の誤りが判明したときの **記録だけ**。`events.jsonl`（sealed GT）には一切書かない。
    /// `promotion_impacting` なら evaluation は INVALIDATED になり、新しい evaluation cycle が要る
    pub fn record_post_seal_errata(&self, unit: &str, description: &str, promotion_impacting: bool, now: &str) -> R<u64> {
        if !self.st.sealed {
            return invalid("seal 前の誤りは correction を使う");
        }
        if !self.art.units.contains_key(unit) {
            return invalid("unit がありません");
        }
        if description.trim().is_empty() {
            return invalid("description が必要です");
        }
        check_note(Some(description))?;
        epoch(now)?;
        let prior = self.read_errata()?;
        let prev = prior.last().and_then(|e| e["errata_commitment"].as_str().map(str::to_string)).unwrap_or_else(|| GENESIS.to_string());
        let mut e = json!({"type": "errata", "seq": prior.len(), "ts": now, "review_seal_sha256": self.st.review_seal, "unit": unit,
            "description": description, "promotion_impacting": promotion_impacting});
        let c = hex_sha(format!("{}\n{}", canon(&e)?, prev).as_bytes());
        e["errata_commitment"] = json!(c);
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.errata_path()).map_err(|e| BlindError::Stop(e.to_string()))?;
        f.write_all(format!("{}\n", canon(&e)?).as_bytes()).map_err(|e| BlindError::Stop(e.to_string()))?;
        f.sync_all().map_err(|e| BlindError::Stop(e.to_string()))?;
        Ok(prior.len() as u64)
    }

    /// sealed GT は immutable。promotion に影響し得る errata が 1 件でもあれば INVALIDATED
    pub fn evaluation_validity(&self) -> R<EvaluationValidity> {
        if !self.st.sealed {
            return invalid("seal 前です");
        }
        let errata = self.read_errata()?;
        Ok(match errata.iter().find(|e| e["promotion_impacting"] == json!(true)) {
            Some(e) => EvaluationValidity::Invalidated { errata_seq: e["seq"].as_u64().unwrap_or(0) },
            None => EvaluationValidity::Valid { recorded_errata: errata.len() },
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EvaluationValidity {
    Valid { recorded_errata: usize },
    /// 新しい evaluation cycle が必要（既存の GT は書き換えない）
    Invalidated { errata_seq: u64 },
}

// ─── pin（init と seal の両方で完全一致を要求）─────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Pins {
    pub protocol_sha256: String,
    pub ui_package_sha256: String,
    pub shuffle_seed_commitment: String,
    pub attestation_policy_sha256: String,
    pub unblind_map_sha256: String,
    pub build_secret_commitment: String,
    /// review 開始前に取った package / UI の leak scan report
    pub a8_leak_scan_report_sha256: String,
    pub machine_freeze_anchor_commitment: String,
}

impl Pins {
    fn fields(&self) -> [(&'static str, &String); 8] {
        [
            ("protocol_sha256", &self.protocol_sha256),
            ("ui_package_sha256", &self.ui_package_sha256),
            ("shuffle_seed_commitment", &self.shuffle_seed_commitment),
            ("attestation_policy_sha256", &self.attestation_policy_sha256),
            ("unblind_map_sha256", &self.unblind_map_sha256),
            ("build_secret_commitment", &self.build_secret_commitment),
            ("a8_leak_scan_report_sha256", &self.a8_leak_scan_report_sha256),
            ("machine_freeze_anchor_commitment", &self.machine_freeze_anchor_commitment),
        ]
    }
    pub fn validate(&self) -> R<()> {
        for (k, v) in self.fields() {
            if !is_hex64(v) {
                return invalid(format!("pin {k} は 64 桁 hex"));
            }
        }
        Ok(())
    }
    fn to_value(&self) -> Value {
        Value::Object(self.fields().iter().map(|(k, v)| (k.to_string(), json!(v))).collect())
    }
    fn from_event(e: &Value) -> R<Pins> {
        let p = e.get("pins").and_then(Value::as_object).ok_or_else(|| BlindError::Stop("event に pins がありません".into()))?;
        if p.len() != 8 {
            return stop("pins のキーが規定と違います");
        }
        let g = |k: &str| -> R<String> {
            p.get(k).and_then(Value::as_str).map(str::to_string).ok_or_else(|| BlindError::Stop(format!("pin {k} がありません")))
        };
        let pins = Pins {
            protocol_sha256: g("protocol_sha256")?,
            ui_package_sha256: g("ui_package_sha256")?,
            shuffle_seed_commitment: g("shuffle_seed_commitment")?,
            attestation_policy_sha256: g("attestation_policy_sha256")?,
            unblind_map_sha256: g("unblind_map_sha256")?,
            build_secret_commitment: g("build_secret_commitment")?,
            a8_leak_scan_report_sha256: g("a8_leak_scan_report_sha256")?,
            machine_freeze_anchor_commitment: g("machine_freeze_anchor_commitment")?,
        };
        pins.validate().map_err(|_| BlindError::Stop("pin が 64 桁 hex ではありません".into()))?;
        Ok(pins)
    }
    /// 一致しなかった pin の名前
    fn diff(&self, other: &Pins) -> Vec<String> {
        self.fields().iter().zip(other.fields().iter()).filter(|(a, b)| a.1 != b.1).map(|(a, _)| a.0.to_string()).collect()
    }
}

// ─── reviewer package の hard check ─────────────────────────────────────────────────────────

/// reviewer が読める package に置いてよいのは `manifest.json` と `units/<id>.json` だけ。
/// build_secret / unblind_map / work id / candidate の対応表が 1 つでも見つかれば STOP
pub fn check_reviewer_package(dir: &Path) -> R<()> {
    const BANNED: [&str; 9] = ["build_secret", "unblind", "work_id", "\"work:", "evaluation_unit_key", "candidate_mapping", "identity_map", "token_map", "\"tmdb_id\""];
    let ls = |p: &Path| -> R<Vec<std::fs::DirEntry>> {
        std::fs::read_dir(p)
            .map_err(|e| BlindError::Invalid(format!("{} を読めません: {e}", p.display())))?
            .map(|e| e.map_err(|e| BlindError::Invalid(e.to_string())))
            .collect()
    };
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in ls(dir)? {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map_err(|e| BlindError::Invalid(e.to_string()))?.is_dir();
        match (name.as_str(), is_dir) {
            ("manifest.json", false) => files.push(entry.path()),
            ("units", true) => {
                for u in ls(&entry.path())? {
                    if u.file_type().map(|t| t.is_dir()).unwrap_or(true) {
                        return stop("reviewer package の units に想定外の項目があります");
                    }
                    files.push(u.path());
                }
            }
            _ => return stop(format!("reviewer package に想定外の項目があります: {name}")),
        }
    }
    for f in files {
        let text = std::fs::read_to_string(&f).map_err(|e| BlindError::Invalid(format!("{} を読めません: {e}", f.display())))?;
        let lower = text.to_lowercase();
        for b in BANNED {
            if lower.contains(b) {
                return stop(format!("reviewer package に禁止された情報があります: {b} ({})", f.file_name().unwrap_or_default().to_string_lossy()));
            }
        }
    }
    Ok(())
}

/// 決定 → ラベルの写像。C5c.5（`gt_review.rs`）と同一（parity test で固定）
pub fn gt_label(decision: &str) -> Option<(&'static str, &'static str)> {
    match decision {
        "confirm" => Some(("tmdb", "review_confirm")),
        "pick_other" => Some(("tmdb", "review_pick_other")),
        "none" => Some(("none", "review_none")),
        _ => None, // defer はラベルではない
    }
}

#[cfg(test)]
#[path = "pr4_blind_gt_tests.rs"]
mod tests;
