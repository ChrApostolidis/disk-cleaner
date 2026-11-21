mod commands;
mod data;
mod fastscan;

/// When the app was started as the fast-scan helper (an elevated copy of itself), runs the
/// scan and returns the exit code instead of opening a window.
pub fn run_helper_if_asked() -> Option<i32> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let (root, job) = fastscan::parse_helper_args(&args)?;
    Some(fastscan::run_helper(
        &devdisk_core::mft::MftScanner,
        &root,
        &fastscan::Job::new(job),
    ))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::AppState::default())
        .setup(|app| {
            commands::load_saved_scan(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::start_scan,
            commands::cancel_scan,
            commands::get_summary,
            commands::list_projects,
            commands::list_files,
            commands::get_volume,
            commands::get_treemap,
            commands::preview_cleanup,
            commands::start_cleanup,
            commands::list_history,
            commands::restore_entries,
            commands::start_reinstall,
            commands::get_recycle_bin,
            commands::list_recycle_bin,
            commands::empty_recycle_bin,
            commands::list_apps,
            commands::list_dev_tools,
            commands::start_duplicates,
            commands::cancel_duplicates,
            commands::get_duplicates,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
