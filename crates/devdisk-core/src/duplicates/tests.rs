use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use super::*;
use crate::index::TreeBuilder;
use crate::projects::find_projects;
use crate::scanner::{NoProgress, ScanOptions, Scanner, WalkScanner};

fn write(path: &Path, content: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn scan(root: &Path) -> Tree {
    WalkScanner
        .scan(root, &ScanOptions::default(), &NoProgress)
        .unwrap()
        .tree
}

/// Small sizes so tests can exercise both hash passes with tiny files.
fn opts() -> DupOptions {
    DupOptions {
        min_size: 1,
        partial_bytes: 4,
        ..DupOptions::default()
    }
}

fn find(tree: &Tree) -> DupResult {
    find_duplicates(tree, &[], &opts(), &NoDupProgress).unwrap()
}

fn names(tree: &Tree, group: &DuplicateGroup) -> Vec<String> {
    group
        .files
        .iter()
        .map(|&id| {
            tree.path(id)
                .strip_prefix(tree.root_path())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

fn set_modified(path: &Path, secs_ago: u64) {
    let f = fs::File::options().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(secs_ago))
        .unwrap();
}

#[test]
fn identical_files_form_one_group() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    write(&r.join("a/photo.jpg"), b"same content here");
    write(&r.join("b/copy.jpg"), b"same content here");
    write(&r.join("c/deep/again.jpg"), b"same content here");
    write(&r.join("other.jpg"), b"diff content here");
    let tree = scan(r);

    let res = find(&tree);
    assert_eq!(res.groups.len(), 1, "{:?}", res.groups);
    let g = &res.groups[0];
    assert_eq!(g.size, 17);
    assert_eq!(g.hash.len(), 64);
    assert!(
        g.hash
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    let mut got = names(&tree, g);
    got.sort();
    assert_eq!(got, ["a/photo.jpg", "b/copy.jpg", "c/deep/again.jpg"]);
    let one = tree.node(g.files[0]).allocated;
    assert_eq!(reclaimable(&tree, g), 2 * one);
    assert!(res.issues.is_empty(), "{:?}", res.issues);
}

#[test]
fn same_size_but_different_content_is_not_a_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("x"), b"aaaa");
    write(&dir.path().join("y"), b"bbbb");
    assert!(find(&scan(dir.path())).groups.is_empty());
}

#[test]
fn same_start_but_different_end_is_not_a_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    // The first 4 bytes (the partial block in these tests) match, the rest does not.
    write(&dir.path().join("x"), b"head-one-tail");
    write(&dir.path().join("y"), b"head-two-tail");
    write(&dir.path().join("z"), b"head-one-tail");
    let tree = scan(dir.path());
    let res = find(&tree);
    assert_eq!(res.groups.len(), 1);
    let mut got = names(&tree, &res.groups[0]);
    got.sort();
    assert_eq!(got, ["x", "z"]);
}

#[test]
fn small_and_empty_files_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("e1"), b"");
    write(&dir.path().join("e2"), b"");
    write(&dir.path().join("s1"), b"tiny");
    write(&dir.path().join("s2"), b"tiny");
    let tree = scan(dir.path());
    let o = DupOptions {
        min_size: 5,
        ..opts()
    };
    assert!(
        find_duplicates(&tree, &[], &o, &NoDupProgress)
            .unwrap()
            .groups
            .is_empty()
    );
    // Empty files are never duplicates worth showing, whatever the threshold.
    let o = DupOptions {
        min_size: 0,
        ..opts()
    };
    let res = find_duplicates(&tree, &[], &o, &NoDupProgress).unwrap();
    assert_eq!(res.groups.len(), 1);
    assert_eq!(res.groups[0].size, 4);
}

#[test]
fn cloud_only_files_are_never_opened() {
    let dir = tempfile::tempdir().unwrap();
    // Neither file exists on disk: opening one would show up as an issue.
    let mut b = TreeBuilder::new(dir.path(), None);
    let root = b.root();
    b.add_file(root, "cloud1.mp4", 5000, 0, None);
    b.add_file(root, "cloud2.mp4", 5000, 0, None);
    let tree = b.finish();
    let res = find(&tree);
    assert!(res.groups.is_empty());
    assert!(res.issues.is_empty(), "{:?}", res.issues);
}

#[test]
fn files_inside_skipped_folders_are_left_out() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("keep/a.bin"), b"twin");
    write(&dir.path().join("skip/inner/b.bin"), b"twin");
    let tree = scan(dir.path());
    let skip = tree.find(&dir.path().join("skip")).unwrap();
    let res = find_duplicates(&tree, &[skip], &opts(), &NoDupProgress).unwrap();
    assert!(res.groups.is_empty());
}

#[test]
fn detached_files_are_left_out() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("a"), b"twin");
    write(&dir.path().join("b"), b"twin");
    let mut tree = scan(dir.path());
    tree.detach(tree.find(&dir.path().join("b")).unwrap());
    assert!(find(&tree).groups.is_empty());
}

#[test]
fn files_changed_or_gone_since_the_scan_are_reported_and_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    write(&r.join("a"), b"twin");
    write(&r.join("b"), b"twin");
    write(&r.join("c"), b"twin");
    let tree = scan(r);
    write(&r.join("b"), b"twin, longer now");
    fs::remove_file(r.join("c")).unwrap();

    let res = find(&tree);
    assert!(res.groups.is_empty(), "{:?}", res.groups);
    let mut issues: Vec<_> = res
        .issues
        .iter()
        .map(|i| i.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    issues.sort();
    assert_eq!(issues, ["b", "c"]);
    let changed = res.issues.iter().find(|i| i.path.ends_with("b")).unwrap();
    assert!(changed.message.contains("changed"), "{}", changed.message);
}

