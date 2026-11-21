// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) = disk_cleaner_lib::run_helper_if_asked() {
        std::process::exit(code);
    }
    disk_cleaner_lib::run()
}
