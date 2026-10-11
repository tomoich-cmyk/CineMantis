mod cache;
mod commands;
mod db;
mod ffmpeg_path;
#[cfg(test)]
mod integration_tests;
mod migrate;
mod models;
mod restore;
mod services;
mod startup;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    startup::install_panic_hook();
    let run_result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let app_dir = match app.path().app_data_dir() {
                Ok(dir) => dir,
                Err(e) => {
                    return Err(report_fatal(
                        &startup::app_dir_or_temp(),
                        "app_data_dir を取得できません",
                        &e.to_string(),
                        "アプリのデータフォルダを取得できなかったため、起動できませんでした。",
                    ));
                }
            };
            if let Err(e) = std::fs::create_dir_all(&app_dir) {
                return Err(report_fatal(
                    &app_dir,
                    "app_data_dir を作成できません",
                    &e.to_string(),
                    "アプリのデータフォルダを作成できなかったため、起動できませんでした。",
                ));
            }
            startup::set_app_dir(&app_dir);
            let db_path = app_dir.join("cinemantis.db");

            // 1) pending restore があれば先に適用（R1A: 検証・退避・rollback 付き。失敗しても現在 DB で起動継続）。
            //    migration より前、同一 DB に対して直列。結果は restore_last_result.json に記録。
            let outcome = restore::apply_pending_restore(&app_dir);
            restore::record_outcome(&app_dir, &outcome);
            match &outcome {
                restore::RestoreOutcome::NoPending => {}
                restore::RestoreOutcome::Applied { .. } | restore::RestoreOutcome::RecoveredOnly { .. } => {
                    startup::log_event(&app_dir, "INFO", &format!("復元処理: {outcome:?}"));
                }
                restore::RestoreOutcome::Failed { .. } => {
                    startup::log_event(
                        &app_dir,
                        "ERROR",
                        &format!("バックアップからの復元に失敗（現在の DB で起動を継続）: {outcome:?}"),
                    );
                    startup::fatal_box("CineMantis", &startup::user_message_for_restore_failure(&app_dir));
                }
            }

            // 2) restore 後の DB に対して migration backup → migration → schema verification（R1B）
            match db::init(&db_path) {
                Ok(report) => {
                    startup::log_event(
                        &app_dir,
                        "INFO",
                        &format!(
                            "DB 準備完了: schema v{} -> v{} / 新規={} / 適用前 backup={:?}",
                            report.from_version, report.to_version, report.fresh, report.backup_path
                        ),
                    );
                    startup::log_event(
                        &app_dir,
                        "INFO",
                        &format!(
                            "migration 開始版 v{}（根拠: {}）/ 実行した段: {:?} / うちデータ更新を伴う段: {:?}",
                            report.start_version, report.start_basis, report.executed, report.data_steps_run
                        ),
                    );
                }
                Err(e) => {
                    let latest = migrate::find_latest_valid_backup(&migrate::backups_dir_for(&db_path));
                    startup::log_event(
                        &app_dir,
                        "ERROR",
                        &format!(
                            "migration 失敗: {} / step={:?} / backup={:?} / DB 未変更={}",
                            e.detail, e.failed_step, e.backup_path, e.db_unchanged
                        ),
                    );
                    let msg = startup::user_message_for_migration(&e, &app_dir, latest.as_deref());
                    startup::fatal_box("CineMantis - 起動できません", &msg);
                    return Err(Box::new(e));
                }
            }
            match db::DbState::new(&db_path) {
                Ok(state) => {
                    app.manage(state);
                }
                Err(e) => {
                    return Err(report_fatal(
                        &app_dir,
                        "DB 接続を開けません",
                        &format!("{e:#}"),
                        "データベースを開けなかったため、起動できませんでした。",
                    ));
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Sources
            commands::source::list_sources,
            commands::source::add_source,
            commands::source::update_source_status,
            commands::source::delete_source,
            commands::source::deduplicate_library_files,
            commands::sync::list_sync_outbox,
            commands::sync::process_sync_outbox,
            // NAS 振り分け
            commands::nas_sort::plan_nas_sort,
            commands::nas_sort::execute_nas_sort,
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
            // 埋め込みメタデータ（PR2.5）
            commands::tags::backfill_container_tags,
            commands::tags::container_tag_coverage,
            commands::tags::scan_metadata_conflicts_command,
            // Thumbnails
            commands::thumbnail::generate_thumbnail,
            commands::thumbnail::generate_thumbnails_batch,
            commands::thumbnail::set_custom_thumb,
            // TMDb
            commands::review::list_gt_review_tasks,
            commands::review::get_gt_review_task,
            commands::review::defer_gt_review_task,
            commands::review::resume_gt_review_task,
            commands::review::preview_gt_review_tmdb_target,
            commands::review::resolve_gt_review_confirm,
            commands::review::resolve_gt_review_pick_other,
            commands::review::resolve_gt_review_none,
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
            commands::backup::get_last_restore_result,
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
        .run(tauri::generate_context!());
    if let Err(e) = run_result {
        // setup 内の失敗は既に通知済み。ここに来るのは WebView2 不在など setup 前後の失敗。
        let dir = startup::app_dir_or_temp();
        startup::log_event(&dir, "ERROR", &format!("アプリの実行に失敗: {e}"));
        startup::fatal_box(
            "CineMantis - 起動できません",
            &format!(
                "CineMantis を起動できませんでした。
詳細は次のログをご確認ください:
{}",
                startup::log_path(&dir).display()
            ),
        );
        std::process::exit(1);
    }
}

/// 致命的な起動失敗を log とダイアログに出し、setup から返すエラーを作る
fn report_fatal(app_dir: &std::path::Path, what: &str, detail: &str, user_msg: &str) -> Box<dyn std::error::Error> {
    startup::log_event(app_dir, "ERROR", &format!("{what}: {detail}"));
    startup::fatal_box(
        "CineMantis - 起動できません",
        &format!("{user_msg}

詳細は次のログをご確認ください:
{}", startup::log_path(app_dir).display()),
    );
    format!("{what}: {detail}").into()
}
