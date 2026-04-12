/// CineMantis ローカルキャッシュのパス管理
///
/// AppData/CineMantis/
///   cache/
///     thumbs/   {work_id}.jpg   — ffmpeg で動画から切り出したサムネイル
///     posters/  {work_id}.jpg   — TMDb から取得したポスター画像
///     actors/   {person_id}.jpg — 人物サムネ（v0.4以降）

use std::path::PathBuf;
use tauri::AppHandle;
use tauri::Manager;

pub struct CacheDir {
    pub root: PathBuf,
    pub thumbs: PathBuf,
    pub posters: PathBuf,
    pub actors: PathBuf,
}

impl CacheDir {
    pub fn new(app: &AppHandle) -> Result<Self, String> {
        let root = app
            .path()
            .app_data_dir()
            .map_err(|e| e.to_string())?
            .join("cache");

        let thumbs = root.join("thumbs");
        let posters = root.join("posters");
        let actors = root.join("actors");

        // Ensure directories exist
        for dir in [&thumbs, &posters, &actors] {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cache dir create failed: {e}"))?;
        }

        Ok(Self { root, thumbs, posters, actors })
    }

    pub fn thumb_path(&self, work_id: i64) -> PathBuf {
        self.thumbs.join(format!("{work_id}.jpg"))
    }

    pub fn poster_path(&self, work_id: i64) -> PathBuf {
        self.posters.join(format!("{work_id}.jpg"))
    }

    pub fn actor_path(&self, person_id: i64) -> PathBuf {
        self.actors.join(format!("{person_id}.jpg"))
    }
}
