use crate::db::DbState;
use crate::models::tmdb::TmdbCredits;
use rusqlite::OptionalExtension;
use serde::Serialize;
use tauri::State;

// ─── 公開型 ──────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone)]
pub struct PersonSummaryRow {
    pub id: i64,
    pub name: String,
    pub profile_path: Option<String>,
    pub work_count: i64,
    pub roles: String,        // "director", "writer", "cast" (カンマ区切りの可能性)
}

#[derive(Debug, Serialize, Clone)]
pub struct WorkPersonRow {
    pub person_id: i64,
    pub name: String,
    pub role: String,
    pub character_name: Option<String>,
    pub display_order: i64,
}

// ─── Tauri コマンド ──────────────────────────────────────────────────────────

/// 人物一覧（作品数降順）
/// role_filter: "director" | "writer" | "cast" | null（全ロール）
#[tauri::command]
pub fn list_persons(
    state: State<'_, DbState>,
    role_filter: Option<String>,
) -> Result<Vec<PersonSummaryRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT p.id, p.name, p.profile_path,
                    COUNT(DISTINCT wp.work_id) AS work_count,
                    GROUP_CONCAT(DISTINCT wp.role) AS roles
             FROM persons p
             JOIN work_persons wp ON wp.person_id = p.id
             WHERE (?1 IS NULL OR wp.role = ?1)
             GROUP BY p.id
             ORDER BY work_count DESC, p.name ASC
             LIMIT 500",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(rusqlite::params![role_filter], |row| {
            Ok(PersonSummaryRow {
                id: row.get(0)?,
                name: row.get(1)?,
                profile_path: row.get(2)?,
                work_count: row.get(3)?,
                roles: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(rows)
}

/// 作品に紐付く人物一覧（DetailPane 表示用）
#[tauri::command]
pub fn get_work_persons(
    state: State<'_, DbState>,
    work_id: i64,
) -> Result<Vec<WorkPersonRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT wp.person_id, p.name, wp.role, wp.character_name, wp.display_order
             FROM work_persons wp
             JOIN persons p ON p.id = wp.person_id
             WHERE wp.work_id = ?1
             ORDER BY
                 CASE wp.role
                     WHEN 'director' THEN 0
                     WHEN 'writer'   THEN 1
                     ELSE 2
                 END,
                 wp.display_order ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(rusqlite::params![work_id], |row| {
            Ok(WorkPersonRow {
                person_id: row.get(0)?,
                name: row.get(1)?,
                role: row.get(2)?,
                character_name: row.get(3)?,
                display_order: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(rows)
}

/// 特定人物が関わる作品一覧
#[tauri::command]
pub fn get_person_works(
    state: State<'_, DbState>,
    person_id: i64,
    role_filter: Option<String>,
) -> Result<Vec<crate::commands::work::WorkSummaryRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT w.id, w.title, w.year, w.work_type,
                    COALESCE(w.poster_path, w.thumb_path),
                    us.user_rating, COALESCE(us.play_count, 0),
                    COALESCE(us.watch_status, 'unwatched'),
                    COALESCE(us.is_favorite, 0),
                    w.runtime_sec, us.resume_position_sec, w.external_rating, w.match_status
             FROM work_persons wp
             JOIN works w ON w.id = wp.work_id
             LEFT JOIN user_stats us ON us.work_id = w.id
             WHERE wp.person_id = ?1
               AND (?2 IS NULL OR wp.role = ?2)
             ORDER BY w.year DESC NULLS LAST, w.sort_title ASC NULLS LAST",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(rusqlite::params![person_id, role_filter], |row| {
            Ok(crate::commands::work::WorkSummaryRow {
                id: row.get(0)?,
                title: row.get(1)?,
                year: row.get(2)?,
                work_type: row.get(3)?,
                poster_path: row.get(4)?,
                user_rating: row.get(5)?,
                play_count: row.get(6)?,
                watch_status: row.get(7)?,
                is_favorite: row.get::<_, i64>(8)? != 0,
                runtime_sec: row.get(9)?,
                resume_position_sec: row.get(10)?,
                external_rating: row.get(11)?,
                match_status: row.get(12)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(rows)
}

/// 人物 1 件の詳細
#[tauri::command]
pub fn get_person(
    state: State<'_, DbState>,
    person_id: i64,
) -> Result<Option<PersonSummaryRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT p.id, p.name, p.profile_path,
                COUNT(DISTINCT wp.work_id) AS work_count,
                GROUP_CONCAT(DISTINCT wp.role) AS roles
         FROM persons p
         LEFT JOIN work_persons wp ON wp.person_id = p.id
         WHERE p.id = ?1
         GROUP BY p.id",
        rusqlite::params![person_id],
        |row| {
            Ok(PersonSummaryRow {
                id: row.get(0)?,
                name: row.get(1)?,
                profile_path: row.get(2)?,
                work_count: row.get(3)?,
                roles: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

// ─── 内部ヘルパー（tmdb.rs から呼ぶ） ─────────────────────────────────────────

const DIRECTOR_JOBS: &[&str] = &["Director"];
const WRITER_JOBS: &[&str]   = &["Screenplay", "Writer", "Story", "Novel", "Script"];
const MAX_CAST: usize = 20;

/// TMDb credits を受け取り、persons / work_persons に upsert する
pub fn store_credits(db: &DbState, work_id: i64, credits: &TmdbCredits) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // 既存の人物情報をクリアして再挿入（再取得時の重複防止）
    conn.execute(
        "DELETE FROM work_persons WHERE work_id = ?1",
        rusqlite::params![work_id],
    )
    .map_err(|e| e.to_string())?;

    // ── crew ────────────────────────────────────────────────────────────────
    for member in &credits.crew {
        let role = if DIRECTOR_JOBS.contains(&member.job.as_str()) {
            "director"
        } else if WRITER_JOBS.contains(&member.job.as_str()) {
            "writer"
        } else {
            continue;
        };

        let person_id = upsert_person(&conn, member.id, &member.name, member.profile_path.as_deref())?;

        conn.execute(
            "INSERT OR IGNORE INTO work_persons (work_id, person_id, role, display_order)
             VALUES (?1, ?2, ?3, 0)",
            rusqlite::params![work_id, person_id, role],
        )
        .map_err(|e| e.to_string())?;
    }

    // ── cast（上位 MAX_CAST 件）─────────────────────────────────────────────
    let cast_iter = credits.cast.iter().take(MAX_CAST);
    for (order, member) in cast_iter.enumerate() {
        let person_id = upsert_person(&conn, member.id, &member.name, member.profile_path.as_deref())?;

        conn.execute(
            "INSERT OR IGNORE INTO work_persons
                 (work_id, person_id, role, character_name, display_order)
             VALUES (?1, ?2, 'cast', ?3, ?4)",
            rusqlite::params![work_id, person_id, member.character.as_deref(), order as i64],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// persons テーブルへの upsert（tmdb_id で重複回避）
fn upsert_person(
    conn: &rusqlite::Connection,
    tmdb_id: i64,
    name: &str,
    _profile_path: Option<&str>,
) -> Result<i64, String> {
    conn.execute(
        "INSERT OR IGNORE INTO persons (tmdb_id, name) VALUES (?1, ?2)",
        rusqlite::params![tmdb_id, name],
    )
    .map_err(|e| e.to_string())?;

    conn.query_row(
        "SELECT id FROM persons WHERE tmdb_id = ?1",
        rusqlite::params![tmdb_id],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}
