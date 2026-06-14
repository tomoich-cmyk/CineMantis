use serde::{Deserialize, Serialize};

// ─── TMDb API レスポンス型 ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct TmdbSearchMovie {
    pub id: i64,
    pub title: String,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub release_date: Option<String>,
    pub poster_path: Option<String>,
    pub original_language: Option<String>,
    pub vote_average: Option<f64>,
    pub genre_ids: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TmdbSearchTv {
    pub id: i64,
    pub name: String,
    pub original_name: Option<String>,
    pub overview: Option<String>,
    pub first_air_date: Option<String>,
    pub poster_path: Option<String>,
    pub original_language: Option<String>,
    pub vote_average: Option<f64>,
    pub genre_ids: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize)]
pub struct TmdbSearchResponse<T> {
    pub results: Vec<T>,
    pub total_results: Option<i64>,
}

/// 映画詳細
#[derive(Debug, Deserialize, Clone)]
pub struct TmdbMovieDetail {
    pub id: i64,
    pub title: String,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub release_date: Option<String>,
    pub poster_path: Option<String>,
    pub runtime: Option<i64>,
    pub vote_average: Option<f64>,
    pub genres: Option<Vec<TmdbGenre>>,
    pub production_countries: Option<Vec<TmdbCountry>>,
    pub imdb_id: Option<String>,
    pub belongs_to_collection: Option<TmdbCollection>,
}

/// TVシリーズ詳細
#[derive(Debug, Deserialize, Clone)]
pub struct TmdbTvDetail {
    pub id: i64,
    pub name: String,
    pub original_name: Option<String>,
    pub overview: Option<String>,
    pub first_air_date: Option<String>,
    pub poster_path: Option<String>,
    pub episode_run_time: Option<Vec<i64>>,
    pub vote_average: Option<f64>,
    pub genres: Option<Vec<TmdbGenre>>,
    pub origin_country: Option<Vec<String>>,
    pub number_of_seasons: Option<i64>,
    pub number_of_episodes: Option<i64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TmdbGenre {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TmdbCountry {
    pub iso_3166_1: String,
    pub name: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TmdbCollection {
    pub id: i64,
    pub name: String,
    pub poster_path: Option<String>,
}

/// Credits（映画・TV共通）
#[derive(Debug, Deserialize, Clone)]
pub struct TmdbCredits {
    pub cast: Vec<TmdbCastMember>,
    pub crew: Vec<TmdbCrewMember>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TmdbCastMember {
    pub id: i64,
    pub name: String,
    pub character: Option<String>,
    pub order: Option<i32>,
    pub profile_path: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TmdbCrewMember {
    pub id: i64,
    pub name: String,
    pub job: String,
    pub department: Option<String>,
    pub profile_path: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TmdbPersonDetail {
    pub name: String,
    #[serde(default)]
    pub also_known_as: Vec<String>,
}

// ─── アプリ内ビュー型（Tauri command が返す） ──────────────────────────────────

/// 候補1件。スコアリング済み
#[derive(Debug, Serialize, Clone)]
pub struct TmdbCandidate {
    pub tmdb_id: i64,
    pub media_type: String, // "movie" | "tv"
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub poster_path: Option<String>, // TMDb 相対パス (/abc.jpg)
    pub overview: Option<String>,
    pub confidence: i32, // 0–100
    pub reasons: Vec<String>,
}

/// 自動照合1件の結果
#[derive(Debug, Serialize, Clone)]
pub struct AutoMatchResult {
    pub work_id: i64,
    pub matched: bool,
    pub status: String, // "matched" | "manual" | "locked" | "unmatched"
    pub confidence: i32,
    pub tmdb_id: Option<i64>,
    pub title: Option<String>,
    pub poster_local_path: Option<String>,
}

/// バッチ照合の進捗イベント
#[derive(Debug, Serialize, Clone)]
pub struct MetadataBatchProgress {
    pub source_id: Option<i64>,
    pub processed: usize,
    pub total: usize,
    pub matched: usize,
    pub skipped: usize,
    pub failed: usize,
    /// デバッグ用: 最初に発生したエラー or 候補なしの理由
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// metadata:updated イベントペイロード
#[derive(Debug, Serialize, Clone)]
pub struct MetadataUpdatedEvent {
    pub work_id: i64,
    pub status: String,
    pub poster_local_path: Option<String>,
    pub title: String,
    pub year: Option<i32>,
}
