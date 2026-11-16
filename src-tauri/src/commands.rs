//! Thin Tauri wrappers around `data`. No logic lives here beyond state and threading.

use crate::data::{
    CleanupDoneDto, CleanupProgressDto, CleanupResultItemDto, FileEntryDto, ProjectDto,
    ReinstallDoneDto, ReinstallOutputDto, RestoreResultDto, ScanData, ScanErrorDto, ScanSummaryDto,
    batch_id, check_can_clean, check_can_scan, error_dto, find_project_by_path, freed_bytes,
    progress_dto, query_files, query_projects, reinstall_offers, restore_dtos, result_items,
    run_cleanup, run_scan, treemap_at,
};
use devdisk_core::cleanup::{
    CleanupContext, CleanupError, CleanupLog, HistoryEntry, Preview, SystemBin, SystemEnvironment,
    TrashMover, preview, restore,
};
use devdisk_core::rules::Rules;
use devdisk_core::scanner::{Progress, ProgressSink};
use devdisk_core::treemap::TreemapNode;
use devdisk_core::volume::{VolumeInfo, volume_info};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;
use tauri::{AppHandle, Emitter, Manager, State};

pub const EVENT_PROGRESS: &str = "scan-progress";
pub const EVENT_DONE: &str = "scan-done";
pub const EVENT_ERROR: &str = "scan-error";
pub const EVENT_CLEANUP_PROGRESS: &str = "cleanup-progress";
pub const EVENT_CLEANUP_DONE: &str = "cleanup-done";
pub const EVENT_REINSTALL_OUTPUT: &str = "reinstall-output";
pub const EVENT_REINSTALL_DONE: &str = "reinstall-done";

/// Mutexes are only held for a pointer swap, never across a scan.
#[derive(Default)]
pub struct AppState {
    /// Last finished scan; replaced only when a new one succeeds.
    data: Mutex<Option<Arc<ScanData>>>,
    /// Cancel flag of the running scan; `Some` means a scan is in flight.
    running: Mutex<Option<Arc<AtomicBool>>>,
    /// True while a cleanup thread runs. Always locked after `running` when both are needed.
    cleaning: Mutex<bool>,
    /// Lowercased project paths with a reinstall in flight.
    reinstalling: Mutex<HashSet<String>>,
    /// Opened on first use; the connection is not `Sync`, so it lives behind the mutex.
    log: Mutex<Option<CleanupLog>>,
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
        check_can_scan(*lock(&state.cleaning))?;
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

#[tauri::command]
pub async fn get_volume(state: State<'_, AppState>) -> Result<Option<VolumeInfo>, String> {
    match state.current() {
        // Free space changes while the app runs, so this is never cached.
        Some(data) => volume_info(data.tree.root_path())
            .map(Some)
            .map_err(|e| format!("could not read the drive: {e}")),
        None => Ok(None),
    }
}

#[tauri::command]
pub async fn get_treemap(
    state: State<'_, AppState>,
    path: String,
    depth: u32,
) -> Result<TreemapNode, String> {
    let data = state.current().ok_or("no scan yet")?;
    treemap_at(&data, &path, depth)
}

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data folder: {e}"))?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Runs `f` with the cleanup log, opening it first if this is the first use.
fn with_log<T>(
    app: &AppHandle,
    f: impl FnOnce(&CleanupLog) -> Result<T, CleanupError>,
) -> Result<T, String> {
    let state = app.state::<AppState>();
    let mut slot = lock(&state.log);
    if slot.is_none() {
        let path = app_data_dir(app)?.join("cleanup-log.sqlite");
        *slot = Some(CleanupLog::open(&path).map_err(|e| e.to_string())?);
    }
    let log = slot.as_ref().expect("log was just opened");
    f(log).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn preview_cleanup(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<Preview, String> {
    let data = state.current().ok_or("no scan yet")?;
    let protected = [app_data_dir(&app)?];
    let rules = Rules::builtin();
    let ctx = CleanupContext {
        tree: &data.tree,
        projects: &data.projects,
        rules: &rules,
        protected: &protected,
    };
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    preview(&ctx, &paths, &SystemEnvironment).map_err(|e| e.to_string())
}

/// Clears the cleanup flag on every exit path of the cleanup thread, including panics.
struct CleaningGuard(AppHandle);

impl Drop for CleaningGuard {
    fn drop(&mut self) {
        *lock(&self.0.state::<AppState>().cleaning) = false;
    }
}

fn cleanup_thread(
    app: AppHandle,
    data: Arc<ScanData>,
    paths: Vec<PathBuf>,
    protected: Vec<PathBuf>,
    batch: String,
) {
    let guard = CleaningGuard(app.clone());
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // The log mutex is held for the whole run, which also keeps `list_history` from
        // reading a half-written batch.
        let state = app.state::<AppState>();
        let mut slot = lock(&state.log);
        if slot.is_none() {
            let path = app_data_dir(&app)?.join("cleanup-log.sqlite");
            *slot = Some(CleanupLog::open(&path).map_err(|e| e.to_string())?);
        }
        let log = slot.as_ref().expect("log was just opened");
        let mut progress = |done: usize, total: usize, current: &std::path::Path| {
            let _ = app.emit(
                EVENT_CLEANUP_PROGRESS,
                CleanupProgressDto {
                    batch_id: batch.clone(),
                    done: done as u64,
                    total: total as u64,
                    current_path: current.to_string_lossy().into_owned(),
                },
            );
        };
        run_cleanup(
            &data,
            &paths,
            &protected,
            &SystemEnvironment,
            &TrashMover,
            log,
            &batch,
            &mut progress,
        )
        .map_err(|e| e.to_string())
    }));
    let done = match outcome {
        Ok(Ok((outcomes, updated))) => {
            let reinstall = reinstall_offers(&data, &Rules::builtin(), &outcomes);
            *lock(&app.state::<AppState>().data) = Some(Arc::new(updated));
            CleanupDoneDto {
                batch_id: batch,
                freed: freed_bytes(&outcomes),
                items: result_items(&outcomes),
                reinstall,
            }
        }
        // Nothing was reported as removed, so the UI must not be left waiting.
        failed => {
            let why = match failed {
                Ok(Err(e)) => e,
                _ => "the cleanup crashed unexpectedly".into(),
            };
            CleanupDoneDto {
                batch_id: batch,
                freed: 0,
                items: paths
                    .iter()
                    .map(|p| CleanupResultItemDto {
                        path: p.to_string_lossy().into_owned(),
                        ok: false,
                        error: Some(why.clone()),
                        entry_id: None,
                    })
                    .collect(),
                reinstall: Vec::new(),
            }
        }
    };
    // Release the flag first so a scan started from the done handler is accepted.
    drop(guard);
    let _ = app.emit(EVENT_CLEANUP_DONE, done);
}

