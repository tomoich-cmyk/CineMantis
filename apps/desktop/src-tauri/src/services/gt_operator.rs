//! ground truth 収集の手動ランナー（PR3 C5c.4A）。
//!
//! **製品には出ない。** このモジュールは `#[cfg(test)]` でしかコンパイルされず、
//! Tauri command も UI も増やさない。中の手順はすべて `#[ignore]` 付きなので、
//! 名前を明示して `cargo test -- --ignored` したときだけ動く。
//!
//! ```text
//! 手順 1  manual_preflight_report   CM_GT_ACTION=preflight   読むだけ（query_only）
//! 手順 2  manual_freeze_sample      CM_GT_ACTION=freeze      抽出だけ（TMDB を呼ばない）
//! 手順 3  manual_generate_runs      CM_GT_ACTION=generate    候補検索 + run 記録（実 TMDB）
//! 手順 4  manual_sample_report      CM_GT_ACTION=report      読むだけ（進捗の確認）
//! ```
//!
//! `#[ignore]` だけでは足りない。`cargo test -- --ignored` は無視されている
//! テストを**まとめて**走らせるので、名前を指定し忘れると抽出や生成まで
//! 動いてしまう。そこで各手順は最初に `CM_GT_ACTION` が自分の名前かを確かめ、
//! 違えばその場で止まる。まとめ実行では 1 手順しか進まない。
//!
//! # 固定していること
//!
//! - cohort は `reconstructed_clean`、purpose は `development` の **ハードコード**。
//!   `live` を指定する入口が無いので、holdout の 28 件を誤って開けない。
//! - Jev / TypeSafe は呼ばない（`jev_sidecar` にも触れない）。
//! - production の割り当て（`works` / `match_status` / poster / credits / series）は
//!   書かない。書くのは `audit_sample` 課題と safe run とその候補・判定だけ。
//! - TMDB API キーは DB から読むが、**値も長さも出力しない**。
//!
//! # 入力
//!
//! 引数は環境変数で渡す。既定値は用意しない（うっかり動かさないため）。
//!
//! | 変数 | 意味 |
//! |---|---|
//! | `CM_GT_ACTION` | これから行う手順の名前。**指定した 1 手順しか動かない** |
//! | `CM_GT_PROFILE` | 作業の規模。`pilot`（1〜10 件）か `expansion`（ちょうど 50 件） |
//! | `CM_GT_DB` | 対象 DB のフルパス。コピーか production かは呼ぶ人が決める |
//! | `CM_GT_SAMPLE_ID` | 抽出の識別子。run の `batch_id` と seed を兼ねる |
//! | `CM_GT_TARGET_COUNT` | 何件抽出するか（手順 2 のみ）。profile が範囲を決める |
//! | `CM_GT_MAX_WORKS` | 1 回の生成で処理する上限（手順 3 のみ） |
//! | `CM_GT_MAX_HTTP` | 送ってよい HTTP の本数の上限（手順 3 のみ） |
//!
//! `pilot` は 1〜10 件 / 1〜90 attempts の範囲で受ける。`expansion` は
//! 50 件 / 50 works / 450 attempts の **ちょうどその値**だけを受ける。
//! 抽出の件数は凍結されて後から足せないので、拡張側は範囲にしない。
//! | `CM_GT_SOURCE_SHA` | 抽出時のコードの commit SHA（40 桁 hex、手順 2 のみ） |

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension};

use crate::services::audit_run::{
    self, GenerationBudget, TmdbCandidateProvider, STATE_GENERATION_ERROR, STATE_READY,
    STATE_STALE,
};
use crate::services::gt_sampling::{self, SamplePurpose, SampleRequest};
use crate::services::match_history::POLICY_VERSION;
use crate::services::prematch_snapshot::STATE_SCHEMA_VERSION;
use crate::services::tmdb_client::TmdbClient;

/// **ここは動かせない。** live を development に回さないための固定値
const COHORT: &str = "reconstructed_clean";
const PURPOSE: SamplePurpose = SamplePurpose::Development;

/// C5c.4 の pilot で許す上限。
///
/// 初回の実運用なので、既定の生成予算（600 attempts）はここでは使わない。
/// 10 件 × 最悪 9 attempts = 90 を天井にして、数字を打ち間違えても
/// 大きく踏み外さないようにする。
const MAX_PILOT_WORKS: usize = 10;
const MAX_PILOT_HTTP: usize = 90;

