//! Tauri-free logic: scanning, the stored scan data, and the DTOs the UI receives.
//! Everything here is unit-testable without a Tauri runtime.

use devdisk_core::index::{NodeId, NodeKind, Tree};
use devdisk_core::projects::{Project, find_projects};
use devdisk_core::rules::Rules;
use devdisk_core::scanner::{
    Progress, ProgressSink, ScanError, ScanIssue, ScanOptions, Scanner, WalkScanner,
};
use devdisk_core::search::{FileSort, Query, filter_projects, find_files};
use devdisk_core::treemap::{ProjectIndex, TreemapNode, treemap};
#[cfg(test)]
use devdisk_core::{treemap::TreemapKind, volume::VolumeInfo};
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

/// Upper bound for `list_files`, so a typo cannot ask for millions of rows over IPC.
pub const MAX_FILE_LIMIT: usize = 5000;

/// Per-folder cut for treemap requests; the rest is folded into one "others" entry.
pub const TREEMAP_MAX_CHILDREN: usize = 60;

#[derive(Debug, Clone, Serialize, PartialEq)]
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
}

#[derive(Debug, Clone, Serialize, PartialEq)]
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
pub struct ScanData {
    pub tree: Tree,
    pub projects: Vec<Project>,
    pub project_index: ProjectIndex,
    /// Kept for the upcoming issues view; only the count is surfaced today.
    #[allow(dead_code)]
    pub issues: Vec<ScanIssue>,
    pub summary: ScanSummaryDto,
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
    let options = ScanOptions {
        cancel: Some(cancel),
        ..ScanOptions::default()
    };
    let result = WalkScanner.scan(root, &options, sink)?;
    let tree = result.tree;
    let projects = find_projects(&tree, &Rules::builtin());

    let root_node = tree.node(tree.root());
    let dirs = (0..tree.len())
        .filter(|&id| tree.node(id).kind == NodeKind::Dir)
        .count()
        .saturating_sub(1) as u64;
    let summary = ScanSummaryDto {
        root: tree.root_path().to_string_lossy().into_owned(),
        files: root_node.file_count,
        dirs,
        allocated: root_node.allocated,
        size: root_node.size,
        elapsed_ms: result.elapsed.as_millis() as u64,
        issues: result.issues.len() as u64,
        projects: projects.len() as u64,
        reclaimable: projects.iter().map(|p| p.reclaimable_allocated).sum(),
    };
    let project_index = ProjectIndex::new(&tree, &projects);
    Ok(ScanData {
        tree,
        projects,
        project_index,
        issues: result.issues,
        summary,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
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
                "reclaimable"
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
}
