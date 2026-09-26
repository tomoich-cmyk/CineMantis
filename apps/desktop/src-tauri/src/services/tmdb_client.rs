use crate::models::tmdb::*;
use reqwest::{Client, Response};

pub const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/w500";
const TMDB_API_BASE: &str = "https://api.themoviedb.org/3";

/// 1 回の論理的な呼び出し（search_movie など）で送り得る HTTP の最大回数。
///
/// 失敗したときに同じ URL を送り直すので、**論理 1 回 = HTTP 最大 3 回**になる。
/// 予算の見積もりでこの値を使うため、retry ループと同じ定数を共有する
/// （production の再試行の挙動自体は変えない）。
pub const MAX_HTTP_ATTEMPTS: usize = 3;

// ─── クライアント ─────────────────────────────────────────────────────────────

pub struct TmdbClient {
    client: Client,
    api_key: String,
}

impl TmdbClient {
    pub fn new(api_key: String) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { client, api_key }
    }

    // ─── 検索 ──────────────────────────────────────────────────────────────

    pub async fn search_movie(
        &self,
        query: &str,
        year: Option<i32>,
    ) -> Result<Vec<TmdbSearchMovie>, String> {
        let mut url = format!(
            "{TMDB_API_BASE}/search/movie?api_key={}&query={}&language=ja-JP&include_adult=false",
            self.api_key,
            urlencoding(query)
        );
        if let Some(y) = year {
            url.push_str(&format!("&year={y}"));
        }

        let data: TmdbSearchResponse<TmdbSearchMovie> = self.fetch_json(&url).await?;
        Ok(data.results)
    }

    pub async fn search_tv(
        &self,
        query: &str,
        first_air_year: Option<i32>,
    ) -> Result<Vec<TmdbSearchTv>, String> {
        let mut url = format!(
            "{TMDB_API_BASE}/search/tv?api_key={}&query={}&language=ja-JP&include_adult=false",
            self.api_key,
            urlencoding(query)
        );
        if let Some(y) = first_air_year {
            url.push_str(&format!("&first_air_date_year={y}"));
        }

        let data: TmdbSearchResponse<TmdbSearchTv> = self.fetch_json(&url).await?;
        Ok(data.results)
    }

    // ─── 詳細取得 ─────────────────────────────────────────────────────────

    /// 汎用 JSON GET ヘルパー
    async fn fetch_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, String> {
        let resp = self.send_with_retry(url).await?;
        if !resp.status().is_success() {
            return Err(format!("TMDb HTTP {}", resp.status()));
        }
        resp.json::<T>()
            .await
            .map_err(|_| "TMDb response could not be decoded".to_string())
    }

    async fn send_with_retry(&self, url: &str) -> Result<Response, String> {
        let mut last_kind = "request failed";
        for attempt in 0..MAX_HTTP_ATTEMPTS {
            match self.client.get(url).send().await {
                Ok(response) => return Ok(response),
                Err(error) => {
                    last_kind = if error.is_timeout() {
                        "connection timed out"
                    } else if error.is_connect() {
                        "connection could not be established"
                    } else if error.is_request() {
                        "connection was interrupted"
                    } else {
                        "request failed"
                    };
                    if attempt + 1 < MAX_HTTP_ATTEMPTS {
                        tokio::time::sleep(std::time::Duration::from_millis(
                            400 * (attempt + 1) as u64,
                        ))
                        .await;
                    }
                }
            }
        }
        Err(format!("TMDb {last_kind} after {MAX_HTTP_ATTEMPTS} attempts"))
    }

    pub async fn get_movie_detail(&self, tmdb_id: i64) -> Result<TmdbMovieDetail, String> {
        let url = format!(
            "{TMDB_API_BASE}/movie/{tmdb_id}?api_key={}&language=ja-JP",
            self.api_key
        );
        self.fetch_json(&url).await
    }

    pub async fn get_tv_detail(&self, tmdb_id: i64) -> Result<TmdbTvDetail, String> {
        let url = format!(
            "{TMDB_API_BASE}/tv/{tmdb_id}?api_key={}&language=ja-JP",
            self.api_key
        );
        self.fetch_json(&url).await
    }

    // ─── Credits ──────────────────────────────────────────────────────────

    pub async fn get_movie_credits(&self, tmdb_id: i64) -> Result<TmdbCredits, String> {
        let url = format!(
            "{TMDB_API_BASE}/movie/{tmdb_id}/credits?api_key={}&language=ja-JP",
            self.api_key
        );
        self.fetch_json(&url).await
    }

    pub async fn get_tv_credits(&self, tmdb_id: i64) -> Result<TmdbCredits, String> {
        let url = format!(
            "{TMDB_API_BASE}/tv/{tmdb_id}/credits?api_key={}&language=ja-JP",
            self.api_key
        );
        self.fetch_json(&url).await
    }

    pub async fn get_person_detail(&self, tmdb_id: i64) -> Result<TmdbPersonDetail, String> {
        let url = format!(
            "{TMDB_API_BASE}/person/{tmdb_id}?api_key={}&language=ja-JP",
            self.api_key
        );
        self.fetch_json(&url).await
    }

    // ─── 画像ダウンロード ──────────────────────────────────────────────────

    /// poster_path = "/abc123.jpg"（TMDb relative）
    pub async fn download_poster(&self, poster_path: &str) -> Result<Vec<u8>, String> {
        let url = format!("{TMDB_IMAGE_BASE}{poster_path}");
        let resp = self.send_with_retry(&url).await?;
        if !resp.status().is_success() {
            return Err(format!("poster download HTTP {}", resp.status()));
        }
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|_| "poster response could not be read".to_string())
    }
}

// ─── ユーティリティ ───────────────────────────────────────────────────────────

fn urlencoding(s: &str) -> String {
    // 簡易 percent-encoding（非ASCII・スペース対応）
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".to_string(),
            c => {
                let mut buf = [0u8; 4];
                let bytes = c.encode_utf8(&mut buf);
                bytes
                    .bytes()
                    .map(|b| format!("%{b:02X}"))
                    .collect::<String>()
            }
        })
        .collect()
}
