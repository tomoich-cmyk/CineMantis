//! PR4-PF0G2（1/2）: reviewer packager・A8 leak scan・UI package pin・本体アプリ probe。
//!
//! 入力は **PF0F が凍結した reviewer projection**（`REVIEWER_PROJECTION`）と、PF0E 固定 payload の raw 証拠だけ。
//! TMDB は呼ばない（このファイルに通信コードは無い。ソース走査テストで固定）。
//! machine output（decision / score / rank / source / reasons）を受け取る型を持たない。
//! 出力は 2 つに分離する: reviewer package（`manifest.json` + `units/`）と sealed side（unblind map・build secret・handoff）。
//! PF0G1 の `pr4_blind_gt.rs` は変更しない（公開 API だけを使う）。

#![allow(dead_code)] // reviewer の bin は PF0G3 で追加する。それまで公開 API の一部は未使用
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::pr4_blind_gt::{
    assign_positions, artifact_sha256, canon, check_reviewer_package, derive_candidate_token, derive_review_unit_id, derive_unit_salt,
    hex_sha, order_commitment, overlap_digest, scan_forbidden, validate_unit_key, Artifact, BlindError, Candidate, Evidence,
    EvidenceFile, MainAppProbe, Manifest, Pins, ReviewUnit, MANIFEST_SCHEMA, UNIT_SCHEMA,
};

type R<T> = Result<T, BlindError>;

pub const UI_PROTOCOL_VERSION: &str = "pr4-blind-ui-1";
pub const PACKAGER_VERSION: &str = "pr4-pf0g2-packager-1";
pub const BUILD_ATTESTATION_SCHEMA: &str = "pr4-pf0g-build-attestation-1";
/// PF0I が定義した bundle 形式（docs/pr4_pf0i0_gt_adapter_schema.json の `unblind_bundle`）。packager はこの形で出す
pub const UNBLIND_BUNDLE_VERSION: &str = "pr4-pf0i-unblind-bundle-1";
pub const LEAK_SCAN_SCHEMA: &str = "pr4-pf0g-a8-leak-scan-1";
const MAX_TMDB_ID: i64 = 9_007_199_254_740_992;
const SHUFFLE_COMMIT_DOMAIN: &str = "pr4-pf0g-shuffle-seed-commitment-1";

fn stop<T>(m: impl Into<String>) -> R<T> {
    Err(BlindError::Stop(m.into()))
}
fn invalid<T>(m: impl Into<String>) -> R<T> {
    Err(BlindError::Invalid(m.into()))
}
fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

// ─── 入力（PF0F の凍結 projection + PF0E の raw 証拠）──────────────────────────────────────────

/// 1 unit 分。`projection` は PF0F `REVIEWER_PROJECTION` の body そのまま（再 fetch・加工しない）
#[derive(Clone)]
pub struct PackagerUnit {
    pub evaluation_unit_key: String,
    pub projection: Value,
    pub evidence: Vec<EvidenceFile>,
}

#[derive(Clone)]
pub struct PackagerInput {
    pub build_id: String,
    pub build_secret_hex: String,
    pub units: Vec<PackagerUnit>,
    pub machine_freeze_commitment_sha256: String,
    pub pf0e_ledger_head_sha256: String,
    pub pf0e_record_count: i64,
    pub built_at_utc: String,
    /// PF0H から受け取る値（PF0I が exact-match する）。ここでは計算せず、sealed handoff に載せるだけ
    pub authoritative_membership_commitment: String,
    pub pf0h_final_manifest_sha256: String,
    /// audit 側が pin した `build_secret_commitment`。build_secret から再導出した値と exact-match しなければ何も出力しない（M2）
    pub pinned_build_secret_commitment: String,
}

impl std::fmt::Debug for PackagerInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PackagerInput(build_id={}, units={}, build_secret=<redacted>)", self.build_id, self.units.len())
    }
}

/// projection の candidate の厳格な型。キー集合は schema と完全一致（未知キー = rank 等は拒否）
struct ProjCandidate {
    tmdb_id: i64,
    media_type: String,
    title: String,
    original_title: Option<String>,
    year: Option<i64>,
    overview: Option<String>,
    poster_path: Option<String>,
    /// 凍結 artifact の値そのもの（frozen JSON の再掲）。reviewer には出さない
    raw: Value,
}

