use crate::models::tmdb::*;
use reqwest::Client;

pub const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/w500";
const TMDB_API_BASE: &str = "https://api.themoviedb.org/3";

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

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            return Err(format!("TMDb HTTP {}", resp.status()));
        }

        let data: TmdbSearchResponse<TmdbSearchMovie> =
            resp.json().await.map_err(|e| e.to_string())?;
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

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            return Err(format!("TMDb HTTP {}", resp.status()));
        }

        let data: TmdbSearchResponse<TmdbSearchTv> =
            resp.json().await.map_err(|e| e.to_string())?;
        Ok(data.results)
    }

    // ─── 詳細取得 ─────────────────────────────────────────────────────────

    /// 汎用 JSON GET ヘルパー
    async fn fetch_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, String> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("TMDb HTTP {}", resp.status()));
        }
        resp.json::<T>().await.map_err(|e| e.to_string())
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

    // ─── 画像ダウンロード ──────────────────────────────────────────────────

    /// poster_path = "/abc123.jpg"（TMDb relative）
    pub async fn download_poster(&self, poster_path: &str) -> Result<Vec<u8>, String> {
        let url = format!("{TMDB_IMAGE_BASE}{poster_path}");
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("poster download HTTP {}", resp.status()));
        }
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| e.to_string())
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
