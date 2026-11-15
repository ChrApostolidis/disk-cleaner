mod commands;
mod data;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::start_scan,
            commands::cancel_scan,
            commands::get_summary,
            commands::list_projects,
            commands::list_files,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
