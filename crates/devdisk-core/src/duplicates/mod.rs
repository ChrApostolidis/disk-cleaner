//! Content-based duplicate finder: same size → same hash of the first block → same hash of
//! the whole file (blake3). Only reads files; removing copies goes through `cleanup`.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rayon::prelude::*;

use crate::index::{NodeId, NodeKind, Tree};
use crate::projects::Project;
use crate::rules::Rules;

#[derive(Debug, Clone)]
pub struct DupOptions {
    /// Files smaller than this are ignored. Default 1 MiB.
    pub min_size: u64,
    /// How much of the start of each file the first hash pass reads. Default 1 MiB.
    pub partial_bytes: u64,
    /// Set to true from another thread to stop; returns `DupError::Cancelled`.
    pub cancel: Option<Arc<AtomicBool>>,
    /// Minimum time between progress reports. Default 100 ms.
    pub progress_interval: Duration,
    /// Files hashed at the same time, on a pool of their own so folder walks never wait for
    /// it. Default 4.
    pub threads: usize,
}

impl Default for DupOptions {
    fn default() -> Self {
        Self {
            min_size: 1 << 20,
            partial_bytes: 1 << 20,
            cancel: None,
            progress_interval: Duration::from_millis(100),
            threads: 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DupPhase {
    /// Hashing the first block of every file that shares its size with another.
    Partial,
    /// Hashing whole files whose first blocks matched.
    Full,
}

#[derive(Debug, Clone)]
pub struct DupProgress {
    pub phase: DupPhase,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

pub trait DupProgressSink: Send + Sync {
    fn report(&self, progress: &DupProgress);
}

pub struct NoDupProgress;

impl DupProgressSink for NoDupProgress {
    fn report(&self, _progress: &DupProgress) {}
}

#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    /// Logical size of each copy.
    pub size: u64,
    /// blake3 of the content, lowercase hex.
    pub hash: String,
    /// Oldest modified first (likely the original), then by path.
    pub files: Vec<NodeId>,
}

#[derive(Debug, Clone)]
pub struct DupIssue {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct DupResult {
    /// Biggest `reclaimable` first.
    pub groups: Vec<DuplicateGroup>,
    /// Files that could not be read or changed since the scan; they are left out.
    pub issues: Vec<DupIssue>,
    /// Files left out because they have more than one hard link.
    pub hard_linked: u64,
    pub elapsed: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum DupError {
    #[error("duplicate search cancelled")]
    Cancelled,
    #[error("could not start the hashing threads: {0}")]
    Threads(String),
}

/// Size on disk freed by keeping only the copy that takes the most space, counting only the
/// copies still in the tree.
pub fn reclaimable(tree: &Tree, group: &DuplicateGroup) -> u64 {
    let live: Vec<u64> = group
        .files
        .iter()
        .filter(|&&id| tree.is_live(id))
        .map(|&id| tree.node(id).allocated)
        .collect();
    let max = live.iter().copied().max().unwrap_or(0);
    live.iter().sum::<u64>() - max
}

/// Component-wise and ASCII case-insensitive, like Windows paths.
fn is_within(path: &Path, base: &Path) -> bool {
    let lower = |p: &Path| -> Vec<String> {
        p.components()
            .filter(|c| *c != Component::CurDir)
            .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
            .collect()
    };
    let (p, b) = (lower(path), lower(base));
    !b.is_empty() && p.starts_with(&b)
}

/// Folders whose files are never offered as duplicates: `skip_dirs` (`.git`, `node_modules`),
/// regenerable folders of detected projects, and protected system folders. Removing single
/// files there breaks a repository, a project or an app, and cleanup would refuse most of it.
pub fn default_skips(tree: &Tree, projects: &[Project], rules: &Rules) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = (1..tree.len())
        .filter(|&id| {
            let n = tree.node(id);
            tree.is_live(id) && n.kind == NodeKind::Dir && rules.is_skip_dir(&n.name)
        })
        .collect();
    for p in projects {
        for name in &p.info.regenerable {
            if let Some(c) = tree.child_by_name(p.node, name)
                && tree.node(c).kind == NodeKind::Dir
            {
                out.push(c);
            }
        }
    }
    for ig in rules.ignore_paths() {
        if is_within(tree.root_path(), &ig) {
            out.push(tree.root());
        } else if let Some(id) = tree.find(&ig) {
            out.push(id);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

struct Candidate {
    id: NodeId,
    path: PathBuf,
    size: u64,
}

/// What reading one file gave.
enum Read {
    Hash([u8; 32]),
    HardLinked,
    Failed(String),
}

/// Shared by the hashing threads; reports at most once per interval, plus once at the end.
struct Meter<'a> {
    phase: DupPhase,
    files_done: AtomicU64,
    bytes_done: AtomicU64,
    files_total: u64,
    bytes_total: u64,
    last: Mutex<Instant>,
    interval: Duration,
    sink: &'a dyn DupProgressSink,
}

impl<'a> Meter<'a> {
    fn new(
        phase: DupPhase,
        candidates: &[Candidate],
        limit: u64,
        options: &DupOptions,
        sink: &'a dyn DupProgressSink,
    ) -> Self {
        Self {
            phase,
            files_done: AtomicU64::new(0),
            bytes_done: AtomicU64::new(0),
            files_total: candidates.len() as u64,
            bytes_total: candidates.iter().map(|c| c.size.min(limit)).sum(),
            last: Mutex::new(Instant::now()),
            interval: options.progress_interval,
            sink,
        }
    }

    fn snapshot(&self) -> DupProgress {
        DupProgress {
            phase: self.phase,
            files_done: self.files_done.load(Ordering::Relaxed),
            files_total: self.files_total,
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            bytes_total: self.bytes_total,
        }
    }

    fn file_done(&self, bytes: u64) {
        self.files_done.fetch_add(1, Ordering::Relaxed);
        self.bytes_done.fetch_add(bytes, Ordering::Relaxed);
        // Another thread reporting right now is as good as reporting here.
        if let Ok(mut last) = self.last.try_lock()
            && last.elapsed() >= self.interval
        {
            *last = Instant::now();
            self.sink.report(&self.snapshot());
        }
    }

    fn finish(&self) {
        self.sink.report(&self.snapshot());
    }
}

fn is_cancelled(cancel: &Option<Arc<AtomicBool>>) -> bool {
    cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed))
}

#[cfg(windows)]
fn link_count(file: &File) -> Option<u32> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    // SAFETY: all-zero is a valid value of this plain C struct.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle stays open for the call and `info` is a valid out pointer.
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
    (ok != 0).then_some(info.nNumberOfLinks)
}

#[cfg(unix)]
fn link_count(file: &File) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    file.metadata().ok().map(|m| m.nlink() as u32)
}

#[cfg(not(any(windows, unix)))]
fn link_count(_file: &File) -> Option<u32> {
    None
}

const CHUNK: usize = 1 << 20;

/// Hashes the first `limit` bytes of the file, after checking it is still the file the scan saw.
fn hash_file(c: &Candidate, limit: u64, cancel: &Option<Arc<AtomicBool>>) -> Read {
    let file = match File::open(&c.path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Read::Failed("no longer exists".into());
        }
        Err(e) => return Read::Failed(e.to_string()),
    };
    match file.metadata() {
        Ok(m) if m.len() != c.size => {
            return Read::Failed("changed since the scan; scan again to include it".into());
        }
        Ok(_) => {}
        Err(e) => return Read::Failed(e.to_string()),
    }
    if link_count(&file).is_some_and(|n| n > 1) {
        return Read::HardLinked;
    }
    let mut hasher = blake3::Hasher::new();
    let mut reader = file.take(limit);
    let mut buf = vec![0u8; CHUNK];
    loop {
        // A big file can take seconds, so cancelling must not wait for its end.
        if is_cancelled(cancel) {
            return Read::Failed("cancelled".into());
        }
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buf[..n]);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Read::Failed(e.to_string()),
        }
    }
    Read::Hash(*hasher.finalize().as_bytes())
}

