use crate::db::DbState;
use serde::Serialize;
use tauri::State;

// ─── 重複グループ ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone)]
pub struct DuplicateGroup {
    pub reason: String,       // "same_tmdb_id" | "same_title_year"
    pub key: String,          // tmdb_id as string, or "normalized_title|year"
    pub work_ids: Vec<i64>,
    pub titles: Vec<String>,
    pub years: Vec<Option<i32>>,
}

// ─── 整合性レポート ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone)]
pub struct IntegrityIssue {
    pub work_id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub issues: Vec<String>, // issue コード一覧
}

#[derive(Debug, Serialize)]
pub struct IntegrityReport {
    pub issues: Vec<IntegrityIssue>,
    pub total_works: i64,
    pub total_issues: i64,
}

// ─── コマンド ─────────────────────────────────────────────────────────────────

/// 重複候補グループを返す（同一 tmdb_id / 正規化タイトル+年）
#[tauri::command]
pub fn get_duplicate_groups(
    state: State<'_, DbState>,
) -> Result<Vec<DuplicateGroup>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut groups: Vec<DuplicateGroup> = Vec::new();

    // ── 同一 tmdb_id ─────────────────────────────────────────────────────────
    {
        let mut stmt = conn
            .prepare(
                "SELECT
                   tmdb_id,
                   GROUP_CONCAT(id,    ',') AS ids,
                   GROUP_CONCAT(title, '§') AS titles,
                   GROUP_CONCAT(COALESCE(CAST(year AS TEXT), ''), ',') AS years
                 FROM works
                 WHERE tmdb_id IS NOT NULL
                 GROUP BY tmdb_id
                 HAVING COUNT(*) > 1
                 ORDER BY COUNT(*) DESC
                 LIMIT 100",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;

        for row in rows.flatten() {
            let (tmdb_id, ids_str, titles_str, years_str) = row;
            let work_ids: Vec<i64> = ids_str
                .split(',')
                .filter_map(|s| s.parse().ok())
                .collect();
            let titles: Vec<String> = titles_str.split('§').map(|s| s.to_string()).collect();
            let years: Vec<Option<i32>> =
                years_str.split(',').map(|s| s.parse().ok()).collect();
            groups.push(DuplicateGroup {
                reason: "same_tmdb_id".to_string(),
                key: tmdb_id.to_string(),
                work_ids,
                titles,
                years,
            });
        }
    }

    // ── 正規化タイトル + 年 ──────────────────────────────────────────────────
    {
        let mut stmt = conn
            .prepare(
                "SELECT
                   LOWER(TRIM(title)) AS norm_title,
                   year,
                   GROUP_CONCAT(id,    ',') AS ids,
                   GROUP_CONCAT(title, '§') AS titles
                 FROM works
                 WHERE year IS NOT NULL
                 GROUP BY norm_title, year
                 HAVING COUNT(*) > 1
                 ORDER BY COUNT(*) DESC
                 LIMIT 100",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;

        for row in rows.flatten() {
            let (norm_title, year, ids_str, titles_str) = row;
            let work_ids: Vec<i64> = ids_str
                .split(',')
                .filter_map(|s| s.parse().ok())
                .collect();
            let titles: Vec<String> = titles_str.split('§').map(|s| s.to_string()).collect();
            let years: Vec<Option<i32>> = work_ids.iter().map(|_| Some(year)).collect();
            groups.push(DuplicateGroup {
                reason: "same_title_year".to_string(),
                key: format!("{}|{}", norm_title, year),
                work_ids,
                titles,
                years,
            });
        }
    }

    Ok(groups)
}

/// 整合性レポートを返す
/// チェック項目は要確認センターと同じ条件に揃える。
///   unmatched    … TMDb 未照合
///   missing_meta … 製作年またはジャンルが未入力
///   no_persons   … 人物情報未取得
///   file_missing … 登録済みファイルが見つからない
#[tauri::command]
pub fn get_integrity_report(
    state: State<'_, DbState>,
) -> Result<IntegrityReport, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;

    let total_works: i64 = conn
        .query_row("SELECT COUNT(*) FROM works", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare(
            "SELECT
               w.id,
               w.title,
               w.year,
               CASE WHEN COALESCE(w.match_status, 'unmatched') = 'unmatched'
                    THEN 1 ELSE 0 END AS unmatched,
               CASE WHEN w.year IS NULL OR w.genres_json IS NULL
                    THEN 1 ELSE 0 END AS missing_meta,
               CASE WHEN NOT EXISTS (
                    SELECT 1 FROM work_persons wp_person WHERE wp_person.work_id = w.id)
                    THEN 1 ELSE 0 END AS no_persons,
               CASE WHEN EXISTS (
                    SELECT 1
                      FROM work_parts wp_missing
                      JOIN files f_missing ON f_missing.id = wp_missing.file_id
                     WHERE wp_missing.work_id = w.id
                       AND f_missing.availability_status = 'missing')
                    THEN 1 ELSE 0 END AS file_missing
             FROM works w
             ORDER BY w.id",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i32>>(2)?,
                row.get::<_, i32>(3)?,
                row.get::<_, i32>(4)?,
                row.get::<_, i32>(5)?,
                row.get::<_, i32>(6)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut issues: Vec<IntegrityIssue> = Vec::new();

    for row in rows.flatten() {
        let (
            id,
            title,
            year,
            unmatched,
            missing_meta,
            no_persons,
            file_missing,
        ) = row;
        let mut issue_list: Vec<String> = Vec::new();
        if unmatched == 1 {
            issue_list.push("unmatched".to_string());
        }
        if missing_meta == 1 {
            issue_list.push("missing_meta".to_string());
        }
        if no_persons == 1 {
            issue_list.push("no_persons".to_string());
        }
        if file_missing == 1 {
            issue_list.push("file_missing".to_string());
        }
        if !issue_list.is_empty() {
            issues.push(IntegrityIssue {
                work_id: id,
                title,
                year,
                issues: issue_list,
            });
        }
    }

    let total_issues = issues
        .iter()
        .map(|issue| issue.issues.len() as i64)
        .sum();
    Ok(IntegrityReport {
        issues,
        total_works,
        total_issues,
    })
}
