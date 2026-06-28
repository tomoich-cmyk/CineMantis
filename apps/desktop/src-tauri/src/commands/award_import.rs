use crate::db::DbState;
use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::Duration;
use tauri::State;

const WIKIDATA_ENDPOINT: &str = "https://query.wikidata.org/sparql";
const WIKIDATA_USER_AGENT: &str = "CineMantis/1.0 (local personal movie library app)";
const WIKIDATA_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AwardImportJobSummary {
    pub job_id: i64,
    pub status: String,
    pub total_items: i64,
    pub inserted_items: i64,
    pub skipped_items: i64,
    pub error_message: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AwardImportMatchSummary {
    pub job_id: i64,
    pub total_items: i64,
    pub high_confidence: i64,
    pub needs_review: i64,
    pub low_confidence: i64,
    pub unmatched: i64,
    pub already_matched: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AwardImportItemView {
    pub id: i64,
    pub job_id: i64,
    pub award_body_id: i64,
    pub award_body_name: String,
    pub award_category_id: Option<i64>,
    pub award_category_name: Option<String>,
    pub raw_year: Option<i32>,
    pub raw_result_type: String,
    pub raw_film_id: String,
    pub raw_title_ja: Option<String>,
    pub raw_title_en: Option<String>,
    pub raw_imdb_id: Option<String>,
    pub raw_tmdb_id: Option<String>,
    pub raw_source_url: Option<String>,
    pub raw_award_name_en: Option<String>,
    pub raw_award_name_ja: Option<String>,
    pub matched_work_id: Option<i64>,
    pub matched_work_title: Option<String>,
    pub matched_work_year: Option<i32>,
    pub matched_work_tmdb_id: Option<String>,
    pub match_score: Option<f64>,
    pub match_method: Option<String>,
    pub status: String,
    pub approved_at: Option<String>,
    pub already_confirmed: bool,
    pub candidates: Vec<MatchCandidateView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchCandidateView {
    pub id: i64,
    pub work_id: i64,
    pub work_title: String,
    pub work_original_title: Option<String>,
    pub work_year: Option<i32>,
    pub work_source_path: Option<String>,
    pub work_tmdb_id: Option<String>,
    pub work_imdb_id: Option<String>,
    pub score: f64,
    pub match_method: String,
    pub is_selected: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkSearchResult {
    pub id: i64,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub tmdb_id: Option<String>,
    pub imdb_id: Option<String>,
    pub source_path: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkOperationResult {
    pub approved: i64,
    pub rejected: i64,
    pub skipped: i64,
    pub errors: Vec<String>,
}

#[derive(Debug)]
struct ImportItemForMatching {
    id: i64,
    raw_title_ja: Option<String>,
    raw_title_en: Option<String>,
    raw_imdb_id: Option<String>,
    raw_tmdb_id: Option<String>,
    raw_year: Option<i32>,
}

#[derive(Debug)]
struct WorkForMatching {
    id: i64,
    title: String,
    original_title: Option<String>,
    title_guess: Option<String>,
    release_year: Option<i32>,
    tmdb_id: Option<i64>,
    imdb_id: Option<String>,
}

#[derive(Debug, Clone)]
struct ScoredCandidate {
    work_id: i64,
    score: f64,
    match_method: String,
}

#[derive(Debug)]
struct ImportItemForApproval {
    id: i64,
    award_body_id: i64,
    matched_award_category_id: Option<i64>,
    matched_work_id: Option<i64>,
    raw_year: Option<i32>,
    raw_result_type: String,
    raw_source_url: Option<String>,
    match_score: Option<f64>,
}

#[derive(Debug)]
struct WikidataIds {
    body_qid: String,
    category_qid: Option<String>,
}

#[derive(Debug)]
struct AwardCategoryNames {
    id: i64,
    name_en: String,
    name_ja: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResultType {
    Winner,
    Nominee,
}

impl ResultType {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Winner => "winner",
            Self::Nominee => "nominee",
        }
    }

    fn priority(&self) -> u8 {
        match self {
            Self::Winner => 1,
            Self::Nominee => 0,
        }
    }
}

#[derive(Debug, Clone)]
struct WikidataFilmResult {
    film_qid: String,
    title_ja: Option<String>,
    title_en: Option<String>,
    result_type: ResultType,
    year: Option<i32>,
    imdb_id: Option<String>,
    tmdb_id: Option<String>,
    wikidata_url: Option<String>,
    award_qid: Option<String>,
    award_name_ja: Option<String>,
    award_name_en: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SparqlResponse {
    results: SparqlResults,
}

#[derive(Debug, Deserialize)]
struct SparqlResults {
    bindings: Vec<SparqlBinding>,
}

#[derive(Debug, Deserialize)]
struct SparqlBinding {
    film: Option<SparqlValue>,
    #[serde(rename = "titleJa")]
    title_ja: Option<SparqlValue>,
    #[serde(rename = "titleEn")]
    title_en: Option<SparqlValue>,
    #[serde(rename = "titleFallback")]
    title_fallback: Option<SparqlValue>,
    #[serde(rename = "resultType")]
    result_type: Option<SparqlValue>,
    year: Option<SparqlValue>,
    #[serde(rename = "imdbId")]
    imdb_id: Option<SparqlValue>,
    #[serde(rename = "tmdbId")]
    tmdb_id: Option<SparqlValue>,
    #[serde(rename = "wikidataUrl")]
    wikidata_url: Option<SparqlValue>,
    award: Option<SparqlValue>,
    #[serde(rename = "awardNameJa")]
    award_name_ja: Option<SparqlValue>,
    #[serde(rename = "awardNameEn")]
    award_name_en: Option<SparqlValue>,
}

#[derive(Debug, Deserialize)]
struct SparqlValue {
    value: String,
}

#[tauri::command]
pub async fn fetch_wikidata_award_items(
    state: State<'_, DbState>,
    award_body_id: i64,
    award_category_id: Option<i64>,
    year: Option<i32>,
) -> Result<AwardImportJobSummary, String> {
    fetch_wikidata_award_items_inner(&state, award_body_id, award_category_id, year)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn match_award_import_items(
    state: State<'_, DbState>,
    job_id: i64,
) -> Result<AwardImportMatchSummary, String> {
    match_award_import_items_inner(&state, job_id).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn list_award_import_items(
    state: State<'_, DbState>,
    job_id: Option<i64>,
    status: Option<String>,
    only_unmatched: Option<bool>,
) -> Result<Vec<AwardImportItemView>, String> {
    list_award_import_items_inner(&state, job_id, status, only_unmatched.unwrap_or(false))
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn select_award_match_candidate(
    state: State<'_, DbState>,
    import_item_id: i64,
    candidate_id: i64,
) -> Result<(), String> {
    select_award_match_candidate_inner(&state, import_item_id, candidate_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn approve_award_import_item(
    state: State<'_, DbState>,
    import_item_id: i64,
) -> Result<crate::commands::awards::WorkAwardResultViewRow, String> {
    approve_award_import_item_inner(&state, import_item_id).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn reject_award_import_item(state: State<'_, DbState>, import_item_id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|err| err.to_string())?;
    conn.execute(
        "UPDATE award_import_items SET status = 'rejected' WHERE id = ?1",
        params![import_item_id],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn search_works_for_award_match(
    state: State<'_, DbState>,
    query: String,
    year: Option<i32>,
) -> Result<Vec<WorkSearchResult>, String> {
    search_works_for_award_match_inner(&state, &query, year).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn add_manual_award_match_candidate(
    state: State<'_, DbState>,
    import_item_id: i64,
    work_id: i64,
) -> Result<i64, String> {
    add_manual_award_match_candidate_inner(&state, import_item_id, work_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn bulk_approve_award_import_items(
    state: State<'_, DbState>,
    import_item_ids: Vec<i64>,
) -> Result<BulkOperationResult, String> {
    let mut result = BulkOperationResult::default();

    for import_item_id in import_item_ids {
        match approve_award_import_item_inner(&state, import_item_id) {
            Ok(_) => result.approved += 1,
            Err(err) => {
                let message = err.to_string();
                if message.contains("import item not found")
                    || message.contains("already")
                    || message.contains("辣ｧ蜷井ｽ懷刀")
                    || message.contains("雉槭き繝")
                    || message.contains("蟷ｴ蠎ｦ")
                {
                    result.skipped += 1;
                } else {
                    result.errors.push(format!("item {import_item_id}: {message}"));
                }
            }
        }
    }

    Ok(result)
}

#[tauri::command]
pub fn bulk_reject_award_import_items(
    state: State<'_, DbState>,
    import_item_ids: Vec<i64>,
) -> Result<BulkOperationResult, String> {
    let conn = state.0.lock().map_err(|err| err.to_string())?;
    let mut result = BulkOperationResult::default();

    for import_item_id in import_item_ids {
        match conn.execute(
            "UPDATE award_import_items SET status = 'rejected' WHERE id = ?1 AND status = 'pending'",
            params![import_item_id],
        ) {
            Ok(affected) if affected > 0 => result.rejected += 1,
            Ok(_) => result.skipped += 1,
            Err(err) => result.errors.push(format!("item {import_item_id}: {err}")),
        }
    }

    Ok(result)
}

async fn fetch_wikidata_award_items_inner(
    db: &DbState,
    award_body_id: i64,
    award_category_id: Option<i64>,
    year: Option<i32>,
) -> Result<AwardImportJobSummary> {
    check_no_running_job(db, award_body_id)?;
    let ids = resolve_wikidata_ids(db, award_body_id, award_category_id)?;
    let category = resolve_award_category(db, award_category_id)?;
    let query = build_sparql(&ids, year);
    let job_id = create_import_job(db, award_body_id, year, &ids, &query)?;

    match run_import(db, job_id, award_category_id, category.as_ref(), year, &query).await {
        Ok(summary) => Ok(summary),
        Err(err) => {
            let message = err.to_string();
            fail_job(db, job_id, &message)?;
            Ok(AwardImportJobSummary {
                job_id,
                status: "error".to_string(),
                total_items: 0,
                inserted_items: 0,
                skipped_items: 0,
                error_message: Some(message),
            })
        }
    }
}

async fn run_import(
    db: &DbState,
    job_id: i64,
    award_category_id: Option<i64>,
    category: Option<&AwardCategoryNames>,
    year: Option<i32>,
    query: &str,
) -> Result<AwardImportJobSummary> {
    let response = execute_sparql(query).await?;
    let parsed = parse_sparql_response(response);
    let deduped = dedupe_winner_priority(parsed);

    let mut inserted_items = 0i64;
    let mut skipped_items = 0i64;

    for item in &deduped {
        if year.is_some() && item.year != year {
            skipped_items += 1;
            continue;
        }

        if item.result_type == ResultType::Winner {
            upgrade_pending_nominee_to_winner(db, item, award_category_id)?;
        }

        if insert_import_item(db, job_id, item, category)? {
            inserted_items += 1;
        } else {
            skipped_items += 1;
        }
    }

    let total_items = inserted_items + skipped_items;
    finish_job(db, job_id, total_items, inserted_items)?;

    Ok(AwardImportJobSummary {
        job_id,
        status: "done".to_string(),
        total_items,
        inserted_items,
        skipped_items,
        error_message: None,
    })
}

fn resolve_wikidata_ids(
    db: &DbState,
    award_body_id: i64,
    award_category_id: Option<i64>,
) -> Result<WikidataIds> {
    let conn = db.0.lock().map_err(|err| anyhow!(err.to_string()))?;
    let body_qid: Option<String> = conn
        .query_row(
            "SELECT wikidata_entity_id FROM award_bodies WHERE id = ?1",
            params![award_body_id],
            |row| row.get(0),
        )
        .optional()?;

    let body_qid = body_qid
        .filter(|qid| !qid.trim().is_empty())
        .ok_or_else(|| anyhow!("award_body の wikidata_entity_id が未設定です"))?;

    let category_qid = if let Some(category_id) = award_category_id {
        let qid = conn.query_row(
            "SELECT wikidata_entity_id FROM award_categories WHERE id = ?1 AND award_body_id = ?2",
            params![category_id, award_body_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .filter(|qid| !qid.trim().is_empty());

        if qid.is_none() {
            return Err(anyhow!("selected award category has no Wikidata ID"));
        }
        qid
    } else {
        None
    };

    Ok(WikidataIds {
        body_qid,
        category_qid,
    })
}

fn resolve_award_category(
    db: &DbState,
    award_category_id: Option<i64>,
) -> Result<Option<AwardCategoryNames>> {
    let Some(category_id) = award_category_id else {
        return Ok(None);
    };

    let conn = db.0.lock().map_err(|err| anyhow!(err.to_string()))?;
    conn.query_row(
        "SELECT id, name, display_name_ja FROM award_categories WHERE id = ?1",
        params![category_id],
        |row| {
            Ok(AwardCategoryNames {
                id: row.get(0)?,
                name_en: row.get(1)?,
                name_ja: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn check_no_running_job(db: &DbState, award_body_id: i64) -> Result<()> {
    let conn = db.0.lock().map_err(|err| anyhow!(err.to_string()))?;
    let running_job_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM award_import_jobs WHERE award_body_id = ?1 AND status = 'running' LIMIT 1",
            params![award_body_id],
            |row| row.get(0),
        )
        .optional()?;

    if let Some(job_id) = running_job_id {
        return Err(anyhow!("同じ映画賞の取得が実行中です: job_id={job_id}"));
    }

    Ok(())
}

fn create_import_job(
    db: &DbState,
    award_body_id: i64,
    year: Option<i32>,
    ids: &WikidataIds,
    query: &str,
) -> Result<i64> {
    let source_detail = format!(
        "https://query.wikidata.org/sparql category={} body={} query={}",
        ids.category_qid.as_deref().unwrap_or("none"),
        ids.body_qid,
        query
    );
    let wikidata_award_id = ids.category_qid.as_deref().unwrap_or(&ids.body_qid);

    let conn = db.0.lock().map_err(|err| anyhow!(err.to_string()))?;
    conn.execute(
        "INSERT INTO award_import_jobs
         (award_body_id, edition_year, source, source_detail, wikidata_award_id, status)
         VALUES (?1, ?2, 'wikidata', ?3, ?4, 'running')",
        params![award_body_id, year, source_detail, wikidata_award_id],
    )?;
    Ok(conn.last_insert_rowid())
}

fn finish_job(db: &DbState, job_id: i64, total_items: i64, matched_items: i64) -> Result<()> {
    let conn = db.0.lock().map_err(|err| anyhow!(err.to_string()))?;
    conn.execute(
        "UPDATE award_import_jobs
         SET status = 'done',
             total_items = ?2,
             matched_items = ?3,
             finished_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
             error_message = NULL
         WHERE id = ?1",
        params![job_id, total_items, matched_items],
    )?;
    Ok(())
}

fn fail_job(db: &DbState, job_id: i64, error_message: &str) -> Result<()> {
    let conn = db.0.lock().map_err(|err| anyhow!(err.to_string()))?;
    conn.execute(
        "UPDATE award_import_jobs
         SET status = 'error',
             error_message = ?2,
             finished_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        params![job_id, error_message],
    )?;
    Ok(())
}

async fn execute_sparql(query: &str) -> Result<SparqlResponse> {
    let client = Client::builder()
        .timeout(Duration::from_secs(WIKIDATA_TIMEOUT_SECS))
        .user_agent(WIKIDATA_USER_AGENT)
        .build()
        .context("Wikidata HTTP クライアントの初期化に失敗しました")?;

    let request = if query.len() > 1800 {
        client
            .post(WIKIDATA_ENDPOINT)
            .header("Accept", "application/sparql-results+json")
            .form(&[("query", query), ("format", "json")])
    } else {
        client
            .get(WIKIDATA_ENDPOINT)
            .query(&[("query", query), ("format", "json")])
    };

    let response = request
        .send()
        .await
        .context("Wikidata へのネットワークリクエストに失敗しました")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "Wikidata が HTTP {} を返しました: {}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }

    response
        .json::<SparqlResponse>()
        .await
        .context("Wikidata レスポンスの JSON パースに失敗しました")
}

fn build_sparql(ids: &WikidataIds, year: Option<i32>) -> String {
    if let Some(category_qid) = &ids.category_qid {
        build_sparql_category(category_qid, year)
    } else {
        build_sparql_body(&ids.body_qid, year)
    }
}

fn build_sparql_category(category_qid: &str, year: Option<i32>) -> String {
    let award_values = wikidata_values(category_qid);
    let year_filter = year
        .map(|year| format!("FILTER(!BOUND(?date) || YEAR(?date) = {year})"))
        .unwrap_or_default();

    format!(
        r#"
SELECT DISTINCT
  ?film
  (SAMPLE(?titleJa) AS ?titleJa)
  (SAMPLE(?titleEn) AS ?titleEn)
  (SAMPLE(?titleFallback) AS ?titleFallback)
  ?award
  (SAMPLE(?awardNameJa) AS ?awardNameJa)
  (SAMPLE(?awardNameEn) AS ?awardNameEn)
  ?resultType
  ?year
  ?imdbId
  ?tmdbId
  (CONCAT("https://www.wikidata.org/wiki/",
    STRAFTER(STR(?film), "http://www.wikidata.org/entity/")
  ) AS ?wikidataUrl)
WHERE {{
  VALUES (?prop ?propS ?resultType) {{
    (p:P166  ps:P166  "winner")
    (p:P1411 ps:P1411 "nominee")
  }}
  VALUES ?award {{ {award_values} }}
  {{
    ?film wdt:P31/wdt:P279* wd:Q11424 .
    ?film ?prop ?stmt .
    ?stmt ?propS ?award .
  }}
  UNION
  {{
    ?person ?prop ?stmt .
    ?stmt ?propS ?award .
    ?stmt pq:P1686 ?film .
    ?film wdt:P31/wdt:P279* wd:Q11424 .
  }}
  OPTIONAL {{
    ?stmt pq:P585 ?date .
    BIND(YEAR(?date) AS ?year)
  }}
  {year_filter}
  OPTIONAL {{ ?film wdt:P345 ?imdbId }}
  OPTIONAL {{ ?film wdt:P4947 ?tmdbId }}
  OPTIONAL {{ ?film rdfs:label ?titleJa . FILTER(LANG(?titleJa) = "ja") }}
  OPTIONAL {{ ?film rdfs:label ?titleEn . FILTER(LANG(?titleEn) = "en") }}
  OPTIONAL {{ ?film rdfs:label ?titleFallback . FILTER(LANG(?titleFallback) IN ("es", "ca", "fr", "de", "it", "pt")) }}
  OPTIONAL {{ ?award rdfs:label ?awardNameJa . FILTER(LANG(?awardNameJa) = "ja") }}
  OPTIONAL {{ ?award rdfs:label ?awardNameEn . FILTER(LANG(?awardNameEn) = "en") }}
}}
GROUP BY ?film ?award ?resultType ?year ?imdbId ?tmdbId
ORDER BY DESC(?year) ?resultType
"#
    )
}

fn wikidata_values(raw: &str) -> String {
    let values: Vec<String> = raw
        .split(|ch: char| ch == ',' || ch == ';' || ch.is_whitespace())
        .map(str::trim)
        .filter(|qid| qid.starts_with('Q') && qid[1..].chars().all(|ch| ch.is_ascii_digit()))
        .map(|qid| format!("wd:{qid}"))
        .collect();

    if values.is_empty() {
        "wd:Q0".to_string()
    } else {
        values.join(" ")
    }
}

fn build_sparql_body(body_qid: &str, year: Option<i32>) -> String {
    let year_filter = year
        .map(|year| format!("FILTER(YEAR(?date) = {year})"))
        .unwrap_or_default();

    format!(
        r#"
SELECT DISTINCT
  ?film
  (SAMPLE(?titleJa) AS ?titleJa)
  (SAMPLE(?titleEn) AS ?titleEn)
  (SAMPLE(?titleFallback) AS ?titleFallback)
  ?award
  (SAMPLE(?awardNameJa) AS ?awardNameJa)
  (SAMPLE(?awardNameEn) AS ?awardNameEn)
  ?resultType
  ?year
  ?imdbId
  ?tmdbId
  (CONCAT("https://www.wikidata.org/wiki/",
    STRAFTER(STR(?film), "http://www.wikidata.org/entity/")
  ) AS ?wikidataUrl)
WHERE {{
  VALUES (?prop ?propS ?resultType) {{
    (p:P166  ps:P166  "winner")
    (p:P1411 ps:P1411 "nominee")
  }}
  ?film wdt:P31/wdt:P279* wd:Q11424 .
  ?film ?prop ?stmt .
  ?stmt ?propS ?award .
  ?award wdt:P1027 wd:{body_qid} .
  OPTIONAL {{
    ?stmt pq:P585 ?date .
    BIND(YEAR(?date) AS ?year)
    {year_filter}
  }}
  OPTIONAL {{ ?film wdt:P345 ?imdbId }}
  OPTIONAL {{ ?film wdt:P4947 ?tmdbId }}
  OPTIONAL {{ ?film rdfs:label ?titleJa . FILTER(LANG(?titleJa) = "ja") }}
  OPTIONAL {{ ?film rdfs:label ?titleEn . FILTER(LANG(?titleEn) = "en") }}
  OPTIONAL {{ ?film rdfs:label ?titleFallback . FILTER(LANG(?titleFallback) IN ("es", "ca", "fr", "de", "it", "pt")) }}
  OPTIONAL {{ ?award rdfs:label ?awardNameJa . FILTER(LANG(?awardNameJa) = "ja") }}
  OPTIONAL {{ ?award rdfs:label ?awardNameEn . FILTER(LANG(?awardNameEn) = "en") }}
}}
GROUP BY ?film ?award ?resultType ?year ?imdbId ?tmdbId
ORDER BY DESC(?year) ?awardNameEn ?resultType
"#
    )
}

fn parse_sparql_response(response: SparqlResponse) -> Vec<WikidataFilmResult> {
    response
        .results
        .bindings
        .iter()
        .filter_map(parse_binding)
        .collect()
}

fn parse_binding(binding: &SparqlBinding) -> Option<WikidataFilmResult> {
    let film_qid = extract_qid(binding.film.as_ref()?.value.as_str());
    let result_type = match binding.result_type.as_ref()?.value.as_str() {
        "winner" => ResultType::Winner,
        "nominee" => ResultType::Nominee,
        _ => return None,
    };

    Some(WikidataFilmResult {
        film_qid,
        title_ja: binding.title_ja.as_ref().map(|value| value.value.clone()),
        title_en: binding
            .title_en
            .as_ref()
            .or(binding.title_fallback.as_ref())
            .map(|value| value.value.clone()),
        result_type,
        year: binding
            .year
            .as_ref()
            .and_then(|value| value.value.parse::<i32>().ok()),
        imdb_id: binding.imdb_id.as_ref().map(|value| value.value.clone()),
        tmdb_id: binding.tmdb_id.as_ref().map(|value| value.value.clone()),
        wikidata_url: binding
            .wikidata_url
            .as_ref()
            .map(|value| value.value.clone()),
        award_qid: binding
            .award
            .as_ref()
            .map(|value| extract_qid(value.value.as_str())),
        award_name_ja: binding
            .award_name_ja
            .as_ref()
            .map(|value| value.value.clone()),
        award_name_en: binding
            .award_name_en
            .as_ref()
            .map(|value| value.value.clone()),
    })
}

fn extract_qid(uri: &str) -> String {
    uri.rsplit('/').next().unwrap_or(uri).to_string()
}

fn dedupe_winner_priority(items: Vec<WikidataFilmResult>) -> Vec<WikidataFilmResult> {
    let mut map: HashMap<(String, String), WikidataFilmResult> = HashMap::new();

    for item in items {
        let award_key = item
            .award_qid
            .clone()
            .unwrap_or_else(|| "category".to_string());
        let key = (item.film_qid.clone(), award_key);

        match map.get(&key) {
            Some(existing)
                if existing.result_type.priority() > item.result_type.priority()
                    || (existing.result_type.priority() == item.result_type.priority()
                        && existing.year >= item.year) => {}
            _ => {
                map.insert(key, item);
            }
        }
    }

    let mut values: Vec<_> = map.into_values().collect();
    values.sort_by(|a, b| {
        b.year
            .cmp(&a.year)
            .then_with(|| b.result_type.priority().cmp(&a.result_type.priority()))
            .then_with(|| a.title_en.cmp(&b.title_en))
    });
    values
}

fn upgrade_pending_nominee_to_winner(
    db: &DbState,
    item: &WikidataFilmResult,
    award_category_id: Option<i64>,
) -> Result<()> {
    let conn = db.0.lock().map_err(|err| anyhow!(err.to_string()))?;
    conn.execute(
        "UPDATE award_import_items
         SET raw_result_type = 'winner'
         WHERE raw_film_id = ?1
           AND COALESCE(matched_award_category_id, -1) = COALESCE(?2, -1)
           AND raw_result_type = 'nominee'
           AND status = 'pending'",
        params![item.film_qid, award_category_id],
    )?;
    Ok(())
}

fn insert_import_item(
    db: &DbState,
    job_id: i64,
    item: &WikidataFilmResult,
    category: Option<&AwardCategoryNames>,
) -> Result<bool> {
    let award_name_en = category
        .map(|category| category.name_en.as_str())
        .or(item.award_name_en.as_deref());
    let award_name_ja = category
        .map(|category| category.name_ja.as_str())
        .or(item.award_name_ja.as_deref());
    let award_category_id = category.map(|category| category.id);

    let conn = db.0.lock().map_err(|err| anyhow!(err.to_string()))?;
    let changed = conn.execute(
        "INSERT OR IGNORE INTO award_import_items (
             job_id,
             raw_film_id,
             raw_title_ja,
             raw_title_en,
             raw_imdb_id,
             raw_tmdb_id,
             raw_award_name_en,
             raw_award_name_ja,
             raw_year,
             raw_result_type,
             raw_source_url,
             matched_award_category_id,
             status
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'pending')",
        params![
            job_id,
            item.film_qid,
            item.title_ja,
            item.title_en,
            item.imdb_id,
            item.tmdb_id,
            award_name_en,
            award_name_ja,
            item.year,
            item.result_type.as_str(),
            item.wikidata_url,
            award_category_id,
        ],
    )?;
    Ok(changed > 0)
}

fn match_award_import_items_inner(
    db: &DbState,
    job_id: i64,
) -> Result<AwardImportMatchSummary> {
    let conn = db.0.lock().map_err(|err| anyhow!(err.to_string()))?;
    let items = fetch_pending_items_for_matching(&conn, job_id)?;
    let works = fetch_all_works_for_matching(&conn)?;
    let mut summary = AwardImportMatchSummary {
        job_id,
        total_items: items.len() as i64,
        ..Default::default()
    };

    for item in &items {
        if has_match_candidates(&conn, item.id)? {
            summary.already_matched += 1;
            continue;
        }

        let candidates = score_candidates(item, &works);
        if candidates.is_empty() {
            summary.unmatched += 1;
            continue;
        }

        save_match_candidates(&conn, item.id, &candidates)?;
        let best = &candidates[0];
        update_import_item_match(&conn, item.id, best)?;

        if best.score >= 90.0 {
            summary.high_confidence += 1;
        } else if best.score >= 70.0 {
            summary.needs_review += 1;
        } else {
            summary.low_confidence += 1;
        }
    }

    Ok(summary)
}

fn fetch_pending_items_for_matching(
    conn: &Connection,
    job_id: i64,
) -> Result<Vec<ImportItemForMatching>> {
    let mut stmt = conn.prepare(
        "SELECT id, raw_title_ja, raw_title_en, raw_imdb_id, raw_tmdb_id, raw_year
         FROM award_import_items
         WHERE job_id = ?1
           AND status = 'pending'
         ORDER BY raw_year DESC NULLS LAST, id ASC",
    )?;
    let rows = stmt.query_map(params![job_id], |row| {
        Ok(ImportItemForMatching {
            id: row.get(0)?,
            raw_title_ja: row.get(1)?,
            raw_title_en: row.get(2)?,
            raw_imdb_id: row.get(3)?,
            raw_tmdb_id: row.get(4)?,
            raw_year: row.get(5)?,
        })
    })?
    .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn fetch_all_works_for_matching(conn: &Connection) -> Result<Vec<WorkForMatching>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, original_title, title_guess, COALESCE(release_year, year), tmdb_id, imdb_id
         FROM works
         WHERE work_type IN ('movie', 'other', 'special', 'ova')",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(WorkForMatching {
            id: row.get(0)?,
            title: row.get(1)?,
            original_title: row.get(2)?,
            title_guess: row.get(3)?,
            release_year: row.get(4)?,
            tmdb_id: row.get(5)?,
            imdb_id: row.get(6)?,
        })
    })?
    .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn has_match_candidates(conn: &Connection, import_item_id: i64) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM work_award_match_candidates WHERE import_item_id = ?1",
        params![import_item_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn score_candidates(
    item: &ImportItemForMatching,
    works: &[WorkForMatching],
) -> Vec<ScoredCandidate> {
    let mut candidates: Vec<_> = works
        .iter()
        .filter_map(|work| score_single_candidate(item, work))
        .filter(|candidate| candidate.score >= 40.0)
        .collect();

    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.work_id.cmp(&b.work_id))
    });
    candidates.truncate(10);
    candidates
}

fn score_single_candidate(
    item: &ImportItemForMatching,
    work: &WorkForMatching,
) -> Option<ScoredCandidate> {
    if let (Some(raw_tmdb_id), Some(work_tmdb_id)) = (&item.raw_tmdb_id, work.tmdb_id) {
        if raw_tmdb_id.parse::<i64>().ok() == Some(work_tmdb_id) {
            return Some(ScoredCandidate {
                work_id: work.id,
                score: 100.0,
                match_method: "tmdb_id".to_string(),
            });
        }
    }

    if let (Some(raw_imdb_id), Some(work_imdb_id)) = (&item.raw_imdb_id, &work.imdb_id) {
        if normalize_imdb_id(raw_imdb_id) == normalize_imdb_id(work_imdb_id) {
            return Some(ScoredCandidate {
                work_id: work.id,
                score: 98.0,
                match_method: "imdb_id".to_string(),
            });
        }
    }

    let year_adjustment = year_adjustment(item.raw_year, work.release_year);
    let mut best: Option<ScoredCandidate> = None;
    let work_titles = [
        Some(work.title.as_str()),
        work.original_title.as_deref(),
        work.title_guess.as_deref(),
    ];
    let raw_titles = [item.raw_title_ja.as_deref(), item.raw_title_en.as_deref()];

    for raw_title in raw_titles.into_iter().flatten() {
        for work_title in work_titles.into_iter().flatten() {
            let (base_score, fuzzy) = score_title(raw_title, work_title)?;
            let score = (base_score + year_adjustment).clamp(0.0, 100.0);
            let candidate = ScoredCandidate {
                work_id: work.id,
                score,
                match_method: if fuzzy {
                    "title_fuzzy".to_string()
                } else {
                    "title_exact".to_string()
                },
            };
            if best.as_ref().map(|existing| existing.score < candidate.score).unwrap_or(true) {
                best = Some(candidate);
            }
        }
    }

    best
}

fn normalize_imdb_id(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("tt")
        .trim_start_matches("TT")
        .to_ascii_lowercase()
}

fn normalize_title(value: &str) -> String {
    let mut normalized = String::new();
    let lower = value.to_lowercase();
    for ch in lower.chars() {
        if ch.is_alphanumeric() || is_japanese_char(ch) {
            normalized.push(ch);
        } else if ch.is_whitespace() || matches!(ch, '-' | '_' | ':' | '/' | '／') {
            normalized.push(' ');
        }
    }
    normalized
        .split_whitespace()
        .filter(|token| !matches!(*token, "the" | "a" | "an"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_japanese_char(ch: char) -> bool {
    ('\u{3040}'..='\u{30ff}').contains(&ch)
        || ('\u{3400}'..='\u{9fff}').contains(&ch)
        || ('\u{ff66}'..='\u{ff9f}').contains(&ch)
}

fn score_title(raw_title: &str, work_title: &str) -> Option<(f64, bool)> {
    if raw_title.trim().eq_ignore_ascii_case(work_title.trim()) {
        return Some((82.0, false));
    }

    let raw_norm = normalize_title(raw_title);
    let work_norm = normalize_title(work_title);
    if raw_norm.is_empty() || work_norm.is_empty() {
        return None;
    }

    if raw_norm == work_norm {
        return Some((75.0, false));
    }

    let similarity = title_similarity(&raw_norm, &work_norm);
    if similarity >= 0.86 {
        return Some((50.0 + similarity * 22.0, true));
    }
    None
}

fn title_similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let max_len = a.chars().count().max(b.chars().count());
    if max_len == 0 {
        return 1.0;
    }
    let distance = levenshtein(a, b);
    1.0 - (distance as f64 / max_len as f64)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut costs: Vec<usize> = (0..=b_chars.len()).collect();

    for (i, ca) in a.chars().enumerate() {
        let mut last = i;
        costs[0] = i + 1;
        for (j, cb) in b_chars.iter().enumerate() {
            let old = costs[j + 1];
            costs[j + 1] = if ca == *cb {
                last
            } else {
                1 + last.min(costs[j]).min(old)
            };
            last = old;
        }
    }
    costs[b_chars.len()]
}

fn year_adjustment(raw_year: Option<i32>, work_year: Option<i32>) -> f64 {
    let (Some(raw_year), Some(work_year)) = (raw_year, work_year) else {
        return 0.0;
    };
    match (raw_year - work_year).abs() {
        0 => 10.0,
        1 => -5.0,
        2 => -10.0,
        _ => -20.0,
    }
}

fn save_match_candidates(
    conn: &Connection,
    import_item_id: i64,
    candidates: &[ScoredCandidate],
) -> Result<()> {
    conn.execute(
        "DELETE FROM work_award_match_candidates WHERE import_item_id = ?1",
        params![import_item_id],
    )?;
    for (index, candidate) in candidates.iter().enumerate() {
        conn.execute(
            "INSERT OR IGNORE INTO work_award_match_candidates
             (import_item_id, work_id, match_score, match_method, is_selected)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                import_item_id,
                candidate.work_id,
                candidate.score,
                candidate.match_method,
                if index == 0 { 1 } else { 0 },
            ],
        )?;
    }
    Ok(())
}

fn update_import_item_match(
    conn: &Connection,
    import_item_id: i64,
    candidate: &ScoredCandidate,
) -> Result<()> {
    conn.execute(
        "UPDATE award_import_items
         SET matched_work_id = ?2,
             match_score = ?3,
             match_method = ?4
         WHERE id = ?1",
        params![
            import_item_id,
            candidate.work_id,
            candidate.score,
            candidate.match_method
        ],
    )?;
    Ok(())
}

fn list_award_import_items_inner(
    db: &DbState,
    job_id: Option<i64>,
    status: Option<String>,
    only_unmatched: bool,
) -> Result<Vec<AwardImportItemView>> {
    let conn = db.0.lock().map_err(|err| anyhow!(err.to_string()))?;
    let mut stmt = conn.prepare(
        "SELECT
             aii.id,
             aii.job_id,
             ab.id AS award_body_id,
             ab.name AS award_body_name,
             ac.id AS award_category_id,
             ac.name AS award_category_name,
             aii.raw_year,
             aii.raw_result_type,
             aii.raw_film_id,
             aii.raw_title_ja,
             aii.raw_title_en,
             aii.raw_imdb_id,
             aii.raw_tmdb_id,
             aii.raw_source_url,
             aii.raw_award_name_en,
             aii.raw_award_name_ja,
             aii.matched_work_id,
             w.title AS matched_work_title,
             COALESCE(w.release_year, w.year) AS matched_work_year,
             CAST(w.tmdb_id AS TEXT) AS matched_work_tmdb_id,
             aii.match_score,
             aii.match_method,
             aii.status,
             aii.approved_at,
             CASE WHEN EXISTS (
                 SELECT 1
                 FROM work_award_results war
                 JOIN award_editions ae ON ae.id = war.award_edition_id
                 WHERE war.work_id = aii.matched_work_id
                   AND war.award_category_id = aii.matched_award_category_id
                   AND ae.year = aii.raw_year
                   AND war.result_type = aii.raw_result_type
             ) THEN 1 ELSE 0 END AS already_confirmed
         FROM award_import_items aii
         JOIN award_import_jobs aij ON aij.id = aii.job_id
         JOIN award_bodies ab ON ab.id = aij.award_body_id
         LEFT JOIN award_categories ac ON ac.id = aii.matched_award_category_id
         LEFT JOIN works w ON w.id = aii.matched_work_id
         WHERE (?1 IS NULL OR aii.job_id = ?1)
           AND (?2 IS NULL OR aii.status = ?2)
           AND (?3 = 0 OR aii.matched_work_id IS NULL OR COALESCE(aii.match_score, 0) < 70)
         ORDER BY aii.raw_year DESC NULLS LAST,
                  COALESCE(aii.match_score, -1) DESC,
                  aii.id ASC",
    )?;

    let mut views = stmt
        .query_map(params![job_id, status, if only_unmatched { 1 } else { 0 }], |row| {
            Ok(AwardImportItemView {
                id: row.get(0)?,
                job_id: row.get(1)?,
                award_body_id: row.get(2)?,
                award_body_name: row.get(3)?,
                award_category_id: row.get(4)?,
                award_category_name: row.get(5)?,
                raw_year: row.get(6)?,
                raw_result_type: row.get(7)?,
                raw_film_id: row.get(8)?,
                raw_title_ja: row.get(9)?,
                raw_title_en: row.get(10)?,
                raw_imdb_id: row.get(11)?,
                raw_tmdb_id: row.get(12)?,
                raw_source_url: row.get(13)?,
                raw_award_name_en: row.get(14)?,
                raw_award_name_ja: row.get(15)?,
                matched_work_id: row.get(16)?,
                matched_work_title: row.get(17)?,
                matched_work_year: row.get(18)?,
                matched_work_tmdb_id: row.get(19)?,
                match_score: row.get(20)?,
                match_method: row.get(21)?,
                status: row.get(22)?,
                approved_at: row.get(23)?,
                already_confirmed: row.get::<_, i64>(24)? != 0,
                candidates: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for view in &mut views {
        view.candidates = list_match_candidates(&conn, view.id)?;
    }
    Ok(views)
}

fn list_match_candidates(conn: &Connection, import_item_id: i64) -> Result<Vec<MatchCandidateView>> {
    let mut stmt = conn.prepare(
        "SELECT
             c.id,
             w.id,
             w.title,
             w.original_title,
             COALESCE(w.release_year, w.year),
             (
               SELECT s.root_path || CASE WHEN f.file_path IS NOT NULL THEN ' / ' || f.file_path ELSE '' END
               FROM work_parts wp
               JOIN files f ON f.id = wp.file_id
               JOIN sources s ON s.id = f.source_id
               WHERE wp.work_id = w.id
               ORDER BY wp.play_order ASC, f.id ASC
               LIMIT 1
             ) AS source_path,
             CAST(w.tmdb_id AS TEXT),
             w.imdb_id,
             c.match_score,
             c.match_method,
             c.is_selected
         FROM work_award_match_candidates c
         JOIN works w ON w.id = c.work_id
         WHERE c.import_item_id = ?1
         ORDER BY c.is_selected DESC, c.match_score DESC, w.title ASC",
    )?;
    let rows = stmt.query_map(params![import_item_id], |row| {
        Ok(MatchCandidateView {
            id: row.get(0)?,
            work_id: row.get(1)?,
            work_title: row.get(2)?,
            work_original_title: row.get(3)?,
            work_year: row.get(4)?,
            work_source_path: row.get(5)?,
            work_tmdb_id: row.get(6)?,
            work_imdb_id: row.get(7)?,
            score: row.get(8)?,
            match_method: row.get(9)?,
            is_selected: row.get::<_, i64>(10)? != 0,
        })
    })?
    .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn select_award_match_candidate_inner(
    db: &DbState,
    import_item_id: i64,
    candidate_id: i64,
) -> Result<()> {
    let conn = db.0.lock().map_err(|err| anyhow!(err.to_string()))?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE work_award_match_candidates SET is_selected = 0 WHERE import_item_id = ?1",
        params![import_item_id],
    )?;
    let candidate = tx
        .query_row(
            "SELECT work_id, match_score, match_method
             FROM work_award_match_candidates
             WHERE id = ?1 AND import_item_id = ?2",
            params![candidate_id, import_item_id],
            |row| {
                Ok(ScoredCandidate {
                    work_id: row.get(0)?,
                    score: row.get(1)?,
                    match_method: row.get(2)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| anyhow!("candidate not found"))?;
    tx.execute(
        "UPDATE work_award_match_candidates SET is_selected = 1 WHERE id = ?1",
        params![candidate_id],
    )?;
    update_import_item_match(&tx, import_item_id, &candidate)?;
    tx.commit()?;
    Ok(())
}

fn add_manual_award_match_candidate_inner(
    db: &DbState,
    import_item_id: i64,
    work_id: i64,
) -> Result<i64> {
    let conn = db.0.lock().map_err(|err| anyhow!(err.to_string()))?;
    let import_exists: Option<i64> = conn
        .query_row(
            "SELECT id FROM award_import_items WHERE id = ?1",
            params![import_item_id],
            |row| row.get(0),
        )
        .optional()?;
    if import_exists.is_none() {
        return Err(anyhow!("import item not found"));
    }

    let work_exists: Option<i64> = conn
        .query_row(
            "SELECT id FROM works WHERE id = ?1",
            params![work_id],
            |row| row.get(0),
        )
        .optional()?;
    if work_exists.is_none() {
        return Err(anyhow!("work not found"));
    }

    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM work_award_match_candidates WHERE import_item_id = ?1 AND work_id = ?2",
            params![import_item_id, work_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(candidate_id) = existing {
        return Ok(candidate_id);
    }

    conn.execute(
        "INSERT INTO work_award_match_candidates
             (import_item_id, work_id, match_score, match_method, is_selected)
         VALUES (?1, ?2, 100.0, 'manual', 0)",
        params![import_item_id, work_id],
    )?;
    Ok(conn.last_insert_rowid())
}

fn approve_award_import_item_inner(
    db: &DbState,
    import_item_id: i64,
) -> Result<crate::commands::awards::WorkAwardResultViewRow> {
    let conn = db.0.lock().map_err(|err| anyhow!(err.to_string()))?;
    let tx = conn.unchecked_transaction()?;
    let item = fetch_import_item_for_approval(&tx, import_item_id)?;
    let work_id = item
        .matched_work_id
        .ok_or_else(|| anyhow!("照合作品が選択されていません"))?;
    let category_id = item
        .matched_award_category_id
        .ok_or_else(|| anyhow!("賞カテゴリが不明です"))?;
    let year = item.raw_year.ok_or_else(|| anyhow!("年度が不明です"))?;
    let edition_id = upsert_award_edition_local(&tx, item.award_body_id, year as i64)?;

    if let Some(existing_id) = find_existing_award_result_local(
        &tx,
        work_id,
        item.award_body_id,
        category_id,
    )? {
        reconcile_existing_award_result_local(&tx, existing_id, edition_id, &item)?;
        tx.execute(
            "UPDATE award_import_items
             SET status = 'approved',
                 approved_at = COALESCE(approved_at, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             WHERE id = ?1",
            params![item.id],
        )?;
        let row = get_work_award_result_local(&tx, existing_id)?;
        tx.commit()?;
        return Ok(row);
    }

    tx.execute(
        "INSERT INTO work_award_results
            (work_id, person_id, award_body_id, award_edition_id, award_category_id,
             result_type, source_url, confidence, is_locked)
         VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
        params![
            work_id,
            item.award_body_id,
            edition_id,
            category_id,
            item.raw_result_type,
            item.raw_source_url,
            item.match_score,
        ],
    )?;
    let result_id = tx.last_insert_rowid();
    tx.execute(
        "UPDATE award_import_items
         SET status = 'approved',
             approved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        params![item.id],
    )?;
    let row = get_work_award_result_local(&tx, result_id)?;
    tx.commit()?;
    Ok(row)
}

fn fetch_import_item_for_approval(
    conn: &Connection,
    import_item_id: i64,
) -> Result<ImportItemForApproval> {
    conn.query_row(
        "SELECT
             aii.id,
             aij.award_body_id,
             aii.matched_award_category_id,
             aii.matched_work_id,
             aii.raw_year,
             aii.raw_result_type,
             aii.raw_source_url,
             aii.match_score
         FROM award_import_items aii
         JOIN award_import_jobs aij ON aij.id = aii.job_id
         WHERE aii.id = ?1",
        params![import_item_id],
        |row| {
            Ok(ImportItemForApproval {
                id: row.get(0)?,
                award_body_id: row.get(1)?,
                matched_award_category_id: row.get(2)?,
                matched_work_id: row.get(3)?,
                raw_year: row.get(4)?,
                raw_result_type: row.get(5)?,
                raw_source_url: row.get(6)?,
                match_score: row.get(7)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| anyhow!("import item not found"))
}

fn upsert_award_edition_local(conn: &Connection, award_body_id: i64, year: i64) -> Result<i64> {
    conn.execute(
        "INSERT OR IGNORE INTO award_editions (award_body_id, year) VALUES (?1, ?2)",
        params![award_body_id, year],
    )?;
    conn.query_row(
        "SELECT id FROM award_editions WHERE award_body_id = ?1 AND year = ?2",
        params![award_body_id, year],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn find_existing_award_result_local(
    conn: &Connection,
    work_id: i64,
    award_body_id: i64,
    award_category_id: i64,
) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT war.id
         FROM work_award_results war
         JOIN award_editions ae ON ae.id = war.award_edition_id
         WHERE war.work_id = ?1
           AND war.person_id IS NULL
           AND war.award_body_id = ?2
           AND war.award_category_id = ?3
         ORDER BY
           CASE war.result_type WHEN 'winner' THEN 0 WHEN 'nominee' THEN 1 ELSE 2 END,
           ae.year DESC,
           war.id ASC
         LIMIT 1",
        params![work_id, award_body_id, award_category_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn reconcile_existing_award_result_local(
    conn: &Connection,
    result_id: i64,
    incoming_edition_id: i64,
    item: &ImportItemForApproval,
) -> Result<()> {
    conn.execute(
        "UPDATE work_award_results
         SET award_edition_id = CASE
               WHEN (
                 SELECT incoming.year >= existing.year
                 FROM award_editions incoming
                 JOIN award_editions existing ON existing.id = work_award_results.award_edition_id
                 WHERE incoming.id = ?2
               )
               THEN ?2
               ELSE award_edition_id
             END,
             result_type = CASE
               WHEN ?3 = 'winner' OR result_type <> 'winner' THEN ?3
               ELSE result_type
             END,
             source_url = COALESCE(source_url, ?4),
             confidence = COALESCE(confidence, ?5)
         WHERE id = ?1",
        params![
            result_id,
            incoming_edition_id,
            item.raw_result_type,
            item.raw_source_url,
            item.match_score
        ],
    )?;
    Ok(())
}

fn get_work_award_result_local(
    conn: &Connection,
    result_id: i64,
) -> Result<crate::commands::awards::WorkAwardResultViewRow> {
    conn.query_row(
        "SELECT
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
         WHERE war.id = ?1",
        params![result_id],
        |row| {
            Ok(crate::commands::awards::WorkAwardResultViewRow {
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
        },
    )
    .map_err(Into::into)
}

fn search_works_for_award_match_inner(
    db: &DbState,
    query: &str,
    year: Option<i32>,
) -> Result<Vec<WorkSearchResult>> {
    let conn = db.0.lock().map_err(|err| anyhow!(err.to_string()))?;
    let like = format!("%{}%", query.trim());
    let mut stmt = conn.prepare(
        "SELECT
             w.id,
             w.title,
             w.original_title,
             COALESCE(w.release_year, w.year),
             CAST(w.tmdb_id AS TEXT),
             w.imdb_id,
             (
               SELECT s.root_path || CASE WHEN f.file_path IS NOT NULL THEN ' / ' || f.file_path ELSE '' END
               FROM work_parts wp
               JOIN files f ON f.id = wp.file_id
               JOIN sources s ON s.id = f.source_id
               WHERE wp.work_id = w.id
               ORDER BY wp.play_order ASC, f.id ASC
               LIMIT 1
             ) AS source_path
         FROM works w
         WHERE (w.title LIKE ?1 OR w.original_title LIKE ?1 OR w.title_guess LIKE ?1)
           AND (?2 IS NULL OR COALESCE(w.release_year, w.year) IS NULL OR ABS(COALESCE(w.release_year, w.year) - ?2) <= 2)
         ORDER BY
           CASE WHEN w.title = ?3 OR w.original_title = ?3 OR w.title_guess = ?3 THEN 0 ELSE 1 END,
           w.title ASC
         LIMIT 30",
    )?;
    let rows = stmt.query_map(params![like, year, query], |row| {
        Ok(WorkSearchResult {
            id: row.get(0)?,
            title: row.get(1)?,
            original_title: row.get(2)?,
            year: row.get(3)?,
            tmdb_id: row.get(4)?,
            imdb_id: row.get(5)?,
            source_path: row.get(6)?,
        })
    })?
    .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(qid: &str, result_type: ResultType, year: Option<i32>) -> WikidataFilmResult {
        WikidataFilmResult {
            film_qid: qid.to_string(),
            title_ja: None,
            title_en: None,
            result_type,
            year,
            imdb_id: None,
            tmdb_id: None,
            wikidata_url: None,
            award_qid: Some("Q102427".to_string()),
            award_name_ja: None,
            award_name_en: None,
        }
    }

    #[test]
    fn dedupe_prefers_winner() {
        let deduped = dedupe_winner_priority(vec![
            item("Q1", ResultType::Nominee, Some(2024)),
            item("Q1", ResultType::Winner, Some(2024)),
        ]);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].result_type, ResultType::Winner);
    }

    #[test]
    fn dedupe_collapses_award_year_duplicates() {
        let deduped = dedupe_winner_priority(vec![
            item("Q1", ResultType::Winner, Some(2018)),
            item("Q1", ResultType::Winner, Some(2019)),
            item("Q1", ResultType::Nominee, Some(2020)),
        ]);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].result_type, ResultType::Winner);
        assert_eq!(deduped[0].year, Some(2019));
    }

    #[test]
    fn sparql_category_contains_award_and_props() {
        let query = build_sparql_category("Q102427", Some(2026));
        assert!(query.contains("wd:Q102427"));
        assert!(query.contains("p:P166"));
        assert!(query.contains("p:P1411"));
        assert!(query.contains("wd:Q11424"));
        assert!(query.contains("pq:P1686"));
        assert!(query.contains("VALUES ?award { wd:Q102427 }"));
        assert!(query.contains("YEAR(?date) = 2026"));
    }

    fn import_item(
        tmdb_id: Option<&str>,
        imdb_id: Option<&str>,
        year: Option<i32>,
        title_ja: Option<&str>,
        title_en: Option<&str>,
    ) -> ImportItemForMatching {
        ImportItemForMatching {
            id: 1,
            raw_title_ja: title_ja.map(str::to_string),
            raw_title_en: title_en.map(str::to_string),
            raw_imdb_id: imdb_id.map(str::to_string),
            raw_tmdb_id: tmdb_id.map(str::to_string),
            raw_year: year,
        }
    }

    fn work(
        title: &str,
        original_title: Option<&str>,
        year: Option<i32>,
        tmdb_id: Option<i64>,
        imdb_id: Option<&str>,
    ) -> WorkForMatching {
        WorkForMatching {
            id: 10,
            title: title.to_string(),
            original_title: original_title.map(str::to_string),
            title_guess: None,
            release_year: year,
            tmdb_id,
            imdb_id: imdb_id.map(str::to_string),
        }
    }

    #[test]
    fn score_prefers_tmdb_id() {
        let item = import_item(Some("872585"), None, Some(2024), None, None);
        let work = work("オッペンハイマー", Some("Oppenheimer"), Some(2023), Some(872585), None);
        let scored = score_single_candidate(&item, &work).unwrap();
        assert_eq!(scored.score, 100.0);
        assert_eq!(scored.match_method, "tmdb_id");
    }

    #[test]
    fn score_imdb_id_ignores_tt_prefix_difference() {
        let item = import_item(None, Some("tt15398776"), Some(2024), None, None);
        let work = work("オッペンハイマー", Some("Oppenheimer"), Some(2023), None, Some("15398776"));
        let scored = score_single_candidate(&item, &work).unwrap();
        assert_eq!(scored.score, 98.0);
        assert_eq!(scored.match_method, "imdb_id");
    }

    #[test]
    fn score_title_exact_with_year_bonus() {
        let item = import_item(None, None, Some(2024), None, Some("Oppenheimer"));
        let work = work("Oppenheimer", Some("Oppenheimer"), Some(2024), None, None);
        let scored = score_single_candidate(&item, &work).unwrap();
        assert!(scored.score >= 90.0);
        assert_eq!(scored.match_method, "title_exact");
    }
}
