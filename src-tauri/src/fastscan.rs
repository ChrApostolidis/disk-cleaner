//! Fast scan through an elevated copy of the app. The window stays a normal process; for a
//! fast scan it starts its own exe as administrator (one UAC prompt per scan) with
//! `--devdisk-mft-scan <root> <job folder>`. That copy reads the MFT and talks back through
//! files in the job folder:
//!
//! - `progress.json`: the latest progress, rewritten as the scan goes
//! - `cancel`: created by the app to ask the scan to stop
//! - `result.ddsnap`: the finished tree, in the saved-scan format
//! - `error.json`: why it failed, when it did
//!
//! Exit code 0 means a result was written, 2 cancelled, anything else failed.

use crate::data::{
    ScanData, ScanErrorDto, ScanProgressDto, data_from_snapshot, now_ms, progress_dto,
};
use devdisk_core::elevate::{ElevateError, ElevatedChild, spawn_elevated};
use devdisk_core::scanner::{Progress, ProgressSink, ScanError, ScanOptions, Scanner};
use devdisk_core::snapshot;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const HELPER_FLAG: &str = "--devdisk-mft-scan";
pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_CANCELLED: i32 = 2;

pub struct Job {
    dir: PathBuf,
}

impl Job {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    fn result(&self) -> PathBuf {
        self.dir.join("result.ddsnap")
    }
    fn progress(&self) -> PathBuf {
        self.dir.join("progress.json")
    }
    fn cancel(&self) -> PathBuf {
        self.dir.join("cancel")
    }
    fn error(&self) -> PathBuf {
        self.dir.join("error.json")
    }
}

pub fn helper_args(root: &Path, job: &Path) -> Vec<String> {
    vec![
        HELPER_FLAG.to_owned(),
        root.to_string_lossy().into_owned(),
        job.to_string_lossy().into_owned(),
    ]
}

/// The root and job folder when the process was started as the fast-scan helper.
/// `args` excludes the program name.
pub fn parse_helper_args(args: &[OsString]) -> Option<(PathBuf, PathBuf)> {
    match args {
        [flag, root, job] if flag == HELPER_FLAG => Some((root.into(), job.into())),
        _ => None,
    }
}

/// Writes to a temp file and renames it, so the reader never sees half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

fn error_dto(message: impl Into<String>) -> ScanErrorDto {
    ScanErrorDto {
        message: message.into(),
        cancelled: false,
    }
}

/// Progress for the app, written into the job folder.
struct FileProgress<'a>(&'a Job);

impl ProgressSink for FileProgress<'_> {
    fn report(&self, p: &Progress) {
        if let Ok(json) = serde_json::to_vec(&progress_dto(p)) {
            // A missed update is harmless; the next one replaces it.
            let _ = write_atomic(&self.0.progress(), &json);
        }
    }
}

fn write_error(job: &Job, e: &ScanErrorDto) {
    if let Ok(json) = serde_json::to_vec(e) {
        let _ = write_atomic(&job.error(), &json);
    }
}

/// The helper's whole life: scan, write the result or the error, return the exit code.
pub fn run_helper(scanner: &dyn Scanner, root: &Path, job: &Job) -> i32 {
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    // The app asks to stop by creating the cancel file.
    let watcher = std::thread::spawn({
        let (cancel, done, path) = (cancel.clone(), done.clone(), job.cancel());
        move || {
            while !done.load(Ordering::Relaxed) {
                if path.exists() {
                    cancel.store(true, Ordering::Relaxed);
                    return;
                }
                std::thread::sleep(POLL);
            }
        }
    });
    let options = ScanOptions {
        cancel: Some(cancel),
        progress_interval: Duration::from_millis(250),
        ..ScanOptions::default()
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        scanner.scan(root, &options, &FileProgress(job))
    }));
    done.store(true, Ordering::Relaxed);
    let _ = watcher.join();
    match outcome {
        Ok(Ok(res)) => {
            let saved = snapshot::save(
                &job.result(),
                &res.tree,
                res.issues.len() as u64,
                res.elapsed.as_millis() as u64,
                now_ms(),
            );
            match saved {
                Ok(()) => EXIT_OK,
                Err(e) => {
                    write_error(
                        job,
                        &error_dto(format!("could not save the fast scan: {e}")),
                    );
                    EXIT_FAILED
                }
            }
        }
        Ok(Err(ScanError::Cancelled)) => EXIT_CANCELLED,
        Ok(Err(e)) => {
            write_error(job, &error_dto(e.to_string()));
            EXIT_FAILED
        }
        Err(_) => {
            write_error(job, &error_dto("the fast scan crashed unexpectedly"));
            EXIT_FAILED
        }
    }
}