/// C5c.5 の development 拡張。**件数を固定する。**
///
/// 50 件 × 最悪 9 attempts = 450。上限ではなく「ちょうどこの値」を求めるのは、
/// 打ち間違いで 5 件だけ凍結された manifest ができると、その sample が
/// 「50 件のつもりだった 5 件」として残り、後から件数を足せないため。
/// 抽出の件数は凍結されるので、ここだけは緩い範囲にしない。
const EXPANSION_WORKS: usize = 50;
const EXPANSION_HTTP: usize = 450;

/// どの規模で作業しているか。**既定値は無い。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Profile {
    /// C5c.4 の 10 件 pilot（上限つきの範囲）
    Pilot,
    /// C5c.5 の 50 件 development 拡張（固定値）
    Expansion,
}

impl Profile {
    fn as_str(self) -> &'static str {
        match self {
            Profile::Pilot => "pilot",
            Profile::Expansion => "expansion",
        }
    }

    /// 抽出する件数。pilot は 1..=10、expansion はちょうど 50
    fn target_count(self) -> usize {
        match self {
            Profile::Pilot => required_bounded("CM_GT_TARGET_COUNT", MAX_PILOT_WORKS),
            Profile::Expansion => required_exact("CM_GT_TARGET_COUNT", EXPANSION_WORKS),
        }
    }

    /// 生成の予算
    fn budget(self) -> GenerationBudget {
        match self {
            Profile::Pilot => GenerationBudget {
                max_works: required_bounded("CM_GT_MAX_WORKS", MAX_PILOT_WORKS),
                max_outbound_http_attempts: required_bounded("CM_GT_MAX_HTTP", MAX_PILOT_HTTP),
            },
            Profile::Expansion => GenerationBudget {
                max_works: required_exact("CM_GT_MAX_WORKS", EXPANSION_WORKS),
                max_outbound_http_attempts: required_exact("CM_GT_MAX_HTTP", EXPANSION_HTTP),
            },
        }
    }

    /// 凍結済みの sample が、いま選んでいる規模のものか
    fn check_manifest(self, manifest: &crate::services::gt_sampling::SampleManifest) {
        let frozen = manifest.config.target_count;
        match self {
            Profile::Expansion => assert_eq!(
                frozen, EXPANSION_WORKS as i64,
                "sample {} は {frozen} 件です。expansion で扱えるのは {EXPANSION_WORKS} 件の sample だけです",
                manifest.sample_id
            ),
            Profile::Pilot => assert!(
                (1..=MAX_PILOT_WORKS as i64).contains(&frozen),
                "sample {} は {frozen} 件です。pilot で扱えるのは 1〜{MAX_PILOT_WORKS} 件の sample だけです",
                manifest.sample_id
            ),
        }
    }
}

/// いまの作業の規模を読む。**既定値は無い。**
fn require_profile() -> Profile {
    let raw = required("CM_GT_PROFILE");
    match raw.as_str() {
        "pilot" => Profile::Pilot,
        "expansion" => Profile::Expansion,
        other => panic!("CM_GT_PROFILE は pilot か expansion です（{other}）"),
    }
}

// ─── 入力 ────────────────────────────────────────────────────────────────────

fn required(name: &str) -> String {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => panic!("環境変数 {name} を指定してください（既定値はありません）"),
    }
}

fn required_usize(name: &str) -> usize {
    let raw = required(name);
    raw.parse::<usize>()
        .unwrap_or_else(|_| panic!("環境変数 {name} は正の整数です（{raw}）"))
}

/// ちょうどこの値でなければ止める（件数を凍結する側で使う）
fn required_exact(name: &str, expected: usize) -> usize {
    let value = required_usize(name);
    assert_eq!(
        value, expected,
        "環境変数 {name} は {expected} です（{value}）"
    );
    value
}

/// 上限つきで読む。既定値は無く、範囲外はその場で止める
fn required_bounded(name: &str, max: usize) -> usize {
    let value = required_usize(name);
    assert!(
        (1..=max).contains(&value),
        "環境変数 {name} は 1〜{max} の範囲です（{value}）"
    );
    value
}

/// **これが各手順の最初の 1 行。** DB を開く前に、いま行う手順かどうかを見る。
///
/// `cargo test -- --ignored` で全部まとめて動かしても、`CM_GT_ACTION` に
/// 書いた 1 手順以外はここで止まる。取り違えて production DB に抽出や
/// 生成をかけてしまう事故を防ぐための関門。
fn require_action(expected: &str) {
    let selected = required("CM_GT_ACTION");
    assert_eq!(
        selected, expected,
        "この手順は CM_GT_ACTION={expected} のときだけ動きます（いまは {selected}）"
    );
}

fn db_path() -> PathBuf {
    let path = PathBuf::from(required("CM_GT_DB"));
    assert!(path.is_file(), "DB が見つかりません: {}", path.display());
    path
}

