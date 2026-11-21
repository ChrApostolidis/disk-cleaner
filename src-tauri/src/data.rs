//! Tauri-free logic: scanning, the stored scan data, and the DTOs the UI receives.
//! Everything here is unit-testable without a Tauri runtime.

use devdisk_core::bin::{BinItem, BinSummary, RecycleBin, list_items};
use devdisk_core::cleanup::{
    CleanupContext, CleanupError, CleanupLog, Environment, HistoryEntry, Mover, Preview,
    RecycleOutcome, RestoreOutcome, Verdict, recycle,
};
use devdisk_core::devtools::DevSizeSource;
use devdisk_core::duplicates::{
    DupError, DupOptions, DupPhase, DupProgress, DupProgressSink, DupResult, default_skips,
    find_duplicates, reclaimable, without_a_kept_copy,
};
use devdisk_core::index::{NodeId, NodeKind, Tree};
use devdisk_core::projects::{Project, find_projects};
use devdisk_core::rules::Rules;
use devdisk_core::scanner::{Progress, ProgressSink, ScanError, ScanOptions, Scanner, WalkScanner};
use devdisk_core::search::{FileSort, Query, filter_projects, find_files};
use devdisk_core::snapshot::{self, Snapshot, SnapshotError};
use devdisk_core::treemap::{ProjectIndex, TreemapNode, treemap};
#[cfg(test)]
use devdisk_core::{treemap::TreemapKind, volume::VolumeInfo};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Upper bound for `list_files`, so a typo cannot ask for millions of rows over IPC.
pub const MAX_FILE_LIMIT: usize = 5000;

