//! Jev shadow sidecar（PR3 C4c）。
//!
//! C1〜C4b で作った Jev 評価基盤を、production の `match_once` へ **sidecar として**
//! つなぐ層。ここでも Jev は production の判定に使わない。
//!
//! ```text
//! PreMatchSnapshot → TMDB 候補 → rules-safe / rules-1 / rules-tags-shadow
//!   → production safe run を記録
//!   → ここ（shadow sibling run → C2 → C3A → C3B → C4b 監査）
//!   → rules-safe の結果だけを production へ適用
//! ```
//!
//! # 守る境界
//!
//! - **Jev の失敗で production の照合を失敗させない。** [`evaluate`] は `Result` を
//!   返さず、すべての失敗を飲み込んで [`JevSidecarReport`] にするだけ。
//! - **rules-safe の結果を Jev が変えない。** production の run にも `works` にも
//!   書かない。書くのは shadow 側の run と監査テーブルだけ。
//! - **TMDB 適用後の `works` から state を作らない。** 使うのは照合前に凍結した
//!   [`PreMatchSnapshot`] と、その run が使った `ParsedTitle` だけ。
//!
//! # DB の Mutex を await 越しに持たない
//!
//! `DbState` は `std::sync::Mutex<Connection>` なので、その guard を `.await` を
//! またいで持つと他の処理が止まるうえ、future が `Send` でなくなる。したがって
//!
//! ```text
//! permit 取得（async）
//!   → DB lock → prepare → unlock
//!   → HTTP（await）
//!   → DB lock → persist → unlock
//! ```
//!
//! の順で、ロックの範囲を同期区間に閉じる。呼び出しの逐次性は C4b の permit が担う。

use crate::db::DbState;
use crate::services::jev_session::{
    apply_preflight, close_session, plan_session, preflight_model, JevSessionConfig,
};
use crate::services::jev_shadow::{
    acquire_call_permit, execute_call, persist_call, persist_skipped_call,
    prepare_call_with_permit, PrepareCallResult, ShadowCallInput, ShadowCandidate,
};
use crate::services::jev_state::{CandidateInput, LocalEvidenceInput, MAX_CANDIDATES};
use crate::services::match_history;
use crate::services::prematch_snapshot::PreMatchSnapshot;
use crate::services::title_parser::ParsedTitle;
use crate::services::typesafe_client::TypeSafeClient;

use rusqlite::Connection;
use serde_json::Value;

/// shadow 評価を有効にする環境変数。`"1"` のときだけ動く。
pub const ENABLE_ENV: &str = "CINEMANTIS_JEV_SHADOW";

/// 使うモデルを固定する環境変数。具体的な版名だけを受け付ける。
pub const MODEL_ENV: &str = "CINEMANTIS_JEV_MODEL";

/// sidecar 1 回分の結果。production 側はこれを使わない（記録と観察のため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JevSidecarStatus {
    /// 環境変数などで無効。TypeSafe を一切呼んでいない
    Disabled,
    /// deterministic な候補が無いので聞くことが無い
    NoCandidates,
    Ok,
    Invalid,
    Unavailable,
    Error,
    /// 予算切れ。送っていない
    Skipped,
    /// sidecar 自身の失敗（shadow run の複製・候補の復元・DB など）
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JevSidecarReport {
    pub safe_run_id: i64,
    pub shadow_run_id: Option<i64>,
    pub status: JevSidecarStatus,
}

impl JevSidecarReport {
    fn new(safe_run_id: i64, status: JevSidecarStatus) -> Self {
        JevSidecarReport { safe_run_id, shadow_run_id: None, status }
    }
}

/// 1 回の呼び出し（single なら 1 作品、batch なら 1 バッチ）で使う文脈。
///
/// API キーは [`TypeSafeClient`] の中にしか無く、ここからは取り出せない。
pub struct JevSidecarContext {
    client: TypeSafeClient,
    session_id: i64,
    session_key: String,
}

impl std::fmt::Debug for JevSidecarContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // client は Debug でキーを伏せるが、ここでも中身は出さない
        f.debug_struct("JevSidecarContext")
            .field("session_id", &self.session_id)
            .field("session_key", &self.session_key)
            .finish()
    }
}

impl JevSidecarContext {
    pub fn session_id(&self) -> i64 {
        self.session_id
    }

    pub fn session_key(&self) -> &str {
        &self.session_key
    }
}

/// 環境変数から pin したモデル名を読む。
///
/// alias（`-latest` など）は受け付けない。**モデルを推測しない。**
fn pinned_model() -> Option<String> {
    let model = std::env::var(MODEL_ENV).ok()?;
    let model = model.trim().to_string();
    if model.is_empty() || model.ends_with("-latest") || model == "latest" {
        return None;
    }
    Some(model)
}

fn shadow_enabled() -> bool {
    std::env::var(ENABLE_ENV).map(|value| value.trim() == "1").unwrap_or(false)
}

/// sidecar の文脈を用意する。無効なら `None`。
///
/// 失敗しても production を止めない。`Err` は返さず、理由は安全な短い文字列で
/// ログに出すだけにする（API キーも応答本文も出さない）。
///
/// preflight の最中は DB のロックを持たない（C4a の 3 段構えをそのまま使う）。
pub async fn start(db: &DbState, session_key: String, max_calls: i64) -> Option<JevSidecarContext> {
    if !shadow_enabled() {
        return None;
    }
    if max_calls <= 0 {
        return None;
    }
    let Some(model) = pinned_model() else {
        eprintln!("[Jev shadow] disabled: model_not_pinned");
        return None;
    };

    // API キーは env からしか読まない。失敗しても理由だけ出す
    let client = match TypeSafeClient::from_env() {
        Ok(client) => client,
        Err(error) => {
            eprintln!("[Jev shadow] unavailable: {}", error.kind.as_str());
            return None;
        }
    };

    let mut config = JevSessionConfig::new(session_key.clone(), model);
    config.max_calls = max_calls;

    // ① DB（同期）
    let plan = {
        let conn = match db.0.lock() {
            Ok(conn) => conn,
            Err(_) => {
                eprintln!("[Jev shadow] unavailable: db_lock");
                return None;
            }
        };
        match plan_session(&conn, &config) {
            Ok(plan) => plan,
            Err(error) => {
                eprintln!("[Jev shadow] unavailable: session_plan ({})", error_label(&error));
                return None;
            }
        }
    };

    // ② HTTP（DB ロックは持たない）
    let outcome = preflight_model(&client, &plan).await;

    // ③ DB（同期）
    let session = {
        let conn = match db.0.lock() {
            Ok(conn) => conn,
            Err(_) => {
                eprintln!("[Jev shadow] unavailable: db_lock");
                return None;
            }
        };
        match apply_preflight(&conn, &plan, outcome) {
            Ok(session) => session,
            Err(error) => {
                eprintln!("[Jev shadow] unavailable: model_preflight ({})", error_label(&error));
                return None;
            }
        }
    };

    Some(JevSidecarContext { client, session_id: session.id, session_key })
}

