//! PR4-PF0E: H2 enrollment ledger（**audit 側・append-only・手動専用・test ビルドだけ・製品には出ない**）。
//!
//! production の DB には何も書かない。入力は recovery 済みの immutable snapshot だけ（PF0A と同じ preflight）。
//! 1 回の run の中では、population の判定と入力の capture を同じ read transaction（同じ snapshot）から行う。
//!
//! # 母集団（PF0D 契約）
//! - 最初の run（`init_baseline`）が **T0**。その時点で存在する全 work の ID を baseline として固定し、以後永久に登録しない（遡及登録なし）。
//! - 以後の run（`enroll`）は、baseline にも ledger にも無い work のうち、`evidence_class = live` のものを **work id 順**に登録する（裁量なし）。
//! - strong / manual ラベル・machine decision・candidate・GT は、selection に一切使わない（このファイルはそれらの表を読まない）。
//! - 登録した unit は外さない。ledger は追記のみ。N（`CLOSE_AT` = 946）件に達したら `close` record を書いて tranche を閉じ、以後の流入は登録しない。
//! - 598 は設計の下限（count milestone）。停止条件ではない。
//!
//! # 入力 payload と enrollment context（hash を分ける）
//! - `matcher_input_payload`: matcher を再実行するのに要る入力。raw の最小集合 + `file_path` に依存する値を **値として**固定したもの（`file_path` 自体は保存しない）。
//!   hash = `matcher_input_sha256`。canonical JSON（`pr4-canonical-json-1`）。`container_tags_json` は **DB の文字列そのもの（literal）** を保存する。
//! - `enrollment_context`: 登録時点の membership / provenance（evidence_class_at_enrollment・first_seen_at・規則版・tool の sha・audit aid の evidence hash）。hash = `enrollment_context_sha256`。
//! - audit aid の `local/embedded/decision_evidence_sha256`: payload から `LocalEvidence` / `EmbeddedEvidence` を生成した結果の hash。binding な入力は payload で、これは検算用。
//!   将来 parser が変わって hash が一致しなくなっても、membership は外さない（差分は provenance として記録する）。
//!
//! # H2 enrollment event（PF0D の「作成時に登録」を置き換える正式な定義）
//! 「T0 の後の最初の scheduled observation で、非 baseline の work が target live population にいると観測された時点」。`first_seen_at` はこの意味。
//! 登録後の改名・strong ラベル・手動訂正・削除では membership を変えない。cadence は DAILY。実行できなかった scheduled run は `missed` record として残す
//! （遡及登録・裁量での追加 scan は禁止）。1 run で 946 件を超える候補があるときは work id 昇順で 946 件目まで登録して close する。
//!
//! # identity の前提
//! `works.id` は `INTEGER PRIMARY KEY AUTOINCREMENT`（`001_initial.sql`）で、削除後も再利用されない。アプリに `sqlite_sequence` を書く経路は無い。
//! それでも backup 復元などで前提が崩れたときに誤った登録をしないよう、baseline に `works_sequence`（`sqlite_sequence.seq`）を固定し、
//! 「baseline に無く id ≤ baseline の `works_sequence` の work が現れた」「`works_sequence` が減った」場合は STOP する。
//!
//! # 独立した checkpoint chain
//! 成功した run ごとに、ledger とは別のディレクトリへ checkpoint（record_count・ledger_head・works_sequence・規則 JSON の sha・tool の sha と head・前 checkpoint の sha）を追記し、
//! 次の run は ledger と checkpoint chain の両方を検証して前回値を読む（operator が値を渡さない）。checkpoint chain の head sha は report に出るので、audit freeze へ別途固定する。
//!
//! # record commitment
//! `SHA256(canonical(record without commitment) + "\n" + previous_commitment)`。先頭の previous は 64 個の `0`。`ledger_head_sha256` と `record_count` だけを外に出す。
//! 末尾の削除は chain だけでは検出できないので、`verify_ledger` は前回公表した (count, head) を受け取って照合する。

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::services::container_tags::ContainerTags;
use crate::services::pr4_3v_runner::open_snapshot;
use crate::services::pr4_pf0a::{preflight, Config as SnapConfig, Stop};
use crate::services::prematch_snapshot::{EmbeddedInputs, EvidenceClass, PreMatchSnapshot, SnapshotFile, TitleProvenance, STATE_SCHEMA_VERSION};

pub const PAYLOAD_SCHEMA_VERSION: &str = "pr4-h2-input-payload-1";
pub const CANONICALIZATION_VERSION: &str = "pr4-canonical-json-1";
pub const ENROLLMENT_RULE_VERSION: &str = "pr4-h2-enrollment-rules-2";
pub const MILESTONE_MIN: usize = 598;
pub const CLOSE_AT: usize = 946;
const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const LEDGER_FILE: &str = "ledger.jsonl";
const BASELINE_FILE: &str = "baseline_ids.json";
const PAYLOAD_DIR: &str = "payloads";
const RULES_JSON: &str = include_str!("../../../../../docs/pr4_h2_enrollment_rules.json");

fn stop<T>(m: impl Into<String>) -> Result<T, Stop> {
    Err(Stop(m.into()))
}

fn hex_sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// この tool の sha256（ソース全体）。record の `enrollment_tool_sha256` になる
pub fn tool_sha256() -> String {
    hex_sha(include_str!("pr4_pf0e.rs").as_bytes())
}

/// 凍結した規則 JSON（`docs/pr4_h2_enrollment_rules.json`）の sha256
pub fn rules_sha256() -> String {
    hex_sha(RULES_JSON.as_bytes())
}

/// `works.id` の AUTOINCREMENT 側の最大値。id の最大値より小さければ identity の前提が崩れている
fn works_sequence(conn: &Connection) -> Result<i64, Stop> {
    let seq: i64 = conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM sqlite_sequence WHERE name = 'works'", [], |r| r.get(0)).map_err(|e| Stop(e.to_string()))?;
    let max_id: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) FROM works", [], |r| r.get(0)).map_err(|e| Stop(e.to_string()))?;
    if max_id > seq {
        return stop("works の最大 id が sqlite_sequence を超えています（identity の前提が崩れています）");
    }
    Ok(seq)
}

/// canonical JSON: キーは UTF-8 のバイト順、空白なし、整数のみ（浮動小数は拒否）、null と欠落は区別する（欠落させない）
pub fn canon(v: &Value) -> Result<String, Stop> {
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
        Value::String(s) => serde_json::to_string(s).map_err(|e| Stop(e.to_string()))?,
        Value::Array(a) => format!("[{}]", a.iter().map(canon).collect::<Result<Vec<_>, _>>()?.join(",")),
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            let mut parts = Vec::with_capacity(keys.len());
            for k in keys {
                parts.push(format!("{}:{}", serde_json::to_string(k).map_err(|e| Stop(e.to_string()))?, canon(&o[k])?));
            }
            format!("{{{}}}", parts.join(","))
        }
    })
}

fn sha_of(v: &Value) -> Result<String, Stop> {
    Ok(hex_sha(canon(v)?.as_bytes()))
}

// ─── matcher_input_payload ──────────────────────────────────────────────────────────────────────