const PROJ_TOP_KEYS: [&str; 4] = ["evaluation_unit_key", "attempt", "candidates", "shown_set_sha256"];
const PROJ_CAND_KEYS: [&str; 10] = [
    "tmdb_id", "media_type", "title", "original_title", "year", "release_date", "overview", "poster_path", "release_year_from_date",
    "year_consistent_with_release_date",
];

fn opt_str(v: &Value, k: &str) -> R<Option<String>> {
    match &v[k] {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        _ => invalid(format!("projection.candidates.{k} は string|null")),
    }
}
fn opt_int(v: &Value, k: &str) -> R<Option<i64>> {
    match &v[k] {
        Value::Null => Ok(None),
        n if n.is_i64() => Ok(n.as_i64()),
        _ => invalid(format!("projection.candidates.{k} は integer|null")),
    }
}

/// PF0F1 `REVIEWER_PROJECTION` の検証。未知キー・並び違い・重複・`shown_set_sha256` 不一致は全て拒否（既定値で救わない）
fn parse_projection(key: &str, p: &Value) -> R<Vec<ProjCandidate>> {
    let Some(o) = p.as_object() else { return invalid("projection は object") };
    if o.keys().any(|k| !PROJ_TOP_KEYS.contains(&k.as_str())) {
        return invalid("projection に schema 外のキーがあります");
    }
    for req in ["evaluation_unit_key", "candidates", "shown_set_sha256"] {
        if !o.contains_key(req) {
            return invalid(format!("projection.{req} がありません"));
        }
    }
    if p["evaluation_unit_key"] != json!(key) {
        return invalid("projection.evaluation_unit_key が unit と一致しません");
    }
    if let Some(a) = o.get("attempt") {
        if a != &json!(1) && a != &json!(2) {
            return invalid("projection.attempt は 1|2");
        }
    }
    let Some(list) = p["candidates"].as_array() else { return invalid("projection.candidates は array") };
    let mut out: Vec<ProjCandidate> = Vec::new();
    for c in list {
        let Some(co) = c.as_object() else { return invalid("candidate は object") };
        if co.len() != PROJ_CAND_KEYS.len() || PROJ_CAND_KEYS.iter().any(|k| !co.contains_key(*k)) {
            return invalid("candidate のキー集合が schema と一致しません（rank / score / source 等は載せられない）");
        }
        let Some(tmdb_id) = c["tmdb_id"].as_i64().filter(|n| (1..=MAX_TMDB_ID).contains(n)) else { return invalid("candidate.tmdb_id が不正") };
        let media_type = match c["media_type"].as_str() {
            Some(m @ ("movie" | "tv")) => m.to_string(),
            _ => return invalid("candidate.media_type は movie|tv"),
        };
        let Some(title) = c["title"].as_str() else { return invalid("candidate.title は string") };
        let (original_title, overview, poster_path) = (opt_str(c, "original_title")?, opt_str(c, "overview")?, opt_str(c, "poster_path")?);
        opt_str(c, "release_date")?;
        let year = opt_int(c, "year")?;
        opt_int(c, "release_year_from_date")?;
        if !c["year_consistent_with_release_date"].is_boolean() {
            return invalid("candidate.year_consistent_with_release_date は boolean");
        }
        if let Some(y) = year {
            if !(1..=9999).contains(&y) {
                return invalid("candidate.year が範囲外");
            }
        }
        out.push(ProjCandidate { tmdb_id, media_type, title: title.to_string(), original_title, year, overview, poster_path, raw: c.clone() });
    }
    if !out.windows(2).all(|w| (w[0].media_type.as_str(), w[0].tmdb_id) < (w[1].media_type.as_str(), w[1].tmdb_id)) {
        return invalid("projection.candidates が (media_type, tmdb_id) の厳密昇順ではありません（重複を含む）");
    }
    let shown = hex_sha(canon(&Value::Array(out.iter().map(|c| c.raw.clone()).collect()))?.as_bytes());
    if p["shown_set_sha256"].as_str() != Some(shown.as_str()) {
        return invalid("projection.shown_set_sha256 が候補集合と一致しません");
    }
    Ok(out)
}