#[tauri::command]
pub async fn start_cleanup(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<String, String> {
    let data = state.current().ok_or("no scan yet")?;
    let protected = vec![app_data_dir(&app)?];
    // Fail here rather than in the thread if the log cannot be opened.
    with_log(&app, |_| Ok(()))?;
    {
        let running = lock(&state.running);
        let mut cleaning = lock(&state.cleaning);
        check_can_clean(running.is_some(), *cleaning)?;
        *cleaning = true;
    }
    let batch = batch_id();
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let (thread_app, thread_batch) = (app.clone(), batch.clone());
    let spawned = std::thread::Builder::new()
        .name("devdisk-cleanup".into())
        .spawn(move || cleanup_thread(thread_app, data, paths, protected, thread_batch));
    if let Err(e) = spawned {
        *lock(&state.cleaning) = false;
        return Err(format!("could not start the cleanup: {e}"));
    }
    Ok(batch)
}

#[tauri::command]
pub async fn list_history(app: AppHandle) -> Result<Vec<HistoryEntry>, String> {
    with_log(&app, |log| log.list())
}

#[tauri::command]
pub async fn restore_entries(
    app: AppHandle,
    ids: Vec<i64>,
) -> Result<Vec<RestoreResultDto>, String> {
    // Listing a large Recycle Bin is slow, so keep it off the async runtime.
    tauri::async_runtime::spawn_blocking(move || {
        with_log(&app, |log| Ok(restore(log, &ids, &SystemBin)))
    })
    .await
    .map_err(|e| format!("restore failed: {e}"))?
    .map(restore_dtos)
}

/// Removes the project's "reinstall running" mark on every exit path.
struct ReinstallGuard(AppHandle, String);

impl Drop for ReinstallGuard {
    fn drop(&mut self) {
        lock(&self.0.state::<AppState>().reinstalling).remove(&self.1.to_ascii_lowercase());
    }
}

/// Forwards a stream line by line. Bytes are decoded lossily because npm can print
/// non-UTF-8 text on some consoles.
fn pump_lines(app: AppHandle, project: String, stream: &'static str, source: impl Read) {
    let mut reader = BufReader::new(source);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf);
                let _ = app.emit(
                    EVENT_REINSTALL_OUTPUT,
                    ReinstallOutputDto {
                        project_path: project.clone(),
                        stream,
                        line: text.trim_end_matches(['\r', '\n']).to_owned(),
                    },
                );
            }
        }
    }
}

fn reinstall_command_line(command: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = Command::new("cmd");
        // raw_arg: cmd parses its own command line, Rust's quoting would wrap it in quotes.
        cmd.arg("/C")
            .raw_arg(command)
            .creation_flags(CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

#[tauri::command]
pub async fn start_reinstall(
    app: AppHandle,
    state: State<'_, AppState>,
    project_path: String,
) -> Result<(), String> {
    let data = state.current().ok_or("no scan yet")?;
    let project = find_project_by_path(&data, &project_path).ok_or("unknown project")?;
    // Only the command from the built-in rules runs, never text sent by the UI.
    let command = project
        .info
        .reinstall_command
        .clone()
        .ok_or("this project has no reinstall command")?;
    let dir = data.tree.path(project.node);
    let path = dir.to_string_lossy().into_owned();

    if !lock(&state.reinstalling).insert(path.to_ascii_lowercase()) {
        return Err("a reinstall is already running for this project".into());
    }
    let guard = ReinstallGuard(app.clone(), path.clone());

    let mut child = reinstall_command_line(&command)
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run `{command}`: {e}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    std::thread::Builder::new()
        .name("devdisk-reinstall".into())
        .spawn(move || {
            let mut readers = Vec::new();
            if let Some(s) = stdout {
                let (app, path) = (app.clone(), path.clone());
                readers.push(std::thread::spawn(move || {
                    pump_lines(app, path, "stdout", s)
                }));
            }
            if let Some(s) = stderr {
                let (app, path) = (app.clone(), path.clone());
                readers.push(std::thread::spawn(move || {
                    pump_lines(app, path, "stderr", s)
                }));
            }
            let status = child.wait();
            for r in readers {
                let _ = r.join();
            }
            let (success, exit_code) = match status {
                Ok(s) => (s.success(), s.code()),
                Err(_) => (false, None),
            };
            drop(guard);
            let _ = app.emit(
                EVENT_REINSTALL_DONE,
                ReinstallDoneDto {
                    project_path: path,
                    success,
                    exit_code,
                },
            );
        })
        .map_err(|e| format!("could not start the reinstall: {e}"))?;
    Ok(())
}