#[test]
fn hard_linked_files_are_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    write(&r.join("a"), b"linked data");
    fs::hard_link(r.join("a"), r.join("a-link")).unwrap();
    write(&r.join("b"), b"linked data");
    let res = find(&scan(r));
    // `a` and `a-link` are one file on disk; deleting either frees nothing.
    assert!(res.groups.is_empty(), "{:?}", res.groups);
    assert_eq!(res.hard_linked, 2);
}

#[test]
fn groups_by_reclaimable_and_files_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    write(&r.join("small/new"), b"pair");
    write(&r.join("small/old"), b"pair");
    let big = vec![9u8; 64 * 1024];
    write(&r.join("big/1"), &big);
    write(&r.join("big/2"), &big);
    set_modified(&r.join("small/new"), 10);
    set_modified(&r.join("small/old"), 10_000);
    let tree = scan(r);

    let res = find(&tree);
    assert_eq!(res.groups.len(), 2);
    assert_eq!(res.groups[0].size, 64 * 1024);
    assert_eq!(names(&tree, &res.groups[1]), ["small/old", "small/new"]);
}

#[test]
fn cancelled_search_returns_an_error() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("a"), b"twin");
    write(&dir.path().join("b"), b"twin");
    let tree = scan(dir.path());
    let o = DupOptions {
        cancel: Some(Arc::new(AtomicBool::new(true))),
        ..opts()
    };
    let err = find_duplicates(&tree, &[], &o, &NoDupProgress).err();
    assert!(matches!(err, Some(DupError::Cancelled)), "{err:?}");
}

#[test]
fn progress_ends_with_the_full_pass_totals() {
    struct Collect(Mutex<Vec<DupProgress>>);
    impl DupProgressSink for Collect {
        fn report(&self, p: &DupProgress) {
            self.0.lock().unwrap().push(p.clone());
        }
    }
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("a"), b"twin-long");
    write(&dir.path().join("b"), b"twin-long");
    write(&dir.path().join("c"), b"diff-long");
    write(&dir.path().join("lonely"), b"no other file this size");
    let sink = Collect(Mutex::new(Vec::new()));
    find_duplicates(&scan(dir.path()), &[], &opts(), &sink).unwrap();

    let all = sink.0.lock().unwrap();
    let partial = all
        .iter()
        .rev()
        .find(|p| p.phase == DupPhase::Partial)
        .expect("a partial report");
    assert_eq!((partial.files_done, partial.files_total), (3, 3));
    let last = all.last().unwrap();
    assert_eq!(last.phase, DupPhase::Full);
    // "twin" and "diff" differ in the first block, so only a and b are read in full.
    assert_eq!((last.files_done, last.files_total), (2, 2));
    assert_eq!((last.bytes_done, last.bytes_total), (18, 18));
}

#[test]
fn default_skips_cover_dependency_build_and_git_folders() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    write(
        &r.join("shop/package.json"),
        br#"{"dependencies":{"react":"18"}}"#,
    );
    write(&r.join("shop/package-lock.json"), b"{}");
    write(&r.join("shop/node_modules/x/index.js"), b"x");
    write(&r.join("shop/dist/app.js"), b"x");
    write(&r.join("shop/src/app.js"), b"x");
    write(&r.join("shop/.git/HEAD"), b"x");
    let tree = scan(r);
    let projects = find_projects(&tree, &Rules::builtin());

    let skips = default_skips(&tree, &projects, &Rules::builtin());
    let mut got: Vec<String> = skips
        .iter()
        .map(|&id| {
            tree.path(id)
                .strip_prefix(r)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    got.sort();
    got.dedup();
    assert_eq!(got, ["shop/.git", "shop/dist", "shop/node_modules"]);
}

#[test]
fn without_a_kept_copy_flags_groups_whose_every_copy_is_selected() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    write(&r.join("a/one"), b"twin");
    write(&r.join("b/two"), b"twin");
    write(&r.join("c/three"), b"trio");
    write(&r.join("d/four"), b"trio");
    let tree = scan(r);
    let res = find(&tree);
    assert_eq!(res.groups.len(), 2);
    let id = |p: &str| tree.find(&r.join(p)).unwrap();

    // One copy of each group stays: nothing to flag.
    let sel = [id("a/one"), id("c/three")];
    assert!(without_a_kept_copy(&tree, &res.groups, &sel).is_empty());

    // Both twins go, one directly and one inside a selected folder; only the file the user
    // picked as a duplicate is returned.
    let sel = [id("a/one"), id("b"), id("c/three")];
    assert_eq!(without_a_kept_copy(&tree, &res.groups, &sel), [id("a/one")]);
}

#[test]
fn reclaimable_ignores_detached_copies() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("a"), b"twin");
    write(&dir.path().join("b"), b"twin");
    write(&dir.path().join("c"), b"twin");
    let mut tree = scan(dir.path());
    let res = find(&tree);
    let one = tree.node(res.groups[0].files[0]).allocated;
    tree.detach(tree.find(&dir.path().join("c")).unwrap());
    assert_eq!(reclaimable(&tree, &res.groups[0]), one);
    tree.detach(tree.find(&dir.path().join("b")).unwrap());
    assert_eq!(reclaimable(&tree, &res.groups[0]), 0);
}