fn validate_evidence(files: &[EvidenceFile]) -> R<()> {
    if files.is_empty() {
        return invalid("evidence.files が空です");
    }
    let mut parts = BTreeSet::new();
    for f in files {
        if f.part_no < 1 || !parts.insert(f.part_no) {
            return invalid("part_no が不正または重複しています");
        }
        if f.original_file_name.is_empty() || f.source_media_kind.is_empty() {
            return invalid("evidence の必須項目が空です");
        }
    }
    Ok(())
}

// ─── 出力 ──────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct BuildOutput {
    pub unit_count: usize,
    pub manifest_sha256: String,
    pub artifact_sha256: String,
    pub unblind_map_sha256: String,
    pub build_secret_commitment: String,
    pub shuffle_seed_commitment: String,
}

pub fn build_secret_commitment(build_secret_hex: &str) -> String {
    // PF0I 確定契約: SHA256(ASCII(lowercase_hex(build_secret_bytes)))。0x・空白・domain なし
    hex_sha(build_secret_hex.as_bytes())
}
pub fn shuffle_seed_commitment(build_secret_hex: &str) -> String {
    hex_sha(format!("{SHUFFLE_COMMIT_DOMAIN}\n{build_secret_hex}").as_bytes())
}

/// packager 自身の SHA（manifest.packager_sha256）
pub fn packager_sha256() -> String {
    hex_sha(include_str!("pr4_pf0g2.rs").as_bytes())
}

fn write_new(path: &Path, bytes: &[u8]) -> R<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(bytes)?;
            f.sync_all()
        })
        .map_err(|e| BlindError::Stop(format!("{} を書けません: {e}", path.display())))
}