/// Hashes every candidate (up to `limit` bytes each) on the pool, in input order.
fn hash_all(
    pool: &rayon::ThreadPool,
    candidates: &[Candidate],
    limit: u64,
    meter: &Meter,
    cancel: &Option<Arc<AtomicBool>>,
) -> Result<Vec<Read>, DupError> {
    let reads: Vec<Read> = pool.install(|| {
        candidates
            .par_iter()
            .map(|c| {
                if is_cancelled(cancel) {
                    return Read::Failed("cancelled".into());
                }
                let r = hash_file(c, limit, cancel);
                meter.file_done(c.size.min(limit));
                r
            })
            .collect()
    });
    if is_cancelled(cancel) {
        return Err(DupError::Cancelled);
    }
    meter.finish();
    Ok(reads)
}

type Hashed = (Candidate, [u8; 32]);

/// Splits hashed candidates into groups of two or more with the same (size, hash), recording
/// failures and hard links on the way.
fn regroup(
    candidates: Vec<Candidate>,
    reads: Vec<Read>,
    issues: &mut Vec<DupIssue>,
    hard_linked: &mut u64,
) -> Vec<Vec<Hashed>> {
    let mut by_key: HashMap<(u64, [u8; 32]), Vec<Hashed>> = HashMap::new();
    for (c, r) in candidates.into_iter().zip(reads) {
        match r {
            Read::Hash(h) => by_key.entry((c.size, h)).or_default().push((c, h)),
            Read::HardLinked => *hard_linked += 1,
            Read::Failed(message) => issues.push(DupIssue {
                path: c.path,
                message,
            }),
        }
    }
    by_key.into_values().filter(|g| g.len() >= 2).collect()
}