/// 1 作品の matcher 入力を登録時点の値で取り出す。`file_path` は読むが、payload には保存しない（そこから導かれる値だけ保存する）。
pub fn capture_payload(conn: &Connection, work_id: i64) -> Result<Value, Stop> {
    let title_guess: Option<String> = conn
        .query_row("SELECT title_guess FROM works WHERE id = ?1", [work_id], |r| r.get(0))
        .map_err(|e| Stop(format!("work を読めません: {e}")))?;
    let mut stmt = conn
        .prepare(
            "SELECT f.original_file_name, f.original_rel_path, f.original_captured, f.renamed_by_app,
                    f.container_tags_json, f.tags_provenance, f.tags_provider_hint, f.file_path, s.media_kind
               FROM work_parts wp JOIN files f ON f.id = wp.file_id JOIN sources s ON s.id = f.source_id
              WHERE wp.work_id = ?1 ORDER BY wp.part_no, f.id",
        )
        .map_err(|e| Stop(e.to_string()))?;
    let rows = stmt
        .query_map([work_id], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })
        .map_err(|e| Stop(e.to_string()))?;
    let mut files = Vec::new();
    for row in rows {
        let (name, rel, captured, renamed, tags_json, prov, hint, file_path, kind) = row.map_err(|e| Stop(e.to_string()))?;
        // file_path に依存する値は、ここで一度だけ計算して値として固定する
        let path_derived = match tags_json.as_deref().and_then(ContainerTags::from_json) {
            Some(tags) => {
                let path_hint = file_path.as_deref();
                json!({
                    "effective_provenance": prov.clone().unwrap_or_else(|| tags.provenance(path_hint).as_str().to_string()),
                    "effective_provider_hint": hint.clone().or_else(|| tags.provider_hint().map(str::to_string)),
                    "episode_id_marks_movie": tags.episode_id_marks_movie(path_hint),
                })
            }
            None => Value::Null,
        };
        files.push(json!({
            "original_file_name": name,
            "original_rel_path": rel,
            "original_captured": captured.unwrap_or(0) == 1,
            "renamed_by_app": renamed,
            "container_tags_json": tags_json,
            "tags_provenance": prov,
            "tags_provider_hint": hint,
            "source_media_kind": kind.unwrap_or_else(|| "unknown".to_string()),
            "path_derived": path_derived,
        }));
    }
    Ok(json!({
        "payload_schema_version": PAYLOAD_SCHEMA_VERSION,
        "canonicalization_version": CANONICALIZATION_VERSION,
        "file_order_basis": "work_parts.part_no,files.id",
        "work": { "title_guess": title_guess },
        "files": files,
    }))
}

