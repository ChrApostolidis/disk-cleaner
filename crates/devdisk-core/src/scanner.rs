//! Scanner trait and the standard parallel directory walker (WalkScanner).

use crate::index::{Tree, TreeBuilder};
use jwalk::{ClientState, WalkDirGeneric};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Set to true from another thread to stop the scan; scan returns ScanError::Cancelled.
    pub cancel: Option<Arc<AtomicBool>>,
    /// Minimum time between progress reports. Default 100 ms.
    pub progress_interval: Duration,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            cancel: None,
            progress_interval: Duration::from_millis(100),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub files: u64,
    pub dirs: u64,
    /// Allocated bytes so far.
    pub bytes: u64,
    pub current_path: PathBuf,
}

pub trait ProgressSink: Send + Sync {
    fn report(&self, progress: &Progress);
}

pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn report(&self, _progress: &Progress) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    AccessDenied,
    SkippedLink,
    Other,
}

#[derive(Debug, Clone)]
pub struct ScanIssue {
    pub path: PathBuf,
    pub kind: IssueKind,
    pub message: String,
}

pub struct ScanResult {
    pub tree: Tree,
    pub issues: Vec<ScanIssue>,
    pub elapsed: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("path not found: {0}")]
    RootNotFound(PathBuf),
    #[error("not a directory: {0}")]
    RootNotADirectory(PathBuf),
    #[error("scan cancelled")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub trait Scanner {
    fn scan(
        &self,
        root: &Path,
        options: &ScanOptions,
        progress: &dyn ProgressSink,
    ) -> Result<ScanResult, ScanError>;
}

#[derive(Debug, Default)]
pub struct WalkScanner;

/// What the parallel read-dir callback learned about one entry, so the single consumer
/// thread never has to touch the filesystem.
#[derive(Debug, Default)]
enum EntryInfo {
    #[default]
    None,
    Meta {
        len: u64,
        allocated: u64,
        modified: Option<SystemTime>,
    },
    Failed(io::ErrorKind, String),
}

#[derive(Debug, Default, Clone)]
struct WalkState;

impl ClientState for WalkState {
    type ReadDirState = ();
    type DirEntryState = EntryInfo;
}

fn is_cancelled(cancel: &Option<Arc<AtomicBool>>) -> bool {
    cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed))
}

fn issue_kind(kind: io::ErrorKind) -> IssueKind {
    if kind == io::ErrorKind::PermissionDenied {
        IssueKind::AccessDenied
    } else {
        IssueKind::Other
    }
}

fn round_up(n: u64, cluster: u64) -> u64 {
    n.div_ceil(cluster) * cluster
}

fn inspect(path: &Path, cluster: u64) -> EntryInfo {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => EntryInfo::Meta {
            len: meta.len(),
            allocated: if meta.is_dir() {
                0
            } else {
                allocated_size(path, &meta, cluster)
            },
            modified: meta.modified().ok(),
        },
        Err(e) => EntryInfo::Failed(e.kind(), e.to_string()),
    }
}

#[cfg(windows)]
fn allocated_size(path: &Path, meta: &std::fs::Metadata, cluster: u64) -> u64 {
    use std::os::windows::fs::MetadataExt;
    const COMPRESSED: u32 = 0x800;
    const SPARSE: u32 = 0x200;
    const OFFLINE: u32 = 0x1000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
    let attrs = meta.file_attributes();
    // Cloud-only placeholders occupy no local space; opening them would download them.
    if attrs & (OFFLINE | RECALL_ON_DATA_ACCESS) != 0 {
        return 0;
    }
    if attrs & (COMPRESSED | SPARSE) != 0
        && let Some(n) = win::compressed_size(path)
    {
        return round_up(n, cluster);
    }
    round_up(meta.len(), cluster)
}

#[cfg(not(windows))]
fn allocated_size(_path: &Path, meta: &std::fs::Metadata, _cluster: u64) -> u64 {
    meta.len()
}

#[cfg(windows)]
fn cluster_size(root: &Path) -> u64 {
    win::cluster_size(root).unwrap_or(4096)
}

