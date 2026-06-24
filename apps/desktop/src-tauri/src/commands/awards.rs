use crate::db::DbState;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Serialize)]
pub struct AwardBodyRow {
    pub id: i64,
    pub name: String,
    pub display_name_ja: String,
    pub original_name: Option<String>,
    pub sort_name: Option<String>,
    pub body_type: String,
    pub prestige_tier: String,
    pub award_scope: String,
    pub country: Option<String>,
    pub city: Option<String>,
    pub official_url: Option<String>,
    pub is_active: bool,
    pub display_order: i64,
    pub note: Option<String>,
    pub category_count: i64,
    pub registered_work_count: i64,
    pub winner_count: i64,
    pub current_edition_year: i64,
    pub current_data_complete: bool,
    pub alert_status: String,
    pub alert_label: String,
    pub data_source_url: Option<String>,
    pub wikidata_entity_id: Option<String>,
    pub schedule_note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AwardCategoryRow {
    pub id: i64,
    pub award_body_id: i64,
    pub name: String,
    pub display_name_ja: String,
    pub original_name: Option<String>,
    pub category_type: String,
    pub target_type: String,
    pub is_top_prize: bool,
    pub is_major_category: bool,
    pub display_order: i64,
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkAwardResultViewRow {
    pub id: i64,
    pub work_id: i64,
    pub person_id: Option<i64>,
    pub award_body_id: i64,
    pub award_edition_id: Option<i64>,
    pub award_category_id: i64,
    pub result_type: String,
    pub section_name: Option<String>,
    pub source_url: Option<String>,
    pub confidence: Option<f64>,
    pub is_locked: bool,
    pub note: Option<String>,
    pub award_body_name: String,
    pub award_body_display_name_ja: String,
    pub prestige_tier: String,
    pub award_scope: String,
    pub award_year: Option<i64>,
    pub category_name: String,
    pub category_display_name_ja: String,
    pub person_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AwardWorkRow {
    pub result_id: i64,
    pub work_id: i64,
    pub title: String,
    pub year: Option<i64>,
    pub poster_path: Option<String>,
    pub award_year: Option<i64>,
    pub award_category_id: i64,
    pub category_display_name_ja: String,
    pub category_name: String,
    pub result_type: String,
    pub person_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddWorkAwardResultInput {
    pub work_id: i64,
    pub person_id: Option<i64>,
    pub award_body_id: i64,
    pub award_edition_id: Option<i64>,
    pub award_year: Option<i64>,
    pub award_category_id: i64,
    pub result_type: String,
    pub section_name: Option<String>,
    pub source_url: Option<String>,
    pub note: Option<String>,
    pub is_locked: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateWorkAwardResultInput {
    pub id: i64,
    pub result_type: Option<String>,
    pub section_name: Option<String>,
    pub source_url: Option<String>,
    pub note: Option<String>,
    pub is_locked: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AwardWorkFilter {
    pub award_body_id: Option<i64>,
    pub award_category_id: Option<i64>,
    pub prestige_tier: Option<String>,
    pub result_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetOrCreateAwardEditionInput {
    pub award_body_id: i64,
    pub year: i64,
}

#[derive(Debug, Serialize)]
pub struct AwardEditionRow {
    pub id: i64,
    pub award_body_id: i64,
    pub year: i64,
    pub edition_no: Option<i64>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub ceremony_date: Option<String>,
    pub official_url: Option<String>,
    pub note: Option<String>,
}

#[tauri::command]
pub fn list_award_bodies(state: State<'_, DbState>) -> Result<Vec<AwardBodyRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare(
        "WITH now_values AS (
             SELECT
                 CAST(strftime('%Y', 'now', 'localtime') AS INTEGER) AS current_year,
                 CAST(strftime('%m', 'now', 'localtime') AS INTEGER) AS current_month
         )
         SELECT
             ab.id, ab.name, ab.display_name_ja, ab.original_name, ab.sort_name,
             ab.body_type, ab.prestige_tier, ab.award_scope, ab.country, ab.city,
             ab.official_url, ab.is_active, ab.display_order, ab.note,
             COALESCE(cat.category_count, 0) AS category_count,
             COALESCE(res.registered_work_count, 0) AS registered_work_count,
             COALESCE(res.winner_count, 0) AS winner_count,
             nv.current_year,
             nv.current_month,
             sr.nomination_month_start,
             sr.nomination_month_end,
             sr.result_month_start,
             sr.result_month_end,
             sr.ceremony_month,
             sr.data_source_url,
             sr.wikidata_entity_id,
             sr.notes,
             COALESCE(ds.is_data_complete, 0) AS current_data_complete
         FROM award_bodies ab
         CROSS JOIN now_values nv
         LEFT JOIN (
             SELECT award_body_id, COUNT(*) AS category_count
             FROM award_categories
             GROUP BY award_body_id
         ) cat ON cat.award_body_id = ab.id
         LEFT JOIN (
             SELECT award_body_id,
                    COUNT(DISTINCT work_id) AS registered_work_count,
                    SUM(CASE WHEN result_type = 'winner' THEN 1 ELSE 0 END) AS winner_count
             FROM work_award_results
             GROUP BY award_body_id
         ) res ON res.award_body_id = ab.id
         LEFT JOIN award_body_schedule_rules sr ON sr.award_body_id = ab.id
         LEFT JOIN award_editions ae ON ae.award_body_id = ab.id AND ae.year = nv.current_year
         LEFT JOIN award_edition_data_status ds ON ds.award_edition_id = ae.id
         WHERE ab.is_active = 1
         ORDER BY
             CASE ab.prestige_tier WHEN 'S' THEN 0 WHEN 'A' THEN 1 WHEN 'B' THEN 2 ELSE 3 END,
             ab.display_order ASC,
             ab.display_name_ja ASC",
    ).map_err(|e| e.to_string())?;

    let rows = stmt.query_map([], map_award_body_row)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

#[tauri::command]
pub fn get_award_body_detail(
    state: State<'_, DbState>,
    award_body_id: i64,
) -> Result<Option<AwardBodyRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "WITH now_values AS (
             SELECT
                 CAST(strftime('%Y', 'now', 'localtime') AS INTEGER) AS current_year,
                 CAST(strftime('%m', 'now', 'localtime') AS INTEGER) AS current_month
         )
         SELECT
             ab.id, ab.name, ab.display_name_ja, ab.original_name, ab.sort_name,
             ab.body_type, ab.prestige_tier, ab.award_scope, ab.country, ab.city,
             ab.official_url, ab.is_active, ab.display_order, ab.note,
             COALESCE(cat.category_count, 0) AS category_count,
             COALESCE(res.registered_work_count, 0) AS registered_work_count,
             COALESCE(res.winner_count, 0) AS winner_count,
             nv.current_year,
             nv.current_month,
             sr.nomination_month_start,
             sr.nomination_month_end,
             sr.result_month_start,
             sr.result_month_end,
             sr.ceremony_month,
             sr.data_source_url,
             sr.wikidata_entity_id,
             sr.notes,
             COALESCE(ds.is_data_complete, 0) AS current_data_complete
         FROM award_bodies ab
         CROSS JOIN now_values nv
         LEFT JOIN (
             SELECT award_body_id, COUNT(*) AS category_count
             FROM award_categories
             GROUP BY award_body_id
         ) cat ON cat.award_body_id = ab.id
         LEFT JOIN (
             SELECT award_body_id,
                    COUNT(DISTINCT work_id) AS registered_work_count,
                    SUM(CASE WHEN result_type = 'winner' THEN 1 ELSE 0 END) AS winner_count
             FROM work_award_results
             GROUP BY award_body_id
         ) res ON res.award_body_id = ab.id
         LEFT JOIN award_body_schedule_rules sr ON sr.award_body_id = ab.id
         LEFT JOIN award_editions ae ON ae.award_body_id = ab.id AND ae.year = nv.current_year
         LEFT JOIN award_edition_data_status ds ON ds.award_edition_id = ae.id
         WHERE ab.id = ?1
         ",
        params![award_body_id],
        map_award_body_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_award_categories(
    state: State<'_, DbState>,
    award_body_id: i64,
) -> Result<Vec<AwardCategoryRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare(
        "SELECT id, award_body_id, name, display_name_ja, original_name,
                category_type, target_type, is_top_prize, is_major_category,
                display_order, note
         FROM award_categories
         WHERE award_body_id = ?1
         ORDER BY is_top_prize DESC, display_order ASC, display_name_ja ASC",
    ).map_err(|e| e.to_string())?;

    let rows = stmt.query_map(params![award_body_id], |row| {
        Ok(AwardCategoryRow {
            id: row.get(0)?,
            award_body_id: row.get(1)?,
            name: row.get(2)?,
            display_name_ja: row.get(3)?,
            original_name: row.get(4)?,
            category_type: row.get(5)?,
            target_type: row.get(6)?,
            is_top_prize: row.get::<_, i64>(7)? != 0,
            is_major_category: row.get::<_, i64>(8)? != 0,
            display_order: row.get(9)?,
            note: row.get(10)?,
        })
    })
    .map_err(|e| e.to_string())?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| e.to_string())?;
    Ok(rows)
}

#[tauri::command]
pub fn get_or_create_award_edition(
    state: State<'_, DbState>,
    input: GetOrCreateAwardEditionInput,
) -> Result<AwardEditionRow, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let id = upsert_award_edition(&conn, input.award_body_id, input.year)?;
    conn.query_row(
        "SELECT id, award_body_id, year, edition_no, start_date, end_date, ceremony_date, official_url, note
         FROM award_editions
         WHERE id = ?1",
        params![id],
        |row| {
            Ok(AwardEditionRow {
                id: row.get(0)?,
                award_body_id: row.get(1)?,
                year: row.get(2)?,
                edition_no: row.get(3)?,
                start_date: row.get(4)?,
                end_date: row.get(5)?,
                ceremony_date: row.get(6)?,
                official_url: row.get(7)?,
                note: row.get(8)?,
            })
        },
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_work_awards(
    state: State<'_, DbState>,
    work_id: i64,
) -> Result<Vec<WorkAwardResultViewRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list_work_awards_for_conn(&conn, work_id)
}

#[tauri::command]
pub fn add_work_award_result(
    state: State<'_, DbState>,
    input: AddWorkAwardResultInput,
) -> Result<WorkAwardResultViewRow, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let edition_id = match (input.award_edition_id, input.award_year) {
        (Some(id), _) => Some(id),
        (None, Some(year)) => Some(upsert_award_edition(&conn, input.award_body_id, year)?),
        (None, None) => None,
    };

    if let Some(existing_id) = find_existing_result(
        &conn,
        input.work_id,
        input.person_id,
        input.award_body_id,
        edition_id,
        input.award_category_id,
        &input.result_type,
    )? {
        return get_work_award_result(&conn, existing_id);
    }

    conn.execute(
        "INSERT INTO work_award_results
            (work_id, person_id, award_body_id, award_edition_id, award_category_id,
             result_type, section_name, source_url, note, is_locked)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            input.work_id,
            input.person_id,
            input.award_body_id,
            edition_id,
            input.award_category_id,
            input.result_type,
            input.section_name,
            input.source_url,
            input.note,
            if input.is_locked.unwrap_or(false) { 1 } else { 0 }
        ],
    )
    .map_err(|e| e.to_string())?;

    get_work_award_result(&conn, conn.last_insert_rowid())
}

#[tauri::command]
pub fn update_work_award_result(
    state: State<'_, DbState>,
    input: UpdateWorkAwardResultInput,
) -> Result<WorkAwardResultViewRow, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let locked: bool = conn
        .query_row(
            "SELECT is_locked FROM work_award_results WHERE id = ?1",
            params![input.id],
            |row| Ok(row.get::<_, i64>(0)? != 0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "award result not found".to_string())?;

    if locked && input.is_locked != Some(false) {
        return Err("locked award result cannot be edited".to_string());
    }

    conn.execute(
        "UPDATE work_award_results
         SET result_type = COALESCE(?2, result_type),
             section_name = COALESCE(?3, section_name),
             source_url = COALESCE(?4, source_url),
             note = COALESCE(?5, note),
             is_locked = COALESCE(?6, is_locked)
         WHERE id = ?1",
        params![
            input.id,
            input.result_type,
            input.section_name,
            input.source_url,
            input.note,
            input.is_locked.map(|v| if v { 1 } else { 0 }),
        ],
    )
    .map_err(|e| e.to_string())?;

    get_work_award_result(&conn, input.id)
}

#[tauri::command]
pub fn delete_work_award_result(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let locked: Option<i64> = conn
        .query_row(
            "SELECT is_locked FROM work_award_results WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if locked == Some(1) {
        return Err("locked award result cannot be deleted".to_string());
    }
    conn.execute("DELETE FROM work_award_results WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn list_award_winning_works(
    state: State<'_, DbState>,
    filter: AwardWorkFilter,
) -> Result<Vec<AwardWorkRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare(
        "SELECT war.id, w.id, w.title, COALESCE(w.release_year, w.year),
                COALESCE(w.poster_path, w.thumb_path), ae.year,
                war.award_category_id, ac.display_name_ja, ac.name, war.result_type, p.name
         FROM work_award_results war
         JOIN works w ON w.id = war.work_id
         JOIN award_bodies ab ON ab.id = war.award_body_id
         JOIN award_categories ac ON ac.id = war.award_category_id
         LEFT JOIN award_editions ae ON ae.id = war.award_edition_id
         LEFT JOIN persons p ON p.id = war.person_id
         WHERE (?1 IS NULL OR war.award_body_id = ?1)
           AND (?2 IS NULL OR war.award_category_id = ?2)
           AND (?3 IS NULL OR ab.prestige_tier = ?3)
           AND (?4 IS NULL OR war.result_type = ?4)
         ORDER BY ae.year DESC NULLS LAST,
             CASE war.result_type WHEN 'winner' THEN 0 WHEN 'nominee' THEN 1 ELSE 2 END,
             ac.display_order ASC,
             w.title ASC
         LIMIT 1000",
    ).map_err(|e| e.to_string())?;

    let rows = stmt.query_map(
        params![
            filter.award_body_id,
            filter.award_category_id,
            filter.prestige_tier,
            filter.result_type
        ],
        |row| {
            Ok(AwardWorkRow {
                result_id: row.get(0)?,
                work_id: row.get(1)?,
                title: row.get(2)?,
                year: row.get(3)?,
                poster_path: row.get(4)?,
                award_year: row.get(5)?,
                award_category_id: row.get(6)?,
                category_display_name_ja: row.get(7)?,
                category_name: row.get(8)?,
                result_type: row.get(9)?,
                person_name: row.get(10)?,
            })
        },
    )
    .map_err(|e| e.to_string())?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| e.to_string())?;
    Ok(rows)
}

