mod cache;
mod db;
mod models;
mod services;
mod commands;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let app_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_dir)?;
            let db_path = app_dir.join("cinemantis.db");
            db::init(&db_path)?;
            app.manage(db::DbState::new(&db_path)?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Sources
            commands::source::list_sources,
            commands::source::add_source,
            commands::source::update_source_status,
            // Works
            commands::work::list_works,
            commands::work::get_work,
            // User stats
            commands::stats::update_user_stats,
            commands::stats::get_user_stats,
            commands::stats::record_play,
            // Tags
            commands::tag::list_tags,
            commands::tag::add_tag,
            commands::tag::tag_work,
            commands::tag::untag_work,
            commands::tag::list_work_tags,
            // Scan
            commands::scan::scan_source,
            // Thumbnails
            commands::thumbnail::generate_thumbnail,
            commands::thumbnail::generate_thumbnails_batch,
            commands::thumbnail::set_custom_thumb,
            // TMDb
            commands::tmdb::search_tmdb_candidates,
            commands::tmdb::auto_match_work,
            commands::tmdb::auto_match_source,
            commands::tmdb::apply_tmdb_match,
            commands::tmdb::refresh_tmdb_metadata,
            commands::tmdb::clear_tmdb_match,
            commands::tmdb::unlock_tmdb_match,
            // Settings
            commands::settings::get_setting,
            commands::settings::set_setting,
            commands::settings::get_tmdb_api_key_masked,
        ])
        .run(tauri::generate_context!())
        .expect("error while running CineMantis");
}