/// Files of `tree` worth hashing: live, outside `skip`, at least `min_size`, sharing their
/// size with another candidate, and with local content.
fn candidates(tree: &Tree, skip: &[NodeId], min_size: u64) -> Vec<Candidate> {
    // A parent always comes before its children, so one forward pass marks whole subtrees.
    let skip: HashSet<NodeId> = skip.iter().copied().collect();
    let mut excluded = vec![false; tree.len()];
    for id in 0..tree.len() {
        excluded[id] = !tree.is_live(id)
            || skip.contains(&id)
            || tree.node(id).parent.is_some_and(|p| excluded[p]);
    }
    let min_size = min_size.max(1);
    let mut by_size: HashMap<u64, Vec<NodeId>> = HashMap::new();
    for (id, &ex) in excluded.iter().enumerate() {
        let n = tree.node(id);
        // Size on disk 0 with content means a cloud-only placeholder: opening it downloads it.
        if !ex && n.kind == NodeKind::File && n.size >= min_size && n.allocated > 0 {
            by_size.entry(n.size).or_default().push(id);
        }
    }
    by_size
        .into_values()
        .filter(|ids| ids.len() >= 2)
        .flatten()
        .map(|id| Candidate {
            id,
            path: tree.path(id),
            size: tree.node(id).size,
        })
        .collect()
}

/// Oldest first, unknown times last, then by path.
fn by_age_then_path(
    a: &(Option<SystemTime>, PathBuf),
    b: &(Option<SystemTime>, PathBuf),
) -> std::cmp::Ordering {
    use std::cmp::Ordering::{Equal, Greater, Less};
    match (a.0, b.0) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Less,
        (None, Some(_)) => Greater,
        (None, None) => Equal,
    }
    .then_with(|| a.1.cmp(&b.1))
}

fn to_group(tree: &Tree, members: Vec<Hashed>) -> DuplicateGroup {
    let size = members[0].0.size;
    let hash = blake3::Hash::from_bytes(members[0].1).to_hex().to_string();
    let mut files: Vec<((Option<SystemTime>, PathBuf), NodeId)> = members
        .into_iter()
        .map(|(c, _)| ((tree.node(c.id).modified, c.path), c.id))
        .collect();
    files.sort_by(|a, b| by_age_then_path(&a.0, &b.0));
    DuplicateGroup {
        size,
        hash,
        files: files.into_iter().map(|f| f.1).collect(),
    }
}

/// Finds groups of files with identical content among the live files of `tree`, outside the
/// `skip` subtrees. Reads files from disk; never opens cloud-only files.
pub fn find_duplicates(
    tree: &Tree,
    skip: &[NodeId],
    options: &DupOptions,
    progress: &dyn DupProgressSink,
) -> Result<DupResult, DupError> {
    let started = Instant::now();
    if is_cancelled(&options.cancel) {
        return Err(DupError::Cancelled);
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(options.threads.max(1))
        .thread_name(|i| format!("devdisk-hash-{i}"))
        .build()
        .map_err(|e| DupError::Threads(e.to_string()))?;
    let mut issues = Vec::new();
    let mut hard_linked = 0;

    let first = candidates(tree, skip, options.min_size);
    let partial = options.partial_bytes.max(1);
    let meter = Meter::new(DupPhase::Partial, &first, partial, options, progress);
    let reads = hash_all(&pool, &first, partial, &meter, &options.cancel)?;
    let mut finished = Vec::new();
    let mut to_full = Vec::new();
    for group in regroup(first, reads, &mut issues, &mut hard_linked) {
        // The first block was the whole file, so its hash already is the full hash.
        if group[0].0.size <= partial {
            finished.push(group);
        } else {
            to_full.extend(group.into_iter().map(|(c, _)| c));
        }
    }

    let meter = Meter::new(DupPhase::Full, &to_full, u64::MAX, options, progress);
    let reads = hash_all(&pool, &to_full, u64::MAX, &meter, &options.cancel)?;
    finished.extend(regroup(to_full, reads, &mut issues, &mut hard_linked));

    let mut groups: Vec<(DuplicateGroup, u64)> = finished
        .into_iter()
        .map(|members| {
            let g = to_group(tree, members);
            let r = reclaimable(tree, &g);
            (g, r)
        })
        .collect();
    groups.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.0.size.cmp(&a.0.size))
            .then_with(|| a.0.hash.cmp(&b.0.hash))
    });
    issues.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(DupResult {
        groups: groups.into_iter().map(|(g, _)| g).collect(),
        issues,
        hard_linked,
        elapsed: started.elapsed(),
    })
}

/// Of the `selected` nodes (files or folders about to be removed), the duplicate files whose
/// group would have no copy left.
pub fn without_a_kept_copy(
    tree: &Tree,
    groups: &[DuplicateGroup],
    selected: &[NodeId],
) -> Vec<NodeId> {
    let selected: HashSet<NodeId> = selected.iter().copied().collect();
    let covered = |id: NodeId| {
        let mut cur = Some(id);
        while let Some(c) = cur {
            if selected.contains(&c) {
                return true;
            }
            cur = tree.node(c).parent;
        }
        false
    };
    let mut out = Vec::new();
    for g in groups {
        let live: Vec<NodeId> = g
            .files
            .iter()
            .copied()
            .filter(|&id| tree.is_live(id))
            .collect();
        if !live.is_empty() && live.iter().all(|&id| covered(id)) {
            out.extend(live.into_iter().filter(|id| selected.contains(id)));
        }
    }
    out
}

#[cfg(test)]
mod tests;