fn map_award_body_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AwardBodyRow> {
    let current_year: i64 = row.get(17)?;
    let current_month: i64 = row.get(18)?;
    let nomination_start: Option<i64> = row.get(19)?;
    let nomination_end: Option<i64> = row.get(20)?;
    let result_start: Option<i64> = row.get(21)?;
    let result_end: Option<i64> = row.get(22)?;
    let current_data_complete = row.get::<_, i64>(27)? != 0;
    let (alert_status, alert_label) = award_alert(
        current_year,
        current_month,
        current_data_complete,
        nomination_start,
        nomination_end,
        result_start,
        result_end,
    );

    Ok(AwardBodyRow {
        id: row.get(0)?,
        name: row.get(1)?,
        display_name_ja: row.get(2)?,
        original_name: row.get(3)?,
        sort_name: row.get(4)?,
        body_type: row.get(5)?,
        prestige_tier: row.get(6)?,
        award_scope: row.get(7)?,
        country: row.get(8)?,
        city: row.get(9)?,
        official_url: row.get(10)?,
        is_active: row.get::<_, i64>(11)? != 0,
        display_order: row.get(12)?,
        note: row.get(13)?,
        category_count: row.get(14)?,
        registered_work_count: row.get(15)?,
        winner_count: row.get(16)?,
        current_edition_year: current_year,
        current_data_complete,
        alert_status,
        alert_label,
        data_source_url: row.get(24)?,
        wikidata_entity_id: row.get(25)?,
        schedule_note: row.get(26)?,
    })
}