#[cfg(not(windows))]
fn cluster_size(_root: &Path) -> u64 {
    4096
}

#[cfg(windows)]
mod win {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Foundation::{GetLastError, NO_ERROR};
    use windows_sys::Win32::Storage::FileSystem::{
        GetCompressedFileSizeW, GetDiskFreeSpaceW, GetVolumePathNameW, INVALID_FILE_SIZE,
    };

    /// Absolute path with the `\\?\` prefix so Win32 calls accept more than 260 chars.
    fn wide_long_path(path: &Path) -> Vec<u16> {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let s = abs.as_os_str().to_string_lossy();
        let full: PathBuf = if s.starts_with(r"\\?\") {
            abs
        } else if let Some(unc) = s.strip_prefix(r"\\") {
            PathBuf::from(format!(r"\\?\UNC\{unc}"))
        } else {
            PathBuf::from(format!(r"\\?\{s}"))
        };
        full.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    pub fn compressed_size(path: &Path) -> Option<u64> {
        let wide = wide_long_path(path);
        let mut high = 0u32;
        // SAFETY: `wide` is NUL-terminated and outlives the call; `high` is a valid out pointer.
        let low = unsafe { GetCompressedFileSizeW(wide.as_ptr(), &mut high) };
        if low == INVALID_FILE_SIZE && unsafe { GetLastError() } != NO_ERROR {
            return None;
        }
        Some((u64::from(high) << 32) | u64::from(low))
    }

    pub fn cluster_size(root: &Path) -> Option<u64> {
        let abs = std::path::absolute(root).ok()?;
        let wide: Vec<u16> = abs.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut volume = vec![0u16; 1024];
        // SAFETY: both buffers are valid for the lengths passed.
        let ok =
            unsafe { GetVolumePathNameW(wide.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) };
        if ok == 0 {
            return None;
        }
        let len = volume.iter().position(|&c| c == 0)?;
        let volume_path = std::ffi::OsString::from_wide(&volume[..len]);
        let mut volume_wide: Vec<u16> = volume_path.encode_wide().collect();
        volume_wide.push(0);
        let (mut spc, mut bps, mut free, mut total) = (0u32, 0u32, 0u32, 0u32);
        // SAFETY: the path is NUL-terminated and all out pointers are valid.
        let ok = unsafe {
            GetDiskFreeSpaceW(
                volume_wide.as_ptr(),
                &mut spc,
                &mut bps,
                &mut free,
                &mut total,
            )
        };
        let cluster = u64::from(spc) * u64::from(bps);
        (ok != 0 && cluster > 0).then_some(cluster)
    }
}

impl Scanner for WalkScanner {
    fn scan(
        &self,
        root: &Path,
        options: &ScanOptions,
        progress: &dyn ProgressSink,
    ) -> Result<ScanResult, ScanError> {
        let started = Instant::now();
        if is_cancelled(&options.cancel) {
            return Err(ScanError::Cancelled);
        }
        let root_meta = std::fs::metadata(root).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => ScanError::RootNotFound(root.to_path_buf()),
            _ => ScanError::Io(e),
        })?;
        if !root_meta.is_dir() {
            return Err(ScanError::RootNotADirectory(root.to_path_buf()));
        }

        let cluster = cluster_size(root);
        let cancel = options.cancel.clone();
        let walker = WalkDirGeneric::<WalkState>::new(root)
            .follow_links(false)
            .skip_hidden(false)
            .sort(false)
            .process_read_dir(move |_, dir, _, entries| {
                // Stopping here ends the walk at the next directory instead of the next entry.
                if is_cancelled(&cancel) {
                    entries.clear();
                    return;
                }
                for entry in entries.iter_mut().flatten() {
                    if !entry.file_type.is_symlink() {
                        entry.client_state = inspect(&dir.join(&entry.file_name), cluster);
                    }
                }
            });