fn opt_str(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

/// payload だけから `PreMatchSnapshot` を作る（DB を読まない）。`evidence_class` は matcher が読まないので、呼び出し側が渡す（登録時の値）。
/// `capture` と同じ選択規則（複数ファイルは中身のあるタグの最初のファイル）を使う。一致はテストで固定している。
pub fn snapshot_from_payload(payload: &Value, work_id: i64, evidence_class: EvidenceClass) -> Result<PreMatchSnapshot, Stop> {
    if payload["payload_schema_version"] != PAYLOAD_SCHEMA_VERSION {
        return stop("payload_schema_version が違います");
    }
    let title_guess = opt_str(&payload["work"]["title_guess"]);
    let raw_files = payload["files"].as_array().ok_or_else(|| Stop("files が配列ではありません".into()))?;
    let mut files = Vec::new();
    let mut tag_inputs: Vec<EmbeddedInputs> = Vec::new();
    for f in raw_files {
        files.push(SnapshotFile {
            original_file_name: opt_str(&f["original_file_name"]),
            original_rel_path: opt_str(&f["original_rel_path"]),
            original_captured: f["original_captured"].as_bool().unwrap_or(false),
            renamed_by_app: opt_str(&f["renamed_by_app"]),
            extension: None,
            duration_sec: None,
            source_media_kind: opt_str(&f["source_media_kind"]).unwrap_or_else(|| "unknown".to_string()),
            tags_captured: false,
        });
        if let Some(tags) = f["container_tags_json"].as_str().and_then(ContainerTags::from_json) {
            let pd = &f["path_derived"];
            tag_inputs.push(EmbeddedInputs {
                title: tags.cleaned_title(),
                title_raw: tags.title.clone(),
                show: tags.cleaned_show(),
                year: tags.year(),
                cut_editions: tags.cut_editions(),
                audio_languages: tags.audio_languages.clone(),
                subtitle_languages: tags.subtitle_languages.clone(),
                cast: tags.cast(),
                description: tags.description.clone(),
                episode_id: tags.episode_id.clone(),
                episode_id_marks_movie: pd["episode_id_marks_movie"].as_bool().unwrap_or(false),
                provenance: opt_str(&pd["effective_provenance"]),
                provider_hint: opt_str(&pd["effective_provider_hint"]),
                truncated: !tags.truncated.is_empty(),
            });
        }
    }
    let embedded = tag_inputs
        .iter()
        .find(|i| i.title.is_some() || i.year.is_some())
        .or_else(|| tag_inputs.first())
        .cloned()
        .unwrap_or_default();
    let title_guess = title_guess.filter(|t| !t.trim().is_empty());
    let first_original = files.iter().find_map(|f| f.original_file_name.as_deref().filter(|n| !n.trim().is_empty()));
    let (derived_title, title_provenance) = match (&title_guess, first_original) {
        (Some(g), _) => (g.clone(), TitleProvenance::TitleGuess),
        (None, Some(name)) => (crate::commands::scan::estimate_title(name), TitleProvenance::OriginalFileName),
        (None, None) => (String::new(), TitleProvenance::None),
    };
    let media_kind = files
        .iter()
        .map(|f| f.source_media_kind.as_str())
        .find(|k| *k == "movie" || *k == "tv")
        .unwrap_or("unknown")
        .to_string();
    let filename_year = files.iter().filter_map(|f| f.original_file_name.as_deref()).find_map(crate::services::container_tags::extract_year);
    let title_guess_year = title_guess.as_deref().and_then(crate::services::container_tags::extract_year);
    Ok(PreMatchSnapshot {
        state_schema_version: STATE_SCHEMA_VERSION,
        work_id,
        work_created_at: None,
        title_guess,
        derived_title,
        title_provenance,
        media_kind,
        // 以下は matcher の判定に入らない（record only / cohort）。payload には持たない
        country_type: "unknown".to_string(),
        filename_year,
        title_guess_year,
        embedded,
        files,
        embedded_ids: Vec::new(),
        evidence_class,
    })
}

/// (local, embedded, decision) の evidence hash。audit aid
pub fn evidence_hashes(snap: &PreMatchSnapshot) -> Result<(String, String, String), Stop> {
    let le = snap.local_evidence();
    let ee = snap.embedded_evidence();
    let local = json!({
        "title": le.title(),
        "embedded_title": le.embedded_title(),
        "media_kind": le.media_kind(),
        "filename_year": le.filename_year(),
        "embedded_year": le.embedded_year(),
        "search_inputs": le.search_inputs().iter().map(|s| json!({"source": s.source.as_str(), "title": s.title, "year": s.year})).collect::<Vec<_>>(),
    });
    let embedded = json!({
        "embedded_year": ee.embedded_year,
        "filename_year": ee.filename_year,
        "audio_languages": ee.audio_languages,
        "cut_editions": ee.cut_editions,
        "episode_id_marks_movie": ee.episode_id_marks_movie,
    });
    let decision = json!({"local": local, "embedded": embedded});
    Ok((sha_of(&local)?, sha_of(&embedded)?, sha_of(&decision)?))
}

// ─── ledger ────────────────────────────────────────────────────────────────────────────────────

fn valid_time(t: &str) -> bool {
    let b = t.as_bytes();
    t.len() == 20
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && b[19] == b'Z'
        && t.bytes().enumerate().all(|(i, c)| matches!(i, 4 | 7 | 10 | 13 | 16 | 19) || c.is_ascii_digit())
}

fn commitment(record_without: &Value, prev: &str) -> Result<String, Stop> {
    Ok(hex_sha(format!("{}\n{}", canon(record_without)?, prev).as_bytes()))
}

fn with_commitment(mut record: Value, prev: &str) -> Result<Value, Stop> {
    let c = commitment(&record, prev)?;
    record["record_commitment"] = json!(c);
    Ok(record)
}

#[derive(Debug, Clone)]
pub struct LedgerState {
    pub records: Vec<Value>,
    pub head: String,
    pub baseline: BTreeSet<i64>,
    pub enrolled: BTreeSet<i64>,
    pub closed: bool,
    pub last_observed: String,
    pub t0: String,
    pub baseline_sequence: i64,
}

fn baseline_hash(ids: &BTreeSet<i64>) -> String {
    let mut pre = String::from("pr4-h2-baseline-1\n");
    for id in ids {
        pre.push_str(&format!("work:{id}\n"));
    }
    hex_sha(pre.as_bytes())
}

fn read_baseline(dir: &Path) -> Result<BTreeSet<i64>, Stop> {
    let v: Value = serde_json::from_slice(&std::fs::read(dir.join(BASELINE_FILE)).map_err(|e| Stop(format!("baseline を読めません: {e}")))?).map_err(|e| Stop(e.to_string()))?;
    Ok(v["ids"].as_array().ok_or_else(|| Stop("baseline の形式が不正です".into()))?.iter().filter_map(Value::as_i64).collect())
}

/// ledger を頭から検証する。壊れていれば Stop（fail closed）。`expected` は前回公表した (record_count, head)
pub fn verify_ledger(dir: &Path, expected: Option<(usize, &str)>) -> Result<LedgerState, Stop> {
    let text = std::fs::read_to_string(dir.join(LEDGER_FILE)).map_err(|e| Stop(format!("ledger を読めません: {e}")))?;
    let mut st = LedgerState { records: Vec::new(), head: GENESIS.to_string(), baseline: BTreeSet::new(), enrolled: BTreeSet::new(), closed: false, last_observed: String::new(), t0: String::new(), baseline_sequence: 0 };
    for (i, line) in text.lines().enumerate() {
        let rec: Value = serde_json::from_str(line).map_err(|e| Stop(format!("record {} を解釈できません: {e}", i + 1)))?;
        let mut body = rec.clone();
        let stored = body.as_object_mut().and_then(|o| o.remove("record_commitment")).and_then(|v| v.as_str().map(str::to_string)).ok_or_else(|| Stop("record_commitment がありません".into()))?;
        if body["seq"].as_u64() != Some(i as u64 + 1) {
            return stop(format!("record {} の seq が連続していません", i + 1));
        }
        if body["prev_commitment"].as_str() != Some(st.head.as_str()) {
            return stop(format!("record {} の prev_commitment が一致しません", i + 1));
        }
        if commitment(&body, &st.head)? != stored {
            return stop(format!("record {} の commitment が一致しません（改変）", i + 1));
        }
        let kind = body["kind"].as_str().unwrap_or("");
        if (i == 0) != (kind == "baseline") {
            return stop("baseline は先頭に 1 つだけ必要です");
        }
        if st.closed {
            return stop("close の後に record があります");
        }
        let observed = body["observed_at"].as_str().or_else(|| body["first_seen_at"].as_str()).unwrap_or("").to_string();
        if !valid_time(&observed) || observed < st.last_observed {
            return stop(format!("record {} の時刻が不正か、逆戻りしています", i + 1));
        }
        match kind {
            "baseline" => {
                st.baseline = read_baseline(dir)?;
                if baseline_hash(&st.baseline) != body["baseline_membership_sha256"].as_str().unwrap_or("") || st.baseline.len() as i64 != body["baseline_count"].as_i64().unwrap_or(-1) {
                    return stop("baseline が record と一致しません");
                }
                st.baseline_sequence = body["works_sequence"].as_i64().ok_or_else(|| Stop("baseline に works_sequence がありません".into()))?;
                st.t0 = observed.clone();
            }
            "enroll" => {
                let key = body["evaluation_unit_key"].as_str().unwrap_or("");
                let id: i64 = key.strip_prefix("work:").and_then(|s| s.parse().ok()).ok_or_else(|| Stop("evaluation_unit_key が不正です".into()))?;
                if id <= st.baseline_sequence {
                    return stop("baseline の works_sequence 以下の id が登録されています（identity の前提が崩れています）");
                }
                if st.baseline.contains(&id) || !st.enrolled.insert(id) {
                    return stop("baseline または登録済みの unit が再登録されています");
                }
                if sha_of(&body["enrollment_context"])? != body["enrollment_context_sha256"].as_str().unwrap_or("") {
                    return stop("enrollment_context の hash が一致しません");
                }
                let sha = body["matcher_input_sha256"].as_str().unwrap_or("");
                let path = dir.join(PAYLOAD_DIR).join(format!("{sha}.json"));
                let bytes = std::fs::read(&path).map_err(|_| Stop(format!("record {} の payload がありません", i + 1)))?;
                let pv: Value = serde_json::from_slice(&bytes).map_err(|e| Stop(e.to_string()))?;
                if sha_of(&pv)? != sha {
                    return stop(format!("record {} の payload が hash と一致しません", i + 1));
                }
            }
            "missed" => {}
            "close" => {
                if body["enrolled_count"].as_u64() != Some(st.enrolled.len() as u64) {
                    return stop("close の件数が一致しません");
                }
                st.closed = true;
            }
            _ => return stop("未知の record 種別です"),
        }
        st.last_observed = observed;
        st.head = stored;
        st.records.push(rec);
    }
    if st.records.is_empty() {
        return stop("ledger が空です");
    }
    if let Some((count, head)) = expected {
        if count > st.records.len() || (count > 0 && st.records[count - 1]["record_commitment"].as_str() != Some(head)) {
            return stop("公表済みの (count, head) と一致しません（削除・差し替え）");
        }
    }
    Ok(st)
}

fn append_line(dir: &Path, record: &Value) -> Result<(), Stop> {
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(LEDGER_FILE)).map_err(|e| Stop(e.to_string()))?;
    f.write_all(format!("{}\n", canon(record)?).as_bytes()).map_err(|e| Stop(e.to_string()))?;
    f.sync_all().map_err(|e| Stop(e.to_string()))
}

fn write_payload(dir: &Path, payload: &Value) -> Result<String, Stop> {
    let text = canon(payload)?;
    let sha = hex_sha(text.as_bytes());
    let d = dir.join(PAYLOAD_DIR);
    std::fs::create_dir_all(&d).map_err(|e| Stop(e.to_string()))?;
    let p = d.join(format!("{sha}.json"));
    if p.exists() {
        if std::fs::read_to_string(&p).map_err(|e| Stop(e.to_string()))? != text {
            return stop("同じ hash の payload が内容違いで存在します");
        }
    } else {
        std::fs::write(&p, text).map_err(|e| Stop(e.to_string()))?;
    }
    Ok(sha)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub kind: &'static str,
    pub enrolled_now: usize,
    pub skipped_not_live: usize,
    pub total_enrolled: usize,
    pub closed: bool,
    pub record_count: usize,
    pub ledger_head_sha256: String,
    pub works_sequence: i64,
}

fn check_time(t: &str) -> Result<(), Stop> {
    if valid_time(t) {
        Ok(())
    } else {
        stop("observed_at は YYYY-MM-DDTHH:MM:SSZ で指定してください")
    }
}