fn award_alert(
    year: i64,
    month: i64,
    complete: bool,
    nomination_start: Option<i64>,
    nomination_end: Option<i64>,
    result_start: Option<i64>,
    result_end: Option<i64>,
) -> (String, String) {
    if complete {
        return ("up_to_date".to_string(), format!("{year}年度データ登録済み"));
    }

    let Some(nomination_start) = nomination_start else {
        return ("unscheduled".to_string(), "更新時期未設定".to_string());
    };
    let nomination_end = nomination_end.unwrap_or(nomination_start);
    let result_start = result_start.unwrap_or(nomination_end);
    let result_end = result_end.unwrap_or(result_start);

    if month_in_range(month, result_start, result_end) {
        return ("result_season".to_string(), format!("{year}年度の受賞結果発表時期"));
    }
    if month_after_range(month, result_start, result_end) {
        return ("data_stale".to_string(), format!("{year}年度データ未登録"));
    }
    if month_in_range(month, nomination_start, nomination_end) {
        return (
            "nomination_season".to_string(),
            format!("{year}年度のノミネート発表時期"),
        );
    }
    if months_until(month, nomination_start) == 1 {
        return (
            "nomination_soon".to_string(),
            format!("{year}年度のノミネート時期が近い"),
        );
    }

    ("scheduled".to_string(), format!("{year}年度データ未登録"))
}