        let mut builder = TreeBuilder::new(root, root_meta.modified().ok());
        let mut issues = Vec::new();
        // stack[d] is the directory node at depth d.
        let mut stack = vec![builder.root()];
        let (mut files, mut dirs, mut bytes) = (0u64, 0u64, 0u64);
        let mut last_report = Instant::now();
        let mut since_check = 0u32;

        for result in walker {
            let entry = match result {
                Ok(e) => e,
                Err(e) => {
                    let kind = e.io_error().map_or(io::ErrorKind::Other, io::Error::kind);
                    issues.push(ScanIssue {
                        path: e.path().map(Path::to_path_buf).unwrap_or_default(),
                        kind: issue_kind(kind),
                        message: e.to_string(),
                    });
                    continue;
                }
            };
            if entry.depth == 0 {
                continue;
            }
            let name = entry.file_name.to_string_lossy();
            let parent = stack[entry.depth - 1];

            if entry.file_type.is_symlink() {
                issues.push(ScanIssue {
                    path: entry.parent_path.join(&entry.file_name),
                    kind: IssueKind::SkippedLink,
                    message: "symlink or junction not followed".into(),
                });
            } else {
                match entry.client_state {
                    EntryInfo::Meta {
                        len,
                        allocated,
                        modified,
                    } => {
                        if entry.file_type.is_dir() {
                            let id = builder.add_dir(parent, &name, modified);
                            stack.truncate(entry.depth);
                            stack.push(id);
                            dirs += 1;
                        } else {
                            builder.add_file(parent, &name, len, allocated, modified);
                            files += 1;
                            bytes += allocated;
                        }
                    }
                    EntryInfo::Failed(kind, message) => {
                        issues.push(ScanIssue {
                            path: entry.parent_path.join(&entry.file_name),
                            kind: issue_kind(kind),
                            message,
                        });
                        if entry.file_type.is_dir() {
                            // Keep the folder so its children (if readable) have a parent.
                            let id = builder.add_dir(parent, &name, None);
                            stack.truncate(entry.depth);
                            stack.push(id);
                            dirs += 1;
                        }
                    }
                    EntryInfo::None => {}
                }
            }

            since_check += 1;
            if since_check >= 64 {
                since_check = 0;
                if is_cancelled(&options.cancel) {
                    return Err(ScanError::Cancelled);
                }
                if last_report.elapsed() >= options.progress_interval {
                    last_report = Instant::now();
                    progress.report(&Progress {
                        files,
                        dirs,
                        bytes,
                        current_path: entry.parent_path.join(&entry.file_name),
                    });
                }
            }
        }

        if is_cancelled(&options.cancel) {
            return Err(ScanError::Cancelled);
        }
        progress.report(&Progress {
            files,
            dirs,
            bytes,
            current_path: root.to_path_buf(),
        });
        Ok(ScanResult {
            tree: builder.finish(),
            issues,
            elapsed: started.elapsed(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    fn write(path: &Path, len: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![7u8; len]).unwrap();
    }

    fn scan(root: &Path) -> ScanResult {
        WalkScanner
            .scan(root, &ScanOptions::default(), &NoProgress)
            .unwrap()
    }

    #[test]
    fn nested_tree_sizes_and_counts() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("a/b.txt"), 1000);
        write(&r.join("a/c/d.bin"), 5000);
        write(&r.join("e.txt"), 1);
        fs::create_dir_all(r.join("empty/inner")).unwrap();

        let res = scan(r);
        let tree = &res.tree;
        let root = tree.node(tree.root());
        assert_eq!(root.size, 6001);
        assert_eq!(root.file_count, 3);
        assert!(root.allocated >= root.size);
        assert!(root.newest_modified.is_some());
        let a = tree.find(&r.join("a")).unwrap();
        assert_eq!(tree.node(a).size, 6000);
        assert_eq!(tree.node(a).file_count, 2);
        let empty = tree.find(&r.join("empty")).unwrap();
        assert_eq!(tree.node(empty).size, 0);
        assert_eq!(tree.node(empty).file_count, 0);
        assert!(tree.find(&r.join("empty/inner")).is_some());
        assert!(res.issues.is_empty(), "{:?}", res.issues);
        // Largest allocation first.
        let first = tree.children(tree.root())[0];
        assert_eq!(tree.node(first).name, "a");
    }

