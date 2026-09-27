//! ground truth レビューのコマンド（PR3 C5c.3A）。
//!
//! `audit_sample` 課題を人が確かめるための入口。**production の割り当ては
//! 変えない**ので、`works` / `match_status` / poster / detail / credits /
//! series / Jev には一切触れない。ここで作るのは強いラベルと課題の解決だけ。

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::commands::settings::get_api_key_internal;
use crate::db::DbState;
use crate::services::gt_review::{
    self, ReviewStateChange, ReviewTaskDetail, ReviewTaskSummary, TmdbTargetVerifier,
    VerifiedTarget,
};
use crate::services::tmdb_client::TmdbClient;

/// 確定の結果（画面へ返すのはラベルの id と、閉じた課題だけ）
#[derive(Debug, Clone, Serialize)]
pub struct GtReviewResolution {
    pub task_id: i64,
    pub label_id: i64,
    pub state: String,
}

/// TMDB detail で存在を確かめる実装。
///
/// 取ってきた内容は画面に見せるだけで、`works` には書かない
/// （production の metadata 取り込みとは別経路）。
struct DetailVerifier {
    client: TmdbClient,
}

impl TmdbTargetVerifier for DetailVerifier {
    fn verify<'a>(
        &'a self,
        tmdb_id: i64,
        media_type: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<VerifiedTarget, String>> + Send + 'a>,
    > {
        Box::pin(async move {
            match media_type {
                "movie" => {
                    let detail = self.client.get_movie_detail(tmdb_id).await?;
                    Ok(VerifiedTarget {
                        tmdb_id: detail.id,
                        media_type: "movie".to_string(),
                        title: detail.title,
                        original_title: detail.original_title,
                        year: detail.release_date,
                        overview: detail.overview,
                        poster_path: detail.poster_path,
                    })
                }
                "tv" => {
                    let detail = self.client.get_tv_detail(tmdb_id).await?;
                    Ok(VerifiedTarget {
                        tmdb_id: detail.id,
                        media_type: "tv".to_string(),
                        title: detail.name,
                        original_title: detail.original_name,
                        year: detail.first_air_date,
                        overview: detail.overview,
                        poster_path: detail.poster_path,
                    })
                }
                other => Err(format!("media_type が不正です: {other}")),
            }
        })
    }
}

fn verifier(state: &State<'_, DbState>) -> Result<DetailVerifier, String> {
    let api_key = get_api_key_internal(state)?;
    Ok(DetailVerifier { client: TmdbClient::new(api_key) })
}

// ─── 一覧・詳細 ──────────────────────────────────────────────────────────────

/// sample の課題を並べる。`states` を渡すとその状態だけ
#[tauri::command]
pub fn list_gt_review_tasks(
    state: State<'_, DbState>,
    sample_id: String,
    states: Option<Vec<String>>,
) -> Result<Vec<ReviewTaskSummary>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let filter: Vec<String> = states.unwrap_or_default();
    let refs: Vec<&str> = filter.iter().map(String::as_str).collect();
    gt_review::list_tasks(&conn, &sample_id, &refs).map_err(|e| e.to_string())
}

/// 1 件の中身（候補は task.run_id のものだけ。判定器の出力は含まない）
#[tauri::command]
pub fn get_gt_review_task(
    state: State<'_, DbState>,
    task_id: i64,
) -> Result<ReviewTaskDetail, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    gt_review::get_task(&conn, task_id).map_err(|e| e.to_string())
}

// ─── ready ⇄ deferred ────────────────────────────────────────────────────────

/// 「後で」にする
#[tauri::command]
pub fn defer_gt_review_task(state: State<'_, DbState>, task_id: i64) -> Result<bool, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    gt_review::defer(&conn, task_id)
        .map(|change| change == ReviewStateChange::Updated)
        .map_err(|e| e.to_string())
}

/// 「後で」から戻す
#[tauri::command]
pub fn resume_gt_review_task(state: State<'_, DbState>, task_id: i64) -> Result<bool, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    gt_review::resume(&conn, task_id)
        .map(|change| change == ReviewStateChange::Updated)
        .map_err(|e| e.to_string())
}

// ─── 候補外 TMDB ID ──────────────────────────────────────────────────────────

/// 候補に無い TMDB ID を、確定する前に見てみる（DB は変えない）
#[tauri::command]
pub async fn preview_gt_review_tmdb_target(
    state: State<'_, DbState>,
    tmdb_id: i64,
    media_type: String,
) -> Result<VerifiedTarget, String> {
    let verifier = verifier(&state)?;
    gt_review::preview_target(&verifier, tmdb_id, &media_type)
        .await
        .map_err(|e| e.to_string())
}

// ─── 確定 ────────────────────────────────────────────────────────────────────

/// run の候補から選んで確定する
#[tauri::command]
pub fn resolve_gt_review_confirm(
    state: State<'_, DbState>,
    task_id: i64,
    candidate_id: i64,
    note: Option<String>,
) -> Result<GtReviewResolution, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let label_id = gt_review::resolve_confirm(&conn, task_id, candidate_id, note.as_deref())
        .map_err(|e| e.to_string())?;
    Ok(GtReviewResolution {
        task_id,
        label_id,
        state: gt_review::STATE_RESOLVED.to_string(),
    })
}

/// 候補に無い TMDB 作品で確定する。
///
/// TMDB detail で存在を確かめてから書く。確認の間は DB の lock を持たない。
#[tauri::command]
pub async fn resolve_gt_review_pick_other(
    state: State<'_, DbState>,
    task_id: i64,
    tmdb_id: i64,
    media_type: String,
    note: Option<String>,
) -> Result<GtReviewResolution, String> {
    let verifier = verifier(&state)?;
    let db = Arc::clone(&state.0);
    let label_id = gt_review::resolve_pick_other_verified(
        &db,
        &verifier,
        task_id,
        tmdb_id,
        &media_type,
        note.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(GtReviewResolution {
        task_id,
        label_id,
        state: gt_review::STATE_RESOLVED.to_string(),
    })
}

/// 「TMDB に該当なし」で確定する
#[tauri::command]
pub fn resolve_gt_review_none(
    state: State<'_, DbState>,
    task_id: i64,
    note: Option<String>,
) -> Result<GtReviewResolution, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let label_id =
        gt_review::resolve_none(&conn, task_id, note.as_deref()).map_err(|e| e.to_string())?;
    Ok(GtReviewResolution {
        task_id,
        label_id,
        state: gt_review::STATE_RESOLVED.to_string(),
    })
}