/// T0 の run。存在する全 work を baseline として固定する。ledger が既にあれば STOP
pub fn init_baseline(conn: &Connection, dir: &Path, observed_at: &str) -> Result<Report, Stop> {
    check_time(observed_at)?;
    if dir.join(LEDGER_FILE).exists() || dir.join(BASELINE_FILE).exists() {
        return stop("ledger が既にあります（上書きしない）");
    }
    std::fs::create_dir_all(dir).map_err(|e| Stop(e.to_string()))?;
    let mut stmt = conn.prepare("SELECT id FROM works ORDER BY id").map_err(|e| Stop(e.to_string()))?;
    let ids: BTreeSet<i64> = stmt.query_map([], |r| r.get::<_, i64>(0)).map_err(|e| Stop(e.to_string()))?.collect::<Result<_, _>>().map_err(|e| Stop(e.to_string()))?;
    std::fs::write(dir.join(BASELINE_FILE), canon(&json!({"ids": ids.iter().collect::<Vec<_>>()}))?).map_err(|e| Stop(e.to_string()))?;
    let rec = with_commitment(
        json!({"seq": 1, "kind": "baseline", "observed_at": observed_at, "baseline_count": ids.len(), "works_sequence": works_sequence(conn)?, "baseline_membership_sha256": baseline_hash(&ids),
               "enrollment_rule_version": ENROLLMENT_RULE_VERSION, "enrollment_tool_sha256": tool_sha256(), "prev_commitment": GENESIS}),
        GENESIS,
    )?;
    append_line(dir, &rec)?;
    let st = verify_ledger(dir, None)?;
    Ok(Report { kind: "baseline", enrolled_now: 0, skipped_not_live: 0, total_enrolled: 0, closed: false, record_count: 1, ledger_head_sha256: st.head, works_sequence: st.baseline_sequence })
}

/// 通常の run。`cap` は tranche を閉じる件数（製品の値は `CLOSE_AT`）
pub fn enroll_with_cap(conn: &Connection, dir: &Path, observed_at: &str, expected: Option<(usize, &str)>, cap: usize) -> Result<Report, Stop> {
    check_time(observed_at)?;
    let mut st = verify_ledger(dir, expected)?;
    let seq_now = works_sequence(conn)?;
    if seq_now < st.baseline_sequence {
        return stop("works_sequence が baseline より減っています（復元・巻き戻しの疑い）");
    }
    let report = |st: &LedgerState, now: usize, skipped: usize| Report { kind: "enroll", enrolled_now: now, skipped_not_live: skipped, total_enrolled: st.enrolled.len(), closed: st.closed, record_count: st.records.len(), ledger_head_sha256: st.head.clone(), works_sequence: seq_now };
    if st.closed {
        return Ok(report(&st, 0, 0));
    }
    if observed_at < st.last_observed.as_str() {
        return stop("observed_at が前の record より前です");
    }
    // population の判定と capture を、同じ read transaction（同じ snapshot）から行う
    conn.execute_batch("BEGIN").map_err(|e| Stop(e.to_string()))?;
    let result = (|| -> Result<(usize, usize), Stop> {
        let mut stmt = conn.prepare("SELECT id FROM works ORDER BY id").map_err(|e| Stop(e.to_string()))?;
        let ids: Vec<i64> = stmt.query_map([], |r| r.get::<_, i64>(0)).map_err(|e| Stop(e.to_string()))?.collect::<Result<_, _>>().map_err(|e| Stop(e.to_string()))?;
        let (mut now, mut skipped) = (0usize, 0usize);
        for id in ids {
            if st.enrolled.len() >= cap {
                break;
            }
            if st.baseline.contains(&id) || st.enrolled.contains(&id) {
                continue;
            }
            if id <= st.baseline_sequence {
                return stop("baseline に無く id が baseline の works_sequence 以下の work があります（identity の前提が崩れています）");
            }
            let snap = PreMatchSnapshot::capture(conn, id).map_err(Stop)?;
            if snap.evidence_class != EvidenceClass::Live {
                skipped += 1;
                continue;
            }
            let payload = capture_payload(conn, id)?;
            let (local, embedded, decision) = evidence_hashes(&snapshot_from_payload(&payload, id, snap.evidence_class)?)?;
            let input_sha = write_payload(dir, &payload)?;
            let context = json!({
                "evidence_class_at_enrollment": snap.evidence_class.as_str(),
                "first_seen_at": observed_at,
                "enrollment_rule_version": ENROLLMENT_RULE_VERSION,
                "enrollment_tool_sha256": tool_sha256(),
                "payload_schema_version": PAYLOAD_SCHEMA_VERSION,
                "canonicalization_version": CANONICALIZATION_VERSION,
                "local_evidence_sha256": local,
                "embedded_evidence_sha256": embedded,
                "decision_evidence_sha256": decision,
            });
            let rec = with_commitment(
                json!({"seq": st.records.len() + 1, "kind": "enroll", "evaluation_unit_key": format!("work:{id}"), "first_seen_at": observed_at,
                       "matcher_input_sha256": input_sha, "enrollment_context_sha256": sha_of(&context)?, "enrollment_context": context, "prev_commitment": st.head}),
                &st.head,
            )?;
            append_line(dir, &rec)?;
            st.head = rec["record_commitment"].as_str().unwrap_or_default().to_string();
            st.records.push(rec);
            st.enrolled.insert(id);
            now += 1;
        }
        if st.enrolled.len() >= cap {
            let rec = with_commitment(
                json!({"seq": st.records.len() + 1, "kind": "close", "observed_at": observed_at, "enrolled_count": st.enrolled.len(), "prev_commitment": st.head}),
                &st.head,
            )?;
            append_line(dir, &rec)?;
        }
        Ok((now, skipped))
    })();
    let _ = conn.execute_batch("ROLLBACK");
    let (now, skipped) = result?;
    let after = verify_ledger(dir, None)?;
    Ok(report(&after, now, skipped))
}

pub fn enroll(conn: &Connection, dir: &Path, observed_at: &str, expected: Option<(usize, &str)>) -> Result<Report, Stop> {
    enroll_with_cap(conn, dir, observed_at, expected, CLOSE_AT)
}

/// 公開してよい出力。件数と head だけ（キー・work id・payload・判定を含まない）
pub fn report_json(r: &Report, observed_at: &str, snapshot_sha: &str, source_head: &str) -> Value {
    json!({
        "protocol": "pr4-pf0e-enrollment-run-1",
        "kind": r.kind,
        "observed_at": observed_at,
        "source_head": source_head,
        "snapshot_sha256": snapshot_sha,
        "enrollment_tool_sha256": tool_sha256(),
        "enrolled_now": r.enrolled_now,
        "skipped_not_live": r.skipped_not_live,
        "total_enrolled": r.total_enrolled,
        "closed": r.closed,
        "record_count": r.record_count,
        "ledger_head_sha256": r.ledger_head_sha256,
        "milestone_min_reached": r.total_enrolled >= MILESTONE_MIN,
        "limits": "count-only; no unit keys, no payloads, no judgments, no performance figures",
    })
}

pub fn assert_report_clean(v: &Value) -> Result<(), Stop> {
    let text = v.to_string();
    for banned in ["work:", "work_id", "tmdb", "title", "verdict", "candidate", "decision", "precision", "label"] {
        if text.contains(banned) {
            return stop(format!("出力に {banned} が含まれています"));
        }
    }
    if v.as_object().map(|o| o.values().any(|x| x.is_array() || x.is_object())).unwrap_or(true) {
        return stop("出力に配列・object が含まれています");
    }
    Ok(())
}

