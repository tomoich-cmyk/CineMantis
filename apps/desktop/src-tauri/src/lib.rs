mod db;
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running CineMantis");
}