fn month_in_range(month: i64, start: i64, end: i64) -> bool {
    if start <= end {
        (start..=end).contains(&month)
    } else {
        month >= start || month <= end
    }
}

fn months_until(current: i64, target: i64) -> i64 {
    (target - current + 12) % 12
}

fn month_after_range(month: i64, start: i64, end: i64) -> bool {
    if start <= end {
        month > end
    } else {
        month > end && month < start
    }
}

fn upsert_award_edition(
    conn: &rusqlite::Connection,
    award_body_id: i64,
    year: i64,
) -> Result<i64, String> {
    conn.execute(
        "INSERT OR IGNORE INTO award_editions (award_body_id, year) VALUES (?1, ?2)",
        params![award_body_id, year],
    )
    .map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT id FROM award_editions WHERE award_body_id = ?1 AND year = ?2",
        params![award_body_id, year],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

fn find_existing_result(
    conn: &rusqlite::Connection,
    work_id: i64,
    person_id: Option<i64>,
    award_body_id: i64,
    award_edition_id: Option<i64>,
    award_category_id: i64,
    result_type: &str,
) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT id FROM work_award_results
         WHERE work_id = ?1
           AND ((person_id IS NULL AND ?2 IS NULL) OR person_id = ?2)
           AND award_body_id = ?3
           AND ((award_edition_id IS NULL AND ?4 IS NULL) OR award_edition_id = ?4)
           AND award_category_id = ?5
           AND result_type = ?6
         LIMIT 1",
        params![work_id, person_id, award_body_id, award_edition_id, award_category_id, result_type],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn list_work_awards_for_conn(
    conn: &rusqlite::Connection,
    work_id: i64,
) -> Result<Vec<WorkAwardResultViewRow>, String> {
    let mut stmt = conn.prepare(WORK_AWARD_SELECT_WITH_WHERE)
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![work_id], map_work_award_view_row)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

fn get_work_award_result(
    conn: &rusqlite::Connection,
    result_id: i64,
) -> Result<WorkAwardResultViewRow, String> {
    conn.query_row(WORK_AWARD_SELECT_BY_ID, params![result_id], map_work_award_view_row)
        .map_err(|e| e.to_string())
}

const WORK_AWARD_SELECT_WITH_WHERE: &str = "
SELECT
  war.id, war.work_id, war.person_id, war.award_body_id, war.award_edition_id,
  war.award_category_id, war.result_type, war.section_name, war.source_url,
  war.confidence, war.is_locked, war.note,
  ab.name, ab.display_name_ja, ab.prestige_tier, ab.award_scope,
  ae.year,
  ac.name, ac.display_name_ja,
  p.name
FROM work_award_results war
JOIN award_bodies ab ON ab.id = war.award_body_id
LEFT JOIN award_editions ae ON ae.id = war.award_edition_id
JOIN award_categories ac ON ac.id = war.award_category_id
LEFT JOIN persons p ON p.id = war.person_id
WHERE war.work_id = ?1
ORDER BY
  CASE ab.prestige_tier WHEN 'S' THEN 0 WHEN 'A' THEN 1 WHEN 'B' THEN 2 ELSE 3 END,
  ae.year DESC NULLS LAST,
  ac.is_top_prize DESC,
  ac.display_order ASC";

const WORK_AWARD_SELECT_BY_ID: &str = "
SELECT
  war.id, war.work_id, war.person_id, war.award_body_id, war.award_edition_id,
  war.award_category_id, war.result_type, war.section_name, war.source_url,
  war.confidence, war.is_locked, war.note,
  ab.name, ab.display_name_ja, ab.prestige_tier, ab.award_scope,
  ae.year,
  ac.name, ac.display_name_ja,
  p.name
FROM work_award_results war
JOIN award_bodies ab ON ab.id = war.award_body_id
LEFT JOIN award_editions ae ON ae.id = war.award_edition_id
JOIN award_categories ac ON ac.id = war.award_category_id
LEFT JOIN persons p ON p.id = war.person_id
WHERE war.id = ?1";

fn map_work_award_view_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkAwardResultViewRow> {
    Ok(WorkAwardResultViewRow {
        id: row.get(0)?,
        work_id: row.get(1)?,
        person_id: row.get(2)?,
        award_body_id: row.get(3)?,
        award_edition_id: row.get(4)?,
        award_category_id: row.get(5)?,
        result_type: row.get(6)?,
        section_name: row.get(7)?,
        source_url: row.get(8)?,
        confidence: row.get(9)?,
        is_locked: row.get::<_, i64>(10)? != 0,
        note: row.get(11)?,
        award_body_name: row.get(12)?,
        award_body_display_name_ja: row.get(13)?,
        prestige_tier: row.get(14)?,
        award_scope: row.get(15)?,
        award_year: row.get(16)?,
        category_name: row.get(17)?,
        category_display_name_ja: row.get(18)?,
        person_name: row.get(19)?,
    })
}