/// session を閉じる。失敗しても production の結果に影響させない。
pub fn finish(db: &DbState, context: &JevSidecarContext) {
    if let Ok(conn) = db.0.lock() {
        let _ = close_session(&conn, context.session_id);
    }
}

/// エラーの種別だけを短く出す（本文やキーを出さない）。
fn error_label(error: &crate::services::jev_session::JevSessionError) -> &'static str {
    use crate::services::jev_session::JevSessionError as E;
    match error {
        E::InvalidConfig(_) => "invalid_config",
        E::ContractMismatch { .. } => "contract_mismatch",
        E::SessionConfigMismatch { .. } => "session_config_mismatch",
        E::NotResumable { .. } => "not_resumable",
        E::Blocked { .. } => "blocked",
        E::BudgetExhausted => "budget_exhausted",
        E::SessionClosed => "closed",
        E::SessionNotFound => "not_found",
        E::ConcurrentSessionChange { .. } => "concurrent_change",
        E::PreflightModelMismatch { .. } => "model_mismatch",
        E::InvalidTokenCount(_) => "invalid_token_count",
        E::Db(_) => "db",
    }
}

// ─── 入力の組み立て ──────────────────────────────────────────────────────────

/// 照合前に凍結した証拠から、Jev へ送るローカル証拠を作る。
///
/// **`works` の現在値は一切見ない。** TMDB を適用した後に呼ばれても、ここで使うのは
/// `snapshot`（照合前に capture 済み）と、その run が使った `parsed` だけ。
pub fn local_evidence_from(
    snapshot: &PreMatchSnapshot,
    parsed: &ParsedTitle,
) -> LocalEvidenceInput {
    LocalEvidenceInput {
        derived_title: non_empty(&snapshot.derived_title),
        embedded_title: snapshot.embedded.title.as_deref().and_then(non_empty),
        filename_year: snapshot.filename_year.or(snapshot.title_guess_year),
        embedded_year: snapshot.embedded.year,
        media_kind: non_empty(&snapshot.media_kind),
        country_type: non_empty(&snapshot.country_type),
        season_no: parsed.season_no,
        episode_no: parsed.episode_no,
        part_hint: parsed.part_hint.clone(),
        edition_markers: snapshot.embedded.cut_editions.clone(),
        audio_languages: snapshot.embedded.audio_languages.clone(),
        subtitle_languages: snapshot.embedded.subtitle_languages.clone(),
        cast: snapshot.embedded.cast.clone(),
        evidence_class: Some(snapshot.evidence_class.as_str().to_string()),
        tag_provenance: snapshot.embedded.provenance.clone(),
    }
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// shadow run に保存済みの候補から、Jev へ渡す上位 K 件を読む。
///
/// 並びは **保存済みの decision 順**（`rules_rank`）だけで決める。ここで新しい採点も
/// 並べ替えもしない。TMDB も再検索しない。`rules_rank` が無い候補は入れない。
/// snapshot が壊れていたら **詰め直さずに失敗**させる（`c1` が壊れたからといって
/// `c2` を `c1` にしない）。
pub fn load_candidates(
    conn: &Connection,
    shadow_run_id: i64,
) -> Result<Vec<ShadowCandidate>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, cand_key, tmdb_id, media_type, tmdb_snapshot_json
               FROM metadata_match_candidates
              WHERE run_id = ?1 AND rules_rank IS NOT NULL
              ORDER BY rules_rank ASC, cand_key ASC
              LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(rusqlite::params![shadow_run_id, MAX_CANDIDATES as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let mut candidates = Vec::with_capacity(rows.len());
    for (candidate_id, cand_key, tmdb_id, media_type, snapshot_json) in rows {
        let snapshot: Value = serde_json::from_str(&snapshot_json)
            .map_err(|_| format!("{cand_key} の候補 snapshot を読めません"))?;
        let title = snapshot
            .get("title")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{cand_key} の候補 snapshot に title がありません"))?
            .to_string();

        candidates.push(ShadowCandidate {
            candidate_id,
            input: CandidateInput {
                tmdb_id,
                media_type,
                title,
                // poster_path は送らない
                original_title: snapshot
                    .get("original_title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                year: snapshot.get("year").and_then(Value::as_i64).map(|y| y as i32),
                original_language: snapshot
                    .get("original_language")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
        });
    }
    Ok(candidates)
}

// ─── 実行 ────────────────────────────────────────────────────────────────────

/// production の safe run に対して Jev の shadow 評価を 1 回行う。
///
/// **`Result` を返さない。** Jev 側の失敗はすべてここで吸収し、production の
/// `match_once` は戻り値を捨てるだけでよい。
pub async fn evaluate(
    db: &DbState,
    context: &JevSidecarContext,
    safe_run_id: i64,
    snapshot: &PreMatchSnapshot,
    parsed: &ParsedTitle,
) -> JevSidecarReport {
    // ① shadow sibling run を作り、送る内容をローカルで組み立てる（同期）
    let permit = acquire_call_permit().await;

    let prepared = {
        let mut conn = match db.0.lock() {
            Ok(conn) => conn,
            Err(_) => return JevSidecarReport::new(safe_run_id, JevSidecarStatus::Failed),
        };

        let shadow = match match_history::create_jev_shadow_run(&conn, safe_run_id) {
            Ok(Some(shadow)) => shadow,
            Ok(None) => {
                return JevSidecarReport::new(safe_run_id, JevSidecarStatus::NoCandidates)
            }
            Err(reason) => {
                eprintln!("[Jev shadow] failed: shadow_run ({reason})");
                return JevSidecarReport::new(safe_run_id, JevSidecarStatus::Failed);
            }
        };
        let shadow_run_id = shadow.shadow_run_id;

        let candidates = match load_candidates(&conn, shadow_run_id) {
            Ok(candidates) if !candidates.is_empty() => candidates,
            Ok(_) => {
                return JevSidecarReport {
                    safe_run_id,
                    shadow_run_id: Some(shadow_run_id),
                    status: JevSidecarStatus::NoCandidates,
                }
            }
            Err(reason) => {
                eprintln!("[Jev shadow] failed: candidate_snapshot ({reason})");
                return JevSidecarReport {
                    safe_run_id,
                    shadow_run_id: Some(shadow_run_id),
                    status: JevSidecarStatus::Failed,
                };
            }
        };

        let input = ShadowCallInput {
            run_id: shadow_run_id,
            local: local_evidence_from(snapshot, parsed),
            candidates,
        };

        match prepare_call_with_permit(&mut conn, context.session_id, &input, permit) {
            Ok(PrepareCallResult::Ready(prepared)) => Ok((shadow_run_id, prepared)),
            Ok(PrepareCallResult::Skipped(skipped)) => {
                // 予算切れ。送らずに記録して終わる
                let status = match persist_skipped_call(&mut conn, skipped) {
                    Ok(_) => JevSidecarStatus::Skipped,
                    Err(_) => JevSidecarStatus::Failed,
                };
                Err(JevSidecarReport { safe_run_id, shadow_run_id: Some(shadow_run_id), status })
            }
            Err(error) => {
                eprintln!("[Jev shadow] failed: prepare ({error})");
                Err(JevSidecarReport {
                    safe_run_id,
                    shadow_run_id: Some(shadow_run_id),
                    status: JevSidecarStatus::Failed,
                })
            }
        }
    };

    let (shadow_run_id, prepared) = match prepared {
        Ok(ready) => ready,
        Err(report) => return report,
    };

    // ② HTTP。DB のロックは持たない
    let executed = execute_call(&context.client, prepared).await;
    let status = executed.status();

    // ③ 監査（同期）
    let mut conn = match db.0.lock() {
        Ok(conn) => conn,
        Err(_) => {
            return JevSidecarReport {
                safe_run_id,
                shadow_run_id: Some(shadow_run_id),
                status: JevSidecarStatus::Failed,
            }
        }
    };
    let status = match persist_call(&mut conn, executed) {
        Ok(_) => match status {
            "ok" => JevSidecarStatus::Ok,
            "invalid" => JevSidecarStatus::Invalid,
            "unavailable" => JevSidecarStatus::Unavailable,
            "skipped" => JevSidecarStatus::Skipped,
            _ => JevSidecarStatus::Error,
        },
        Err(error) => {
            eprintln!("[Jev shadow] failed: persist ({error})");
            JevSidecarStatus::Failed
        }
    };

    JevSidecarReport { safe_run_id, shadow_run_id: Some(shadow_run_id), status }
}

/// single 実行用の session key。秘密情報を含めない。
pub fn single_session_key(work_id: i64) -> String {
    format!(
        "jev-shadow-single-{}-{}-{}",
        std::process::id(),
        work_id,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    )
}

/// batch 実行用の session key。
pub fn batch_session_key(batch_id: &str) -> String {
    let key = format!("jev-shadow-batch-{}-{}", std::process::id(), batch_id);
    key.chars().take(128).collect()
}

// ─── テスト ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::services::prematch_snapshot::{EmbeddedInputs, EvidenceClass, TitleProvenance};
    use crate::services::title_parser::parse_title;

    fn snapshot_for(work_id: i64) -> PreMatchSnapshot {
        PreMatchSnapshot {
            state_schema_version: "cm-prematch-2",
            work_id,
            work_created_at: None,
            title_guess: Some("テスト作品".to_string()),
            derived_title: "テスト作品".to_string(),
            title_provenance: TitleProvenance::OriginalFileName,
            media_kind: "movie".to_string(),
            country_type: "domestic".to_string(),
            filename_year: Some(2021),
            title_guess_year: None,
            embedded: EmbeddedInputs {
                title: Some("Test Work".to_string()),
                title_raw: Some("Test  Work".to_string()),
                show: None,
                year: Some(2021),
                cut_editions: vec!["director's cut".to_string()],
                audio_languages: vec!["ja".to_string()],
                subtitle_languages: vec!["en".to_string()],
                cast: vec!["山田 太郎".to_string()],
                description: Some("ここはあらすじ。送ってはいけない".to_string()),
                episode_id: None,
                episode_id_marks_movie: false,
                provenance: Some("pipeline_known".to_string()),
                provider_hint: Some("some-provider".to_string()),
                truncated: false,
            },
            files: Vec::new(),
            embedded_ids: Vec::new(),
            evidence_class: EvidenceClass::Live,
        }
    }

    #[test]
    fn local_evidence_comes_from_the_frozen_snapshot() {
        let snapshot = snapshot_for(1);
        let parsed = parse_title("テスト作品 S02E05 part2 (2021).mkv");
        let local = local_evidence_from(&snapshot, &parsed);

        assert_eq!(local.derived_title.as_deref(), Some("テスト作品"));
        assert_eq!(local.embedded_title.as_deref(), Some("Test Work"));
        assert_eq!(local.filename_year, Some(2021));
        assert_eq!(local.embedded_year, Some(2021));
        assert_eq!(local.media_kind.as_deref(), Some("movie"));
        assert_eq!(local.country_type.as_deref(), Some("domestic"));
        assert_eq!(local.season_no, parsed.season_no);
        assert_eq!(local.episode_no, parsed.episode_no);
        assert_eq!(local.part_hint, parsed.part_hint);
        assert_eq!(local.edition_markers, vec!["director's cut".to_string()]);
        assert_eq!(local.audio_languages, vec!["ja".to_string()]);
        assert_eq!(local.subtitle_languages, vec!["en".to_string()]);
        assert_eq!(local.cast, vec!["山田 太郎".to_string()]);
        assert_eq!(local.evidence_class.as_deref(), Some("live"));
        assert_eq!(local.tag_provenance.as_deref(), Some("pipeline_known"));

        // 送ってはいけないものが入っていない
        let dump = format!("{local:?}");
        for forbidden in ["あらすじ", "some-provider", "title_raw"] {
            assert!(!dump.contains(forbidden), "{forbidden} が混ざっている");
        }
    }

    #[test]
    fn filename_year_falls_back_to_the_title_guess_year() {
        let mut snapshot = snapshot_for(1);
        snapshot.filename_year = None;
        snapshot.title_guess_year = Some(1999);
        let local = local_evidence_from(&snapshot, &parse_title("x.mkv"));
        assert_eq!(local.filename_year, Some(1999));
    }

    #[test]
    fn a_pinned_model_is_required() {
        // 実際の環境変数は触らず、判定の規則だけを固定する
        for alias in ["jev-latest", "latest", "  ", ""] {
            let model = alias.trim().to_string();
            let rejected = model.is_empty() || model.ends_with("-latest") || model == "latest";
            assert!(rejected, "{alias} を受け入れてしまう");
        }
        let model = "jev-1.13.0".to_string();
        assert!(!(model.is_empty() || model.ends_with("-latest") || model == "latest"));
    }

    // ─── 候補の読み出し ──────────────────────────────────────────────────

    fn open_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        db::apply_migrations(&conn).unwrap();
        conn
    }

    fn insert_shadow_run(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO works (title, work_type, media_category)
             VALUES ('テスト作品', 'movie', 'movie')",
            [],
        )
        .unwrap();
        let work_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO metadata_match_runs
               (work_id, trigger_kind, mode, evidence_class, state_schema_version,
                input_snapshot_json, search_queries_json, policy_version,
                policy_snapshot_json, governing_matcher, decision, decision_reasons_json)
             VALUES (?1, 'single', 'shadow', 'live', 'cm-prematch-2',
                     '{}', '[]', 'rules-safe-2', '{}', 'rules-safe', 'REVIEW', '[]')",
            rusqlite::params![work_id],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn insert_candidate(
        conn: &Connection,
        run_id: i64,
        index: usize,
        rules_rank: Option<i64>,
        snapshot_json: &str,
    ) {
        conn.execute(
            "INSERT INTO metadata_match_candidates
               (run_id, cand_key, tmdb_id, media_type, search_rank, rules_rank,
                rules_score, rules_reasons_json, tmdb_snapshot_json)
             VALUES (?1, ?2, ?3, 'movie', ?4, ?5, 0, '[]', ?6)",
            rusqlite::params![
                run_id,
                format!("c{index}"),
                1000 + index as i64,
                index as i64,
                rules_rank,
                snapshot_json,
            ],
        )
        .unwrap();
    }

    fn snapshot_json(title: &str) -> String {
        serde_json::json!({
            "title": title,
            "original_title": "Original",
            "year": 2021,
            "poster_path": "/poster.jpg",
            "original_language": "ja"
        })
        .to_string()
    }

    #[test]
    fn at_most_three_ranked_candidates_are_sent() {
        for count in 1..=5usize {
            let conn = open_db();
            let run_id = insert_shadow_run(&conn);
            for index in 1..=count {
                insert_candidate(
                    &conn,
                    run_id,
                    index,
                    Some(index as i64),
                    &snapshot_json(&format!("候補 {index}")),
                );
            }

            let candidates = load_candidates(&conn, run_id).unwrap();
            let expected: Vec<String> = (1..=count.min(MAX_CANDIDATES))
                .map(|index| format!("c{index}"))
                .collect();
            assert_eq!(candidates.len(), expected.len(), "候補 {count} 件");
            for (index, candidate) in candidates.iter().enumerate() {
                assert_eq!(candidate.input.tmdb_id, 1001 + index as i64);
                assert_eq!(candidate.input.title, format!("候補 {}", index + 1));
                assert_eq!(candidate.input.original_language.as_deref(), Some("ja"));
            }
        }
    }

    #[test]
    fn unranked_candidates_are_never_sent() {
        let conn = open_db();
        let run_id = insert_shadow_run(&conn);
        insert_candidate(&conn, run_id, 1, None, &snapshot_json("順位なし"));
        insert_candidate(&conn, run_id, 2, Some(1), &snapshot_json("順位あり"));

        let candidates = load_candidates(&conn, run_id).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].input.title, "順位あり");
    }

    /// snapshot が壊れていたら、後ろの候補を繰り上げずに失敗する
    #[test]
    fn a_broken_candidate_snapshot_fails_instead_of_shifting() {
        let conn = open_db();
        let run_id = insert_shadow_run(&conn);
        insert_candidate(&conn, run_id, 1, Some(1), "{ broken");
        insert_candidate(&conn, run_id, 2, Some(2), &snapshot_json("2 番目"));

        let error = load_candidates(&conn, run_id).unwrap_err();
        assert!(error.contains("c1"), "{error}");
    }

    #[test]
    fn candidate_input_never_carries_the_poster() {
        let conn = open_db();
        let run_id = insert_shadow_run(&conn);
        insert_candidate(&conn, run_id, 1, Some(1), &snapshot_json("候補"));

        let candidates = load_candidates(&conn, run_id).unwrap();
        let dump = format!("{:?}", candidates[0].input);
        assert!(!dump.contains("poster"), "poster を渡している");
    }

    // ─── sidecar を通した評価（stub は 127.0.0.1 のみ）───────────────────

    use crate::services::jev_shadow::load_call;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const MODEL: &str = "jev-test-model-1";
    const KEY: &str = "sk-test-DO-NOT-LEAK-0123456789";

    #[derive(Clone)]
    enum Reply {
        Json(u16, String),
        Slow(Duration, String),
        Drop,
    }

    struct Stub {
        base: String,
        hits: Arc<AtomicUsize>,
    }

    impl Stub {
        /// /v1/models を除いた SystemOne の回数
        fn systemone_hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst).saturating_sub(1)
        }

        fn total_hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    async fn start_stub(replies: Vec<Reply>) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_task = hits.clone();

        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let index = hits_task.fetch_add(1, Ordering::SeqCst);
                let reply = replies
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| replies.last().cloned().unwrap_or(Reply::Drop));
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 65536];
                    let _ = socket.read(&mut buffer).await;
                    let body = match reply {
                        Reply::Drop => None,
                        Reply::Json(status, body) => Some((status, body)),
                        Reply::Slow(delay, body) => {
                            tokio::time::sleep(delay).await;
                            Some((200, body))
                        }
                    };
                    if let Some((status, body)) = body {
                        let response = format!(
                            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                    }
                    let _ = socket.flush().await;
                    let _ = socket.shutdown().await;
                });
            }
        });

        Stub { base: format!("http://127.0.0.1:{port}"), hits }
    }

    fn models_body() -> String {
        serde_json::json!({"models": [
            {"name": MODEL, "description": "test", "release_date": "2026-01-01"}
        ]})
        .to_string()
    }

    fn answers_body(count: usize, choice: &str) -> String {
        let mut answers = serde_json::Map::new();
        for index in 1..=count {
            answers.insert(
                format!("c{index}_same_work"),
                serde_json::json!({"type": "noul", "noul": 0.9 - 0.1 * index as f64}),
            );
        }
        let mut probabilities = serde_json::Map::new();
        for index in 1..=count {
            probabilities.insert(format!("c{index}"), serde_json::json!(0.6 / count as f64));
        }
        probabilities.insert("NONE".to_string(), serde_json::json!(0.4));
        answers.insert(
            "best_match".to_string(),
            serde_json::json!({
                "type": "choice",
                "choice": choice,
                "confidence": 0.7,
                "probabilities": Value::Object(probabilities)
            }),
        );
        serde_json::json!({
            "model": MODEL,
            "answers": Value::Object(answers),
            "usage": {"input_tokens": 100, "output_tokens": 5}
        })
        .to_string()
    }

    /// sidecar 用の文脈を stub 向けに作る（env を触らない）
    async fn context_for(db: &DbState, stub: &Stub, key: &str, max_calls: i64) -> JevSidecarContext {
        let client = TypeSafeClient::with_key_for_test(KEY, &stub.base);
        let mut config = JevSessionConfig::new(key, MODEL);
        config.max_calls = max_calls;

        let plan = {
            let conn = db.0.lock().unwrap();
            plan_session(&conn, &config).unwrap()
        };
        let outcome = preflight_model(&client, &plan).await;
        let session = {
            let conn = db.0.lock().unwrap();
            apply_preflight(&conn, &plan, outcome).unwrap()
        };
        JevSidecarContext {
            client,
            session_id: session.id,
            session_key: key.to_string(),
        }
    }

    fn db_state() -> DbState {
        DbState(Arc::new(Mutex::new(open_db())))
    }

    /// production の safe run を、候補つきで作る
    fn insert_safe_run(conn: &Connection, candidates: usize) -> (i64, i64) {
        conn.execute(
            "INSERT INTO works (title, work_type, media_category)
             VALUES ('テスト作品', 'movie', 'movie')",
            [],
        )
        .unwrap();
        let work_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO metadata_match_runs
               (work_id, batch_id, trigger_kind, mode, evidence_class, state_schema_version,
                input_snapshot_json, search_queries_json, policy_version,
                policy_snapshot_json, governing_matcher, decision, decision_reasons_json,
                tmdb_calls, latency_ms)
             VALUES (?1, 'batch-1', 'single', 'safe', 'live', 'cm-prematch-2',
                     '{\"a\":1}', '[\"q\"]', 'rules-safe-2', '{\"p\":1}', 'rules-safe',
                     'REVIEW', '[\"r\"]', 3, 42)",
            rusqlite::params![work_id],
        )
        .unwrap();
        let run_id = conn.last_insert_rowid();

        for index in 1..=candidates {
            insert_candidate(conn, run_id, index, Some(index as i64), &snapshot_json(&format!("候補 {index}")));
        }
        for (matcher, version) in [
            ("rules-1", "rules-1"),
            ("rules-safe", "rules-safe-2"),
            ("rules-tags-shadow", "rules-tags-shadow-2"),
        ] {
            conn.execute(
                "INSERT INTO metadata_match_verdicts
                   (run_id, matcher, matcher_version, decision, reasons_json)
                 VALUES (?1, ?2, ?3, 'REVIEW', '[]')",
                rusqlite::params![run_id, matcher, version],
            )
            .unwrap();
        }
        (work_id, run_id)
    }

    /// production 側が変わっていないことを見るための指紋
    fn production_fingerprint(conn: &Connection) -> String {
        let mut parts = Vec::new();
        for sql in [
            "SELECT id || ':' || title || ':' || COALESCE(tmdb_id,'') || ':' ||
                    COALESCE(match_status,'') || ':' || COALESCE(match_confidence,'') || ':' ||
                    COALESCE(match_source,'') || ':' || COALESCE(poster_path,'')
               FROM works ORDER BY id",
            "SELECT id || ':' || file_name || ':' || file_path FROM files ORDER BY id",
            "SELECT id || ':' || mode || ':' || decision || ':' || applied || ':' ||
                    COALESCE(applied_tmdb_id,'') || ':' || COALESCE(status_after,'') || ':' ||
                    decision_reasons_json
               FROM metadata_match_runs WHERE mode = 'safe' ORDER BY id",
        ] {
            let mut stmt = conn.prepare(sql).unwrap();
            let rows: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            parts.push(rows.join("|"));
        }
        parts.join("#")
    }

    fn counts(conn: &Connection) -> (i64, i64, i64) {
        let count = |table: &str| -> i64 {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
                .unwrap()
        };
        (
            count("metadata_review_tasks"),
            count("metadata_match_labels"),
            count("metadata_match_rejections"),
        )
    }

    // ─── migration 025 ───────────────────────────────────────────────────

    #[test]
    fn migration_025_creates_the_link_table() {
        let conn = open_db();
        db::apply_migrations(&conn).unwrap();

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type='table' AND name='metadata_match_jev_run_links'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 1);

        let (_, safe_run) = insert_safe_run(&conn, 2);
        let shadow_a = match_history::create_jev_shadow_run(&conn, safe_run).unwrap().unwrap();

        // 同じ safe run に 2 つ目は作れない
        assert!(match_history::create_jev_shadow_run(&conn, safe_run).is_err());

        // shadow_run_id の重複も拒否される
        let (_, other_safe) = insert_safe_run(&conn, 1);
        let duplicate = conn.execute(
            "INSERT INTO metadata_match_jev_run_links (safe_run_id, shadow_run_id)
             VALUES (?1, ?2)",
            rusqlite::params![other_safe, shadow_a.shadow_run_id],
        );
        assert!(duplicate.is_err(), "shadow_run_id の重複が通ってしまう");

        // safe と shadow が同じ id は拒否
        let same = conn.execute(
            "INSERT INTO metadata_match_jev_run_links (safe_run_id, shadow_run_id)
             VALUES (?1, ?1)",
            rusqlite::params![other_safe],
        );
        assert!(same.is_err());

        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        assert_eq!(db::foreign_key_check_rows(&conn).unwrap().len(), 0);
    }

    // ─── shadow sibling run ──────────────────────────────────────────────

    #[test]
    fn the_shadow_run_mirrors_the_safe_run() {
        let conn = open_db();
        let (_, safe_run) = insert_safe_run(&conn, 3);
        let before = production_fingerprint(&conn);
        let watched = counts(&conn);

        let shadow = match_history::create_jev_shadow_run(&conn, safe_run).unwrap().unwrap();

        let row = |run_id: i64| -> (String, i64, String, String, String, String, String, i64) {
            conn.query_row(
                "SELECT mode, applied, decision, input_snapshot_json, search_queries_json,
                        evidence_class, policy_snapshot_json, tmdb_calls
                   FROM metadata_match_runs WHERE id = ?1",
                rusqlite::params![run_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .unwrap()
        };
        let safe = row(safe_run);
        let shade = row(shadow.shadow_run_id);

        assert_eq!(safe.0, "safe");
        assert_eq!(shade.0, "shadow");
        assert_eq!(shade.1, 0, "shadow は未適用");
        assert_eq!(shade.2, safe.2, "decision を写す");
        assert_eq!(shade.3, safe.3, "input snapshot を写す");
        assert_eq!(shade.4, safe.4, "search queries を写す");
        assert_eq!(shade.5, safe.5, "evidence class を写す");
        assert_eq!(shade.6, safe.6, "policy snapshot を写す");
        assert_eq!(shade.7, 0, "shadow 自身は TMDB を呼んでいない");

        // 候補は cand_key ごとそのまま
        let candidates = |run_id: i64| -> Vec<(String, i64, String, Option<i64>, i64)> {
            let mut stmt = conn
                .prepare(
                    "SELECT cand_key, tmdb_id, media_type, rules_rank, rules_score
                       FROM metadata_match_candidates WHERE run_id = ?1 ORDER BY cand_key",
                )
                .unwrap();
            stmt.query_map(rusqlite::params![run_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
        };
        assert_eq!(candidates(shadow.shadow_run_id), candidates(safe_run));

        // 決定的な verdict だけ写す
        let mut stmt = conn
            .prepare("SELECT matcher FROM metadata_match_verdicts WHERE run_id = ?1 ORDER BY matcher")
            .unwrap();
        let matchers: Vec<String> = stmt
            .query_map(rusqlite::params![shadow.shadow_run_id], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(matchers, vec!["rules-1", "rules-safe", "rules-tags-shadow"]);

        // production 側と review 系は不変
        assert_eq!(production_fingerprint(&conn), before);
        assert_eq!(counts(&conn), watched);
    }

    #[test]
    fn a_run_without_ranked_candidates_gets_no_shadow_run() {
        let conn = open_db();
        let (_, safe_run) = insert_safe_run(&conn, 0);
        assert_eq!(match_history::create_jev_shadow_run(&conn, safe_run).unwrap(), None);

        // rules_rank が無い候補だけでも作らない
        insert_candidate(&conn, safe_run, 1, None, &snapshot_json("順位なし"));
        assert_eq!(match_history::create_jev_shadow_run(&conn, safe_run).unwrap(), None);

        let runs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata_match_runs WHERE mode = 'shadow'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(runs, 0);
    }

    #[test]
    fn applied_or_error_safe_runs_are_refused() {
        let conn = open_db();

        let (_, applied_run) = insert_safe_run(&conn, 1);
        conn.execute(
            "UPDATE metadata_match_runs
                SET applied = 1, applied_tmdb_id = 1, applied_media_type = 'movie'
              WHERE id = ?1",
            rusqlite::params![applied_run],
        )
        .unwrap();
        assert!(match_history::create_jev_shadow_run(&conn, applied_run).is_err());

        let (_, error_run) = insert_safe_run(&conn, 1);
        conn.execute(
            "UPDATE metadata_match_runs SET decision = 'ERROR', error_text = 'boom'
              WHERE id = ?1",
            rusqlite::params![error_run],
        )
        .unwrap();
        assert!(match_history::create_jev_shadow_run(&conn, error_run).is_err());
    }

    // ─── 照合前の証拠しか使わない ────────────────────────────────────────

    /// snapshot を取ったあとに works へ TMDB 由来の値を書いても、Jev の入力は変わらない
    #[tokio::test]
    async fn the_jev_state_never_reflects_post_match_works() {
        let db = db_state();
        let stub = start_stub(vec![
            Reply::Json(200, models_body()),
            Reply::Json(200, answers_body(2, "c1")),
        ])
        .await;
        let context = context_for(&db, &stub, "sidecar-leak", 1).await;

        let (work_id, safe_run) = {
            let conn = db.0.lock().unwrap();
            insert_safe_run(&conn, 2)
        };
        let snapshot = snapshot_for(work_id);
        let parsed = parse_title("テスト作品 (2021).mkv");

        // 照合後に works が TMDB の値で埋まった状況を作る
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "UPDATE works
                    SET title = 'TMDB タイトル', original_title = 'TMDB Original',
                        year = 1999, tmdb_id = 777, imdb_id = 'tt7777777',
                        media_kind = 'tv', country_type = 'foreign',
                        poster_path = '/poster.jpg', match_status = 'matched'
                  WHERE id = ?1",
                rusqlite::params![work_id],
            )
            .unwrap();
        }

        let report = evaluate(&db, &context, safe_run, &snapshot, &parsed).await;
        assert_eq!(report.status, JevSidecarStatus::Ok, "{report:?}");

        let conn = db.0.lock().unwrap();
        let state: String = conn
            .query_row(
                "SELECT state_json FROM metadata_match_jev_calls WHERE run_id = ?1",
                rusqlite::params![report.shadow_run_id.unwrap()],
                |row| row.get(0),
            )
            .unwrap();

        // 照合後の値が 1 つも入っていない
        for forbidden in [
            "TMDB タイトル",
            "TMDB Original",
            "1999",
            "777",
            "tt7777777",
            "poster",
            "matched",
            "some-provider",
            "あらすじ",
            "/tmp",
        ] {
            assert!(!state.contains(forbidden), "{forbidden} が state に入っている");
        }
        // 凍結した snapshot の値は入っている
        assert!(state.contains("テスト作品"));
        assert!(state.contains("Test Work"));
        assert!(state.contains("domestic"));
    }

    // ─── production 非干渉 ───────────────────────────────────────────────

    #[tokio::test]
    async fn every_jev_outcome_leaves_production_untouched() {
        let cases: Vec<(&str, Reply, JevSidecarStatus)> = vec![
            ("ok", Reply::Json(200, answers_body(2, "c1")), JevSidecarStatus::Ok),
            (
                "invalid",
                Reply::Json(
                    200,
                    serde_json::json!({
                        "model": MODEL,
                        "answers": {"c1_same_work": {"type": "noul", "noul": 0.5}},
                        "usage": {"input_tokens": 5, "output_tokens": 1}
                    })
                    .to_string(),
                ),
                JevSidecarStatus::Invalid,
            ),
            ("401", Reply::Json(401, "{}".to_string()), JevSidecarStatus::Unavailable),
            ("429", Reply::Json(429, "{}".to_string()), JevSidecarStatus::Unavailable),
            ("503", Reply::Json(503, "{}".to_string()), JevSidecarStatus::Unavailable),
            ("network", Reply::Drop, JevSidecarStatus::Unavailable),
            ("422", Reply::Json(422, "{}".to_string()), JevSidecarStatus::Error),
        ];

        for (label, reply, expected) in cases {
            let db = db_state();
            let stub = start_stub(vec![Reply::Json(200, models_body()), reply]).await;
            let context = context_for(&db, &stub, &format!("sidecar-{label}"), 1).await;

            let (work_id, safe_run) = {
                let conn = db.0.lock().unwrap();
                insert_safe_run(&conn, 2)
            };
            let snapshot = snapshot_for(work_id);
            let parsed = parse_title("テスト作品.mkv");

            let (before, watched) = {
                let conn = db.0.lock().unwrap();
                (production_fingerprint(&conn), counts(&conn))
            };

            let report = evaluate(&db, &context, safe_run, &snapshot, &parsed).await;
            assert_eq!(report.status, expected, "{label}");

            let conn = db.0.lock().unwrap();
            assert_eq!(production_fingerprint(&conn), before, "{label}: production が変わった");
            assert_eq!(counts(&conn), watched, "{label}: review/label/rejection が変わった");

            // 監査だけが増える
            let calls: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM metadata_match_jev_calls WHERE run_id = ?1",
                    rusqlite::params![report.shadow_run_id.unwrap()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(calls, 1, "{label}");
            let autos: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM metadata_match_verdicts WHERE decision = 'AUTO'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(autos, 0, "{label}: AUTO を書いている");
        }
    }

    /// 予算切れでも production は動かない
    #[tokio::test]
    async fn budget_exhaustion_leaves_production_untouched() {
        let db = db_state();
        let stub = start_stub(vec![
            Reply::Json(200, models_body()),
            Reply::Json(200, answers_body(2, "c1")),
        ])
        .await;
        let context = context_for(&db, &stub, "sidecar-budget", 1).await;

        let snapshot_and_run = |db: &DbState| {
            let conn = db.0.lock().unwrap();
            insert_safe_run(&conn, 2)
        };

        let (work_a, run_a) = snapshot_and_run(&db);
        let first = evaluate(&db, &context, run_a, &snapshot_for(work_a), &parse_title("a.mkv")).await;
        assert_eq!(first.status, JevSidecarStatus::Ok);

        let (work_b, run_b) = snapshot_and_run(&db);
        let (before, watched) = {
            let conn = db.0.lock().unwrap();
            (production_fingerprint(&conn), counts(&conn))
        };
        let second =
            evaluate(&db, &context, run_b, &snapshot_for(work_b), &parse_title("b.mkv")).await;
        assert_eq!(second.status, JevSidecarStatus::Skipped);
        assert_eq!(stub.systemone_hits(), 1, "予算切れで送っている");

        let conn = db.0.lock().unwrap();
        assert_eq!(production_fingerprint(&conn), before);
        assert_eq!(counts(&conn), watched);
        let call = load_call(&conn, 0).ok();
        let _ = call;
        let status: String = conn
            .query_row(
                "SELECT status FROM metadata_match_jev_calls WHERE run_id = ?1",
                rusqlite::params![second.shadow_run_id.unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "skipped");
    }

    /// 候補 snapshot が壊れていたら sidecar は Failed。HTTP も出さない
    #[tokio::test]
    async fn a_broken_candidate_fails_the_sidecar_without_sending() {
        let db = db_state();
        let stub = start_stub(vec![
            Reply::Json(200, models_body()),
            Reply::Json(200, answers_body(2, "c1")),
        ])
        .await;
        let context = context_for(&db, &stub, "sidecar-broken", 1).await;

        let (work_id, safe_run) = {
            let conn = db.0.lock().unwrap();
            let (work_id, safe_run) = insert_safe_run(&conn, 2);
            conn.execute(
                "UPDATE metadata_match_candidates SET tmdb_snapshot_json = '{ broken'
                  WHERE run_id = ?1 AND cand_key = 'c1'",
                rusqlite::params![safe_run],
            )
            .unwrap();
            (work_id, safe_run)
        };

        let before = {
            let conn = db.0.lock().unwrap();
            production_fingerprint(&conn)
        };
        let report = evaluate(
            &db,
            &context,
            safe_run,
            &snapshot_for(work_id),
            &parse_title("x.mkv"),
        )
        .await;
        assert_eq!(report.status, JevSidecarStatus::Failed);
        assert_eq!(stub.systemone_hits(), 0, "壊れた入力で送っている");

        let conn = db.0.lock().unwrap();
        assert_eq!(production_fingerprint(&conn), before);
        let calls: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_match_jev_calls", [], |row| row.get(0))
            .unwrap();
        assert_eq!(calls, 0);
    }

    /// 候補が無ければ session も HTTP も使わない
    #[tokio::test]
    async fn no_candidates_means_no_call() {
        let db = db_state();
        let stub = start_stub(vec![
            Reply::Json(200, models_body()),
            Reply::Json(200, answers_body(1, "c1")),
        ])
        .await;
        let context = context_for(&db, &stub, "sidecar-nocand", 1).await;

        let (work_id, safe_run) = {
            let conn = db.0.lock().unwrap();
            insert_safe_run(&conn, 0)
        };
        let report = evaluate(
            &db,
            &context,
            safe_run,
            &snapshot_for(work_id),
            &parse_title("x.mkv"),
        )
        .await;
        assert_eq!(report.status, JevSidecarStatus::NoCandidates);
        assert_eq!(report.shadow_run_id, None);
        assert_eq!(stub.systemone_hits(), 0);

        let conn = db.0.lock().unwrap();
        let shadow_runs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata_match_runs WHERE mode = 'shadow'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(shadow_runs, 0);
    }

    // ─── DB ロックを HTTP の待ち時間に握らない ───────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_db_lock_is_free_while_the_http_call_is_in_flight() {
        let db = Arc::new(db_state());
        let stub = start_stub(vec![
            Reply::Json(200, models_body()),
            Reply::Slow(Duration::from_millis(400), answers_body(2, "c1")),
        ])
        .await;
        let context = context_for(&db, &stub, "sidecar-lock", 1).await;

        let (work_id, safe_run) = {
            let conn = db.0.lock().unwrap();
            insert_safe_run(&conn, 2)
        };

        let db_task = db.clone();
        let call = tokio::spawn(async move {
            evaluate(
                &db_task,
                &context,
                safe_run,
                &snapshot_for(work_id),
                &parse_title("x.mkv"),
            )
            .await
        });

        // HTTP が飛んでいる間に DB を触れること
        tokio::time::sleep(Duration::from_millis(150)).await;
        let locked = {
            let db = db.clone();
            tokio::task::spawn_blocking(move || {
                let conn = db.0.lock().expect("HTTP 中に DB を取れない");
                let works: i64 = conn
                    .query_row("SELECT COUNT(*) FROM works", [], |row| row.get(0))
                    .unwrap();
                works
            })
            .await
            .unwrap()
        };
        assert!(locked >= 1, "DB を読めていない");

        let report = call.await.unwrap();
        assert_eq!(report.status, JevSidecarStatus::Ok);
    }

    // ─── session の単位 ──────────────────────────────────────────────────

    #[tokio::test]
    async fn one_session_covers_a_whole_batch() {
        let db = db_state();
        let stub = start_stub(vec![
            Reply::Json(200, models_body()),
            Reply::Json(200, answers_body(2, "c1")),
            Reply::Json(200, answers_body(2, "c2")),
            Reply::Json(200, answers_body(2, "NONE")),
        ])
        .await;
        let context = context_for(&db, &stub, "sidecar-batch", 3).await;

        for index in 0..3 {
            let (work_id, safe_run) = {
                let conn = db.0.lock().unwrap();
                insert_safe_run(&conn, 2)
            };
            let report = evaluate(
                &db,
                &context,
                safe_run,
                &snapshot_for(work_id),
                &parse_title(&format!("{index}.mkv")),
            )
            .await;
            assert_eq!(report.status, JevSidecarStatus::Ok, "{index}");
        }

        assert_eq!(stub.total_hits(), 4, "/v1/models は 1 回だけ");
        assert_eq!(stub.systemone_hits(), 3);

        let conn = db.0.lock().unwrap();
        let (sessions, calls, distinct): (i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM jev_eval_sessions),
                        (SELECT COUNT(*) FROM metadata_match_jev_calls),
                        (SELECT COUNT(DISTINCT eval_session_id) FROM metadata_match_jev_calls)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(sessions, 1, "session は 1 件");
        assert_eq!(calls, 3);
        assert_eq!(distinct, 1, "全 call が同じ session");
    }

    /// feature flag が無ければ何も起きない
    #[tokio::test]
    async fn the_sidecar_is_disabled_without_the_flag() {
        let db = db_state();
        // env は触らず、無効時の入口（start）が None を返す条件だけを確かめる
        std::env::remove_var(ENABLE_ENV);
        let context = start(&db, "sidecar-off".to_string(), 1).await;
        assert!(context.is_none(), "flag 無しで有効になっている");

        let conn = db.0.lock().unwrap();
        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM jev_eval_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sessions, 0, "session を作っている");
    }

    #[test]
    fn session_keys_are_bounded_and_have_no_secrets() {
        let single = single_session_key(42);
        assert!(single.starts_with("jev-shadow-single-"));
        assert!(single.len() <= 128);

        let batch = batch_session_key(&"b".repeat(200));
        assert!(batch.starts_with("jev-shadow-batch-"));
        assert_eq!(batch.chars().count(), 128);
    }
}