    #[test]
    fn allocated_rounds_up_to_clusters() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("f"), 1);
        let res = scan(dir.path());
        let f = res.tree.find(&dir.path().join("f")).unwrap();
        let alloc = res.tree.node(f).allocated;
        assert!(alloc >= 512 && alloc.is_multiple_of(512), "{alloc}");
    }

    #[test]
    fn missing_root() {
        let dir = tempfile::tempdir().unwrap();
        let err = WalkScanner
            .scan(
                &dir.path().join("nope"),
                &ScanOptions::default(),
                &NoProgress,
            )
            .err()
            .unwrap();
        assert!(matches!(err, ScanError::RootNotFound(_)));
    }

    #[test]
    fn file_as_root() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        write(&f, 3);
        let err = WalkScanner
            .scan(&f, &ScanOptions::default(), &NoProgress)
            .err()
            .unwrap();
        assert!(matches!(err, ScanError::RootNotADirectory(_)));
    }

    #[test]
    fn cancel_before_scan() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("f"), 3);
        let opts = ScanOptions {
            cancel: Some(Arc::new(AtomicBool::new(true))),
            ..Default::default()
        };
        let err = WalkScanner
            .scan(dir.path(), &opts, &NoProgress)
            .err()
            .unwrap();
        assert!(matches!(err, ScanError::Cancelled));
    }

    #[test]
    fn progress_is_reported_with_final_totals() {
        struct Collect(Mutex<Vec<Progress>>);
        impl ProgressSink for Collect {
            fn report(&self, p: &Progress) {
                self.0.lock().unwrap().push(p.clone());
            }
        }
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("x/a"), 10);
        write(&dir.path().join("b"), 10);
        let sink = Collect(Mutex::new(Vec::new()));
        WalkScanner
            .scan(dir.path(), &ScanOptions::default(), &sink)
            .unwrap();
        let all = sink.0.lock().unwrap();
        let last = all.last().expect("at least one report");
        assert_eq!(last.files, 2);
        assert_eq!(last.dirs, 1);
        assert!(last.bytes > 0);
    }

    #[test]
    fn long_paths_are_scanned() {
        let dir = tempfile::tempdir().unwrap();
        let mut deep = dir.path().to_path_buf();
        for i in 0..6 {
            deep.push(format!("{i}{}", "x".repeat(50)));
        }
        assert!(deep.as_os_str().len() > 260);
        write(&deep.join("big.bin"), 4321);
        let res = scan(dir.path());
        assert_eq!(res.tree.node(res.tree.root()).size, 4321);
        assert!(res.tree.find(&deep.join("big.bin")).is_some());
        assert!(res.issues.is_empty(), "{:?}", res.issues);
    }

    #[cfg(windows)]
    #[test]
    fn junction_is_reported_by_std_as_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::create_dir_all(&target).unwrap();
        let link = dir.path().join("link");
        junction::create(&target, &link).unwrap();
        let ft = fs::symlink_metadata(&link).unwrap().file_type();
        println!(
            "junction is_symlink={} is_dir={}",
            ft.is_symlink(),
            ft.is_dir()
        );
        assert!(ft.is_symlink());
    }

    #[cfg(windows)]
    #[test]
    fn junction_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        write(&target.join("big.bin"), 100_000);
        let scanned = dir.path().join("scanned");
        write(&scanned.join("small.txt"), 10);
        junction::create(&target, scanned.join("link")).unwrap();

        let res = scan(&scanned);
        let root = res.tree.node(res.tree.root());
        assert_eq!(root.size, 10);
        assert_eq!(root.file_count, 1);
        assert!(res.tree.child_by_name(res.tree.root(), "link").is_none());
        let links: Vec<_> = res
            .issues
            .iter()
            .filter(|i| i.kind == IssueKind::SkippedLink)
            .collect();
        assert_eq!(links.len(), 1);
        assert!(links[0].path.ends_with("link"));
    }
}
