//! Finding all projects in a scanned tree and computing their reclaimable size.

use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use crate::detect::{ChildEntry, DetectedProject, detect_dir};
use crate::index::{NodeId, NodeKind, Tree};
use crate::rules::Rules;

#[derive(Debug, Clone)]
pub struct Project {
    pub node: NodeId,
    pub info: DetectedProject,
    /// Index into the returned Vec of the nearest enclosing project (monorepos, Tauri apps with src-tauri inside).
    pub parent: Option<usize>,
    /// Allocated size of the project folder.
    pub total_allocated: u64,
    /// Sum of `allocated` of its regenerable child dirs.
    pub reclaimable_allocated: u64,
    /// Newest file mtime in the project, ignoring its regenerable dirs. A fresh `npm install` must not make an old project look active.
    pub last_activity: Option<SystemTime>,
}

/// Depth-first, parent before children.
pub fn find_projects(tree: &Tree, rules: &Rules) -> Vec<Project> {
    find_projects_with(tree, rules, &rules.ignore_paths())
}

fn find_projects_with(tree: &Tree, rules: &Rules, ignore: &[PathBuf]) -> Vec<Project> {
    let ignore: Vec<Vec<String>> = ignore.iter().map(|p| lower_components(p)).collect();
    let mut out = Vec::new();
    walk(tree, rules, &ignore, tree.root(), None, &mut out);
    out
}

fn lower_components(path: &Path) -> Vec<String> {
    path.components()
        .filter(|c| *c != Component::CurDir)
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

/// Component-wise so that an ignored "Apps" folder does not also block "Appsmith".
fn is_ignored(path: &Path, ignore: &[Vec<String>]) -> bool {
    let comps = lower_components(path);
    ignore
        .iter()
        .any(|ig| !ig.is_empty() && comps.starts_with(ig))
}

fn walk(
    tree: &Tree,
    rules: &Rules,
    ignore: &[Vec<String>],
    id: NodeId,
    enclosing: Option<usize>,
    out: &mut Vec<Project>,
) {
    let path = tree.path(id);
    if is_ignored(&path, ignore) {
        return;
    }
    let children = tree.children(id);
    let entries: Vec<ChildEntry> = children
        .iter()
        .map(|&c| ChildEntry {
            name: &tree.node(c).name,
            is_dir: tree.node(c).kind == NodeKind::Dir,
        })
        .collect();

    let mut here = enclosing;
    let mut regenerable: Vec<String> = Vec::new();
    if let Some(info) = detect_dir(&path, &entries, rules) {
        regenerable = info.regenerable.clone();
        let is_regen = |name: &str| regenerable.iter().any(|r| r.eq_ignore_ascii_case(name));
        let mut reclaimable = 0;
        let mut last_activity: Option<SystemTime> = None;
        for &c in children {
            let n = tree.node(c);
            if n.kind == NodeKind::Dir && is_regen(&n.name) {
                reclaimable += n.allocated;
            } else {
                last_activity = last_activity.max(n.newest_modified);
            }
        }
        out.push(Project {
            node: id,
            info,
            parent: enclosing,
            total_allocated: tree.node(id).allocated,
            reclaimable_allocated: reclaimable,
            last_activity,
        });
        here = Some(out.len() - 1);
    }

    for &c in children {
        let n = tree.node(c);
        if n.kind != NodeKind::Dir || rules.is_skip_dir(&n.name) {
            continue;
        }
        if regenerable.iter().any(|r| r.eq_ignore_ascii_case(&n.name)) {
            continue;
        }
        walk(tree, rules, ignore, c, here, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::TreeBuilder;
    use std::fs;
    use std::time::Duration;

    fn t(secs: u64) -> Option<SystemTime> {
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
    }

    /// Detection reads manifests from disk, so the marker files exist for real
    /// while the tree is built by hand with the sizes the test wants.
    fn touch(dir: &Path, rel: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "").unwrap();
    }

    #[test]
    fn nested_project_found_and_regenerable_dir_not_descended() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "app/Cargo.toml");
        touch(tmp.path(), "app/crates/inner/Cargo.toml");
        // Would be a project if descended into.
        touch(tmp.path(), "app/target/dep/Cargo.toml");

        let mut b = TreeBuilder::new(tmp.path(), None);
        let app = b.add_dir(b.root(), "app", None);
        b.add_file(app, "Cargo.toml", 0, 0, t(10));
        let target = b.add_dir(app, "target", None);
        let dep = b.add_dir(target, "dep", None);
        b.add_file(dep, "Cargo.toml", 0, 0, t(500));
        b.add_file(target, "big.bin", 900, 1000, t(900));
        let crates = b.add_dir(app, "crates", None);
        let inner = b.add_dir(crates, "inner", None);
        b.add_file(inner, "Cargo.toml", 0, 0, t(20));
        let tree = b.finish();

        let projects = find_projects_with(&tree, &Rules::builtin(), &[]);
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].node, app);
        assert_eq!(projects[0].parent, None);
        assert_eq!(projects[1].node, inner);
        assert_eq!(projects[1].parent, Some(0));
        assert_eq!(projects[0].reclaimable_allocated, 1000);
        assert_eq!(projects[0].last_activity, t(20));
        assert_eq!(projects[0].total_allocated, tree.node(app).allocated);
    }

    #[test]
    fn skip_dirs_are_not_descended() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), ".git/Cargo.toml");
        let mut b = TreeBuilder::new(tmp.path(), None);
        let git = b.add_dir(b.root(), ".git", None);
        b.add_file(git, "Cargo.toml", 0, 0, t(1));
        let tree = b.finish();
        assert!(find_projects_with(&tree, &Rules::builtin(), &[]).is_empty());
    }

    #[test]
    fn ignored_paths_are_not_descended_case_insensitively() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "Apps/x/Cargo.toml");
        touch(tmp.path(), "Appsmith/Cargo.toml");
        let mut b = TreeBuilder::new(tmp.path(), None);
        let apps = b.add_dir(b.root(), "Apps", None);
        let x = b.add_dir(apps, "x", None);
        b.add_file(x, "Cargo.toml", 0, 0, t(1));
        let smith = b.add_dir(b.root(), "Appsmith", None);
        b.add_file(smith, "Cargo.toml", 0, 0, t(1));
        let tree = b.finish();

        // Component-wise: "apps" must not also block "Appsmith".
        let ignore = [PathBuf::from(tmp.path().to_string_lossy().to_uppercase()).join("apps")];
        let projects = find_projects_with(&tree, &Rules::builtin(), &ignore);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].node, smith);
    }

    #[test]
    fn last_activity_counts_direct_child_files() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "p/Cargo.toml");
        let mut b = TreeBuilder::new(tmp.path(), None);
        let p = b.add_dir(b.root(), "p", None);
        b.add_file(p, "Cargo.toml", 0, 0, t(5));
        b.add_file(p, "README.md", 0, 0, t(50));
        let tree = b.finish();
        let projects = find_projects_with(&tree, &Rules::builtin(), &[]);
        assert_eq!(projects[0].last_activity, t(50));
    }
}