/// Starts the helper process.
pub trait Launcher {
    fn launch(&self, args: &[String]) -> Result<Box<dyn ElevatedChild>, ElevateError>;
}

/// Starts this same exe through the UAC prompt.
pub struct UacLauncher;

impl Launcher for UacLauncher {
    fn launch(&self, args: &[String]) -> Result<Box<dyn ElevatedChild>, ElevateError> {
        let exe = std::env::current_exe().map_err(|e| ElevateError::Failed(e.to_string()))?;
        spawn_elevated(&exe, args)
    }
}

const POLL: Duration = Duration::from_millis(100);

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// Runs a fast scan through the helper and loads its result. Progress is forwarded to
/// `sink`; setting `cancel` asks the helper to stop.
pub fn run_elevated_scan(
    root: &Path,
    job: &Job,
    launcher: &dyn Launcher,
    cancel: &AtomicBool,
    sink: &dyn ProgressSink,
) -> Result<ScanData, ScanErrorDto> {
    // Anything left from an earlier run must not be mistaken for this run's answer.
    match fs::remove_dir_all(job.dir()) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            return Err(error_dto(format!("could not prepare the fast scan: {e}")));
        }
        _ => {}
    }
    fs::create_dir_all(job.dir())
        .map_err(|e| error_dto(format!("could not prepare the fast scan: {e}")))?;

    let mut child = launcher
        .launch(&helper_args(root, job.dir()))
        .map_err(|e| match e {
            ElevateError::Declined => error_dto(
                "the fast scan needs administrator permission, and Windows did not get it",
            ),
            ElevateError::Failed(why) => error_dto(format!("could not start the fast scan: {why}")),
        })?;
    let mut asked_to_stop = false;
    let mut last: Option<ScanProgressDto> = None;
    let code = loop {
        if let Some(code) = child.wait(POLL) {
            break code;
        }
        if !asked_to_stop && cancel.load(Ordering::Relaxed) {
            asked_to_stop = fs::write(job.cancel(), b"").is_ok();
        }
        if let Some(p) = read_json::<ScanProgressDto>(&job.progress())
            && last.as_ref() != Some(&p)
        {
            sink.report(&Progress {
                files: p.files,
                dirs: p.dirs,
                bytes: p.bytes,
                current_path: PathBuf::from(&p.current_path),
            });
            last = Some(p);
        }
    };
    let outcome = match code {
        EXIT_OK => snapshot::load(&job.result())
            .map(data_from_snapshot)
            .map_err(|e| error_dto(format!("could not read the fast scan's result: {e}"))),
        EXIT_CANCELLED => Err(ScanErrorDto {
            message: ScanError::Cancelled.to_string(),
            cancelled: true,
        }),
        other => Err(read_json::<ScanErrorDto>(&job.error()).unwrap_or_else(|| {
            error_dto(format!(
                "the fast scan stopped unexpectedly (exit code {other})"
            ))
        })),
    };
    let _ = fs::remove_dir_all(job.dir());
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use devdisk_core::scanner::{NoProgress, ScanResult, WalkScanner};
    use std::sync::Mutex;
    use std::thread::JoinHandle;
    use std::time::Instant;

    /// Runs the helper code in a thread instead of an elevated process.
    struct ThreadLauncher<F: Fn() -> Box<dyn Scanner + Send> + Sync> {
        scanner: F,
    }

    struct ThreadChild(Option<JoinHandle<i32>>);

    impl ElevatedChild for ThreadChild {
        fn wait(&mut self, timeout: Duration) -> Option<i32> {
            let end = Instant::now() + timeout;
            loop {
                if self.0.as_ref().is_some_and(|h| h.is_finished()) {
                    return Some(self.0.take().unwrap().join().unwrap());
                }
                if Instant::now() >= end {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    impl<F: Fn() -> Box<dyn Scanner + Send> + Sync> Launcher for ThreadLauncher<F> {
        fn launch(&self, args: &[String]) -> Result<Box<dyn ElevatedChild>, ElevateError> {
            let os: Vec<OsString> = args.iter().map(OsString::from).collect();
            let (root, job) = parse_helper_args(&os).expect("helper args");
            let scanner = (self.scanner)();
            let h = std::thread::spawn(move || run_helper(scanner.as_ref(), &root, &Job::new(job)));
            Ok(Box::new(ThreadChild(Some(h))))
        }
    }

    /// Exits with `code` at once, writing nothing.
    struct CrashLauncher(i32);

    impl Launcher for CrashLauncher {
        fn launch(&self, _args: &[String]) -> Result<Box<dyn ElevatedChild>, ElevateError> {
            let h = std::thread::spawn({
                let code = self.0;
                move || code
            });
            Ok(Box::new(ThreadChild(Some(h))))
        }
    }

    struct DeclineLauncher;

    impl Launcher for DeclineLauncher {
        fn launch(&self, _args: &[String]) -> Result<Box<dyn ElevatedChild>, ElevateError> {
            Err(ElevateError::Declined)
        }
    }

    /// Reports one progress line, waits a bit so the app sees it, then walks for real.
    struct SlowWalk;

    impl Scanner for SlowWalk {
        fn scan(
            &self,
            root: &Path,
            options: &ScanOptions,
            progress: &dyn ProgressSink,
        ) -> Result<ScanResult, ScanError> {
            progress.report(&Progress {
                files: 42,
                dirs: 7,
                bytes: 4096,
                current_path: root.join("halfway"),
            });
            std::thread::sleep(Duration::from_millis(600));
            WalkScanner.scan(root, options, progress)
        }
    }

    /// Runs until it is cancelled.
    struct UntilCancelled;

    impl Scanner for UntilCancelled {
        fn scan(
            &self,
            _root: &Path,
            options: &ScanOptions,
            _progress: &dyn ProgressSink,
        ) -> Result<ScanResult, ScanError> {
            let end = Instant::now() + Duration::from_secs(10);
            while Instant::now() < end {
                if options
                    .cancel
                    .as_ref()
                    .is_some_and(|c| c.load(Ordering::Relaxed))
                {
                    return Err(ScanError::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("never cancelled");
        }
    }

    struct Fails;

    impl Scanner for Fails {
        fn scan(
            &self,
            _root: &Path,
            _options: &ScanOptions,
            _progress: &dyn ProgressSink,
        ) -> Result<ScanResult, ScanError> {
            Err(ScanError::NotNtfs("FAT32".into()))
        }
    }

    struct Collect(Mutex<Vec<Progress>>);

    impl ProgressSink for Collect {
        fn report(&self, p: &Progress) {
            self.0.lock().unwrap().push(p.clone());
        }
    }

    fn folders() -> (tempfile::TempDir, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("a")).unwrap();
        std::fs::write(root.path().join("a/one.bin"), vec![1u8; 5000]).unwrap();
        std::fs::write(root.path().join("two.txt"), b"hi").unwrap();
        (root, tempfile::tempdir().unwrap())
    }

    fn walk_launcher() -> ThreadLauncher<impl Fn() -> Box<dyn Scanner + Send> + Sync> {
        ThreadLauncher {
            scanner: || Box::new(SlowWalk) as Box<dyn Scanner + Send>,
        }
    }

    #[test]
    fn helper_args_round_trip_and_normal_starts_are_not_helpers() {
        let args = helper_args(Path::new(r"C:\My Files\"), Path::new(r"C:\data\job"));
        assert_eq!(args[0], HELPER_FLAG);
        let os: Vec<OsString> = args.iter().map(OsString::from).collect();
        assert_eq!(
            parse_helper_args(&os),
            Some((
                PathBuf::from(r"C:\My Files\"),
                PathBuf::from(r"C:\data\job")
            ))
        );
        assert_eq!(parse_helper_args(&[]), None);
        assert_eq!(parse_helper_args(&[OsString::from("--other")]), None);
        assert_eq!(parse_helper_args(&[OsString::from(HELPER_FLAG)]), None);
    }

    #[test]
    fn the_helpers_result_is_loaded_as_scan_data() {
        let (root, job) = folders();
        let job = Job::new(job.path().join("fast-scan"));
        let data = run_elevated_scan(
            root.path(),
            &job,
            &walk_launcher(),
            &AtomicBool::new(false),
            &NoProgress,
        )
        .unwrap();
        assert_eq!(data.summary.files, 2);
        assert_eq!(PathBuf::from(&data.summary.root), root.path());
        assert!(data.tree.find(&root.path().join("a/one.bin")).is_some());
        // The job folder's files are cleaned up afterwards.
        assert!(!job.result().exists() && !job.progress().exists());
    }

    #[test]
    fn progress_written_by_the_helper_reaches_the_app() {
        let (root, job) = folders();
        let sink = Collect(Mutex::new(Vec::new()));
        run_elevated_scan(
            root.path(),
            &Job::new(job.path().to_path_buf()),
            &walk_launcher(),
            &AtomicBool::new(false),
            &sink,
        )
        .unwrap();
        let seen = sink.0.lock().unwrap();
        assert!(
            seen.iter().any(|p| p.files == 42 && p.dirs == 7),
            "{seen:?}"
        );
    }

    #[test]
    fn cancelling_in_the_app_stops_the_helper() {
        let (root, job) = folders();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::Relaxed);
        });
        let launcher = ThreadLauncher {
            scanner: || Box::new(UntilCancelled) as Box<dyn Scanner + Send>,
        };
        let err = run_elevated_scan(
            root.path(),
            &Job::new(job.path().to_path_buf()),
            &launcher,
            &cancel,
            &NoProgress,
        )
        .err()
        .unwrap();
        assert!(err.cancelled, "{err:?}");
    }

    #[test]
    fn a_helper_error_comes_back_with_its_message() {
        let (root, job) = folders();
        let launcher = ThreadLauncher {
            scanner: || Box::new(Fails) as Box<dyn Scanner + Send>,
        };
        let err = run_elevated_scan(
            root.path(),
            &Job::new(job.path().to_path_buf()),
            &launcher,
            &AtomicBool::new(false),
            &NoProgress,
        )
        .err()
        .unwrap();
        assert!(!err.cancelled);
        assert!(
            err.message.contains("NTFS") && err.message.contains("FAT32"),
            "{}",
            err.message
        );
    }

    #[test]
    fn saying_no_to_uac_is_reported_plainly() {
        let (root, job) = folders();
        let err = run_elevated_scan(
            root.path(),
            &Job::new(job.path().to_path_buf()),
            &DeclineLauncher,
            &AtomicBool::new(false),
            &NoProgress,
        )
        .err()
        .unwrap();
        assert!(!err.cancelled);
        assert!(
            err.message.contains("administrator permission"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_helper_that_dies_without_a_word_is_an_error_and_old_results_are_not_reused() {
        let (root, job) = folders();
        let job = Job::new(job.path().to_path_buf());
        // A finished result from an earlier run is still lying in the job folder.
        run_elevated_scan(
            root.path(),
            &job,
            &walk_launcher(),
            &AtomicBool::new(false),
            &NoProgress,
        )
        .unwrap();
        std::fs::create_dir_all(job.dir()).unwrap();
        std::fs::write(job.result(), b"stale").unwrap();
        let err = run_elevated_scan(
            root.path(),
            &job,
            &CrashLauncher(3),
            &AtomicBool::new(false),
            &NoProgress,
        )
        .err()
        .unwrap();
        assert!(err.message.contains("exit code 3"), "{}", err.message);
    }
}