/// 実行できなかった scheduled run の記録（遡及登録はしない）。`scheduled_at` はその run の予定時刻。
/// missed run は DB を観測しないので、`works_sequence` は直前 checkpoint の値を持ち越す（baseline の値に戻さない）
pub fn record_missed(dir: &Path, scheduled_at: &str, expected: Option<(usize, &str)>, carried_works_sequence: i64) -> Result<Report, Stop> {
    check_time(scheduled_at)?;
    let st = verify_ledger(dir, expected)?;
    if st.closed || scheduled_at < st.last_observed.as_str() {
        return stop("閉じた ledger か、時刻が前の record より前です");
    }
    let rec = with_commitment(json!({"seq": st.records.len() + 1, "kind": "missed", "observed_at": scheduled_at, "prev_commitment": st.head}), &st.head)?;
    append_line(dir, &rec)?;
    let after = verify_ledger(dir, None)?;
    Ok(Report { kind: "missed", enrolled_now: 0, skipped_not_live: 0, total_enrolled: after.enrolled.len(), closed: false, record_count: after.records.len(), ledger_head_sha256: after.head, works_sequence: carried_works_sequence.max(after.baseline_sequence) })
}

// ─── ledger とは独立した checkpoint chain ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub seq: usize,
    pub record_count: usize,
    pub ledger_head_sha256: String,
    pub works_sequence: i64,
    pub sha256: String,
}

/// checkpoint chain を頭から検証し、最後の checkpoint を返す。無ければ None
pub fn latest_checkpoint(dir: &Path) -> Result<Option<Checkpoint>, Stop> {
    if !dir.exists() {
        return Ok(None);
    }
    let mut names: Vec<String> = std::fs::read_dir(dir).map_err(|e| Stop(e.to_string()))?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
    names.sort();
    let mut prev = GENESIS.to_string();
    let mut last = None;
    for (i, name) in names.iter().enumerate() {
        if *name != format!("{:04}.json", i + 1) {
            return stop("checkpoint の連番が不正です");
        }
        let v: Value = serde_json::from_slice(&std::fs::read(dir.join(name)).map_err(|e| Stop(e.to_string()))?).map_err(|e| Stop(e.to_string()))?;
        if v["prev_checkpoint_sha256"].as_str() != Some(prev.as_str()) || v["seq"].as_u64() != Some(i as u64 + 1) {
            return stop("checkpoint の chain が一致しません");
        }
        prev = sha_of(&v)?;
        last = Some(Checkpoint {
            seq: i + 1,
            record_count: v["record_count"].as_u64().ok_or_else(|| Stop("checkpoint が不正です".into()))? as usize,
            ledger_head_sha256: v["ledger_head_sha256"].as_str().unwrap_or("").to_string(),
            works_sequence: v["works_sequence"].as_i64().unwrap_or(-1),
            sha256: prev.clone(),
        });
    }
    Ok(last)
}

