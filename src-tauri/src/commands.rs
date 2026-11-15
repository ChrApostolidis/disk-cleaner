//! Thin Tauri wrappers around `data`. No logic lives here beyond state and threading.

use crate::data::{
    FileEntryDto, ProjectDto, ScanData, ScanErrorDto, ScanSummaryDto, error_dto, progress_dto,
    query_files, query_projects, run_scan,
};
use devdisk_core::scanner::{Progress, ProgressSink};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;
use tauri::{AppHandle, Emitter, Manager, State};

pub const EVENT_PROGRESS: &str = "scan-progress";
pub const EVENT_DONE: &str = "scan-done";
pub const EVENT_ERROR: &str = "scan-error";

/// Mutexes are only held for a pointer swap, never across a scan.
#[derive(Default)]
pub struct AppState {
    /// Last finished scan; replaced only when a new one succeeds.
    data: Mutex<Option<Arc<ScanData>>>,
    /// Cancel flag of the running scan; `Some` means a scan is in flight.
    running: Mutex<Option<Arc<AtomicBool>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panicking scan thread must not wedge the app behind a poisoned lock.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AppState {
    fn current(&self) -> Option<Arc<ScanData>> {
        lock(&self.data).clone()
    }
}

struct EmitSink(AppHandle);

impl ProgressSink for EmitSink {
    fn report(&self, p: &Progress) {
        let _ = self.0.emit(EVENT_PROGRESS, progress_dto(p));
    }
}

/// Clears the running flag on every exit path of the scan thread, including panics.
struct RunningGuard(AppHandle);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        *lock(&self.0.state::<AppState>().running) = None;
    }
}

fn scan_thread(app: AppHandle, root: PathBuf, cancel: Arc<AtomicBool>) {
    let guard = RunningGuard(app.clone());
    let sink = EmitSink(app.clone());
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_scan(&root, cancel, &sink)
    }));
    // Release the running flag before telling the UI, so a rescan started right
    // after `scan-done` is not rejected as "already running".
    drop(guard);
    match outcome {
        Ok(Ok(data)) => {
            let summary = data.summary.clone();
            *lock(&app.state::<AppState>().data) = Some(Arc::new(data));
            let _ = app.emit(EVENT_DONE, summary);
        }
        Ok(Err(e)) => {
            let _ = app.emit(EVENT_ERROR, error_dto(&e));
        }
        Err(_) => {
            let _ = app.emit(
                EVENT_ERROR,
                ScanErrorDto {
                    message: "the scan crashed unexpectedly".into(),
                    cancelled: false,
                },
            );
        }
    }
}

#[tauri::command]
pub async fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    root: String,
) -> Result<(), String> {
    let path = PathBuf::from(&root);
    if !path.is_dir() {
        return Err(format!("not a folder: {root}"));
    }
    let path = std::path::absolute(&path).map_err(|e| e.to_string())?;

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut running = lock(&state.running);
        if running.is_some() {
            return Err("a scan is already running".into());
        }
        *running = Some(cancel.clone());
    }
    let thread_app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("devdisk-scan".into())
        .spawn(move || scan_thread(thread_app, path, cancel));
    if let Err(e) = spawned {
        *lock(&state.running) = None;
        return Err(format!("could not start the scan: {e}"));
    }
    Ok(())
}

#[tauri::command]
pub async fn cancel_scan(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(flag) = lock(&state.running).as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
    Ok(())
}

#[tauri::command]
pub async fn get_summary(state: State<'_, AppState>) -> Result<Option<ScanSummaryDto>, String> {
    Ok(state.current().map(|d| d.summary.clone()))
}

#[tauri::command]
pub async fn list_projects(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<ProjectDto>, String> {
    match state.current() {
        Some(data) => query_projects(&data, &query, SystemTime::now()),
        None => Ok(Vec::new()),
    }
}

#[tauri::command]
pub async fn list_files(
    state: State<'_, AppState>,
    query: String,
    sort: String,
    descending: bool,
    limit: usize,
) -> Result<Vec<FileEntryDto>, String> {
    match state.current() {
        Some(data) => query_files(&data, &query, &sort, descending, limit, SystemTime::now()),
        None => Ok(Vec::new()),
    }
}
