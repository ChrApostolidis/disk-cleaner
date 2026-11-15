//! Real folders, real scanner: scan a temp dir and check which projects come out.

use devdisk_core::projects::{Project, find_projects};
use devdisk_core::rules::Rules;
use devdisk_core::scanner::{NoProgress, ScanOptions, Scanner, WalkScanner};
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

fn write(root: &Path, rel: &str, bytes: usize) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, vec![b'x'; bytes]).unwrap();
}

fn set_mtime(root: &Path, rel: &str, t: SystemTime) {
    let f = fs::File::options()
        .write(true)
        .open(root.join(rel))
        .unwrap();
    f.set_modified(t).unwrap();
}

fn scan(root: &Path) -> (devdisk_core::index::Tree, Vec<Project>) {
    let result = WalkScanner
        .scan(root, &ScanOptions::default(), &NoProgress)
        .unwrap();
    let projects = find_projects(&result.tree, &Rules::builtin());
    (result.tree, projects)
}

fn rel(tree: &devdisk_core::index::Tree, p: &Project) -> String {
    tree.path(p.node)
        .strip_prefix(tree.root_path())
        .unwrap()
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

#[test]
fn nextjs_app_is_found_once_with_reclaimable_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    write(r, "web/package.json", 0);
    fs::write(
        r.join("web/package.json"),
        r#"{"dependencies":{"next":"^14.2.3","react":"18"}}"#,
    )
    .unwrap();
    write(r, "web/package-lock.json", 10);
    write(r, "web/src/index.js", 100);
    write(r, "web/node_modules/left-pad/package.json", 50_000);
    write(r, "web/node_modules/big/blob.bin", 200_000);
    write(r, "web/.next/cache/a.bin", 120_000);

    let (tree, projects) = scan(r);
    assert_eq!(
        projects.len(),
        1,
        "{:?}",
        projects.iter().map(|p| rel(&tree, p)).collect::<Vec<_>>()
    );
    let p = &projects[0];
    assert_eq!(rel(&tree, p), "web");
    assert_eq!(p.info.label, "Next.js");
    let nm = tree.child_by_name(p.node, "node_modules").unwrap();
    let next = tree.child_by_name(p.node, ".next").unwrap();
    assert_eq!(
        p.reclaimable_allocated,
        tree.node(nm).allocated + tree.node(next).allocated
    );
    assert!(p.reclaimable_allocated > 0);
}

#[test]
fn rust_project_with_target() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    write(r, "tool/Cargo.toml", 0);
    write(r, "tool/src/main.rs", 100);
    write(r, "tool/target/debug/tool.exe", 300_000);

    let (tree, projects) = scan(r);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].info.label, "Rust");
    let target = tree.child_by_name(projects[0].node, "target").unwrap();
    assert_eq!(
        projects[0].reclaimable_allocated,
        tree.node(target).allocated
    );
}

#[test]
fn monorepo_package_points_at_its_root() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    write(r, "mono/package.json", 0);
    write(r, "mono/packages/a/package.json", 0);
    write(r, "mono/packages/a/node_modules/x/y.bin", 40_000);
    write(r, "mono/node_modules/z/z.bin", 40_000);

    let (tree, projects) = scan(r);
    let names: Vec<String> = projects.iter().map(|p| rel(&tree, p)).collect();
    assert_eq!(names, ["mono", "mono/packages/a"]);
    assert_eq!(projects[0].parent, None);
    assert_eq!(projects[1].parent, Some(0));
    // The nested project's regenerable dir is not counted in its parent's reclaimable.
    let root_nm = tree
        .child_by_name(projects[0].node, "node_modules")
        .unwrap();
    assert_eq!(
        projects[0].reclaimable_allocated,
        tree.node(root_nm).allocated
    );
}

#[test]
fn fresh_install_does_not_make_project_look_active() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    write(r, "old/package.json", 5);
    write(r, "old/index.js", 5);
    write(r, "old/node_modules/new.js", 5);
    let old = SystemTime::now() - Duration::from_secs(400 * 86_400);
    set_mtime(r, "old/package.json", old);
    set_mtime(r, "old/index.js", old);

    let (_tree, projects) = scan(r);
    assert_eq!(projects.len(), 1);
    let activity = projects[0].last_activity.unwrap();
    let diff = activity.duration_since(old).unwrap_or_default();
    assert!(diff < Duration::from_secs(5), "activity {diff:?} after old");
}

#[test]
fn plain_folder_is_not_a_project() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "docs/notes.txt", 10);
    let (_tree, projects) = scan(tmp.path());
    assert!(projects.is_empty());
}