fn write_checkpoint(dir: &Path, prev: Option<&Checkpoint>, r: &Report, run_utc: &str, source_head: &str) -> Result<Checkpoint, Stop> {
    std::fs::create_dir_all(dir).map_err(|e| Stop(e.to_string()))?;
    let seq = prev.map(|c| c.seq).unwrap_or(0) + 1;
    let v = json!({
        "seq": seq, "run_utc": run_utc, "kind": r.kind, "record_count": r.record_count, "ledger_head_sha256": r.ledger_head_sha256, "works_sequence": r.works_sequence,
        "enrollment_rule_sha256": rules_sha256(), "enrollment_tool_sha256": tool_sha256(), "tool_head": source_head,
        "prev_checkpoint_sha256": prev.map(|c| c.sha256.clone()).unwrap_or_else(|| GENESIS.to_string()),
    });
    let sha = sha_of(&v)?;
    let path = dir.join(format!("{seq:04}.json"));
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| Stop(format!("checkpoint を書けません: {e}")))?;
    f.write_all(format!("{}
", canon(&v)?).as_bytes()).map_err(|e| Stop(e.to_string()))?;
    f.sync_all().map_err(|e| Stop(e.to_string()))?;
    Ok(Checkpoint { seq, record_count: r.record_count, ledger_head_sha256: r.ledger_head_sha256.clone(), works_sequence: r.works_sequence, sha256: sha })
}

/// 1 回の run（preflight → 前回 checkpoint の検証 → immutable snapshot → init_baseline / enroll / missed → 新しい checkpoint → 件数だけの report）。
/// 前回の (count, head) は operator が渡さず、独立した checkpoint chain から読む。
pub fn run(snap_cfg: &SnapConfig, action: &str, ledger_dir: &Path, checkpoint_dir: &Path, observed_at: &str) -> Result<(PathBuf, Value), Stop> {
    std::fs::create_dir_all(ledger_dir).and(std::fs::create_dir_all(checkpoint_dir)).map_err(|e| Stop(e.to_string()))?;
    let (l, c) = (ledger_dir.canonicalize().map_err(|e| Stop(e.to_string()))?, checkpoint_dir.canonicalize().map_err(|e| Stop(e.to_string()))?);
    if l.starts_with(&c) || c.starts_with(&l) {
        return stop("checkpoint は ledger とは別のディレクトリに置いてください");
    }
    let sha = preflight(snap_cfg)?;
    let prev = latest_checkpoint(checkpoint_dir)?;
    let expected = prev.as_ref().map(|p| (p.record_count, p.ledger_head_sha256.clone()));
    let e = expected.as_ref().map(|(c, h)| (*c, h.as_str()));
    let report = match action {
        "init_baseline" => {
            if prev.is_some() {
                return stop("checkpoint が既にあります（baseline は 1 回だけ）");
            }
            let conn = open_snapshot(&snap_cfg.snapshot_db).map_err(|e| Stop(e.0))?;
            init_baseline(&conn, ledger_dir, observed_at)?
        }
        "enroll" | "missed" => {
            let (Some(p), Some(e)) = (prev.as_ref(), e) else { return stop("前回の checkpoint がありません（init_baseline が先です）") };
            let r = if action == "missed" {
                record_missed(ledger_dir, observed_at, Some(e), p.works_sequence)?
            } else {
                let conn = open_snapshot(&snap_cfg.snapshot_db).map_err(|e| Stop(e.0))?;
                enroll(&conn, ledger_dir, observed_at, Some(e))?
            };
            if r.works_sequence < p.works_sequence {
                return stop("works_sequence が前回の checkpoint より減っています（復元・巻き戻しの疑い）");
            }
            r
        }
        _ => return stop("action は init_baseline / enroll / missed です"),
    };
    let cp = write_checkpoint(checkpoint_dir, prev.as_ref(), &report, observed_at, &snap_cfg.source_head)?;
    let mut out = report_json(&report, observed_at, &sha, &snap_cfg.source_head);
    out["checkpoint_seq"] = json!(cp.seq);
    out["checkpoint_sha256"] = json!(cp.sha256);
    out["works_sequence"] = json!(report.works_sequence);
    assert_report_clean(&out)?;
    let dir = ledger_dir.join("reports");
    std::fs::create_dir_all(&dir).map_err(|e| Stop(e.to_string()))?;
    let path = dir.join(format!("{}_{}.json", observed_at.replace(':', ""), report.record_count));
    if path.exists() {
        return stop("同じ report が既にあります（上書きしない）");
    }
    std::fs::write(&path, serde_json::to_string_pretty(&out).unwrap_or_default() + "
").map_err(|e| Stop(e.to_string()))?;
    Ok((path, out))
}

#[test]
#[ignore = "manual: needs a recovered immutable snapshot; CINEMANTIS_PR4_PF0E_ACTION=init_baseline|enroll|missed"]
fn manual_pr4_pf0e_run() {
    let get = |n: &str| std::env::var(n).ok().filter(|v| !v.trim().is_empty());
    let need = |n: &str| get(n).unwrap_or_else(|| panic!("STOP: 環境変数 {n} が未設定です"));
    let cfg = SnapConfig {
        snapshot_db: PathBuf::from(need("CINEMANTIS_PR4_PF0E_SNAPSHOT_DB")),
        expected_snapshot_sha256: need("CINEMANTIS_PR4_PF0E_SNAPSHOT_SHA256"),
        output_dir: PathBuf::from(need("CINEMANTIS_PR4_PF0E_LEDGER_DIR")),
        source_head: need("CINEMANTIS_PR4_PF0E_SOURCE_HEAD"),
    };
    let checkpoints = PathBuf::from(need("CINEMANTIS_PR4_PF0E_CHECKPOINT_DIR"));
    let (path, out) = run(&cfg, &need("CINEMANTIS_PR4_PF0E_ACTION"), &cfg.output_dir, &checkpoints, &need("CINEMANTIS_PR4_PF0E_OBSERVED_AT")).unwrap_or_else(|e| panic!("STOP: {}", e.0));
    println!("{out}
report = {}", path.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::scan::insert_scanned_work;
    use crate::db::test_support::{insert_source, open_migrated};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);
    const T0: &str = "2026-10-10T00:00:00Z";
    const T1: &str = "2026-10-11T00:00:00Z";
    const T2: &str = "2026-10-12T00:00:00Z";
    const TAGS: &str = r#"{"v":1,"title":"Alien","comment":"https://video.unext.jp/x","episode_id":"-1","audio_languages":["jpn"]}"#;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pr4_pf0e_{tag}_{}_{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// live な work（original_* 済み）。`path` は files.file_path
    fn work(conn: &Connection, index: usize, path: &str, tags: Option<&str>) -> i64 {
        let root = r"D:\Live";
        let source_id = conn.query_row("SELECT id FROM sources WHERE root_path = ?1", [root], |r| r.get::<_, i64>(0)).unwrap_or_else(|_| insert_source(conn, root));
        let w = insert_scanned_work(conn, "movie", "unknown", &format!("w{index}"), "unknown").unwrap();
        conn.execute("INSERT INTO files (source_id, file_path, file_name, extension, container) VALUES (?1, ?2, ?3, 'mkv', 'mkv')", rusqlite::params![source_id, path, format!("{index}.mkv")]).unwrap();
        let f = conn.last_insert_rowid();
        conn.execute("INSERT INTO work_parts (work_id, file_id) VALUES (?1, ?2)", rusqlite::params![w, f]).unwrap();
        conn.execute("UPDATE files SET original_file_name = file_name, original_rel_path = file_name, original_captured = 1 WHERE id = ?1", [f]).unwrap();
        if let Some(t) = tags {
            conn.execute("UPDATE files SET container_tags_json = ?2, tags_captured = 1 WHERE id = ?1", rusqlite::params![f, t]).unwrap();
        }
        w
    }

    fn live(conn: &Connection, i: usize) -> i64 {
        work(conn, i, &format!(r"D:\Live\{i}.mkv"), None)
    }

    /// 非 live（改名済み）
    fn renamed(conn: &Connection, i: usize) -> i64 {
        let w = live(conn, i);
        conn.execute("UPDATE files SET renamed_by_app = 'nas_sort' WHERE id = (SELECT file_id FROM work_parts WHERE work_id = ?1)", [w]).unwrap();
        w
    }

    fn strong(conn: &Connection, w: i64) {
        conn.execute("INSERT INTO metadata_match_labels (work_id, label, tmdb_id, media_type, method, strength) VALUES (?1, 'tmdb', 1, 'movie', 'manual_apply', 'strong')", [w]).unwrap();
    }

    #[test]
    fn canonical_json_is_ordered_compact_integer_only_and_keeps_null() {
        assert_eq!(canon(&json!({"b": 1, "a": [true, null, "é"]})).unwrap(), r#"{"a":[true,null,"é"],"b":1}"#);
        assert_ne!(canon(&json!({"a": null})).unwrap(), canon(&json!({})).unwrap(), "null と欠落は別");
        assert!(canon(&json!({"a": 1.5})).is_err());
        assert_eq!(sha_of(&json!({"x": 1, "y": 2})).unwrap(), sha_of(&json!({"y": 2, "x": 1})).unwrap());
    }

    #[test]
    fn the_payload_regenerates_exactly_the_evidence_the_matcher_sees() {
        let conn = open_migrated();
        let a = work(&conn, 1, r"D:\Live\1.mkv", Some(TAGS));
        let b = live(&conn, 2);
        for w in [a, b] {
            let payload = capture_payload(&conn, w).unwrap();
            let snap = PreMatchSnapshot::capture(&conn, w).unwrap();
            let derived = snapshot_from_payload(&payload, w, snap.evidence_class).unwrap();
            assert_eq!(evidence_hashes(&snap).unwrap(), evidence_hashes(&derived).unwrap());
            assert_eq!(snap.local_evidence(), derived.local_evidence());
            assert_eq!(snap.derived_title, derived.derived_title);
        }
        let p = capture_payload(&conn, a).unwrap();
        assert!(!canon(&p).unwrap().contains(r"D:\"), "file_path は payload に保存しない");
        assert_eq!(p["files"][0]["path_derived"]["episode_id_marks_movie"], true);
        assert_eq!(p["files"][0]["container_tags_json"], TAGS, "container_tags_json は literal");
    }

    #[test]
    fn changing_only_file_path_later_cannot_change_the_frozen_input() {
        let conn = open_migrated();
        // タグに配信元の手がかりが無く、path に u-next が入ると episode_id_marks_movie が変わる（= path 依存が実在する）
        let tags = r#"{"v":1,"title":"Alien","episode_id":"-1"}"#;
        let w = work(&conn, 1, r"D:\Live\1.mkv", Some(tags));
        let frozen = capture_payload(&conn, w).unwrap();
        let before = evidence_hashes(&snapshot_from_payload(&frozen, w, EvidenceClass::Live).unwrap()).unwrap();
        conn.execute("UPDATE files SET file_path = 'D:\\U-Next\\renamed.mkv' WHERE id = (SELECT file_id FROM work_parts WHERE work_id = ?1)", [w]).unwrap();
        let fresh = capture_payload(&conn, w).unwrap();
        assert_ne!(sha_of(&fresh).unwrap(), sha_of(&frozen).unwrap(), "生の再取得は path で変わる（依存の実在）");
        assert_eq!(evidence_hashes(&snapshot_from_payload(&frozen, w, EvidenceClass::Live).unwrap()).unwrap(), before, "固定した payload からの evidence は不変");
    }

    #[test]
    fn baseline_excludes_everything_that_existed_at_t0_and_new_live_works_are_enrolled_once() {
        let conn = open_migrated();
        let old = live(&conn, 1);
        let dir = tmp("enrol");
        let r0 = init_baseline(&conn, &dir, T0).unwrap();
        assert_eq!(r0.record_count, 1);
        let fresh = live(&conn, 2);
        let skipped = renamed(&conn, 3);
        let r1 = enroll(&conn, &dir, T1, Some((1, &r0.ledger_head_sha256))).unwrap();
        assert_eq!((r1.enrolled_now, r1.skipped_not_live, r1.total_enrolled), (1, 1, 1));
        let st = verify_ledger(&dir, None).unwrap();
        assert!(st.enrolled.contains(&fresh) && !st.enrolled.contains(&old) && !st.enrolled.contains(&skipped));
        // 再走査: 追加なし・head 不変
        let r2 = enroll(&conn, &dir, T2, Some((r1.record_count, &r1.ledger_head_sha256))).unwrap();
        assert_eq!((r2.enrolled_now, r2.record_count, r2.ledger_head_sha256.clone()), (0, r1.record_count, r1.ledger_head_sha256.clone()));
        assert!(init_baseline(&conn, &dir, T2).is_err(), "baseline は上書きしない");
        assert!(enroll(&conn, &dir, "2026-10-09T00:00:00Z", None).is_err(), "時刻の逆戻りは STOP");
    }

    #[test]
    fn labels_corrections_and_machine_state_never_change_membership() {
        // 同じ構成の 2 つの DB。片方だけ登録前から strong ラベル・verdict 相当の行がある
        let (plain, loaded) = (open_migrated(), open_migrated());
        for c in [&plain, &loaded] {
            live(c, 1);
        }
        let (dp, dl) = (tmp("plain"), tmp("loaded"));
        let h0 = init_baseline(&plain, &dp, T0).unwrap().ledger_head_sha256;
        assert_eq!(h0, init_baseline(&loaded, &dl, T0).unwrap().ledger_head_sha256);
        let ids: Vec<(i64, i64)> = [2usize, 3].iter().map(|i| (live(&plain, *i), live(&loaded, *i))).collect();
        strong(&loaded, ids[0].1);
        loaded.execute("UPDATE works SET title = '手動で直した題名', match_status = 'locked' WHERE id = ?1", [ids[1].1]).unwrap();
        let rp = enroll(&plain, &dp, T1, Some((1, &h0))).unwrap();
        let rl = enroll(&loaded, &dl, T1, Some((1, &h0))).unwrap();
        assert_eq!((rp.enrolled_now, rp.ledger_head_sha256.clone()), (2, rl.ledger_head_sha256.clone()), "ラベル・手動訂正があっても同じ登録・同じ head");
        // 登録後に strong / manual correction が付いても所属は変わらない
        strong(&plain, ids[1].0);
        plain.execute("UPDATE works SET title = '登録後に直した', match_status = 'matched' WHERE id = ?1", [ids[0].0]).unwrap();
        let after = enroll(&plain, &dp, T2, Some((rp.record_count, &rp.ledger_head_sha256))).unwrap();
        assert_eq!((after.total_enrolled, after.record_count, after.ledger_head_sha256), (2, rp.record_count, rp.ledger_head_sha256));
    }

    #[test]
    fn editing_deleting_or_reordering_ledger_records_is_detected() {
        let conn = open_migrated();
        let dir = tmp("tamper");
        let r0 = init_baseline(&conn, &dir, T0).unwrap();
        for i in 1..=3 {
            live(&conn, i);
        }
        let r1 = enroll(&conn, &dir, T1, Some((1, &r0.ledger_head_sha256))).unwrap();
        assert_eq!(r1.record_count, 4);
        let path = dir.join(LEDGER_FILE);
        let original = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = original.lines().collect();
        let write = |ls: &[&str]| std::fs::write(&path, ls.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        // 改変
        write(&[lines[0], &lines[1].replace("work:", "work:9"), lines[2], lines[3]]);
        assert!(verify_ledger(&dir, None).is_err(), "改変");
        // 中間の削除
        write(&[lines[0], lines[2], lines[3]]);
        assert!(verify_ledger(&dir, None).is_err(), "中間の削除");
        // 並べ替え
        write(&[lines[0], lines[2], lines[1], lines[3]]);
        assert!(verify_ledger(&dir, None).is_err(), "並べ替え");
        // 末尾の削除は chain だけでは通るが、公表済みの (count, head) で検出される
        write(&lines[..3]);
        assert!(verify_ledger(&dir, None).is_ok());
        assert!(verify_ledger(&dir, Some((r1.record_count, &r1.ledger_head_sha256))).is_err(), "末尾の削除");
        // 公表済みの head と合わない差し替え
        write(&lines);
        assert!(verify_ledger(&dir, Some((r1.record_count, &"1".repeat(64)))).is_err());
        assert!(verify_ledger(&dir, Some((r1.record_count, &r1.ledger_head_sha256))).is_ok());
        // payload の改変
        let payload = std::fs::read_dir(dir.join(PAYLOAD_DIR)).unwrap().next().unwrap().unwrap().path();
        std::fs::write(&payload, "{\"x\":1}").unwrap();
        assert!(verify_ledger(&dir, None).is_err(), "payload の改変");
    }

    #[test]
    fn the_tranche_closes_at_the_cap_and_later_inflow_is_not_added() {
        let conn = open_migrated();
        let dir = tmp("cap");
        let r0 = init_baseline(&conn, &dir, T0).unwrap();
        for i in 1..=5 {
            live(&conn, i);
        }
        let r1 = enroll_with_cap(&conn, &dir, T1, Some((1, &r0.ledger_head_sha256)), 3).unwrap();
        assert_eq!((r1.total_enrolled, r1.closed, r1.record_count), (3, true, 5), "id 順に 3 件・close record");
        live(&conn, 6);
        let r2 = enroll_with_cap(&conn, &dir, T2, Some((5, &r1.ledger_head_sha256)), 3).unwrap();
        assert_eq!((r2.enrolled_now, r2.total_enrolled, r2.record_count, r2.ledger_head_sha256), (0, 3, 5, r1.ledger_head_sha256), "閉じた後は追加しない");
        assert!(MILESTONE_MIN == 598 && CLOSE_AT == 946);
    }

    fn run_cfg(conn: &Connection, base: &Path, tag: &str) -> SnapConfig {
        let path = base.join(format!("snapshot_{tag}.db"));
        conn.execute_batch(&format!("VACUUM INTO '{}'", path.to_string_lossy().replace('\'', "''"))).unwrap();
        SnapConfig { snapshot_db: path.clone(), expected_snapshot_sha256: hex_sha(&std::fs::read(&path).unwrap()), output_dir: base.to_path_buf(), source_head: "5be38a1fda7c3c7172945df3811868b479cb4d23".into() }
    }

    #[test]
    fn a_run_never_modifies_the_snapshot_and_the_report_is_counts_only() {
        let conn = open_migrated();
        live(&conn, 1);
        let base = tmp("run");
        let (ledger, cps) = (base.join("ledger"), base.join("checkpoints"));
        let cfg = run_cfg(&conn, &base, "a");
        let sha = cfg.expected_snapshot_sha256.clone();
        let (rp, out) = run(&cfg, "init_baseline", &ledger, &cps, T0).unwrap();
        assert!(assert_report_clean(&out).is_ok() && rp.is_file());
        assert_eq!(hex_sha(&std::fs::read(&cfg.snapshot_db).unwrap()), sha, "snapshot は不変");
        assert_eq!(out["checkpoint_seq"], 1);
        let (_, out2) = run(&cfg, "enroll", &ledger, &cps, T1).unwrap();
        assert_eq!((out2["total_enrolled"].clone(), out2["checkpoint_seq"].clone()), (json!(0), json!(2)));
        let mut bad = out2.clone();
        bad["units"] = json!(["work:1"]);
        assert!(assert_report_clean(&bad).is_err());
        let mut bad_sha = cfg.clone();
        bad_sha.expected_snapshot_sha256 = "0".repeat(64);
        assert!(run(&bad_sha, "init_baseline", &base.join("l2"), &base.join("c2"), T0).is_err());
        assert!(run(&cfg, "init_baseline", &base.join("l3"), &cps, T0).is_err(), "checkpoint があれば baseline は 1 回だけ");
        assert!(run(&cfg, "enroll", &base.join("l4"), &base.join("c4"), T1).is_err(), "checkpoint 無しの enroll は STOP");
        assert!(run(&cfg, "enroll", &ledger, &ledger.join("inner"), T2).is_err(), "checkpoint は ledger の外");
    }

    #[test]
    fn rolling_back_the_ledger_is_caught_by_the_independent_checkpoint_chain() {
        let conn = open_migrated();
        live(&conn, 1);
        let base = tmp("rollback");
        let (ledger, cps) = (base.join("ledger"), base.join("checkpoints"));
        let cfg = run_cfg(&conn, &base, "a");
        run(&cfg, "init_baseline", &ledger, &cps, T0).unwrap();
        live(&conn, 2);
        let cfg2 = run_cfg(&conn, &base, "b");
        let (_, o) = run(&cfg2, "enroll", &ledger, &cps, T1).unwrap();
        assert_eq!(o["total_enrolled"], 1);
        // ledger 末尾（enroll record）を削除。ledger 単体の検証は通るが、checkpoint が record_count 2 を覚えている
        let lp = ledger.join(LEDGER_FILE);
        let text = std::fs::read_to_string(&lp).unwrap();
        std::fs::write(&lp, format!("{}\n", text.lines().next().unwrap())).unwrap();
        assert!(verify_ledger(&ledger, None).is_ok());
        assert!(run(&cfg2, "enroll", &ledger, &cps, T2).is_err(), "ledger の巻き戻し");
        // checkpoint の改変・欠落も検出
        std::fs::write(&lp, text).unwrap();
        let first = cps.join("0001.json");
        let original = std::fs::read_to_string(&first).unwrap();
        std::fs::write(&first, original.replace("\"seq\":1", "\"seq\":1,\"x\":1")).unwrap();
        assert!(latest_checkpoint(&cps).is_err(), "checkpoint の改変");
        std::fs::write(&first, &original).unwrap();
        assert_eq!(latest_checkpoint(&cps).unwrap().unwrap().seq, 2);
        std::fs::remove_file(&first).unwrap();
        assert!(latest_checkpoint(&cps).is_err(), "checkpoint の欠落");
    }

    #[test]
    fn a_missed_run_is_recorded_and_never_backfilled() {
        let conn = open_migrated();
        live(&conn, 1);
        let base = tmp("missed");
        let (ledger, cps) = (base.join("ledger"), base.join("checkpoints"));
        let cfg = run_cfg(&conn, &base, "a");
        run(&cfg, "init_baseline", &ledger, &cps, T0).unwrap();
        let (_, m) = run(&cfg, "missed", &ledger, &cps, T1).unwrap();
        assert_eq!((m["kind"].clone(), m["record_count"].clone(), m["total_enrolled"].clone()), (json!("missed"), json!(2), json!(0)));
        live(&conn, 2);
        let cfg2 = run_cfg(&conn, &base, "b");
        let (_, o) = run(&cfg2, "enroll", &ledger, &cps, T2).unwrap();
        let st = verify_ledger(&ledger, None).unwrap();
        assert_eq!(st.enrolled.len(), 1);
        assert_eq!(st.records[2]["first_seen_at"], T2, "first_seen_at は観測した run の時刻（missed の時刻に遡らない）");
        assert_eq!(o["checkpoint_seq"], 3);
        assert!(run(&cfg2, "missed", &ledger, &cps, T1).is_err(), "時刻の逆戻り");
    }

    #[test]
    fn a_missed_run_after_the_works_sequence_grew_past_the_baseline_still_gets_a_checkpoint() {
        let conn = open_migrated();
        live(&conn, 1);
        let base = tmp("missed_grown");
        let (ledger, cps) = (base.join("ledger"), base.join("checkpoints"));
        let cfg = run_cfg(&conn, &base, "a");
        let (_, o0) = run(&cfg, "init_baseline", &ledger, &cps, T0).unwrap();
        for i in 2..=4 {
            live(&conn, i);
        }
        let cfg2 = run_cfg(&conn, &base, "b");
        let (_, o1) = run(&cfg2, "enroll", &ledger, &cps, T1).unwrap();
        assert!(o1["works_sequence"].as_i64() > o0["works_sequence"].as_i64(), "works_sequence は baseline を超えている");
        let (_, m) = run(&cfg2, "missed", &ledger, &cps, T2).expect("missed は checkpoint まで書けること");
        assert_eq!((m["kind"].clone(), m["checkpoint_seq"].clone(), m["works_sequence"].clone()), (json!("missed"), json!(3), o1["works_sequence"].clone()), "直前 checkpoint の値を持ち越す");
        assert_eq!(latest_checkpoint(&cps).unwrap().unwrap().record_count, verify_ledger(&ledger, None).unwrap().records.len());
    }

    #[test]
    fn works_ids_are_never_reused_and_a_broken_identity_assumption_stops_the_run() {
        // 削除後に作っても id は再利用されない（AUTOINCREMENT）
        let conn = open_migrated();
        let a = live(&conn, 1);
        let dir = tmp("identity");
        let r0 = init_baseline(&conn, &dir, T0).unwrap();
        assert!(r0.works_sequence >= a, "baseline に AUTOINCREMENT の最大値を固定する");
        conn.execute("DELETE FROM work_parts WHERE work_id = ?1", [a]).unwrap();
        conn.execute("DELETE FROM works WHERE id = ?1", [a]).unwrap();
        let fresh = live(&conn, 2);
        assert!(fresh > a && fresh > r0.works_sequence, "削除した id は再利用されない");
        let r = enroll(&conn, &dir, T1, Some((1, &r0.ledger_head_sha256))).unwrap();
        assert_eq!(r.enrolled_now, 1, "T0 後の新規 work は、削除された baseline の id と衝突せず登録される");
        // 前提が崩れた状態（baseline に無く、id が baseline の works_sequence 以下の work）は STOP
        let conn3 = open_migrated();
        live(&conn3, 1);
        conn3.execute("DELETE FROM work_parts", []).unwrap();
        conn3.execute("DELETE FROM works", []).unwrap();
        let dir3 = tmp("identity3");
        let r3 = init_baseline(&conn3, &dir3, T0).unwrap();
        conn3.execute("INSERT INTO works (id, work_type, title, title_guess) VALUES (1, 'movie', 'x', 'x')", []).unwrap();
        assert!(enroll(&conn3, &dir3, T1, Some((1, &r3.ledger_head_sha256))).is_err(), "baseline に無い低い id が現れたら STOP");
        // works_sequence の巻き戻し（最大 id より小さい）
        let conn4 = open_migrated();
        live(&conn4, 1);
        live(&conn4, 2);
        let dir4 = tmp("identity4");
        let r4 = init_baseline(&conn4, &dir4, T0).unwrap();
        conn4.execute("UPDATE sqlite_sequence SET seq = 1 WHERE name = 'works'", []).unwrap();
        assert!(enroll(&conn4, &dir4, T1, Some((1, &r4.ledger_head_sha256))).is_err(), "最大 id より小さい works_sequence は STOP");
    }

    #[test]
    fn the_rules_file_pins_the_enrollment_event_cadence_and_counts() {
        let rules: Value = serde_json::from_str(RULES_JSON).unwrap();
        assert_eq!(rules["h2"]["enrollment_event"], "the first scheduled observation after T0 at which a non-baseline work is observed in the target live population");
        assert_eq!(rules["h2"]["cadence"]["frequency"], "DAILY");
        assert_eq!(rules["h2"]["binding_minimum_eligible"], MILESTONE_MIN);
        assert_eq!(rules["h2"]["operational_target_eligible"], CLOSE_AT);
        assert_eq!(rules_sha256().len(), 64);
    }

    #[test]
    fn the_source_reads_no_label_verdict_candidate_or_gt_tables_and_never_writes_the_database() {
        let src = include_str!("pr4_pf0e.rs");
        let code = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        let body: String = code.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        for banned in ["metadata_match_labels", "metadata_match_verdicts", "metadata_match_candidates", "metadata_match_runs", "metadata_review_tasks", "gt_", "INSERT ", "UPDATE ", "DELETE ", "DROP ", "VACUUM", "reqwest", "TcpStream"] {
            assert!(!body.contains(banned), "{banned}");
        }
        assert!(include_str!("mod.rs").contains("#[cfg(test)]\npub mod pr4_pf0e;"));
    }
}