fn empty_or_absent(dir: &Path) -> R<()> {
    match std::fs::read_dir(dir) {
        Ok(mut rd) => {
            if rd.next().is_some() {
                return stop(format!("出力先が空ではありません: {}", dir.display()));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => stop(e.to_string()),
    }
}

fn abs(p: &Path) -> R<PathBuf> {
    std::path::absolute(p).map_err(|e| BlindError::Stop(e.to_string()))
}

/// 一方向 packager。失敗したら何も残さない（reviewer 側 → sealed 側の順ではなく、検証後に書く）
pub fn build(input: &PackagerInput, reviewer_dir: &Path, sealed_dir: &Path) -> R<BuildOutput> {
    let (rd, sd) = (abs(reviewer_dir)?, abs(sealed_dir)?);
    if rd.starts_with(&sd) || sd.starts_with(&rd) {
        return stop("reviewer package と sealed side は別ディレクトリ（入れ子不可）");
    }
    empty_or_absent(&rd)?;
    empty_or_absent(&sd)?;
    let secret = &input.build_secret_hex;
    if !is_hex64(secret) {
        return invalid("build_secret は 64 桁 hex");
    }
    if !is_hex64(&input.pinned_build_secret_commitment) || build_secret_commitment(secret) != input.pinned_build_secret_commitment {
        return stop("build_secret_commitment が pin と一致しません（出力前に fail closed）");
    }
    for h in [&input.machine_freeze_commitment_sha256, &input.pf0e_ledger_head_sha256, &input.authoritative_membership_commitment, &input.pf0h_final_manifest_sha256] {
        if !is_hex64(h) {
            return invalid("入力の commitment が 64 桁 hex ではありません");
        }
    }
    if input.units.is_empty() || input.build_id.is_empty() {
        return invalid("units / build_id が必要です");
    }
    if input.pf0e_record_count != input.units.len() as i64 {
        return invalid("pf0e_record_count と unit 数が一致しません");
    }
    let keys: Vec<String> = input.units.iter().map(|u| u.evaluation_unit_key.clone()).collect();
    for k in &keys {
        validate_unit_key(k)?;
    }
    if keys.iter().collect::<BTreeSet<_>>().len() != keys.len() {
        return invalid("evaluation_unit_key が重複しています");
    }
    let positions: std::collections::HashMap<String, usize> = assign_positions(secret, &keys)?.into_iter().collect();

    let mut units: Vec<ReviewUnit> = Vec::new();
    let mut unblind_entries: Vec<Value> = Vec::new();
    for pu in &input.units {
        let cands = parse_projection(&pu.evaluation_unit_key, &pu.projection)?;
        validate_evidence(&pu.evidence)?;
        let unit_id = derive_review_unit_id(secret, &pu.evaluation_unit_key)?;
        let salt = derive_unit_salt(secret, &pu.evaluation_unit_key)?;
        let mut candidates = Vec::new();
        let mut entries = Vec::new();
        let mut digests = Vec::new();
        for c in &cands {
            let token = derive_candidate_token(secret, &unit_id, &c.media_type, c.tmdb_id)?;
            digests.push(overlap_digest(&salt, &c.media_type, c.tmdb_id)?);
            // 明示的な field 単位の射影。rank / score / source / release_date 等を持ち込む経路は無い
            candidates.push(Candidate {
                candidate_token: token.clone(),
                title: c.title.clone(),
                original_title: c.original_title.clone(),
                year: c.year.map(|y| y.to_string()),
                overview: c.overview.clone(),
                poster_path: c.poster_path.clone(),
                original_language: None,
            });
            entries.push(json!({"candidate_token": token, "media_type": c.media_type, "tmdb_id": c.tmdb_id}));
        }
        // 提示順（契約 §3.2）: seed = SHA256(review_unit_id ‖ build_secret) による決定論 shuffle。rank / score / source は入力に無い。
        // 各 candidate の sort key = SHA256(seed ‖ "|cand|" ‖ token)。token の lexical 順ではない
        let seed = hex_sha(format!("{unit_id}{secret}").as_bytes());
        candidates.sort_by_cached_key(|c| (hex_sha(format!("{seed}|cand|{}", c.candidate_token).as_bytes()), c.candidate_token.clone()));
        digests.sort();
        let unit = ReviewUnit {
            schema_version: UNIT_SCHEMA.into(),
            review_unit_id: unit_id.clone(),
            position: positions[&pu.evaluation_unit_key] as i64,
            unit_salt: salt,
            overlap_digests: digests,
            evidence: Evidence { files: pu.evidence.clone() },
            candidates,
            unit_sha256: String::new(),
        }
        .sealed()?;
        units.push(unit);
        unblind_entries.push(json!({"review_unit_id": unit_id, "work_key": pu.evaluation_unit_key, "candidates": entries}));
    }
    unblind_entries.sort_by(|a, b| a["review_unit_id"].as_str().cmp(&b["review_unit_id"].as_str()));

    let map: std::collections::BTreeMap<String, ReviewUnit> = units.iter().map(|u| (u.review_unit_id.clone(), u.clone())).collect();
    let manifest = Manifest {
        schema_version: MANIFEST_SCHEMA.into(),
        build_id: input.build_id.clone(),
        unit_count: units.len() as i64,
        order_commitment_sha256: order_commitment(&map),
        artifact_sha256: artifact_sha256(&map),
        machine_freeze_commitment_sha256: input.machine_freeze_commitment_sha256.clone(),
        pf0e_ledger_head_sha256: input.pf0e_ledger_head_sha256.clone(),
        pf0e_record_count: input.pf0e_record_count,
        packager_sha256: packager_sha256(),
        built_at_utc: input.built_at_utc.clone(),
    };
    let art = Artifact::from_parts(manifest.clone(), units.clone())?;

    // sealed side の中身
    let unblind = json!({"bundle_version": UNBLIND_BUNDLE_VERSION, "build_secret_hex": secret, "units": unblind_entries});
    let unblind_bytes = canon(&unblind)?;
    let unblind_sha = hex_sha(unblind_bytes.as_bytes());
    let membership = {
        let mut ks = keys.clone();
        ks.sort();
        hex_sha(canon(&json!(ks))?.as_bytes())
    };
    let attestation = json!({"schema_version": BUILD_ATTESTATION_SCHEMA, "build_id": input.build_id, "packager_version": PACKAGER_VERSION,
        "packager_sha256": packager_sha256(), "manifest_sha256": art.manifest_sha256, "artifact_sha256": art.manifest.artifact_sha256,
        "unblind_map_sha256": unblind_sha, "build_secret_commitment": build_secret_commitment(secret),
        "shuffle_seed_commitment": shuffle_seed_commitment(secret), "unit_count": units.len(),
        "machine_freeze_commitment_sha256": input.machine_freeze_commitment_sha256, "pf0e_ledger_head_sha256": input.pf0e_ledger_head_sha256,
        "pf0e_record_count": input.pf0e_record_count,
        // PF0I handoff: PF0I が exact-match する 3 値のうちの 2 つ（review seal SHA は seal event から）
        "authoritative_membership_commitment": input.authoritative_membership_commitment,
        "pf0h_final_manifest_sha256": input.pf0h_final_manifest_sha256,
        "packager_membership_sha256": membership,
        "packager_membership_rule": "sha256(canon(sorted evaluation_unit_keys)); informational only, not the PF0H authoritative value"});

    std::fs::create_dir_all(&sd).map_err(|e| BlindError::Stop(e.to_string()))?;
    std::fs::create_dir_all(rd.join("units")).map_err(|e| BlindError::Stop(e.to_string()))?;
    write_new(&rd.join("manifest.json"), serde_json::to_string(&manifest).map_err(|e| BlindError::Stop(e.to_string()))?.as_bytes())?;
    for u in &units {
        write_new(&rd.join("units").join(format!("{}.json", u.review_unit_id)), serde_json::to_string(u).map_err(|e| BlindError::Stop(e.to_string()))?.as_bytes())?;
    }
    write_new(&sd.join("unblind_map.json"), unblind_bytes.as_bytes())?;
    write_new(&sd.join("build_attestation.json"), canon(&attestation)?.as_bytes())?;

    // 書いた結果を、reviewer が使うのと同じ関数で再検証する（secret / map が混入していれば STOP）
    check_reviewer_package(&rd)?;
    let loaded = Artifact::load(&rd)?;
    if loaded.manifest_sha256 != art.manifest_sha256 {
        return stop("書き出した package が再読込で一致しません");
    }
    Ok(BuildOutput {
        unit_count: units.len(),
        manifest_sha256: art.manifest_sha256,
        artifact_sha256: art.manifest.artifact_sha256,
        unblind_map_sha256: unblind_sha,
        build_secret_commitment: build_secret_commitment(secret),
        shuffle_seed_commitment: shuffle_seed_commitment(secret),
    })
}

// ─── UI package の pin ─────────────────────────────────────────────────────────────────────

/// reviewer UI（静的ファイル）。`ui_package_sha256` は名前順の `name:sha256\n` の SHA
pub const UI_FILES: [(&str, &str); 1] = [("index.html", include_str!("pr4_pf0g2_ui/index.html"))];

pub fn ui_package_sha256() -> String {
    let mut files: Vec<(&str, String)> = UI_FILES.iter().map(|(n, b)| (*n, hex_sha(b.as_bytes()))).collect();
    files.sort();
    hex_sha(files.iter().map(|(n, h)| format!("{n}:{h}\n")).collect::<String>().as_bytes().to_vec().as_slice())
}

/// init と seal で使う pin 一式。`ui_package_sha256` は常に実際の UI から計算する（呼び出し側が値を差し替えられない）
pub fn make_pins(out: &BuildOutput, protocol_sha256: &str, attestation_policy_sha256: &str, a8_leak_scan_report_sha256: &str, machine_freeze_anchor_commitment: &str) -> Pins {
    Pins {
        protocol_sha256: protocol_sha256.to_string(),
        ui_package_sha256: ui_package_sha256(),
        shuffle_seed_commitment: out.shuffle_seed_commitment.clone(),
        attestation_policy_sha256: attestation_policy_sha256.to_string(),
        unblind_map_sha256: out.unblind_map_sha256.clone(),
        build_secret_commitment: out.build_secret_commitment.clone(),
        a8_leak_scan_report_sha256: a8_leak_scan_report_sha256.to_string(),
        machine_freeze_anchor_commitment: machine_freeze_anchor_commitment.to_string(),
    }
}

// ─── A8 leak scan（review 開始前の package / UI 走査）──────────────────────────────────────────

/// reviewer package と UI を走査する。1 件でも漏洩があれば STOP（report は作らない）。
/// `forbidden_literals` には canary や build_secret の値などを渡す（packager が知っている禁止文字列）。
/// report は決定的（時刻を含まない）なので SHA を pin できる
pub fn a8_leak_scan(reviewer_dir: &Path, forbidden_literals: &[String]) -> R<(Value, String)> {
    check_reviewer_package(reviewer_dir)?;
    let art = Artifact::load(reviewer_dir)?;
    let mut scanned: Vec<(String, String)> = Vec::new();
    let mut texts: Vec<(String, String)> = Vec::new();
    let mf = reviewer_dir.join("manifest.json");
    texts.push(("manifest.json".into(), std::fs::read_to_string(&mf).map_err(|e| BlindError::Stop(e.to_string()))?));
    for id in art.units.keys() {
        let p = reviewer_dir.join("units").join(format!("{id}.json"));
        texts.push((format!("units/{id}.json"), std::fs::read_to_string(&p).map_err(|e| BlindError::Stop(e.to_string()))?));
    }
    for (n, b) in UI_FILES {
        texts.push((format!("ui/{n}"), b.to_string()));
    }
    let lits: Vec<String> = forbidden_literals.iter().filter(|l| !l.is_empty()).map(|l| l.to_lowercase()).collect();
    let mut findings: Vec<String> = Vec::new();
    for (name, text) in &texts {
        scanned.push((name.clone(), hex_sha(text.as_bytes())));
        let lower = text.to_lowercase();
        for l in &lits {
            if lower.contains(l) {
                findings.push(format!("literal in {name}"));
            }
        }
        if name.ends_with(".json") {
            let v: Value = serde_json::from_str(text).map_err(|e| BlindError::Stop(e.to_string()))?;
            for f in scan_forbidden(&v) {
                findings.push(format!("{name}{f}"));
            }
        } else {
            // UI: 契約が禁じる表示物の語（機械由来の情報・内部 ID・display_title）を HTML / JS に書いていないこと
            for banned in ["display_title", "work_id", "candidate_count", "unit_salt", "overlap_digest", "build_secret", "unblind", "evaluation_unit_key", "confidence", "reasons"] {
                if lower.contains(banned) {
                    findings.push(format!("ui-term:{banned} in {name}"));
                }
            }
        }
    }
    if !findings.is_empty() {
        findings.sort();
        findings.dedup();
        return stop(format!("A8 leak scan FAIL: {}", findings.join("; ")));
    }
    scanned.sort();
    let report = json!({"schema_version": LEAK_SCAN_SCHEMA, "result": "PASS", "artifact_sha256": art.manifest.artifact_sha256,
        "manifest_sha256": art.manifest_sha256, "ui_package_sha256": ui_package_sha256(), "unit_count": art.units.len(),
        "files": scanned.iter().map(|(n, h)| json!({"path": n, "sha256": h})).collect::<Vec<_>>(),
        "checks": ["reviewer_package_layout", "banned_tokens", "forbidden_keys_and_values", "forbidden_literals", "ui_terms"]});
    let sha = hex_sha(canon(&report)?.as_bytes());
    Ok((report, sha))
}

// ─── 本体アプリ probe ──────────────────────────────────────────────────────────────────────

pub trait ProcessLister {
    /// 実行中のプロセスのイメージ名の一覧。取得できなければ Err（呼び出し側は「動いている」とみなす）
    fn image_names(&self) -> Result<Vec<String>, String>;
}

pub const MAIN_APP_IMAGES: [&str; 2] = ["cinemantis.exe", "cinemantis"];

/// 本体 CineMantis が動いていれば reviewer 起動拒否（`MainAppRunning`）。一覧が取れなければ **動いているとみなす**（fail closed）
pub struct ProcessProbe<L: ProcessLister> {
    pub lister: L,
}
impl<L: ProcessLister> MainAppProbe for ProcessProbe<L> {
    fn main_app_running(&self) -> bool {
        match self.lister.image_names() {
            Ok(names) => names.iter().any(|n| MAIN_APP_IMAGES.contains(&n.trim().to_lowercase().as_str())),
            Err(_) => true,
        }
    }
}

/// OS の process 一覧（Windows: `tasklist`、それ以外: `ps -A -o comm=`）
pub struct SystemProcessLister;
impl ProcessLister for SystemProcessLister {
    fn image_names(&self) -> Result<Vec<String>, String> {
        let out = if cfg!(windows) {
            std::process::Command::new("tasklist").args(["/FO", "CSV", "/NH"]).output()
        } else {
            std::process::Command::new("ps").args(["-A", "-o", "comm="]).output()
        }
        .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err("process list failed".into());
        }
        Ok(parse_process_list(&String::from_utf8_lossy(&out.stdout)))
    }
}

pub fn parse_process_list(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() {
                return None;
            }
            if let Some(rest) = l.strip_prefix('"') {
                return rest.split('"').next().map(str::to_string);
            }
            Some(l.rsplit(['/', '\\']).next().unwrap_or(l).to_string())
        })
        .collect()
}