/// Per-folder cut for treemap requests; the rest is folded into one "others" entry.
pub const TREEMAP_MAX_CHILDREN: usize = 60;

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgressDto {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub current_path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummaryDto {
    pub root: String,
    pub files: u64,
    pub dirs: u64,
    pub allocated: u64,
    pub size: u64,
    pub elapsed_ms: u64,
    pub issues: u64,
    pub projects: u64,
    pub reclaimable: u64,
    /// Unix milliseconds when the scan finished; a restored snapshot keeps its own.
    pub scanned_at: i64,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScanErrorDto {
    pub message: String,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RegenerableFolderDto {
    pub name: String,
    pub path: String,
    pub allocated: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDto {
    pub id: u64,
    pub path: String,
    pub name: String,
    pub display_name: String,
    pub label: String,
    pub framework: Option<String>,
    pub version: Option<String>,
    pub ecosystems: Vec<String>,
    pub package_manager: Option<String>,
    pub has_lockfile: bool,
    pub reinstall_command: Option<String>,
    pub regenerable: Vec<RegenerableFolderDto>,
    pub total_allocated: u64,
    pub reclaimable: u64,
    pub last_activity: Option<u64>,
    pub parent_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileEntryDto {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub allocated: u64,
    pub modified: Option<u64>,
}

/// One finished scan, shared read-only between commands.
#[derive(Clone)]
pub struct ScanData {
    pub tree: Tree,
    pub projects: Vec<Project>,
    pub project_index: ProjectIndex,
    pub summary: ScanSummaryDto,
}

pub fn now_ms() -> i64 {
    unix_ms(SystemTime::now()).unwrap_or(0) as i64
}

fn unix_ms(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

pub fn progress_dto(p: &Progress) -> ScanProgressDto {
    ScanProgressDto {
        files: p.files,
        dirs: p.dirs,
        bytes: p.bytes,
        current_path: p.current_path.to_string_lossy().into_owned(),
    }
}

pub fn error_dto(e: &ScanError) -> ScanErrorDto {
    ScanErrorDto {
        message: e.to_string(),
        cancelled: matches!(e, ScanError::Cancelled),
    }
}

pub fn run_scan(
    root: &Path,
    cancel: Arc<AtomicBool>,
    sink: &dyn ProgressSink,
) -> Result<ScanData, ScanError> {
    run_scan_with(&WalkScanner, root, cancel, sink)
}

pub fn run_scan_with(
    scanner: &dyn Scanner,
    root: &Path,
    cancel: Arc<AtomicBool>,
    sink: &dyn ProgressSink,
) -> Result<ScanData, ScanError> {
    let options = ScanOptions {
        cancel: Some(cancel),
        ..ScanOptions::default()
    };
    let result = scanner.scan(root, &options, sink)?;
    let elapsed_ms = result.elapsed.as_millis() as u64;
    Ok(build_data(
        result.tree,
        result.issues.len() as u64,
        elapsed_ms,
        now_ms(),
    ))
}

fn live_dirs(tree: &Tree) -> u64 {
    (0..tree.len())
        .filter(|&id| tree.is_live(id) && tree.node(id).kind == NodeKind::Dir)
        .count()
        .saturating_sub(1) as u64
}

/// Derives projects, the treemap index and the summary from a tree, so a fresh scan and a
/// tree that had items detached after a cleanup end up with the same kind of data.
/// A loaded snapshot only knows how many issues there were, hence the count.
fn build_data(tree: Tree, issues: u64, elapsed_ms: u64, scanned_at: i64) -> ScanData {
    let projects = find_projects(&tree, &Rules::builtin());
    let root_node = tree.node(tree.root());
    let summary = ScanSummaryDto {
        root: tree.root_path().to_string_lossy().into_owned(),
        files: root_node.file_count,
        dirs: live_dirs(&tree),
        allocated: root_node.allocated,
        size: root_node.size,
        elapsed_ms,
        issues,
        projects: projects.len() as u64,
        reclaimable: projects.iter().map(|p| p.reclaimable_allocated).sum(),
        scanned_at,
    };
    let project_index = ProjectIndex::new(&tree, &projects);
    ScanData {
        tree,
        projects,
        project_index,
        summary,
    }
}

fn folder_name(tree: &Tree, node: NodeId) -> String {
    tree.path(node)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| tree.node(node).name.clone())
}

/// `ids` are indexes into `data.projects`; they double as the DTO ids.
pub fn project_dtos(data: &ScanData, ids: &[usize]) -> Vec<ProjectDto> {
    let tree = &data.tree;
    ids.iter()
        .map(|&id| {
            let p = &data.projects[id];
            let regenerable = p
                .info
                .regenerable
                .iter()
                .filter_map(|name| {
                    let child = tree.child_by_name(p.node, name)?;
                    Some(RegenerableFolderDto {
                        name: tree.node(child).name.clone(),
                        path: tree.path(child).to_string_lossy().into_owned(),
                        allocated: tree.node(child).allocated,
                    })
                })
                .collect();
            ProjectDto {
                id: id as u64,
                path: tree.path(p.node).to_string_lossy().into_owned(),
                name: folder_name(tree, p.node),
                display_name: p.info.display_name(),
                label: p.info.label.clone(),
                framework: p.info.framework.clone(),
                version: p.info.version.clone(),
                ecosystems: p.info.ecosystems.clone(),
                package_manager: p.info.package_manager.clone(),
                has_lockfile: p.info.has_lockfile,
                reinstall_command: p.info.reinstall_command.clone(),
                regenerable,
                total_allocated: p.total_allocated,
                reclaimable: p.reclaimable_allocated,
                last_activity: p.last_activity.and_then(unix_ms),
                parent_id: p.parent.map(|i| i as u64),
            }
        })
        .collect()
}

pub fn file_dtos(data: &ScanData, ids: &[NodeId]) -> Vec<FileEntryDto> {
    ids.iter()
        .map(|&id| {
            let n = data.tree.node(id);
            FileEntryDto {
                path: data.tree.path(id).to_string_lossy().into_owned(),
                name: n.name.clone(),
                size: n.size,
                allocated: n.allocated,
                modified: n.modified.and_then(unix_ms),
            }
        })
        .collect()
}

pub fn parse_sort(s: &str) -> Result<FileSort, String> {
    match s {
        "allocated" => Ok(FileSort::Allocated),
        "size" => Ok(FileSort::Size),
        "modified" => Ok(FileSort::Modified),
        "name" => Ok(FileSort::Name),
        other => Err(format!("unknown sort `{other}`")),
    }
}

pub fn query_projects(
    data: &ScanData,
    query: &str,
    now: SystemTime,
) -> Result<Vec<ProjectDto>, String> {
    let q = Query::parse(query).map_err(|e| e.to_string())?;
    let ids = filter_projects(&data.tree, &data.projects, &q, now);
    Ok(project_dtos(data, &ids))
}

pub fn query_files(
    data: &ScanData,
    query: &str,
    sort: &str,
    descending: bool,
    limit: usize,
    now: SystemTime,
) -> Result<Vec<FileEntryDto>, String> {
    let q = Query::parse(query).map_err(|e| e.to_string())?;
    let sort = parse_sort(sort)?;
    let ids = find_files(
        &data.tree,
        &q,
        sort,
        descending,
        limit.min(MAX_FILE_LIMIT),
        now,
    );
    Ok(file_dtos(data, &ids))
}

/// Size on disk of a scanned folder, for sizing installed apps. None before a scan, for a
/// file, or for a folder the scan does not cover.
pub fn folder_size_in_scan(data: Option<&ScanData>, path: &Path) -> Option<u64> {
    let data = data?;
    let id = data.tree.find(path)?;
    let node = data.tree.node(id);
    (node.kind == NodeKind::Dir).then_some(node.allocated)
}

/// Size on disk of a language or tool folder: from the scan when it covers the folder,
/// otherwise walked now (SDKs and toolchains usually live outside the scanned root).
pub fn dev_folder_size(data: Option<&ScanData>, path: &Path) -> Option<(u64, DevSizeSource)> {
    if let Some(n) = folder_size_in_scan(data, path) {
        return Some((n, DevSizeSource::Scan));
    }
    if !path.is_dir() {
        return None;
    }
    let res = WalkScanner
        .scan(
            path,
            &ScanOptions::default(),
            &devdisk_core::scanner::NoProgress,
        )
        .ok()?;
    Some((
        res.tree.node(res.tree.root()).allocated,
        DevSizeSource::Measured,
    ))
}

/// The folder at `path` with children `depth` levels deep (clamped to 1..=3).
pub fn treemap_at(data: &ScanData, path: &str, depth: u32) -> Result<TreemapNode, String> {
    let id = data
        .tree
        .find(Path::new(path))
        .ok_or_else(|| format!("not part of the scan: {path}"))?;
    if data.tree.node(id).kind != NodeKind::Dir {
        return Err(format!("not a folder: {path}"));
    }
    Ok(treemap(
        &data.tree,
        &data.projects,
        &data.project_index,
        id,
        depth.clamp(1, 3),
        TREEMAP_MAX_CHILDREN,
    ))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CleanupProgressDto {
    pub batch_id: String,
    pub done: u64,
    pub total: u64,
    pub current_path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResultItemDto {
    pub path: String,
    pub ok: bool,
    pub error: Option<String>,
    pub entry_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReinstallOffer {
    pub project_path: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CleanupDoneDto {
    pub batch_id: String,
    pub freed: u64,
    pub items: Vec<CleanupResultItemDto>,
    pub reinstall: Vec<ReinstallOffer>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResultDto {
    pub id: i64,
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReinstallOutputDto {
    pub project_path: String,
    pub stream: &'static str,
    pub line: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReinstallDoneDto {
    pub project_path: String,
    pub success: bool,
    pub exit_code: Option<i32>,
}

pub fn restore_dtos(outcomes: Vec<RestoreOutcome>) -> Vec<RestoreResultDto> {
    outcomes
        .into_iter()
        .map(|o| RestoreResultDto {
            id: o.id,
            ok: o.ok,
            error: o.error,
        })
        .collect()
}

pub fn result_items(outcomes: &[RecycleOutcome]) -> Vec<CleanupResultItemDto> {
    outcomes
        .iter()
        .map(|o| CleanupResultItemDto {
            path: o.path.to_string_lossy().into_owned(),
            ok: o.ok,
            error: o.error.clone(),
            entry_id: o.entry_id,
        })
        .collect()
}

/// Bytes that really went to the Recycle Bin.
pub fn freed_bytes(outcomes: &[RecycleOutcome]) -> u64 {
    outcomes.iter().filter(|o| o.ok).map(|o| o.allocated).sum()
}

/// Ids are only compared for equality, so time plus a process-wide counter is unique enough
/// (the counter covers two cleanups starting in the same millisecond).
pub fn batch_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ms = unix_ms(SystemTime::now()).unwrap_or(0);
    format!("{ms:x}-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Case-insensitive like Windows paths, so the UI's spelling of a path always finds the project.
pub fn find_project_by_path<'a>(data: &'a ScanData, path: &str) -> Option<&'a Project> {
    data.projects.iter().find(|p| {
        data.tree
            .path(p.node)
            .to_string_lossy()
            .eq_ignore_ascii_case(path)
    })
}

/// Reinstall suggestions for the successful items. Must be given the data from *before*
/// `apply_cleanup`, because detached nodes can no longer be looked up.
pub fn reinstall_offers(
    data: &ScanData,
    rules: &Rules,
    outcomes: &[RecycleOutcome],
) -> Vec<ReinstallOffer> {
    let mut seen = HashSet::new();
    let mut offers = Vec::new();
    for o in outcomes.iter().filter(|o| o.ok) {
        let Some(node) = data.tree.find(&o.path) else {
            continue;
        };
        let n = data.tree.node(node);
        let Some(project) = n
            .parent
            .and_then(|parent| data.projects.iter().find(|p| p.node == parent))
        else {
            continue;
        };
        let Some(command) = &project.info.reinstall_command else {
            continue;
        };
        if !rules.is_dependency_dir(&project.info.ecosystems, &n.name) {
            continue;
        }
        let project_path = data.tree.path(project.node).to_string_lossy().into_owned();
        if seen.insert(project_path.to_ascii_lowercase()) {
            offers.push(ReinstallOffer {
                project_path,
                command: command.clone(),
            });
        }
    }
    offers
}

/// A copy of `data` without the items that were moved to the Recycle Bin.
pub fn apply_cleanup(data: &ScanData, outcomes: &[RecycleOutcome]) -> ScanData {
    let mut tree = data.tree.clone();
    for o in outcomes.iter().filter(|o| o.ok) {
        if let Some(id) = tree.find(&o.path) {
            tree.detach(id);
        }
    }
    // The data is still from the original scan, so its time stays.
    build_data(
        tree,
        data.summary.issues,
        data.summary.elapsed_ms,
        data.summary.scanned_at,
    )
}

/// Runs the checked recycle and returns the outcomes together with the updated scan data.
#[allow(clippy::too_many_arguments)]
pub fn run_cleanup(
    data: &ScanData,
    paths: &[PathBuf],
    protected: &[PathBuf],
    env: &dyn Environment,
    mover: &dyn Mover,
    log: &CleanupLog,
    batch: &str,
    progress: &mut dyn FnMut(usize, usize, &Path),
) -> Result<(Vec<RecycleOutcome>, ScanData), CleanupError> {
    let rules = Rules::builtin();
    let ctx = CleanupContext {
        tree: &data.tree,
        projects: &data.projects,
        rules: &rules,
        protected,
    };
    let outcomes = recycle(&ctx, paths, env, mover, log, batch, progress)?;
    let updated = apply_cleanup(data, &outcomes);
    Ok((outcomes, updated))
}

pub fn snapshot_path(dir: &Path) -> PathBuf {
    dir.join("last-scan.ddsnap")
}

pub fn save_scan(path: &Path, data: &ScanData) -> Result<(), SnapshotError> {
    let s = &data.summary;
    snapshot::save(path, &data.tree, s.issues, s.elapsed_ms, s.scanned_at)
}

/// Projects are recomputed from the tree, so a snapshot written under an older rule set
/// still shows what today's rules detect.
pub fn data_from_snapshot(snap: Snapshot) -> ScanData {
    build_data(snap.tree, snap.issues, snap.elapsed_ms, snap.scanned_at)
}

/// None for a missing file (the normal first run) and for any unreadable one. Bad files are
/// reported but never deleted: the next successful scan overwrites them anyway.
pub fn load_saved(path: &Path) -> Option<ScanData> {
    match snapshot::load(path) {
        Ok(snap) => Some(data_from_snapshot(snap)),
        Err(SnapshotError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            eprintln!("ignoring the saved scan {}: {e}", path.display());
            None
        }
    }
}

/// A scan the user started while the snapshot was loading is newer, so it always wins.
pub fn should_apply_snapshot(scan_running: bool, has_data: bool) -> bool {
    !scan_running && !has_data
}

pub fn bin_summary(bin: &dyn RecycleBin) -> Result<BinSummary, String> {
    bin.summary()
}

/// Entries already restored are no longer in the bin, so they must not mark a lookalike.
pub fn logged_deletions(history: &[HistoryEntry]) -> Vec<(PathBuf, i64)> {
    history
        .iter()
        .filter(|e| e.restored_at.is_none())
        .map(|e| (e.path.clone(), e.deleted_at))
        .collect()
}

pub fn list_bin(bin: &dyn RecycleBin, history: &[HistoryEntry]) -> Result<Vec<BinItem>, String> {
    list_items(bin, &logged_deletions(history))
}

pub fn empty_bin(bin: &dyn RecycleBin) -> Result<BinSummary, String> {
    bin.empty()?;
    bin.summary()
}

/// Items a running cleanup has just moved there would be emptied before they are logged.
pub fn check_can_empty_bin(cleanup_running: bool) -> Result<(), String> {
    if cleanup_running {
        return Err("a cleanup is moving items to the Recycle Bin, wait for it to finish".into());
    }
    Ok(())
}

/// Scan and cleanup both depend on the stored scan, so they never overlap.
pub fn check_can_scan(cleanup_running: bool) -> Result<(), String> {
    if cleanup_running {
        return Err("a cleanup is running, wait for it to finish".into());
    }
    Ok(())
}

pub fn check_can_clean(scan_running: bool, cleanup_running: bool) -> Result<(), String> {
    if scan_running {
        return Err("a scan is running, wait for it to finish".into());
    }
    if cleanup_running {
        return Err("a cleanup is already running".into());
    }
    Ok(())
}

/// Upper bound for duplicate groups sent to the UI; totals still count every group.
pub const MAX_DUP_GROUPS: usize = 1000;

/// Unreadable files listed by name in the UI; the count covers all of them.
const MAX_DUP_ISSUES: usize = 200;

/// A finished duplicate search and the scan it ran on. Node ids stay valid across cleanups
/// (removed nodes are only detached), so it is reused until the next scan.
pub struct DupData {
    pub root: String,
    pub scanned_at: i64,
    pub result: DupResult,
    pub found_at: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DupProgressDto {
    pub phase: DupPhase,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroupDto {
    /// Index of the group in the search result; stable until the next search.
    pub id: u64,
    pub size: u64,
    pub hash: String,
    /// Size on disk freed by keeping one copy.
    pub reclaimable: u64,
    /// Oldest first.
    pub files: Vec<FileEntryDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DupIssueDto {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DuplicatesDto {
    /// Biggest reclaimable first, at most the requested limit.
    pub groups: Vec<DuplicateGroupDto>,
    /// Groups matching the query, including those past the limit.
    pub total_groups: u64,
    pub total_reclaimable: u64,
    pub issues: Vec<DupIssueDto>,
    pub issue_count: u64,
    pub hard_linked: u64,
    pub elapsed_ms: u64,
    pub found_at: i64,
}

pub fn dup_progress_dto(p: &DupProgress) -> DupProgressDto {
    DupProgressDto {
        phase: p.phase,
        files_done: p.files_done,
        files_total: p.files_total,
        bytes_done: p.bytes_done,
        bytes_total: p.bytes_total,
    }
}

pub fn dup_error_dto(e: &DupError) -> ScanErrorDto {
    ScanErrorDto {
        message: e.to_string(),
        cancelled: matches!(e, DupError::Cancelled),
    }
}

/// Hashes files of the scan, leaving out dependency, build, `.git` and system folders.
pub fn run_duplicates(
    data: &ScanData,
    cancel: Arc<AtomicBool>,
    sink: &dyn DupProgressSink,
) -> Result<DupData, DupError> {
    let skip = default_skips(&data.tree, &data.projects, &Rules::builtin());
    let options = DupOptions {
        cancel: Some(cancel),
        ..DupOptions::default()
    };
    let result = find_duplicates(&data.tree, &skip, &options, sink)?;
    Ok(DupData {
        root: data.summary.root.clone(),
        scanned_at: data.summary.scanned_at,
        result,
        found_at: now_ms(),
    })
}

/// A cleanup keeps the scan time, a new scan does not.
pub fn dups_match_scan(dups: &DupData, data: &ScanData) -> bool {
    dups.scanned_at == data.summary.scanned_at && dups.root == data.summary.root
}

/// Groups that still have two live copies and a file matching `query`, biggest reclaimable
/// first. A group is returned whole even when only one of its files matches.
pub fn query_duplicates(
    data: &ScanData,
    dups: &DupData,
    query: &str,
    limit: usize,
    now: SystemTime,
) -> Result<DuplicatesDto, String> {
    let q = Query::parse(query).map_err(|e| e.to_string())?;
    let tree = &data.tree;
    let mut groups: Vec<(usize, Vec<NodeId>, u64)> = dups
        .result
        .groups
        .iter()
        .enumerate()
        .filter_map(|(i, g)| {
            let live: Vec<NodeId> = g
                .files
                .iter()
                .copied()
                .filter(|&id| tree.is_live(id))
                .collect();
            if live.len() < 2 {
                return None;
            }
            let hit = live.iter().any(|&id| {
                let n = tree.node(id);
                q.matches(&[&n.name], n.allocated, n.modified, now)
            });
            hit.then(|| (i, live, reclaimable(tree, g)))
        })
        .collect();
    // A cleanup can shrink a group, so the stored order may no longer hold.
    groups.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    let total_groups = groups.len() as u64;
    let total_reclaimable = groups.iter().map(|g| g.2).sum();
    let groups = groups
        .into_iter()
        .take(limit.min(MAX_DUP_GROUPS))
        .map(|(i, live, reclaimable)| {
            let g = &dups.result.groups[i];
            DuplicateGroupDto {
                id: i as u64,
                size: g.size,
                hash: g.hash.clone(),
                reclaimable,
                files: file_dtos(data, &live),
            }
        })
        .collect();
    let r = &dups.result;
    Ok(DuplicatesDto {
        groups,
        total_groups,
        total_reclaimable,
        issues: r
            .issues
            .iter()
            .take(MAX_DUP_ISSUES)
            .map(|i| DupIssueDto {
                path: i.path.to_string_lossy().into_owned(),
                message: i.message.clone(),
            })
            .collect(),
        issue_count: r.issues.len() as u64,
        hard_linked: r.hard_linked,
        elapsed_ms: r.elapsed.as_millis() as u64,
        found_at: dups.found_at,
    })
}

/// Adds a warning to every selected duplicate whose group would have no copy left, counting
/// copies removed with a selected folder too. Does nothing without a search of this scan.
pub fn mark_unkept_copies(preview: &mut Preview, data: &ScanData, dups: Option<&DupData>) {
    let Some(dups) = dups.filter(|d| dups_match_scan(d, data)) else {
        return;
    };
    let ids: Vec<Option<NodeId>> = preview
        .items
        .iter()
        .map(|i| data.tree.find(&i.path))
        .collect();
    let selected: Vec<NodeId> = ids.iter().flatten().copied().collect();
    let unkept: HashSet<NodeId> = without_a_kept_copy(&data.tree, &dups.result.groups, &selected)
        .into_iter()
        .collect();
    for (item, id) in preview.items.iter_mut().zip(ids) {
        if id.is_some_and(|id| unkept.contains(&id)) {
            item.reasons
                .push("every copy of this duplicate is selected: no copy would be kept".into());
            if item.verdict == Verdict::Ok {
                item.verdict = Verdict::Warn;
            }
        }
    }
}

/// Hashing reads the files a scan replaces or a cleanup moves, so it never overlaps either.
pub fn check_can_find_duplicates(scan_running: bool, cleanup_running: bool) -> Result<(), String> {
    if scan_running {
        return Err("a scan is running, wait for it to finish".into());
    }
    if cleanup_running {
        return Err("a cleanup is running, wait for it to finish".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use devdisk_core::duplicates::NoDupProgress;
    use devdisk_core::scanner::NoProgress;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn write_text(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// root/{shop (Next.js + node_modules), tool (Rust + target), docs/big.bin}
    fn fixture() -> (tempfile::TempDir, ScanData) {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write_text(
            &r.join("shop/package.json"),
            r#"{"name":"shop","dependencies":{"next":"14.0.0","react":"18.0.0"}}"#,
        );
        write(&r.join("shop/node_modules/dep/index.js"), 20_000);
        write(&r.join("shop/src/app.js"), 100);
        write_text(
            &r.join("tool/Cargo.toml"),
            "[package]\nname = \"tool\"\nversion = \"0.1.0\"\n",
        );
        write(&r.join("tool/target/debug/out.bin"), 50_000);
        write(&r.join("docs/big.bin"), 200_000);
        write(&r.join("docs/small.txt"), 10);
        let data = run_scan(r, Arc::new(AtomicBool::new(false)), &NoProgress).unwrap();
        (dir, data)
    }

    fn keys(v: &impl Serialize) -> BTreeSet<String> {
        serde_json::to_value(v)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn scan_summary_numbers() {
        let (dir, data) = fixture();
        let s = &data.summary;
        assert_eq!(s.files, 7);
        // shop, shop/node_modules, shop/node_modules/dep, shop/src, tool, tool/target,
        // tool/target/debug, docs
        assert_eq!(s.dirs, 8);
        assert_eq!(s.projects, 2);
        assert_eq!(s.issues, 0);
        assert!(s.size >= 270_100);
        assert!(s.allocated >= s.size);
        let expected: u64 = data.projects.iter().map(|p| p.reclaimable_allocated).sum();
        assert_eq!(s.reclaimable, expected);
        assert!(s.reclaimable >= 70_000);
        assert_eq!(PathBuf::from(&s.root), dir.path());
    }

    #[test]
    fn project_dtos_carry_detection_and_folders() {
        let (dir, data) = fixture();
        let all = query_projects(&data, "", SystemTime::now()).unwrap();
        assert_eq!(all.len(), 2);
        let shop = all.iter().find(|p| p.name == "shop").unwrap();
        assert_eq!(shop.label, "Next.js");
        assert_eq!(shop.parent_id, None);
        assert_eq!(shop.path, dir.path().join("shop").to_string_lossy());
        assert_eq!(shop.regenerable.len(), 1);
        let nm = &shop.regenerable[0];
        assert_eq!(nm.name, "node_modules");
        assert!(nm.allocated >= 20_000);
        assert_eq!(shop.reclaimable, nm.allocated);
        assert!(shop.total_allocated > shop.reclaimable);
        let node = data.tree.find(&dir.path().join("shop")).unwrap();
        assert_eq!(data.projects[shop.id as usize].node, node);

        let tool = all.iter().find(|p| p.name == "tool").unwrap();
        assert_eq!(tool.regenerable[0].name, "target");
        assert_eq!(tool.ecosystems, vec!["rust".to_string()]);
        // Biggest reclaimable first: target (50k) before node_modules (20k).
        assert_eq!(all[0].name, "tool");
    }

    #[test]
    fn app_folder_size_comes_from_the_scan_only_for_scanned_folders() {
        let (dir, data) = fixture();
        let shop = data.tree.find(&dir.path().join("shop")).unwrap();
        // Install locations often differ in case and end with a backslash.
        let spelled = PathBuf::from(format!("{}\\", dir.path().join("SHOP").display()));
        assert_eq!(
            folder_size_in_scan(Some(&data), &spelled),
            Some(data.tree.node(shop).allocated)
        );
        let file = dir.path().join("docs").join("big.bin");
        assert_eq!(folder_size_in_scan(Some(&data), &file), None, "a file");
        let missing = dir.path().join("nope");
        assert_eq!(folder_size_in_scan(Some(&data), &missing), None);
        assert_eq!(folder_size_in_scan(None, &spelled), None, "no scan yet");
    }

    #[test]
    fn dev_tool_folders_use_the_scan_when_it_covers_them_and_are_walked_otherwise() {
        let (dir, data) = fixture();
        let shop = data.tree.find(&dir.path().join("shop")).unwrap();
        assert_eq!(
            dev_folder_size(Some(&data), &dir.path().join("shop")),
            Some((data.tree.node(shop).allocated, DevSizeSource::Scan))
        );
        let outside = tempfile::tempdir().unwrap();
        write(&outside.path().join("sdk").join("a.bin"), 5000);
        let (size, source) = dev_folder_size(Some(&data), &outside.path().join("sdk")).unwrap();
        assert_eq!(source, DevSizeSource::Measured);
        assert!(size >= 5000, "{size}");
        assert_eq!(
            dev_folder_size(None, &outside.path().join("sdk")).map(|(_, s)| s),
            Some(DevSizeSource::Measured),
            "no scan yet: walked"
        );
        assert_eq!(
            dev_folder_size(Some(&data), &outside.path().join("missing")),
            None
        );
    }

    // Reads this PC: installed languages and tools, with every folder walked.
    #[test]
    #[ignore = "reads and measures this PC's languages and tools"]
    fn real_dev_tools_with_sizes() {
        use devdisk_core::devtools::{DevEnv, ToolRules, detect};
        let t = std::time::Instant::now();
        let measure = |p: &Path| dev_folder_size(None, p);
        let (langs, tools) = detect(&DevEnv::system(Vec::new()), &ToolRules::builtin(), &measure);
        println!("measured in {:?}", t.elapsed());
        for i in langs.iter().chain(&tools) {
            let gb = |b: Option<u64>| {
                b.map(|b| format!("{:.2} GB", b as f64 / 1e9))
                    .unwrap_or("-".into())
            };
            println!("{:<22} {:>10}", i.name, gb(i.size));
            for x in &i.installs {
                println!("    {:<44} {:>10}", x.label, gb(x.size));
            }
        }
    }

    #[test]
    fn project_query_filters() {
        let (_d, data) = fixture();
        let hits = query_projects(&data, "shop", SystemTime::now()).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "shop");
    }

    #[test]
    fn files_sorted_and_limited() {
        let (_d, data) = fixture();
        let now = SystemTime::now();
        let top = query_files(&data, "", "allocated", true, 2, now).unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].name, "big.bin");
        assert_eq!(top[0].size, 200_000);
        assert!(top[0].allocated >= 200_000);
        assert!(top[0].modified.is_some());
        assert!(top[0].path.ends_with("big.bin"));

        let by_name = query_files(&data, "", "name", false, 100, now).unwrap();
        assert_eq!(by_name.len(), 7);
        let names: Vec<_> = by_name.iter().map(|f| f.name.to_lowercase()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);

        let filtered = query_files(&data, "*.bin", "size", false, 100, now).unwrap();
        assert_eq!(filtered.len(), 2);
        assert!(filtered[0].size < filtered[1].size);
    }

    #[test]
    fn limit_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..(MAX_FILE_LIMIT + 10) {
            fs::write(dir.path().join(format!("f{i}.txt")), b"a").unwrap();
        }
        let data = run_scan(dir.path(), Arc::new(AtomicBool::new(false)), &NoProgress).unwrap();
        let out = query_files(&data, "", "name", false, usize::MAX, SystemTime::now()).unwrap();
        assert_eq!(out.len(), MAX_FILE_LIMIT);
    }

    #[test]
    fn bad_query_names_the_token() {
        let (_d, data) = fixture();
        let now = SystemTime::now();
        let e = query_projects(&data, ">12parsecs", now).unwrap_err();
        assert!(e.contains("12parsecs"), "{e}");
        let e = query_files(&data, "older:soon", "name", false, 10, now).unwrap_err();
        assert!(e.contains("soon"), "{e}");
    }

    #[test]
    fn unknown_sort_is_an_error() {
        let (_d, data) = fixture();
        let e = query_files(&data, "", "weight", false, 10, SystemTime::now()).unwrap_err();
        assert!(e.contains("weight"), "{e}");
    }

    #[test]
    fn cancelled_scan_maps_to_error_dto() {
        let (dir, _) = fixture();
        let err = run_scan(dir.path(), Arc::new(AtomicBool::new(true)), &NoProgress)
            .err()
            .expect("cancelled");
        let dto = error_dto(&err);
        assert!(dto.cancelled);
        assert!(!dto.message.is_empty());
    }

    #[test]
    fn json_keys_match_the_typescript_contract() {
        let (_d, data) = fixture();
        let now = SystemTime::now();
        assert_eq!(
            keys(&data.summary),
            set(&[
                "root",
                "files",
                "dirs",
                "allocated",
                "size",
                "elapsedMs",
                "issues",
                "projects",
                "reclaimable",
                "scannedAt"
            ])
        );
        let p = &query_projects(&data, "shop", now).unwrap()[0];
        assert_eq!(
            keys(p),
            set(&[
                "id",
                "path",
                "name",
                "displayName",
                "label",
                "framework",
                "version",
                "ecosystems",
                "packageManager",
                "hasLockfile",
                "reinstallCommand",
                "regenerable",
                "totalAllocated",
                "reclaimable",
                "lastActivity",
                "parentId"
            ])
        );
        assert_eq!(keys(&p.regenerable[0]), set(&["name", "path", "allocated"]));
        let f = &query_files(&data, "", "name", false, 1, now).unwrap()[0];
        assert_eq!(
            keys(f),
            set(&["path", "name", "size", "allocated", "modified"])
        );
        let prog = progress_dto(&Progress {
            files: 1,
            dirs: 2,
            bytes: 3,
            current_path: PathBuf::from("x"),
        });
        assert_eq!(keys(&prog), set(&["files", "dirs", "bytes", "currentPath"]));
        let err = ScanErrorDto {
            message: "m".into(),
            cancelled: false,
        };
        assert_eq!(keys(&err), set(&["message", "cancelled"]));
        // Optionals serialize as null, not as missing keys.
        let v = serde_json::to_value(p).unwrap();
        assert!(v["parentId"].is_null());
    }

    #[test]
    fn treemap_of_a_real_scan() {
        let (dir, data) = fixture();
        let root = dir.path().to_string_lossy().into_owned();
        let n = treemap_at(&data, &root, 2).unwrap();
        assert_eq!(n.kind, TreemapKind::Dir);
        assert_eq!(n.allocated, data.summary.allocated);
        let kids = n.children.as_ref().unwrap();
        // Biggest first: docs (200k) leads.
        assert_eq!(kids[0].name, "docs");
        let shop = kids.iter().find(|k| k.name == "shop").unwrap();
        assert!(shop.project_root);
        assert_eq!(shop.label.as_deref(), Some("Next.js"));
        let nm = shop
            .children
            .as_ref()
            .unwrap()
            .iter()
            .find(|k| k.name == "node_modules")
            .unwrap();
        assert!(nm.regenerable);
        assert_eq!(nm.children, None);
        // Starting below the root still finds the label.
        let below = dir.path().join("shop").join("src");
        let m = treemap_at(&data, &below.to_string_lossy(), 1).unwrap();
        assert_eq!(m.label.as_deref(), Some("Next.js"));
    }

    #[test]
    fn treemap_rejects_unknown_paths_and_files() {
        let (dir, data) = fixture();
        let e = treemap_at(&data, &dir.path().join("nope").to_string_lossy(), 1).unwrap_err();
        assert!(e.contains("nope"), "{e}");
        let file = dir.path().join("docs").join("big.bin");
        let e = treemap_at(&data, &file.to_string_lossy(), 1).unwrap_err();
        assert!(e.contains("not a folder"), "{e}");
    }

    #[test]
    fn treemap_depth_is_clamped() {
        let (dir, data) = fixture();
        let root = dir.path().to_string_lossy().into_owned();
        // 0 behaves like 1: children loaded, grandchildren not.
        let n = treemap_at(&data, &root, 0).unwrap();
        assert!(
            n.children
                .as_ref()
                .unwrap()
                .iter()
                .all(|k| k.children.is_none())
        );
        // 99 behaves like 3: tool/target/debug is 3 levels down, its files are not loaded.
        let n = treemap_at(&data, &root, 99).unwrap();
        let tool = n
            .children
            .as_ref()
            .unwrap()
            .iter()
            .find(|k| k.name == "tool")
            .unwrap();
        let target = tool
            .children
            .as_ref()
            .unwrap()
            .iter()
            .find(|k| k.name == "target")
            .unwrap();
        let debug = target
            .children
            .as_ref()
            .unwrap()
            .iter()
            .find(|k| k.name == "debug")
            .unwrap();
        assert_eq!(debug.children, None);
    }

    #[test]
    fn treemap_and_volume_json_match_the_typescript_contract() {
        let (dir, data) = fixture();
        let n = treemap_at(&data, &dir.path().to_string_lossy(), 1).unwrap();
        let node_keys = set(&[
            "path",
            "name",
            "kind",
            "allocated",
            "fileCount",
            "label",
            "projectRoot",
            "regenerable",
            "children",
        ]);
        assert_eq!(keys(&n), node_keys);
        let v = serde_json::to_value(&n).unwrap();
        assert_eq!(v["kind"], "dir");
        assert!(v["label"].is_null());
        let kids = v["children"].as_array().unwrap();
        assert!(kids.iter().all(|k| k["children"].is_null()));
        let leaf = treemap_at(&data, &dir.path().join("docs").to_string_lossy(), 1).unwrap();
        let file = &serde_json::to_value(&leaf).unwrap()["children"][0];
        assert_eq!(file["kind"], "file");

        // "others" needs more entries than the cut; build one directly.
        let others = TreemapNode {
            path: PathBuf::from("x"),
            name: "1 smaller items".into(),
            kind: TreemapKind::Others,
            allocated: 0,
            file_count: 0,
            label: None,
            project_root: false,
            regenerable: false,
            children: None,
        };
        assert_eq!(serde_json::to_value(&others).unwrap()["kind"], "others");
        assert_eq!(keys(&others), node_keys);

        let vol = VolumeInfo {
            mount: PathBuf::from("C:\\"),
            total: 2,
            free: 1,
        };
        assert_eq!(keys(&vol), set(&["mount", "total", "free"]));
    }

    // ---- cleanup ----

    use devdisk_core::cleanup::{Preview, RecycleBinInfo};

    struct FakeEnv {
        git: Option<bool>,
    }

    impl Environment for FakeEnv {
        fn git_tracked(&self, _path: &Path) -> Option<bool> {
            self.git
        }
        fn recycle_bin(&self, _path: &Path) -> RecycleBinInfo {
            RecycleBinInfo::Enabled {
                max_bytes: 100 << 30,
            }
        }
        fn user_profile(&self) -> Option<PathBuf> {
            None
        }
    }

    /// Stands in for the Recycle Bin by deleting inside the test's tempdir.
    struct FakeMover;

    impl Mover for FakeMover {
        fn to_recycle_bin(&self, path: &Path) -> Result<(), String> {
            if path.is_dir() {
                fs::remove_dir_all(path)
            } else {
                fs::remove_file(path)
            }
            .map_err(|e| e.to_string())
        }
    }

    /// The scanned tree and the log's folder, kept apart so the log is not part of the scan.
    struct Dirs {
        root: tempfile::TempDir,
        _log: tempfile::TempDir,
    }

    impl Dirs {
        fn path(&self) -> &Path {
            self.root.path()
        }
    }

    const NOT_TRACKED: FakeEnv = FakeEnv { git: Some(false) };

    /// root/{shop (Next.js, lockfile, node_modules, .next), tool (Rust, target)}
    fn cleanup_fixture() -> (Dirs, ScanData, CleanupLog) {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write_text(
            &r.join("shop/package.json"),
            r#"{"name":"shop","dependencies":{"next":"14.0.0","react":"18.0.0"}}"#,
        );
        write_text(&r.join("shop/package-lock.json"), "{}");
        write(&r.join("shop/node_modules/dep/index.js"), 20_000);
        write(&r.join("shop/.next/cache/a.bin"), 30_000);
        write(&r.join("shop/src/app.js"), 100);
        write_text(
            &r.join("tool/Cargo.toml"),
            "[package]\nname = \"tool\"\nversion = \"0.1.0\"\n",
        );
        write(&r.join("tool/target/debug/out.bin"), 50_000);
        let data = run_scan(r, Arc::new(AtomicBool::new(false)), &NoProgress).unwrap();
        let log_dir = tempfile::tempdir().unwrap();
        let log = CleanupLog::open(&log_dir.path().join("log.sqlite")).unwrap();
        let dir = Dirs {
            root: dir,
            _log: log_dir,
        };
        (dir, data, log)
    }

    fn run(
        data: &ScanData,
        paths: &[PathBuf],
        env: &dyn Environment,
        log: &CleanupLog,
    ) -> (Vec<RecycleOutcome>, ScanData) {
        run_cleanup(
            data,
            paths,
            &[],
            env,
            &FakeMover,
            log,
            "b1",
            &mut |_, _, _| {},
        )
        .unwrap()
    }

    #[test]
    fn run_cleanup_removes_folders_and_updates_data() {
        let (dir, data, log) = cleanup_fixture();
        let r = dir.path();
        let paths = vec![r.join("shop/node_modules"), r.join("tool/target")];
        let (outcomes, updated) = run(&data, &paths, &NOT_TRACKED, &log);

        assert!(outcomes.iter().all(|o| o.ok), "{outcomes:?}");
        assert!(!r.join("shop/node_modules").exists());
        assert!(!r.join("tool/target").exists());
        assert!(r.join("shop/.next").exists());

        assert!(updated.tree.find(&r.join("shop/node_modules")).is_none());
        assert!(updated.tree.find(&r.join("tool/target")).is_none());
        assert!(updated.tree.find(&r.join("shop/.next")).is_some());
        assert!(updated.summary.allocated < data.summary.allocated);
        assert!(updated.summary.size < data.summary.size);
        assert!(updated.summary.files < data.summary.files);
        // node_modules, dep, target, debug are gone.
        assert_eq!(updated.summary.dirs, data.summary.dirs - 4);
        assert!(updated.summary.reclaimable < data.summary.reclaimable);

        let tool = find_project_by_path(&updated, &r.join("tool").to_string_lossy()).unwrap();
        assert_eq!(tool.reclaimable_allocated, 0);
        let shop = find_project_by_path(&updated, &r.join("shop").to_string_lossy()).unwrap();
        // Only .next is left to reclaim.
        assert!(shop.reclaimable_allocated > 0);
        assert!(shop.reclaimable_allocated < 30_000 + 4096 * 4);
        // The original is untouched.
        assert!(data.tree.find(&r.join("shop/node_modules")).is_some());
    }

    #[test]
    fn reinstall_offer_for_dependency_folder_with_lockfile() {
        let (dir, data, log) = cleanup_fixture();
        let r = dir.path();
        let (outcomes, _) = run(&data, &[r.join("shop/node_modules")], &NOT_TRACKED, &log);
        let offers = reinstall_offers(&data, &Rules::builtin(), &outcomes);
        assert_eq!(
            offers,
            vec![ReinstallOffer {
                project_path: r.join("shop").to_string_lossy().into_owned(),
                command: "npm ci".into(),
            }]
        );
    }

    #[test]
    fn no_reinstall_offer_for_build_output() {
        let (dir, data, log) = cleanup_fixture();
        let r = dir.path();
        let paths = vec![r.join("shop/.next"), r.join("tool/target")];
        let (outcomes, _) = run(&data, &paths, &NOT_TRACKED, &log);
        assert!(outcomes.iter().all(|o| o.ok));
        assert!(reinstall_offers(&data, &Rules::builtin(), &outcomes).is_empty());
    }

    #[test]
    fn reinstall_offers_are_deduplicated_per_project() {
        let (dir, data, _log) = cleanup_fixture();
        let r = dir.path();
        let outcome = |p: PathBuf| RecycleOutcome {
            path: p,
            allocated: 1,
            ok: true,
            error: None,
            entry_id: Some(1),
        };
        let outcomes = vec![
            outcome(r.join("shop/node_modules")),
            outcome(r.join("shop/node_modules")),
        ];
        assert_eq!(
            reinstall_offers(&data, &Rules::builtin(), &outcomes).len(),
            1
        );
    }

    #[test]
    fn refused_items_are_reported_and_kept() {
        let (dir, data, log) = cleanup_fixture();
        let r = dir.path();
        let other = tempfile::tempdir().unwrap();
        write(&other.path().join("x/file.bin"), 10);

        let paths = vec![other.path().join("x"), r.join("tool/target")];
        let (outcomes, updated) = run(&data, &paths, &NOT_TRACKED, &log);
        let outside = outcomes
            .iter()
            .find(|o| o.path.starts_with(other.path()))
            .unwrap();
        assert!(!outside.ok);
        assert!(outside.error.is_some());
        assert!(other.path().join("x/file.bin").exists());
        assert!(updated.tree.find(&r.join("tool/target")).is_none());

        let tracked = FakeEnv { git: Some(true) };
        let (outcomes, updated) = run(&data, &[r.join("shop/node_modules")], &tracked, &log);
        assert!(!outcomes[0].ok);
        assert!(r.join("shop/node_modules").exists());
        assert!(updated.tree.find(&r.join("shop/node_modules")).is_some());
        assert_eq!(updated.summary.allocated, data.summary.allocated);
    }

    #[test]
    fn find_project_by_path_ignores_ascii_case() {
        let (dir, data, _log) = cleanup_fixture();
        let shop = dir
            .path()
            .join("shop")
            .to_string_lossy()
            .to_ascii_uppercase();
        assert!(find_project_by_path(&data, &shop).is_some());
        assert!(find_project_by_path(&data, "C:\\nowhere").is_none());
    }

    #[test]
    fn batch_ids_are_unique() {
        let ids: HashSet<String> = (0..1000).map(|_| batch_id()).collect();
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn scan_and_cleanup_exclude_each_other() {
        assert!(check_can_scan(false).is_ok());
        assert!(check_can_scan(true).is_err());
        assert!(check_can_clean(false, false).is_ok());
        assert!(check_can_clean(true, false).unwrap_err().contains("scan"));
        assert!(check_can_clean(false, true).is_err());
    }

    #[test]
    fn cleanup_dto_keys_match_types_ts() {
        let offer = ReinstallOffer {
            project_path: "p".into(),
            command: "npm ci".into(),
        };
        assert_eq!(keys(&offer), set(&["projectPath", "command"]));
        let item = CleanupResultItemDto {
            path: "p".into(),
            ok: true,
            error: None,
            entry_id: Some(1),
        };
        assert_eq!(keys(&item), set(&["path", "ok", "error", "entryId"]));
        let done = CleanupDoneDto {
            batch_id: "b".into(),
            freed: 1,
            items: vec![item],
            reinstall: vec![offer],
        };
        assert_eq!(
            keys(&done),
            set(&["batchId", "freed", "items", "reinstall"])
        );
        let progress = CleanupProgressDto {
            batch_id: "b".into(),
            done: 0,
            total: 1,
            current_path: "p".into(),
        };
        assert_eq!(
            keys(&progress),
            set(&["batchId", "done", "total", "currentPath"])
        );
        let out = ReinstallOutputDto {
            project_path: "p".into(),
            stream: "stdout",
            line: "l".into(),
        };
        assert_eq!(keys(&out), set(&["projectPath", "stream", "line"]));
        let fin = ReinstallDoneDto {
            project_path: "p".into(),
            success: true,
            exit_code: Some(0),
        };
        assert_eq!(keys(&fin), set(&["projectPath", "success", "exitCode"]));
        let restored = RestoreResultDto {
            id: 1,
            ok: true,
            error: None,
        };
        assert_eq!(keys(&restored), set(&["id", "ok", "error"]));
    }

    #[test]
    fn core_cleanup_types_serialize_like_types_ts() {
        let (dir, data, log) = cleanup_fixture();
        let r = dir.path();
        let rules = Rules::builtin();
        let ctx = CleanupContext {
            tree: &data.tree,
            projects: &data.projects,
            rules: &rules,
            protected: &[],
        };
        let paths = [r.join("shop/node_modules"), r.join("shop/src/app.js")];
        let preview: Preview = devdisk_core::cleanup::preview(&ctx, &paths, &NOT_TRACKED).unwrap();
        assert_eq!(keys(&preview), set(&["items", "totalFreed"]));
        let json = serde_json::to_value(&preview).unwrap();
        let items = json["items"].as_array().unwrap();
        assert_eq!(
            keys(&preview.items[0]),
            set(&[
                "path",
                "name",
                "kind",
                "allocated",
                "regenerable",
                "projectPath",
                "projectLabel",
                "verdict",
                "reasons"
            ])
        );
        let kinds: BTreeSet<&str> = items.iter().map(|i| i["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, BTreeSet::from(["dir", "file"]));
        for i in items {
            assert!(["ok", "warn", "refuse"].contains(&i["verdict"].as_str().unwrap()));
        }

        run(&data, &[r.join("tool/target")], &NOT_TRACKED, &log);
        let history = log.list().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(
            keys(&history[0]),
            set(&[
                "id",
                "batchId",
                "deletedAt",
                "path",
                "allocated",
                "projectPath",
                "projectLabel",
                "restoredAt"
            ])
        );
    }

    // ---- snapshot and Recycle Bin ----

    use devdisk_core::bin::{BinItem, BinSummary, RawBinItem, RecycleBin};
    use std::cell::Cell;

    #[test]
    fn scanned_at_is_set_by_a_scan_and_kept_by_a_cleanup() {
        let before = now_ms();
        let (dir, data, log) = cleanup_fixture();
        let at = data.summary.scanned_at;
        assert!(at >= before && at - before < 1000, "{at} vs {before}");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let (_, updated) = run(&data, &[dir.path().join("tool/target")], &NOT_TRACKED, &log);
        assert_eq!(updated.summary.scanned_at, at);
    }

    #[test]
    fn snapshot_round_trip_keeps_the_summary_and_projects() {
        let (_d, data) = fixture();
        let out = tempfile::tempdir().unwrap();
        let file = out.path().join("last-scan.ddsnap");
        save_scan(&file, &data).unwrap();
        let loaded = load_saved(&file).expect("snapshot loads");
        assert_eq!(loaded.summary, data.summary);
        let labels = |d: &ScanData| -> Vec<String> {
            let mut l: Vec<_> = d.projects.iter().map(|p| p.info.label.clone()).collect();
            l.sort();
            l
        };
        assert_eq!(labels(&loaded), labels(&data));
        assert_eq!(loaded.summary.scanned_at, data.summary.scanned_at);
    }

    #[test]
    fn a_missing_or_damaged_snapshot_is_ignored() {
        let out = tempfile::tempdir().unwrap();
        assert!(load_saved(&out.path().join("nope.ddsnap")).is_none());
        let junk = out.path().join("junk.ddsnap");
        fs::write(&junk, b"definitely not a snapshot").unwrap();
        assert!(load_saved(&junk).is_none());
        let (_d, data) = fixture();
        let cut = out.path().join("cut.ddsnap");
        save_scan(&cut, &data).unwrap();
        let bytes = fs::read(&cut).unwrap();
        fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
        assert!(load_saved(&cut).is_none());
        // The bad files are left alone.
        assert!(junk.exists() && cut.exists());
    }

    #[test]
    fn snapshot_applies_only_to_an_idle_empty_app() {
        assert!(should_apply_snapshot(false, false));
        assert!(!should_apply_snapshot(true, false));
        assert!(!should_apply_snapshot(false, true));
        assert!(!should_apply_snapshot(true, true));
    }

    struct FakeBin {
        raw: Vec<RawBinItem>,
        emptied: Cell<bool>,
    }

    impl FakeBin {
        fn new(raw: Vec<RawBinItem>) -> Self {
            Self {
                raw,
                emptied: Cell::new(false),
            }
        }
    }

    impl RecycleBin for FakeBin {
        fn summary(&self) -> Result<BinSummary, String> {
            if self.emptied.get() {
                return Ok(BinSummary { items: 0, size: 0 });
            }
            Ok(BinSummary {
                items: self.raw.len() as u64,
                size: self.raw.iter().filter_map(|r| r.size).sum(),
            })
        }
        fn items(&self) -> Result<Vec<RawBinItem>, String> {
            Ok(self.raw.clone())
        }
        fn empty(&self) -> Result<(), String> {
            self.emptied.set(true);
            Ok(())
        }
    }

    fn raw(name: &str, size: u64, at: i64) -> RawBinItem {
        RawBinItem {
            id: format!("id-{name}"),
            name: name.into(),
            original_path: PathBuf::from(format!(r"C:\proj\{name}")),
            deleted_at_ms: at,
            is_dir: true,
            size: Some(size),
        }
    }

    fn entry(name: &str, at: i64, restored: Option<i64>) -> devdisk_core::cleanup::HistoryEntry {
        devdisk_core::cleanup::HistoryEntry {
            id: 1,
            batch_id: "b".into(),
            deleted_at: at,
            path: PathBuf::from(format!(r"C:\proj\{name}")),
            allocated: 1,
            project_path: None,
            project_label: None,
            restored_at: restored,
        }
    }

    #[test]
    fn bin_list_marks_logged_entries_and_skips_restored_ones() {
        let bin = FakeBin::new(vec![raw("a", 10, 1_000), raw("b", 20, 2_000)]);
        let history = vec![entry("a", 1_000, None), entry("b", 2_000, Some(3_000))];
        let items = list_bin(&bin, &history).unwrap();
        assert_eq!(items[0].name, "b");
        assert!(!items[0].from_dev_disk, "restored entry must not count");
        assert!(items[1].from_dev_disk);
    }

    #[test]
    fn empty_bin_returns_the_new_summary() {
        let bin = FakeBin::new(vec![raw("a", 10, 1_000)]);
        assert_eq!(bin.summary().unwrap().items, 1);
        let after = empty_bin(&bin).unwrap();
        assert_eq!(after, BinSummary { items: 0, size: 0 });
    }

    #[test]
    fn emptying_is_refused_while_cleaning() {
        assert!(check_can_empty_bin(true).unwrap_err().contains("wait"));
        assert!(check_can_empty_bin(false).is_ok());
    }

    #[test]
    fn bin_json_keys_match_the_typescript_contract() {
        assert_eq!(
            keys(&BinSummary { items: 1, size: 2 }),
            set(&["items", "size"])
        );
        let items = list_bin(&FakeBin::new(vec![raw("a", 10, 1)]), &[]).unwrap();
        let item: &BinItem = &items[0];
        assert_eq!(
            keys(item),
            set(&[
                "id",
                "name",
                "originalPath",
                "deletedAt",
                "kind",
                "size",
                "fromDevDisk"
            ])
        );
        let v = serde_json::to_value(item).unwrap();
        assert_eq!(v["kind"], "dir");
        assert_eq!(v["size"], 10);
        assert_eq!(v["fromDevDisk"], false);
    }

    // ---- duplicates ----

    /// Above the 1 MiB first block, so both hash passes run.
    const BIG: usize = 1_500_000;

    fn write_fill(path: &Path, bytes: usize, fill: u8) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![fill; bytes]).unwrap();
    }

    /// root/{docs/{report.pdf, copy of report.pdf}, music/{song.mp3, song (1).mp3}, other.bin,
    /// shop (Node project) with the same report inside node_modules}
    fn dup_fixture() -> (tempfile::TempDir, ScanData, DupData) {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write_fill(&r.join("docs/report.pdf"), BIG, 1);
        write_fill(&r.join("docs/copy of report.pdf"), BIG, 1);
        write_fill(&r.join("music/song.mp3"), BIG + 10, 2);
        write_fill(&r.join("music/song (1).mp3"), BIG + 10, 2);
        write_fill(&r.join("other.bin"), BIG, 3);
        write_text(&r.join("shop/package.json"), r#"{"name":"shop"}"#);
        write_text(&r.join("shop/package-lock.json"), "{}");
        write_fill(&r.join("shop/node_modules/pkg/report.pdf"), BIG, 1);
        let data = run_scan(r, Arc::new(AtomicBool::new(false)), &NoProgress).unwrap();
        let dups = run_duplicates(&data, Arc::new(AtomicBool::new(false)), &NoDupProgress).unwrap();
        (dir, data, dups)
    }

    fn group_names(g: &DuplicateGroupDto) -> Vec<String> {
        let mut v: Vec<String> = g.files.iter().map(|f| f.name.clone()).collect();
        v.sort();
        v
    }

    #[test]
    fn duplicates_of_a_scan_leave_out_dependency_folders() {
        let (_dir, data, dups) = dup_fixture();
        assert!(dups_match_scan(&dups, &data));
        let dto = query_duplicates(&data, &dups, "", 100, SystemTime::now()).unwrap();
        assert_eq!(dto.total_groups, 2);
        assert_eq!(dto.groups.len(), 2);
        // The bigger songs free more, so they come first.
        assert_eq!(group_names(&dto.groups[0]), ["song (1).mp3", "song.mp3"]);
        assert_eq!(
            group_names(&dto.groups[1]),
            ["copy of report.pdf", "report.pdf"]
        );
        let g = &dto.groups[1];
        assert_eq!(g.size, BIG as u64);
        assert_eq!(g.reclaimable, g.files[0].allocated);
        assert_eq!(
            dto.total_reclaimable,
            dto.groups.iter().map(|g| g.reclaimable).sum::<u64>()
        );
        assert_eq!(dto.issue_count, 0);
    }

    #[test]
    fn duplicate_query_keeps_groups_with_a_matching_file_and_caps_the_list() {
        let (_dir, data, dups) = dup_fixture();
        let now = SystemTime::now();
        let pdf = query_duplicates(&data, &dups, "copy", 100, now).unwrap();
        assert_eq!(pdf.total_groups, 1);
        // The whole group is returned, not only the file that matched.
        assert_eq!(pdf.groups[0].files.len(), 2);
        assert!(
            query_duplicates(&data, &dups, "*.zip", 100, now)
                .unwrap()
                .groups
                .is_empty()
        );
        let one = query_duplicates(&data, &dups, "", 1, now).unwrap();
        assert_eq!(one.groups.len(), 1);
        assert_eq!(one.total_groups, 2);
        assert!(query_duplicates(&data, &dups, ">", 100, now).is_err());
    }

    #[test]
    fn duplicates_follow_a_cleanup_but_not_a_new_scan() {
        let (dir, data, dups) = dup_fixture();
        let r = dir.path();
        let log_dir = tempfile::tempdir().unwrap();
        let log = CleanupLog::open(&log_dir.path().join("log.sqlite")).unwrap();
        let (_, cleaned) = run(&data, &[r.join("music/song (1).mp3")], &NOT_TRACKED, &log);

        assert!(dups_match_scan(&dups, &cleaned));
        let dto = query_duplicates(&cleaned, &dups, "", 100, SystemTime::now()).unwrap();
        assert_eq!(dto.total_groups, 1);
        assert_eq!(
            group_names(&dto.groups[0]),
            ["copy of report.pdf", "report.pdf"]
        );

        let mut rescanned = run_scan(r, Arc::new(AtomicBool::new(false)), &NoProgress).unwrap();
        rescanned.summary.scanned_at = data.summary.scanned_at + 1;
        assert!(!dups_match_scan(&dups, &rescanned));
    }

    #[test]
    fn preview_warns_when_no_copy_of_a_duplicate_is_kept() {
        let (dir, data, dups) = dup_fixture();
        let r = dir.path();
        let rules = Rules::builtin();
        let ctx = CleanupContext {
            tree: &data.tree,
            projects: &data.projects,
            rules: &rules,
            protected: &[],
        };
        let one = [r.join("docs/report.pdf")];
        let mut keeps = devdisk_core::cleanup::preview(&ctx, &one, &NOT_TRACKED).unwrap();
        mark_unkept_copies(&mut keeps, &data, Some(&dups));
        assert_eq!(keeps.items[0].verdict, Verdict::Ok);

        let both = [r.join("docs/report.pdf"), r.join("docs/copy of report.pdf")];
        let mut gone = devdisk_core::cleanup::preview(&ctx, &both, &NOT_TRACKED).unwrap();
        mark_unkept_copies(&mut gone, &data, Some(&dups));
        for item in &gone.items {
            assert_eq!(item.verdict, Verdict::Warn, "{item:?}");
            assert!(
                item.reasons.iter().any(|r| r.contains("no copy")),
                "{item:?}"
            );
        }

        // Without a search, or with one from another scan, nothing is added.
        let mut plain = devdisk_core::cleanup::preview(&ctx, &both, &NOT_TRACKED).unwrap();
        mark_unkept_copies(&mut plain, &data, None);
        assert!(plain.items.iter().all(|i| i.verdict == Verdict::Ok));
    }

    #[test]
    fn duplicate_search_waits_for_scans_and_cleanups() {
        assert!(check_can_find_duplicates(false, false).is_ok());
        assert!(
            check_can_find_duplicates(true, false)
                .unwrap_err()
                .contains("scan")
        );
        assert!(
            check_can_find_duplicates(false, true)
                .unwrap_err()
                .contains("cleanup")
        );
    }

    #[test]
    fn duplicate_json_keys_match_the_typescript_contract() {
        let (_dir, data, dups) = dup_fixture();
        let dto = query_duplicates(&data, &dups, "", 100, SystemTime::now()).unwrap();
        assert_eq!(
            keys(&dto),
            set(&[
                "groups",
                "totalGroups",
                "totalReclaimable",
                "issues",
                "issueCount",
                "hardLinked",
                "elapsedMs",
                "foundAt"
            ])
        );
        assert_eq!(
            keys(&dto.groups[0]),
            set(&["id", "size", "hash", "reclaimable", "files"])
        );
        let issue = DupIssueDto {
            path: "p".into(),
            message: "m".into(),
        };
        assert_eq!(keys(&issue), set(&["path", "message"]));
        let p = dup_progress_dto(&DupProgress {
            phase: DupPhase::Full,
            files_done: 1,
            files_total: 2,
            bytes_done: 3,
            bytes_total: 4,
        });
        assert_eq!(
            keys(&p),
            set(&[
                "phase",
                "filesDone",
                "filesTotal",
                "bytesDone",
                "bytesTotal"
            ])
        );
        assert_eq!(serde_json::to_value(&p).unwrap()["phase"], "full");
    }
}
