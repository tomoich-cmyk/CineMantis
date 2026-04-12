use crate::cache::CacheDir;
use crate::services::tmdb_client::TmdbClient;
use tauri::AppHandle;

/// TMDb poster_path（例: "/abc123.jpg"）をダウンロードして
/// cache/posters/{work_id}.jpg に保存し、ローカルパスを返す
pub async fn store_poster_for_work(
    app: &AppHandle,
    client: &TmdbClient,
    work_id: i64,
    remote_poster_path: &str,
) -> Option<String> {
    let cache = CacheDir::new(app).ok()?;
    let out_path = cache.poster_path(work_id);

    // ダウンロード
    let bytes = client.download_poster(remote_poster_path).await.ok()?;
    if bytes.is_empty() {
        return None;
    }

    // 書き込み
    std::fs::write(&out_path, &bytes).ok()?;

    Some(out_path.to_string_lossy().to_string())
}

/// poster キャッシュを削除（紐付け解除時）
pub fn delete_poster_for_work(app: &AppHandle, work_id: i64) {
    if let Ok(cache) = CacheDir::new(app) {
        let _ = std::fs::remove_file(cache.poster_path(work_id));
    }
}