/// 対象 DB を開く。**migration は走らせない。**
///
/// 手動の作業で schema を作り替えてしまわないよう、開いた先が既に
/// 期待する版（025 まで適用済み）であることだけを確かめる。
fn open_target() -> Connection {
    let path = db_path();
    let conn = Connection::open(&path).expect("DB を開けません");
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

    check_schema(&conn);

    // 壊れた DB の上で ground truth を作らない
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .expect("integrity_check を実行できません");
    assert_eq!(integrity, "ok", "integrity_check が ok ではありません: {integrity}");

    println!("DB: {}", path.display());
    conn
}

/// **migration は絶対に走らせない。** 既に 025 まで当たっている DB かどうかだけを見る。
///
/// テーブルの有無だけでは足りない。022〜025 で足した列が無いまま動かすと、
/// 抽出や run の記録が中途半端な形で通ってしまうので、列まで確かめる。
fn check_schema(conn: &Connection) {
    for table in [
        "metadata_match_runs",
        "metadata_match_candidates",
        "metadata_match_verdicts",
        "metadata_review_tasks",
        "metadata_match_labels",
        "metadata_match_rejections",
        "metadata_match_jev_calls",
        "metadata_match_jev_run_links",
        "jev_eval_sessions",
    ] {
        let found: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
                rusqlite::params![table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            found, 1,
            "{table} がありません。migration 025 まで適用済みの DB を指定してください"
        );
    }

    for (table, column) in [
        ("metadata_review_tasks", "details_json"),
        ("metadata_review_tasks", "sampling_json"),
        ("metadata_match_candidates", "query_source"),
        ("metadata_match_jev_calls", "request_id"),
        ("metadata_match_jev_calls", "eval_session_id"),
        ("jev_eval_sessions", "lifecycle_revision"),
    ] {
        assert!(
            has_column(conn, table, column),
            "{table}.{column} がありません。migration 025 まで適用済みの DB を指定してください"
        );
    }
}

fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("table_info");
    let names = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .expect("table_info rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("table_info names");
    names.iter().any(|name| name == column)
}

/// 凍結済みの sample を読み、**development の対象であること**を確かめる。
///
/// 抽出だけでなく、生成と確認でも毎回これを通す。sample_id を打ち間違えて
/// live holdout の sample を指したとき、TMDB を呼ぶ前にここで止まる。
fn load_development_manifest(
    conn: &Connection,
    sample_id: &str,
) -> Result<crate::services::gt_sampling::SampleManifest, String> {
    let manifest = gt_sampling::load_manifest(conn, sample_id)
        .map_err(|error| format!("manifest を読めません: {error}"))?
        .ok_or_else(|| format!("sample {sample_id} がありません"))?;
    if manifest.cohort != COHORT {
        return Err(format!(
            "この runner が扱えるのは {COHORT} だけです（sample {sample_id} は {}）",
            manifest.cohort
        ));
    }
    if manifest.purpose != PURPOSE {
        return Err(format!(
            "この runner が扱えるのは {} だけです（sample {sample_id} は {}）",
            PURPOSE.as_str(),
            manifest.purpose.as_str()
        ));
    }
    Ok(manifest)
}

/// 読むだけの接続（書き込みは SQLite 側で拒否される）
fn open_read_only() -> Connection {
    let conn = open_target();
    conn.pragma_update(None, "query_only", true).unwrap();
    conn
}

// ─── 手順 1: 読むだけの下見 ──────────────────────────────────────────────────

