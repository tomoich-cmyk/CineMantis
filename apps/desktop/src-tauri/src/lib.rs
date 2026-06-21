mod cache;
mod commands;
mod db;
mod ffmpeg_path;
mod models;
mod services;

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

            // pending restore があれば先にスワップ
            let pending_restore = app_dir.join("cinemantis_restore_pending.db");
            if pending_restore.exists() {
                let _ = std::fs::copy(&pending_restore, &db_path);
                let _ = std::fs::remove_file(&pending_restore);
            }

            db::init(&db_path)?;
            app.manage(db::DbState::new(&db_path)?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Sources
            commands::source::list_sources,
            commands::source::add_source,
            commands::source::update_source_status,
            commands::source::delete_source,
            commands::source::deduplicate_library_files,
            // Works
            commands::work::list_works,
            commands::work::get_work,
            commands::work::get_filter_options,
            commands::work::update_work_library_fields,
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
            commands::tmdb::test_tmdb_api,
            commands::tmdb::apply_tmdb_match,
            commands::tmdb::refresh_tmdb_metadata,
            commands::tmdb::clear_tmdb_match,
            commands::tmdb::unlock_tmdb_match,
            // Persons
            commands::persons::list_persons,
            commands::persons::get_person,
            commands::persons::get_work_persons,
            commands::persons::get_person_works,
            // Series
            commands::series::list_series,
            commands::series::get_series,
            commands::series::get_series_works,
            commands::series::create_series,
            commands::series::add_to_series,
            commands::series::remove_from_series,
            commands::series::delete_series,
            // Backup
            commands::backup::backup_database,
            commands::backup::list_backups,
            commands::backup::restore_database,
            commands::backup::delete_backup,
            // Bulk
            commands::bulk::get_attention_stats,
            commands::bulk::bulk_set_watch_status,
            commands::bulk::bulk_set_favorite,
            commands::bulk::bulk_add_tag,
            commands::bulk::bulk_remove_tag,
            commands::bulk::bulk_set_match_status,
            // Watch
            commands::watch::open_work_file,
            commands::watch::set_watch_status,
            commands::watch::update_resume_position,
            // Settings
            commands::settings::get_setting,
            commands::settings::set_setting,
            commands::settings::get_tmdb_api_key_masked,
            // Audit
            commands::audit::get_duplicate_groups,
            commands::audit::get_integrity_report,
            // Work (delete)
            commands::work::delete_work,
            commands::work::delete_work_files,
            // TMDb repair
            commands::tmdb::repair_fetch_persons,
            commands::tmdb::repair_fetch_all_persons,
            commands::tmdb::localize_person_names,
            commands::tmdb::repair_fetch_all_movie_collections,
            // Platform
            commands::platform::create_desktop_shortcut,
            // Awards
            commands::awards::list_award_bodies,
            commands::awards::get_award_body_detail,
            commands::awards::list_award_categories,
            commands::awards::get_or_create_award_edition,
            commands::awards::list_work_awards,
            commands::awards::add_work_award_result,
            commands::awards::update_work_award_result,
            commands::awards::delete_work_award_result,
            commands::awards::list_award_winning_works,
            commands::award_import::fetch_wikidata_award_items,
            commands::award_import::match_award_import_items,
            commands::award_import::list_award_import_items,
            commands::award_import::select_award_match_candidate,
            commands::award_import::approve_award_import_item,
            commands::award_import::reject_award_import_item,
            commands::award_import::search_works_for_award_match,
            commands::award_import::add_manual_award_match_candidate,
            commands::award_import::bulk_approve_award_import_items,
            commands::award_import::bulk_reject_award_import_items,
        ])
        .run(tauri::generate_context!())
        .expect("error while running CineMantis");
}
