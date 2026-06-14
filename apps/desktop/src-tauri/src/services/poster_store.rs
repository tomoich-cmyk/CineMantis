use crate::cache::CacheDir;
use crate::services::tmdb_client::TmdbClient;
use tauri::AppHandle;

/// TMDb poster_path（例: "/abc123.jpg"）をダウンロードして
/// cache/posters/{work_id}-{tmdb_file_name} に保存し、ローカルパスを返す
pub async fn store_poster_for_work(
    app: &AppHandle,
    client: &TmdbClient,
    work_id: i64,
    remote_poster_path: &str,
) -> Option<String> {
    let cache = CacheDir::new(app).ok()?;
    let remote_name = std::path::Path::new(remote_poster_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("poster.jpg");
    let out_path = cache.posters.join(format!("{work_id}-{remote_name}"));

    // ダウンロード
    let bytes = client.download_poster(remote_poster_path).await.ok()?;
    if bytes.is_empty() {
        return None;
    }

    // 候補変更時に asset URL も変わるよう、古い候補のポスターを除去する。
    if let Ok(entries) = std::fs::read_dir(&cache.posters) {
        let prefix = format!("{work_id}-");
        for entry in entries.flatten() {
            let path = entry.path();
            let is_old = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix));
            if is_old && path != out_path {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    std::fs::write(&out_path, &bytes).ok()?;

    Some(out_path.to_string_lossy().to_string())
}

/// poster キャッシュを削除（紐付け解除時）
pub fn delete_poster_for_work(app: &AppHandle, work_id: i64) {
    if let Ok(cache) = CacheDir::new(app) {
        let _ = std::fs::remove_file(cache.poster_path(work_id));
        if let Ok(entries) = std::fs::read_dir(&cache.posters) {
            let prefix = format!("{work_id}-");
            for entry in entries.flatten() {
                let path = entry.path();
                let matches = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&prefix));
                if matches {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
}