/// 母集団の形を見る。**1 行も書かない。**
///
/// `source_media_kind` の分布を出すのは、1 work あたりの検索回数がここで
/// 決まるから（movie/tv が付いていれば片方だけ、unknown なら両方を引く）。
#[test]
#[ignore = "手動。CM_GT_ACTION=preflight / CM_GT_DB"]
fn manual_preflight_report() {
    require_action("preflight");
    let conn = open_read_only();

    let frame = gt_sampling::build_frame(&conn, COHORT, "preflight-probe")
        .expect("母集団を作れません");
    println!("cohort = {COHORT} / purpose = {}", PURPOSE.as_str());
    println!("母集団: {} 件", frame.len());

    let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
    let mut provenance: std::collections::BTreeMap<String, usize> = Default::default();
    let mut tagged = 0usize;
    let mut planned_total = 0usize;
    for row in &frame {
        *kinds.entry(row.source_media_kind.clone()).or_default() += 1;
        *provenance.entry(row.title_provenance.clone()).or_default() += 1;
        if row.tags_present {
            tagged += 1;
        }
        // 実際に何回引くことになるかは snapshot から数える。
        // 母集団に入っている work の証拠が取れないのは異常なので、握りつぶさない
        let snapshot =
            crate::services::prematch_snapshot::PreMatchSnapshot::capture(&conn, row.work_id)
                .unwrap_or_else(|error| {
                    panic!("work {} の証拠を読めません: {error}", row.work_id)
                });
        planned_total += crate::commands::tmdb::planned_logical_calls(&snapshot.local_evidence());
    }
    println!("source_media_kind の分布: {kinds:?}");
    println!("title_provenance の分布: {provenance:?}");
    println!("読めるタグがある work: {tagged} 件");
    println!(
        "論理検索の見積り: 合計 {planned_total} 回（母集団全体）。1 件あたり平均 {:.2} 回",
        if frame.is_empty() { 0.0 } else { planned_total as f64 / frame.len() as f64 }
    );

    for (label, sql) in [
        ("既存の audit_sample 課題", "SELECT COUNT(*) FROM metadata_review_tasks WHERE reason='audit_sample'"),
        ("強いラベル", "SELECT COUNT(*) FROM metadata_match_labels WHERE strength='strong'"),
        ("safe run", "SELECT COUNT(*) FROM metadata_match_runs WHERE mode='safe'"),
        ("Jev call", "SELECT COUNT(*) FROM metadata_match_jev_calls"),
    ] {
        let count: i64 = conn.query_row(sql, [], |row| row.get(0)).unwrap();
        println!("{label}: {count}");
    }
}

// ─── 手順 2: 抽出（TMDB を呼ばない）─────────────────────────────────────────

/// 対象を凍結する。ここでは通信しない。
#[test]
#[ignore = "手動。CM_GT_ACTION=freeze / CM_GT_PROFILE / CM_GT_DB / CM_GT_SAMPLE_ID / CM_GT_TARGET_COUNT / CM_GT_SOURCE_SHA"]
fn manual_freeze_sample() {
    require_action("freeze");
    let profile = require_profile();
    let mut conn = open_target();
    let request = SampleRequest {
        sample_id: required("CM_GT_SAMPLE_ID"),
        // live を渡す道を作らない
        cohort: COHORT.to_string(),
        purpose: PURPOSE,
        target_count: profile.target_count(),
        source_git_sha: required("CM_GT_SOURCE_SHA"),
        state_schema_version: STATE_SCHEMA_VERSION.to_string(),
        rules_policy_version: POLICY_VERSION.to_string(),
    };
    println!(
        "抽出: profile={} / sample_id={} / target={} / sha={}",
        profile.as_str(),
        request.sample_id,
        request.target_count,
        request.source_git_sha
    );

    let manifest = gt_sampling::freeze_sample(&mut conn, &request).expect("抽出に失敗しました");
    println!(
        "{} 件を凍結（再開: {}）。母集団 {} 件 / frame_sha256 = {}",
        manifest.entries.len(),
        manifest.resumed,
        manifest.frame_size,
        manifest.frame_sha256
    );
    for entry in &manifest.entries {
        println!(
            "  #{:>2} task={} work={} state={}",
            entry.sample_rank, entry.task_id, entry.work_id, entry.state
        );
    }
}

// ─── 手順 3: 候補検索と run の記録（実 TMDB）────────────────────────────────

/// 凍結済みの課題について、候補を集めて safe run を残す。
///
/// **予算は呼ぶ人が明示する。** 既定の 600 は初回の作業には広すぎるので、
/// ここでは環境変数を必須にしてある。足りなければ検索を呼ばずに飛ばす。
#[tokio::test]
#[ignore = "手動。実 TMDB を呼ぶ。CM_GT_ACTION=generate / CM_GT_PROFILE / CM_GT_DB / CM_GT_SAMPLE_ID / CM_GT_MAX_WORKS / CM_GT_MAX_HTTP"]
async fn manual_generate_runs() {
    require_action("generate");
    let profile = require_profile();
    let sample_id = required("CM_GT_SAMPLE_ID");
    let budget = profile.budget();

    let conn = open_target();

    // **API キーを読む前に**、対象が development の sample かを確かめる。
    // ここで落ちれば TMDB には 1 本も飛ばない
    let manifest = load_development_manifest(&conn, &sample_id).expect("対象を確認できません");
    // 選んだ規模と、凍結されている規模が食い違っていないか。
    // pilot のつもりで 50 件の sample を回す（またはその逆）のを防ぐ
    profile.check_manifest(&manifest);
    println!(
        "対象: profile={} sample={} cohort={} purpose={} 件数={} (凍結 {})",
        profile.as_str(),
        manifest.sample_id,
        manifest.cohort,
        manifest.purpose.as_str(),
        manifest.entries.len(),
        manifest.config.target_count
    );

    // キーは DB から読むだけ。値も長さもどこにも出さない
    let api_key: String = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = 'tmdb_api_key'",
            [],
            |row| row.get(0),
        )
        .expect("TMDb API キーが設定されていません");
    assert!(!api_key.trim().is_empty(), "TMDb API キーが空です");
    let client = TmdbClient::new(api_key);
    let provider = TmdbCandidateProvider::new(&client);

    let db = Arc::new(Mutex::new(conn));
    println!(
        "生成: sample_id={sample_id} / max_works={} / max_http={}",
        budget.max_works, budget.max_outbound_http_attempts
    );

    let report = audit_run::generate_for_sample(&db, &provider, &sample_id, budget)
        .await
        .expect("生成に失敗しました");
    println!("{report:#?}");
    println!(
        "ready={} stale={} failed={} skipped_budget={} already_ready={}",
        report.ready, report.stale, report.failed, report.skipped_budget, report.already_ready
    );
    println!(
        "論理検索 {} 回 / 予約した HTTP 上限 {} 本",
        report.logical_tmdb_calls, report.reserved_http_attempts
    );
}

