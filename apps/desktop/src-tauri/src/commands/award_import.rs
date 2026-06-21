use crate::db::DbState;
use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
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

        if import_item_exists(db, item, award_category_id)? {
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
        conn.query_row(
            "SELECT wikidata_entity_id FROM award_categories WHERE id = ?1 AND award_body_id = ?2",
            params![category_id, award_body_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .filter(|qid| !qid.trim().is_empty())
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
    let year_filter = year
        .map(|year| format!("FILTER(YEAR(?date) = {year})"))
        .unwrap_or_default();

    format!(
        r#"
SELECT DISTINCT
  ?film
  (SAMPLE(?titleJa) AS ?titleJa)
  (SAMPLE(?titleEn) AS ?titleEn)
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
  ?stmt ?propS wd:{category_qid} .
  OPTIONAL {{
    ?stmt pq:P585 ?date .
    BIND(YEAR(?date) AS ?year)
    {year_filter}
  }}
  OPTIONAL {{ ?film wdt:P345 ?imdbId }}
  OPTIONAL {{ ?film wdt:P4947 ?tmdbId }}
  OPTIONAL {{ ?film rdfs:label ?titleJa . FILTER(LANG(?titleJa) = "ja") }}
  OPTIONAL {{ ?film rdfs:label ?titleEn . FILTER(LANG(?titleEn) = "en") }}
}}
GROUP BY ?film ?resultType ?year ?imdbId ?tmdbId
ORDER BY DESC(?year) ?resultType
"#
    )
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
        title_en: binding.title_en.as_ref().map(|value| value.value.clone()),
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
    let mut map: HashMap<(String, String, Option<i32>), WikidataFilmResult> = HashMap::new();

    for item in items {
        let award_key = item
            .award_qid
            .clone()
            .unwrap_or_else(|| "category".to_string());
        let key = (item.film_qid.clone(), award_key, item.year);

        match map.get(&key) {
            Some(existing) if existing.result_type.priority() >= item.result_type.priority() => {}
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

fn import_item_exists(
    db: &DbState,
    item: &WikidataFilmResult,
    award_category_id: Option<i64>,
) -> Result<bool> {
    let conn = db.0.lock().map_err(|err| anyhow!(err.to_string()))?;
    let count: i64 = conn.query_row(
        "SELECT COUNT(*)
         FROM award_import_items
         WHERE raw_film_id = ?1
           AND COALESCE(matched_award_category_id, -1) = COALESCE(?2, -1)
           AND COALESCE(raw_year, -1) = COALESCE(?3, -1)
           AND status <> 'error'",
        params![item.film_qid, award_category_id, item.year],
        |row| row.get(0),
    )?;
    Ok(count > 0)
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
           AND COALESCE(raw_year, -1) = COALESCE(?3, -1)
           AND raw_result_type = 'nominee'
           AND status = 'pending'",
        params![item.film_qid, award_category_id, item.year],
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
    fn sparql_category_contains_award_and_props() {
        let query = build_sparql_category("Q102427", Some(2026));
        assert!(query.contains("wd:Q102427"));
        assert!(query.contains("p:P166"));
        assert!(query.contains("p:P1411"));
        assert!(query.contains("wd:Q11424"));
        assert!(query.contains("YEAR(?date) = 2026"));
    }
}