// ─── PF0I handoff ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Pf0iHandoff {
    pub authoritative_membership_commitment: String,
    pub pf0h_final_manifest_sha256: String,
    pub review_seal_sha256: String,
}

/// init event の pin を ledger から読む（Store は pin を公開しない。PF0G1 を変えないための読み取り専用ルート）
pub fn pins_from_ledger(ledger_dir: &Path) -> R<Pins> {
    let text = std::fs::read_to_string(ledger_dir.join("events.jsonl")).map_err(|e| BlindError::Stop(e.to_string()))?;
    let first: Value = serde_json::from_str(text.lines().next().ok_or_else(|| BlindError::Stop("ledger が空".into()))?).map_err(|e| BlindError::Stop(e.to_string()))?;
    if first["type"] != json!("init") {
        return stop("先頭 event が init ではありません");
    }
    let g = |k: &str| first["pins"][k].as_str().map(str::to_string).ok_or_else(|| BlindError::Stop(format!("pin {k} がありません")));
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
    pins.validate()?;
    Ok(pins)
}

/// **seal 後にだけ**開ける sealed side の取り出し口。seal 前は拒否。sealed の unblind map / secret が init の pin と一致することを確かめてから値を返す
pub fn pf0i_handoff(store: &super::pr4_blind_gt::Store, ledger_dir: &Path, sealed_dir: &Path) -> R<Pf0iHandoff> {
    if !store.unblind_allowed() {
        return Err(BlindError::Stop("seal 前は sealed side を開けません".into()));
    }
    let seal = store.review_seal_sha256().ok_or_else(|| BlindError::Stop("seal SHA がありません".into()))?.to_string();
    let pins = pins_from_ledger(ledger_dir)?;
    let rd = |n: &str| std::fs::read(sealed_dir.join(n)).map_err(|e| BlindError::Stop(format!("{n}: {e}")));
    if hex_sha(&rd("unblind_map.json")?) != pins.unblind_map_sha256 {
        return Err(BlindError::PinMismatch(vec!["unblind_map_sha256".into()]));
    }
    let bundle: Value = serde_json::from_slice(&rd("unblind_map.json")?).map_err(|e| BlindError::Stop(e.to_string()))?;
    if bundle["bundle_version"] != json!(UNBLIND_BUNDLE_VERSION) || build_secret_commitment(bundle["build_secret_hex"].as_str().unwrap_or("")) != pins.build_secret_commitment {
        return Err(BlindError::PinMismatch(vec!["build_secret_commitment".into()]));
    }
    let att: Value = serde_json::from_slice(&rd("build_attestation.json")?).map_err(|e| BlindError::Stop(e.to_string()))?;
    if att["unblind_map_sha256"] != json!(pins.unblind_map_sha256) {
        return Err(BlindError::PinMismatch(vec!["unblind_map_sha256".into()]));
    }
    let s = |k: &str| att[k].as_str().filter(|v| is_hex64(v)).map(str::to_string).ok_or_else(|| BlindError::Stop(format!("attestation.{k} が不正")));
    Ok(Pf0iHandoff {
        authoritative_membership_commitment: s("authoritative_membership_commitment")?,
        pf0h_final_manifest_sha256: s("pf0h_final_manifest_sha256")?,
        review_seal_sha256: seal,
    })
}