// ─── 手順 4: 結果を読むだけ ─────────────────────────────────────────────────

/// sample の現状を読む。**1 行も書かない。**
#[test]
#[ignore = "手動。CM_GT_ACTION=report / CM_GT_DB / CM_GT_SAMPLE_ID"]
fn manual_sample_report() {
    require_action("report");
    let sample_id = required("CM_GT_SAMPLE_ID");
    let conn = open_read_only();

    let manifest = load_development_manifest(&conn, &sample_id).expect("対象を確認できません");
    println!(
        "sample={} cohort={} purpose={} 母集団={} target={}（読み取りのみ）",
        manifest.sample_id,
        manifest.cohort,
        manifest.purpose.as_str(),
        manifest.frame_size,
        manifest.config.target_count
    );

    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    for entry in &manifest.entries {
        let bucket = match entry.state.as_str() {
            STATE_READY => "ready",
            STATE_STALE => "stale",
            STATE_GENERATION_ERROR => "generation_error",
            other => other,
        };
        *counts.entry(bucket).or_default() += 1;

        // 「行が無い」と「SQL が失敗した」を混ぜない。
        // .ok() で潰すと、読めなかっただけの課題が「run 無し」に見えてしまう
        let run: Option<(i64, String, i64, Option<String>)> = conn
            .query_row(
                "SELECT r.id, r.mode, COALESCE(r.applied,0), r.status_after
                   FROM metadata_review_tasks t
                   JOIN metadata_match_runs r ON r.id = t.run_id
                  WHERE t.id = ?1",
                rusqlite::params![entry.task_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .unwrap_or_else(|error| panic!("task {} の run を読めません: {error}", entry.task_id));

        if entry.state == STATE_READY {
            let run = run
                .as_ref()
                .unwrap_or_else(|| panic!("task {} は ready なのに run がありません", entry.task_id));
            assert_eq!(run.1, "safe", "task {} の run が safe ではありません", entry.task_id);
            assert_eq!(run.2, 0, "task {} の run が適用済みです", entry.task_id);
            assert!(
                run.3.is_none(),
                "task {} の run に status_after が入っています",
                entry.task_id
            );
        }

        println!(
            "  #{:>2} task={} work={} state={} run={:?}",
            entry.sample_rank, entry.task_id, entry.work_id, entry.state, run
        );
    }
    println!("状態の内訳: {counts:?}");

    for (label, sql) in [
        (
            "この sample の run",
            "SELECT COUNT(*) FROM metadata_match_runs WHERE batch_id = ?1",
        ),
        (
            "候補",
            "SELECT COUNT(*) FROM metadata_match_candidates c
               JOIN metadata_match_runs r ON r.id = c.run_id WHERE r.batch_id = ?1",
        ),
        (
            "判定",
            "SELECT COUNT(*) FROM metadata_match_verdicts v
               JOIN metadata_match_runs r ON r.id = v.run_id WHERE r.batch_id = ?1",
        ),
        (
            "適用済み run（0 でなければ異常）",
            "SELECT COUNT(*) FROM metadata_match_runs WHERE batch_id = ?1 AND applied <> 0",
        ),
    ] {
        let count: i64 = conn
            .query_row(sql, rusqlite::params![sample_id], |row| row.get(0))
            .unwrap();
        println!("{label}: {count}");
    }
}

// ─── ランナー自体の性質を固定するテスト ─────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::scan::insert_scanned_work;
    use crate::db::test_support::insert_source;

    /// 環境変数はプロセス全体で 1 つしかない。
    /// 触るテストが同時に走ると、お互いの値を壊してしまうので順番に通す
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// reconstructed_clean になる work（scan 後に backfill した形）
    fn seed_reconstructed(conn: &Connection, index: usize) -> i64 {
        seed_work(conn, index, r"D:\Import", false)
    }

    /// live になる work（scan 時に original を捕捉した形）
    fn seed_live(conn: &Connection, index: usize) -> i64 {
        seed_work(conn, index + 100, r"D:\Live", true)
    }

    fn seed_work(conn: &Connection, index: usize, root: &str, captured: bool) -> i64 {
        let source_id = conn
            .query_row(
                "SELECT id FROM sources WHERE root_path = ?1",
                rusqlite::params![root],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or_else(|_| insert_source(conn, root));
        let file_name = format!("op-{index}.mkv");
        let work_id =
            insert_scanned_work(conn, "movie", "unknown", &format!("作品 {index}"), "unknown")
                .unwrap();
        if captured {
            conn.execute(
                "INSERT INTO files (source_id, file_path, file_name, extension, container,
                                    original_file_name, original_rel_path, original_captured)
                 VALUES (?1, ?2, ?3, 'mkv', 'mkv', ?3, ?3, 1)",
                rusqlite::params![source_id, format!(r"{root}\{file_name}"), file_name],
            )
            .unwrap();
        } else {
            conn.execute(
                "INSERT INTO files (source_id, file_path, file_name, extension, container)
                 VALUES (?1, ?2, ?3, 'mkv', 'mkv')",
                rusqlite::params![source_id, format!(r"{root}\{file_name}"), file_name],
            )
            .unwrap();
        }
        let file_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO work_parts (work_id, file_id) VALUES (?1, ?2)",
            rusqlite::params![work_id, file_id],
        )
        .unwrap();
        crate::db::backfill_prematch_inputs(conn).unwrap();
        work_id
    }

    /// cohort と purpose は動かせない
    #[test]
    fn the_runner_can_only_touch_the_development_cohort() {
        assert_eq!(COHORT, "reconstructed_clean");
        assert_eq!(PURPOSE, SamplePurpose::Development);
        assert_ne!(COHORT, "live");
    }

    /// 入力は必須。既定値で勝手に動かない
    #[test]
    fn missing_input_is_a_hard_error() {
        let _env = env_guard();
        let name = "CM_GT_TEST_ABSENT_VARIABLE";
        std::env::remove_var(name);
        assert!(std::panic::catch_unwind(|| required(name)).is_err());
    }

    /// development 以外の sample は、TMDB を呼ぶ前に弾く
    #[test]
    fn only_a_development_sample_can_be_operated() {
        use crate::services::gt_sampling::freeze_sample;

        let mut conn = crate::db::test_support::open_migrated();
        for index in 1..=8 {
            seed_reconstructed(&conn, index);
        }
        let ok_request = SampleRequest {
            sample_id: "c5c4-ok".to_string(),
            cohort: COHORT.to_string(),
            purpose: PURPOSE,
            target_count: 2,
            source_git_sha: "a".repeat(40),
            state_schema_version: STATE_SCHEMA_VERSION.to_string(),
            rules_policy_version: POLICY_VERSION.to_string(),
        };
        freeze_sample(&mut conn, &ok_request).unwrap();
        let manifest = load_development_manifest(&conn, "c5c4-ok").unwrap();
        assert_eq!(manifest.cohort, COHORT);
        assert_eq!(manifest.purpose, PURPOSE);

        // live + holdout（正規の手順で作れる組み合わせ）は弾く
        for index in 1..=3 {
            seed_live(&conn, index);
        }
        let holdout = SampleRequest {
            sample_id: "c5c4-holdout".to_string(),
            cohort: "live".to_string(),
            purpose: SamplePurpose::Holdout,
            target_count: 2,
            ..ok_request.clone()
        };
        freeze_sample(&mut conn, &holdout).unwrap();
        let error = load_development_manifest(&conn, "c5c4-holdout").unwrap_err();
        assert!(error.contains("reconstructed_clean"), "{error}");

        // historical_audit_only + development も、この runner の対象ではない
        conn.execute(
            "UPDATE metadata_review_tasks
                SET sampling_json = json_set(sampling_json,'$.cohort','historical_audit_only')
              WHERE json_extract(sampling_json,'$.sample_id') = 'c5c4-ok'",
            [],
        )
        .unwrap();
        let error = load_development_manifest(&conn, "c5c4-ok").unwrap_err();
        assert!(error.contains("reconstructed_clean"), "{error}");

        // そもそも無い sample も静かに通さない
        assert!(load_development_manifest(&conn, "c5c4-missing").is_err());
    }

    /// 手順は 1 つずつしか動かない
    #[test]
    fn an_action_must_be_selected_explicitly() {
        let _env = env_guard();
        // 指定が無い
        std::env::remove_var("CM_GT_ACTION");
        assert!(
            std::panic::catch_unwind(|| require_action("freeze")).is_err(),
            "CM_GT_ACTION 無しで動いてしまう"
        );

        // 別の手順を選んでいる（--ignored でまとめて走らせた状況）
        std::env::set_var("CM_GT_ACTION", "preflight");
        assert!(
            std::panic::catch_unwind(|| require_action("freeze")).is_err(),
            "選んでいない手順まで動いてしまう"
        );
        assert!(
            std::panic::catch_unwind(|| require_action("generate")).is_err(),
            "選んでいない手順まで動いてしまう"
        );

        // 選んだ手順だけ通る
        assert!(std::panic::catch_unwind(|| require_action("preflight")).is_ok());

        // 空白は名前として認めない
        std::env::set_var("CM_GT_ACTION", "   ");
        assert!(std::panic::catch_unwind(|| require_action("preflight")).is_err());

        std::env::remove_var("CM_GT_ACTION");
    }

    // ─── 作業の規模（profile）────────────────────────────────────────

    /// 規模を選ばずには動かない
    #[test]
    fn a_profile_must_be_selected_explicitly() {
        let _env = env_guard();
        std::env::remove_var("CM_GT_PROFILE");
        assert!(
            std::panic::catch_unwind(require_profile).is_err(),
            "CM_GT_PROFILE 無しで動いてしまう"
        );
        for bad in ["", "   ", "Pilot", "PILOT", "dev", "expansion50", "10"] {
            std::env::set_var("CM_GT_PROFILE", bad);
            assert!(
                std::panic::catch_unwind(require_profile).is_err(),
                "{bad} を規模として受け付けている"
            );
        }
        std::env::set_var("CM_GT_PROFILE", "pilot");
        assert_eq!(std::panic::catch_unwind(require_profile).unwrap(), Profile::Pilot);
        std::env::set_var("CM_GT_PROFILE", "expansion");
        assert_eq!(std::panic::catch_unwind(require_profile).unwrap(), Profile::Expansion);
        std::env::remove_var("CM_GT_PROFILE");
    }

    /// pilot は 10 / 90 の範囲のまま
    #[test]
    fn the_pilot_profile_keeps_its_range() {
        let _env = env_guard();
        assert_eq!(MAX_PILOT_WORKS, 10);
        assert_eq!(MAX_PILOT_HTTP, 90);

        for (count, ok) in [("0", false), ("1", true), ("10", true), ("11", false), ("50", false)] {
            std::env::set_var("CM_GT_TARGET_COUNT", count);
            assert_eq!(
                std::panic::catch_unwind(|| Profile::Pilot.target_count()).is_ok(),
                ok,
                "pilot の target_count {count}"
            );
        }
        for (works, http, ok) in [
            ("1", "1", true),
            ("10", "90", true),
            ("11", "90", false),
            ("10", "91", false),
            ("50", "450", false),
        ] {
            std::env::set_var("CM_GT_MAX_WORKS", works);
            std::env::set_var("CM_GT_MAX_HTTP", http);
            assert_eq!(
                std::panic::catch_unwind(|| Profile::Pilot.budget()).is_ok(),
                ok,
                "pilot の budget {works}/{http}"
            );
        }
        clear_numbers();
    }

    /// expansion は 50 / 50 / 450 ちょうどだけ
    #[test]
    fn the_expansion_profile_requires_exact_numbers() {
        let _env = env_guard();
        assert_eq!(EXPANSION_WORKS, 50);
        assert_eq!(EXPANSION_HTTP, 450);

        for (count, ok) in [("49", false), ("50", true), ("51", false), ("10", false), ("0", false)]
        {
            std::env::set_var("CM_GT_TARGET_COUNT", count);
            assert_eq!(
                std::panic::catch_unwind(|| Profile::Expansion.target_count()).is_ok(),
                ok,
                "expansion の target_count {count}"
            );
        }
        for (works, ok) in [("49", false), ("50", true), ("51", false)] {
            std::env::set_var("CM_GT_MAX_WORKS", works);
            std::env::set_var("CM_GT_MAX_HTTP", "450");
            assert_eq!(
                std::panic::catch_unwind(|| Profile::Expansion.budget()).is_ok(),
                ok,
                "expansion の max_works {works}"
            );
        }
        for (http, ok) in [("449", false), ("450", true), ("451", false), ("90", false)] {
            std::env::set_var("CM_GT_MAX_WORKS", "50");
            std::env::set_var("CM_GT_MAX_HTTP", http);
            assert_eq!(
                std::panic::catch_unwind(|| Profile::Expansion.budget()).is_ok(),
                ok,
                "expansion の max_http {http}"
            );
        }
        clear_numbers();
    }

    /// 凍結済みの規模と、選んだ規模が食い違っていたら動かない
    #[test]
    fn a_profile_cannot_operate_another_profiles_manifest() {
        use crate::services::gt_sampling::freeze_sample;

        let mut conn = crate::db::test_support::open_migrated();
        for index in 1..=60 {
            seed_reconstructed(&conn, index);
        }
        let base = SampleRequest {
            sample_id: "c5c5-pilot".to_string(),
            cohort: COHORT.to_string(),
            purpose: PURPOSE,
            target_count: 10,
            source_git_sha: "b".repeat(40),
            state_schema_version: STATE_SCHEMA_VERSION.to_string(),
            rules_policy_version: POLICY_VERSION.to_string(),
        };
        let pilot = freeze_sample(&mut conn, &base).unwrap();
        let expansion = freeze_sample(
            &mut conn,
            &SampleRequest {
                sample_id: "c5c5-expansion".to_string(),
                target_count: EXPANSION_WORKS,
                ..base.clone()
            },
        )
        .unwrap();
        assert_eq!(pilot.config.target_count, 10);
        assert_eq!(expansion.config.target_count, EXPANSION_WORKS as i64);

        // それぞれ自分の規模なら通る
        Profile::Pilot.check_manifest(&pilot);
        Profile::Expansion.check_manifest(&expansion);

        // 取り違えは止める
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Profile::Expansion
                .check_manifest(&pilot)))
            .is_err(),
            "expansion で pilot の sample を動かせてしまう"
        );
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Profile::Pilot
                .check_manifest(&expansion)))
            .is_err(),
            "pilot で expansion の sample を動かせてしまう"
        );
    }

    fn clear_numbers() {
        for name in ["CM_GT_TARGET_COUNT", "CM_GT_MAX_WORKS", "CM_GT_MAX_HTTP"] {
            std::env::remove_var(name);
        }
    }

    /// pilot の上限を超える入力は受け付けない
    #[test]
    fn the_pilot_ceilings_are_enforced() {
        let _env = env_guard();
        let name = "CM_GT_TEST_BOUNDED";
        for (value, ok) in [("0", false), ("1", true), ("10", true), ("11", false)] {
            std::env::set_var(name, value);
            let result = std::panic::catch_unwind(|| required_bounded(name, MAX_PILOT_WORKS));
            assert_eq!(result.is_ok(), ok, "{value} の扱いが違う");
        }
        for (value, ok) in [("90", true), ("91", false)] {
            std::env::set_var(name, value);
            let result = std::panic::catch_unwind(|| required_bounded(name, MAX_PILOT_HTTP));
            assert_eq!(result.is_ok(), ok, "{value} の扱いが違う");
        }
        std::env::remove_var(name);
        assert_eq!(MAX_PILOT_WORKS, 10);
        assert_eq!(MAX_PILOT_HTTP, 90);
    }

    /// 025 まで当たっていない DB は開けない（migration は走らせない）
    #[test]
    fn an_outdated_schema_is_refused() {
        let conn = crate::db::test_support::open_migrated();
        assert!(has_column(&conn, "metadata_match_candidates", "query_source"));
        assert!(has_column(&conn, "jev_eval_sessions", "lifecycle_revision"));
        assert!(!has_column(&conn, "metadata_match_runs", "no_such_column"));
        check_schema(&conn);

        // 023 までの DB（024 以降の列が無い）は弾く
        let old = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::apply_migrations_through_023(&old).unwrap();
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check_schema(&old)))
            .is_err());
    }

    /// 抽出の依頼は、この runner の固定値どおりに組み立てられる
    #[test]
    fn the_request_is_built_from_fixed_policy_versions() {
        let request = SampleRequest {
            sample_id: "c5c4-dev-pilot-01".to_string(),
            cohort: COHORT.to_string(),
            purpose: PURPOSE,
            target_count: 10,
            source_git_sha: "0".repeat(40),
            state_schema_version: STATE_SCHEMA_VERSION.to_string(),
            rules_policy_version: POLICY_VERSION.to_string(),
        };
        // gt_sampling 側の許可表を通ること（live+development ならここで落ちる）
        let mut conn = crate::db::test_support::open_migrated();
        let error = gt_sampling::freeze_sample(&mut conn, &request).unwrap_err();
        assert!(
            matches!(error, gt_sampling::GtSamplingError::FrameTooSmall { .. }),
            "許可表ではなく母集団の少なさで落ちるはず: {error:?}"
        );
    }
}
