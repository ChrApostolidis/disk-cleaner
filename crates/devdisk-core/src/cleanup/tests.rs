//! Nothing here touches the real Recycle Bin, git or registry except the `#[ignore]` tests,
//! which only ever act on files inside a tempdir created by the test itself.

use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use super::env::{bin_info_from, volume_guid};
use super::*;
use crate::index::TreeBuilder;
use crate::projects::find_projects;
use crate::scanner::{NoProgress, ScanOptions, Scanner, WalkScanner};

// ---------- fakes ----------

struct FakeEnv {
    tracked: HashSet<PathBuf>,
    git_missing: bool,
    bin: RecycleBinInfo,
    profile: Option<PathBuf>,
}

impl Default for FakeEnv {
    fn default() -> Self {
        Self {
            tracked: HashSet::new(),
            git_missing: false,
            bin: RecycleBinInfo::Enabled { max_bytes: 1 << 40 },
            profile: Some(PathBuf::from("Z:\\profile")),
        }
    }
}

impl Environment for FakeEnv {
    fn git_tracked(&self, path: &Path) -> Option<bool> {
        (!self.git_missing).then(|| self.tracked.contains(path))
    }
    fn recycle_bin(&self, _path: &Path) -> RecycleBinInfo {
        self.bin
    }
    fn user_profile(&self) -> Option<PathBuf> {
        self.profile.clone()
    }
}

#[derive(Default)]
struct FakeMover {
    fail: HashSet<PathBuf>,
    moved: RefCell<Vec<PathBuf>>,
}

impl Mover for FakeMover {
    fn to_recycle_bin(&self, path: &Path) -> Result<(), String> {
        if self.fail.contains(path) {
            return Err("access denied".into());
        }
        self.moved.borrow_mut().push(path.to_path_buf());
        Ok(())
    }
}

struct FakeBin {
    items: Vec<trash::TrashItem>,
    collide: bool,
    restored: RefCell<Vec<OsString>>,
}

impl BinAccess for FakeBin {
    fn list(&self) -> Result<Vec<trash::TrashItem>, String> {
        Ok(self.items.clone())
    }
    fn restore(&self, item: trash::TrashItem) -> Result<(), BinError> {
        if self.collide {
            return Err(BinError::Collision);
        }
        self.restored.borrow_mut().push(item.id);
        Ok(())
    }
}

fn trash_item(id: &str, path: &Path, deleted_secs: i64) -> trash::TrashItem {
    trash::TrashItem {
        id: id.into(),
        name: path.file_name().unwrap().to_owned(),
        original_parent: path.parent().unwrap().to_owned(),
        time_deleted: deleted_secs,
    }
}

// ---------- fixtures ----------

fn write(path: &Path, len: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![1u8; len]).unwrap();
}

/// Rules without the machine-specific ignore paths, plus the given literal ones.
fn rules(ignore: &[&Path]) -> Rules {
    let mut v = serde_json::to_value(Rules::builtin()).unwrap();
    v["ignore_paths"] = serde_json::json!(
        ignore
            .iter()
            .map(|p| p.to_string_lossy())
            .collect::<Vec<_>>()
    );
    Rules::from_json(&v.to_string()).unwrap()
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    tree: Tree,
    projects: Vec<Project>,
    rules: Rules,
}

impl Fixture {
    fn new() -> Self {
        Self::with_rules(rules(&[]))
    }

    fn with_rules(rules: Rules) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        write(&root.join("web/package.json"), 20);
        write(&root.join("web/node_modules/a.js"), 5000);
        write(&root.join("web/dist/b.js"), 3000);
        write(&root.join("web/src/main.js"), 100);
        write(&root.join("api/Cargo.toml"), 20);
        write(&root.join("api/Cargo.lock"), 20);
        write(&root.join("api/target/x.bin"), 8000);
        write(&root.join("nolock/Cargo.toml"), 20);
        write(&root.join("nolock/target/y.bin"), 4000);
        write(&root.join("locked/package.json"), 20);
        write(&root.join("locked/package-lock.json"), 20);
        write(&root.join("locked/node_modules/c.js"), 4000);
        write(&root.join("loose/file.txt"), 700);
        let res = WalkScanner
            .scan(&root, &ScanOptions::default(), &NoProgress)
            .unwrap();
        let projects = find_projects(&res.tree, &rules);
        Self {
            _dir: dir,
            root,
            tree: res.tree,
            projects,
            rules,
        }
    }

    fn ctx<'a>(&'a self, protected: &'a [PathBuf]) -> CleanupContext<'a> {
        CleanupContext {
            tree: &self.tree,
            projects: &self.projects,
            rules: &self.rules,
            protected,
        }
    }

    fn preview(&self, rel: &[&str], env: &FakeEnv) -> Preview {
        let paths: Vec<PathBuf> = rel.iter().map(|r| self.root.join(r)).collect();
        preview(&self.ctx(&[]), &paths, env).unwrap()
    }

    fn one(&self, rel: &str, env: &FakeEnv) -> PreviewItem {
        self.preview(&[rel], env).items.remove(0)
    }
}

/// Synthetic tree (paths need not exist): C:\r\{ .git\config, src\a, repo\ }.
fn synthetic() -> Tree {
    let mut b = TreeBuilder::new(Path::new("C:\\r"), None);
    let root = b.root();
    let git = b.add_dir(root, ".git", None);
    b.add_file(git, "config", 10, 4096, None);
    let src = b.add_dir(root, "src", None);
    b.add_file(src, "a", 10, 4096, None);
    b.finish()
}

fn reasons_of(item: &PreviewItem) -> String {
    item.reasons.join(" | ")
}

// ---------- refuse ----------

#[test]
fn refuses_drive_root() {
    let mut b = TreeBuilder::new(Path::new("C:\\"), None);
    b.add_dir(b.root(), "x", None);
    let tree = b.finish();
    let ctx = CleanupContext {
        tree: &tree,
        projects: &[],
        rules: &rules(&[]),
        protected: &[],
    };
    let p = preview(&ctx, &[PathBuf::from("C:\\")], &FakeEnv::default()).unwrap();
    assert_eq!(p.items[0].verdict, Verdict::Refuse);
    assert!(reasons_of(&p.items[0]).contains("drive root"));
    assert_eq!(p.total_freed, 0);
}

#[test]
fn refuses_ignore_paths_and_inside_them() {
    let fx = Fixture::new();
    let ignored = rules(&[&fx.root.join("web")]);
    let ctx = CleanupContext {
        tree: &fx.tree,
        projects: &fx.projects,
        rules: &ignored,
        protected: &[],
    };
    let env = FakeEnv::default();
    let paths = [fx.root.join("web"), fx.root.join("loose")];
    let p = preview(&ctx, &paths[..1], &env).unwrap();
    assert_eq!(p.items[0].verdict, Verdict::Refuse);
    assert!(reasons_of(&p.items[0]).contains("protected system folder"));
    let inner = [fx.root.join("web/dist")];
    let p = preview(&ctx, &inner, &env).unwrap();
    assert_eq!(p.items[0].verdict, Verdict::Refuse);
    let p = preview(&ctx, &paths[1..], &env).unwrap();
    assert_eq!(p.items[0].verdict, Verdict::Ok);
}

#[test]
fn refuses_profile_and_its_ancestors() {
    let fx = Fixture::new();
    let env = FakeEnv {
        profile: Some(fx.root.join("loose/deep/profile")),
        ..Default::default()
    };
    // The profile is below `loose`, so removing `loose` would remove it too.
    let item = fx.one("loose", &env);
    assert_eq!(item.verdict, Verdict::Refuse);
    assert!(reasons_of(&item).contains("user profile"));
    let env = FakeEnv {
        profile: Some(fx.root.join("loose")),
        ..Default::default()
    };
    assert_eq!(fx.one("loose", &env).verdict, Verdict::Refuse);
    // A folder inside the profile is fine.
    assert_eq!(fx.one("loose/file.txt", &env).verdict, Verdict::Ok);
}

#[test]
fn refuses_protected_paths() {
    let fx = Fixture::new();
    let protected = [fx.root.join("loose")];
    let ctx = fx.ctx(&protected);
    let env = FakeEnv::default();
    for p in ["loose", "loose/file.txt"] {
        let item = preview(&ctx, &[fx.root.join(p)], &env)
            .unwrap()
            .items
            .remove(0);
        assert_eq!(item.verdict, Verdict::Refuse, "{p}");
    }
}

#[test]
fn refuses_outside_root_without_erroring() {
    let fx = Fixture::new();
    let outside = std::env::temp_dir().join("devdisk-definitely-not-scanned");
    let p = preview(&fx.ctx(&[]), &[outside], &FakeEnv::default()).unwrap();
    assert_eq!(p.items[0].verdict, Verdict::Refuse);
    assert!(reasons_of(&p.items[0]).contains("outside the scanned folder"));
}

#[test]
fn path_inside_root_but_not_scanned_is_an_error() {
    let fx = Fixture::new();
    let err = preview(&fx.ctx(&[]), &[fx.root.join("nope")], &FakeEnv::default()).unwrap_err();
    assert!(matches!(err, CleanupError::NotInScan(_)));
}

#[test]
fn detached_path_is_no_longer_in_the_scan() {
    let mut fx = Fixture::new();
    let id = fx.tree.find(&fx.root.join("loose")).unwrap();
    fx.tree.detach(id);
    let err = preview(&fx.ctx(&[]), &[fx.root.join("loose")], &FakeEnv::default()).unwrap_err();
    assert!(matches!(err, CleanupError::NotInScan(_)));
}

#[test]
fn refuses_dot_git() {
    let tree = synthetic();
    let rules = rules(&[]);
    let ctx = CleanupContext {
        tree: &tree,
        projects: &[],
        rules: &rules,
        protected: &[],
    };
    let env = FakeEnv::default();
    for p in ["C:\\r\\.git", "C:\\r\\.GIT\\config"] {
        let item = preview(&ctx, &[PathBuf::from(p)], &env)
            .unwrap()
            .items
            .remove(0);
        assert_eq!(item.verdict, Verdict::Refuse, "{p}");
        assert!(reasons_of(&item).contains(".git"));
    }
}

#[test]
fn refuses_git_tracked() {
    let fx = Fixture::new();
    let mut env = FakeEnv::default();
    env.tracked.insert(fx.root.join("web/dist"));
    let item = fx.one("web/dist", &env);
    assert_eq!(item.verdict, Verdict::Refuse);
    assert!(reasons_of(&item).contains("tracked by git"));
    assert_eq!(fx.one("web/src", &env).verdict, Verdict::Ok);
}

#[test]
fn refuses_when_bin_disabled() {
    let fx = Fixture::new();
    let env = FakeEnv {
        bin: RecycleBinInfo::Disabled,
        ..Default::default()
    };
    let item = fx.one("loose", &env);
    assert_eq!(item.verdict, Verdict::Refuse);
    assert!(reasons_of(&item).contains("Recycle Bin is turned off for this drive"));
}

#[test]
fn refuses_when_too_big_for_bin() {
    let fx = Fixture::new();
    let env = FakeEnv {
        bin: RecycleBinInfo::Enabled { max_bytes: 100 },
        ..Default::default()
    };
    let item = fx.one("loose", &env);
    assert_eq!(item.verdict, Verdict::Refuse);
    assert!(reasons_of(&item).contains("too big for the Recycle Bin"));
    // Exactly at the limit still fits.
    let fits = FakeEnv {
        bin: RecycleBinInfo::Enabled {
            max_bytes: item.allocated,
        },
        ..Default::default()
    };
    assert_eq!(fx.one("loose", &fits).verdict, Verdict::Ok);
}

#[test]
fn all_reasons_are_collected() {
    let fx = Fixture::new();
    let mut env = FakeEnv {
        bin: RecycleBinInfo::Disabled,
        ..Default::default()
    };
    env.tracked.insert(fx.root.join("loose"));
    let item = fx.one("loose", &env);
    assert_eq!(item.reasons.len(), 2);
}

// ---------- warn ----------

#[test]
fn warns_for_dependency_folder_without_lockfile_only() {
    let fx = Fixture::new();
    let env = FakeEnv::default();
    let nm = fx.one("web/node_modules", &env);
    assert_eq!(nm.verdict, Verdict::Warn);
    assert!(reasons_of(&nm).contains("no lockfile"));
    // Build outputs are not dependency folders.
    assert_eq!(fx.one("web/dist", &env).verdict, Verdict::Ok);
    assert_eq!(fx.one("nolock/target", &env).verdict, Verdict::Ok);
    // With a lockfile the dependency folder is fine.
    assert_eq!(fx.one("locked/node_modules", &env).verdict, Verdict::Ok);
}

#[test]
fn warns_when_bin_size_unknown() {
    let fx = Fixture::new();
    let env = FakeEnv {
        bin: RecycleBinInfo::Unknown,
        ..Default::default()
    };
    let item = fx.one("loose", &env);
    assert_eq!(item.verdict, Verdict::Warn);
    assert!(reasons_of(&item).contains("could not check the Recycle Bin size"));
}

#[test]
fn warns_when_git_unavailable_inside_a_project_only() {
    let fx = Fixture::new();
    let env = FakeEnv {
        git_missing: true,
        ..Default::default()
    };
    let inside = fx.one("api/target", &env);
    assert_eq!(inside.verdict, Verdict::Warn);
    assert!(reasons_of(&inside).contains("could not check git"));
    assert_eq!(fx.one("loose", &env).verdict, Verdict::Ok);
}

// ---------- fields ----------

#[test]
fn fills_regenerable_and_project_fields() {
    let fx = Fixture::new();
    let env = FakeEnv::default();
    let target = fx.one("api/target", &env);
    assert!(target.regenerable);
    assert_eq!(target.kind, ItemKind::Dir);
    assert_eq!(target.name, "target");
    assert_eq!(
        target.project_path.as_deref(),
        Some(fx.root.join("api").as_path())
    );
    assert!(target.project_label.is_some());
    assert!(target.allocated >= 8000);
    // Not in the project's regenerable list.
    let src = fx.one("web/src", &env);
    assert!(!src.regenerable);
    assert!(src.project_path.is_some());
    // Deeper than a direct child: not regenerable, but still has its project.
    let deep = fx.one("api/target/x.bin", &env);
    assert!(!deep.regenerable);
    assert_eq!(deep.kind, ItemKind::File);
    assert_eq!(
        deep.project_path.as_deref(),
        Some(fx.root.join("api").as_path())
    );
    let loose = fx.one("loose", &env);
    assert!(loose.project_path.is_none() && loose.project_label.is_none());
}

#[test]
fn total_freed_excludes_refused_items() {
    let fx = Fixture::new();
    let mut env = FakeEnv::default();
    env.tracked.insert(fx.root.join("web/dist"));
    let p = fx.preview(&["web/dist", "api/target", "loose"], &env);
    let ok: u64 = p
        .items
        .iter()
        .filter(|i| i.verdict != Verdict::Refuse)
        .map(|i| i.allocated)
        .sum();
    assert_eq!(p.total_freed, ok);
    assert!(p.total_freed > 0);
    assert_eq!(p.items.len(), 3);
    assert!(p.items.iter().any(|i| i.verdict == Verdict::Refuse));
}

#[test]
fn duplicate_and_nested_paths_collapse_to_the_folder() {
    let fx = Fixture::new();
    let p = fx.preview(
        &[
            "web/node_modules/a.js",
            "web/node_modules",
            "WEB/Node_Modules",
            "web/dist",
            "web/dist",
        ],
        &FakeEnv::default(),
    );
    let names: Vec<_> = p.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["node_modules", "dist"]);
}

#[test]
fn serializes_to_the_ui_shape() {
    let fx = Fixture::new();
    let p = fx.preview(&["api/target"], &FakeEnv::default());
    let v = serde_json::to_value(&p).unwrap();
    assert!(v["totalFreed"].is_u64());
    let item = &v["items"][0];
    assert_eq!(item["kind"], "dir");
    assert_eq!(item["verdict"], "ok");
    assert!(item["projectPath"].is_string());
    assert!(item["reasons"].as_array().unwrap().is_empty());
    let v =
        serde_json::to_value(fx.preview(&["loose"], &FakeEnv::default()).items[0].clone()).unwrap();
    assert!(v["projectPath"].is_null());
}

// ---------- recycle ----------

struct Run {
    outcomes: Vec<RecycleOutcome>,
    progress: Vec<(usize, usize, PathBuf)>,
}

fn run_recycle(
    fx: &Fixture,
    rel: &[&str],
    env: &FakeEnv,
    mover: &FakeMover,
    log: &CleanupLog,
    batch: &str,
) -> Run {
    let paths: Vec<PathBuf> = rel.iter().map(|r| fx.root.join(r)).collect();
    let mut progress = Vec::new();
    let outcomes = recycle(
        &fx.ctx(&[]),
        &paths,
        env,
        mover,
        log,
        batch,
        &mut |d, t, p| progress.push((d, t, p.to_path_buf())),
    )
    .unwrap();
    Run { outcomes, progress }
}

fn open_log(dir: &Path) -> CleanupLog {
    CleanupLog::open(&dir.join("log.db")).unwrap()
}

#[test]
fn recycle_only_moves_non_refused_items_and_logs_them() {
    let fx = Fixture::new();
    let logdir = tempfile::tempdir().unwrap();
    let log = open_log(logdir.path());
    let mut env = FakeEnv::default();
    env.tracked.insert(fx.root.join("web/dist"));
    let mover = FakeMover::default();
    let run = run_recycle(
        &fx,
        &["web/dist", "api/target", "web/node_modules"],
        &env,
        &mover,
        &log,
        "b1",
    );
    assert_eq!(
        *mover.moved.borrow(),
        [fx.root.join("api/target"), fx.root.join("web/node_modules")]
    );
    let refused = &run.outcomes[0];
    assert!(!refused.ok && refused.entry_id.is_none());
    assert!(refused.error.as_deref().unwrap().contains("tracked by git"));
    assert!(run.outcomes[1].ok && run.outcomes[2].ok);
    let entries = log.list().unwrap();
    assert_eq!(entries.len(), 2);
    assert!(
        entries
            .iter()
            .all(|e| e.batch_id == "b1" && e.restored_at.is_none())
    );
    let target = entries.iter().find(|e| e.path.ends_with("target")).unwrap();
    assert_eq!(Some(target.id), run.outcomes[1].entry_id);
    assert_eq!(
        target.project_path.as_deref(),
        Some(fx.root.join("api").as_path())
    );
    assert!(target.allocated >= 8000);
}

#[test]
fn recycle_reports_mover_failures_and_does_not_log_them() {
    let fx = Fixture::new();
    let logdir = tempfile::tempdir().unwrap();
    let log = open_log(logdir.path());
    let mut mover = FakeMover::default();
    mover.fail.insert(fx.root.join("loose"));
    let run = run_recycle(
        &fx,
        &["loose", "api/target"],
        &FakeEnv::default(),
        &mover,
        &log,
        "b2",
    );
    assert!(!run.outcomes[0].ok);
    assert_eq!(run.outcomes[0].error.as_deref(), Some("access denied"));
    assert!(run.outcomes[1].ok);
    let entries = log.list().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].path.ends_with("target"));
}

#[test]
fn recycle_reports_progress_per_attempted_item() {
    let fx = Fixture::new();
    let logdir = tempfile::tempdir().unwrap();
    let log = open_log(logdir.path());
    let mut env = FakeEnv::default();
    env.tracked.insert(fx.root.join("web/dist"));
    let run = run_recycle(
        &fx,
        &["web/dist", "api/target", "loose"],
        &env,
        &FakeMover::default(),
        &log,
        "b3",
    );
    let seen: Vec<_> = run.progress.iter().map(|(d, t, _)| (*d, *t)).collect();
    assert_eq!(seen, [(0, 2), (1, 2)]);
    assert!(run.progress[0].2.ends_with("target"));
}

#[test]
fn recycle_refuses_everything_when_bin_is_off() {
    let fx = Fixture::new();
    let logdir = tempfile::tempdir().unwrap();
    let log = open_log(logdir.path());
    let env = FakeEnv {
        bin: RecycleBinInfo::Disabled,
        ..Default::default()
    };
    let mover = FakeMover::default();
    let run = run_recycle(&fx, &["loose", "api/target"], &env, &mover, &log, "b4");
    assert!(mover.moved.borrow().is_empty());
    assert!(run.outcomes.iter().all(|o| !o.ok));
    assert!(log.list().unwrap().is_empty());
}

// ---------- log ----------

#[test]
fn log_lists_newest_first_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("sub.db");
    {
        let log = CleanupLog::open(&db).unwrap();
        assert!(log.list().unwrap().is_empty());
        for (i, ms) in [(1, 1000), (2, 3000), (3, 2000)] {
            log.insert(&log::NewEntry {
                batch_id: "b",
                deleted_at: ms,
                path: Path::new(&format!("C:\\p{i}")),
                allocated: 5,
                project_path: None,
                project_label: Some("L"),
            })
            .unwrap();
        }
    }
    let log = CleanupLog::open(&db).unwrap();
    let paths: Vec<_> = log.list().unwrap().into_iter().map(|e| e.path).collect();
    assert_eq!(
        paths,
        [
            Path::new("C:\\p2"),
            Path::new("C:\\p3"),
            Path::new("C:\\p1")
        ]
    );
    let json = serde_json::to_value(&log.list().unwrap()[0]).unwrap();
    assert_eq!(json["batchId"], "b");
    assert_eq!(json["deletedAt"], 3000);
    assert!(json["restoredAt"].is_null());
    assert_eq!(json["projectLabel"], "L");
}

// ---------- restore ----------

fn logged(log: &CleanupLog, path: &Path, deleted_ms: i64) -> i64 {
    log.insert(&log::NewEntry {
        batch_id: "b",
        deleted_at: deleted_ms,
        path,
        allocated: 1,
        project_path: None,
        project_label: None,
    })
    .unwrap()
}

fn bin(items: Vec<trash::TrashItem>) -> FakeBin {
    FakeBin {
        items,
        collide: false,
        restored: RefCell::default(),
    }
}

#[test]
fn restore_picks_matching_path_with_closest_time() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let p = Path::new("C:\\proj\\node_modules");
    let id = logged(&log, p, 5_000_000);
    let b = bin(vec![
        trash_item("old", p, 1_000),
        trash_item("near", &PathBuf::from("c:\\PROJ\\Node_Modules"), 5_002),
        trash_item("other", Path::new("C:\\proj\\dist"), 5_000),
    ]);
    let r = restore(&log, &[id], &b);
    assert_eq!(
        r,
        [RestoreOutcome {
            id,
            ok: true,
            error: None
        }]
    );
    assert_eq!(*b.restored.borrow(), [OsString::from("near")]);
    assert!(log.list().unwrap()[0].restored_at.is_some());
}

#[test]
fn restore_errors() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let p = Path::new("C:\\proj\\node_modules");
    let missing = logged(&log, p, 1000);
    let r = restore(&log, &[missing, 999], &bin(vec![]));
    assert_eq!(r[0].error.as_deref(), Some("no longer in the Recycle Bin"));
    assert_eq!(r[1].error.as_deref(), Some("unknown entry"));
    assert!(!r[0].ok && !r[1].ok);

    let collide = FakeBin {
        items: vec![trash_item("t", p, 1)],
        collide: true,
        restored: RefCell::default(),
    };
    let r = restore(&log, &[missing], &collide);
    assert_eq!(
        r[0].error.as_deref(),
        Some("something already exists at that path")
    );
    assert!(log.list().unwrap()[0].restored_at.is_none());

    let ok = bin(vec![trash_item("t", p, 1)]);
    assert!(restore(&log, &[missing], &ok)[0].ok);
    let again = restore(&log, &[missing], &ok);
    assert_eq!(again[0].error.as_deref(), Some("already put back"));
}

#[test]
fn restore_does_not_reuse_a_bin_item_twice() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let p = Path::new("C:\\proj\\node_modules");
    let a = logged(&log, p, 1_000_000);
    let b_id = logged(&log, p, 9_000_000);
    let b = bin(vec![trash_item("x", p, 1_000), trash_item("y", p, 9_000)]);
    let r = restore(&log, &[a, b_id], &b);
    assert!(r.iter().all(|o| o.ok));
    assert_eq!(
        *b.restored.borrow(),
        [OsString::from("x"), OsString::from("y")]
    );
}

// ---------- environment helpers ----------

#[test]
fn bin_settings_decision() {
    assert_eq!(bin_info_from(Some(1), Some(100)), RecycleBinInfo::Disabled);
    assert_eq!(bin_info_from(Some(1), None), RecycleBinInfo::Disabled);
    assert_eq!(
        bin_info_from(Some(0), Some(100)),
        RecycleBinInfo::Enabled {
            max_bytes: 100 * 1024 * 1024
        }
    );
    assert_eq!(
        bin_info_from(None, Some(5)),
        RecycleBinInfo::Enabled {
            max_bytes: 5 * 1024 * 1024
        }
    );
    assert_eq!(bin_info_from(None, None), RecycleBinInfo::Unknown);
    assert_eq!(bin_info_from(Some(0), None), RecycleBinInfo::Unknown);
}

#[test]
fn extracts_the_volume_guid() {
    assert_eq!(
        volume_guid("\\\\?\\Volume{0a1b2c3d-0000-1111-2222-333344445555}\\"),
        Some("{0a1b2c3d-0000-1111-2222-333344445555}")
    );
    assert_eq!(volume_guid("C:\\"), None);
}

#[test]
fn verbatim_paths_cannot_dodge_the_checks() {
    assert!(is_within(
        Path::new("\\\\?\\C:\\Windows\\x"),
        Path::new("c:\\windows")
    ));
    assert!(!is_within(
        Path::new("C:\\WindowsX"),
        Path::new("C:\\Windows")
    ));
}

#[cfg(windows)]
#[test]
fn long_form_expands_short_names_even_after_the_item_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let expected = fs::canonicalize(dir.path()).unwrap();
    let expected = PathBuf::from(expected.to_string_lossy().trim_start_matches(r"\\?\"));
    assert_eq!(log::long_form(dir.path()), expected);
    // A recycled item no longer exists, so only its folder can be expanded.
    let gone = dir.path().join("gone.txt");
    assert_eq!(log::long_form(&gone), expected.join("gone.txt"));
}

// ---------- ignored: touch the real system, only inside tempdirs ----------

#[test]
#[ignore = "moves a tempdir file to the real Recycle Bin"]
fn cleanup_real_recycle_round_trip() {
    // Deliberately the raw tempdir: TEMP is often an 8.3 short path (APOSTO~1) while the
    // Recycle Bin records the long one, and restore must still find the item.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("devdisk-roundtrip-test.txt");
    fs::write(&file, b"hello bin").unwrap();

    let res = WalkScanner
        .scan(dir.path(), &ScanOptions::default(), &NoProgress)
        .unwrap();
    let r = rules(&[]);
    let projects = find_projects(&res.tree, &r);
    let ctx = CleanupContext {
        tree: &res.tree,
        projects: &projects,
        rules: &r,
        protected: &[],
    };
    let logdir = tempfile::tempdir().unwrap();
    let log = open_log(logdir.path());
    let out = recycle(
        &ctx,
        std::slice::from_ref(&file),
        &SystemEnvironment,
        &TrashMover,
        &log,
        "ignored-test",
        &mut |_, _, _| {},
    )
    .unwrap();
    assert!(out[0].ok, "{:?}", out[0].error);
    assert!(!file.exists());

    let id = out[0].entry_id.unwrap();
    let back = restore(&log, &[id], &SystemBin);
    assert!(back[0].ok, "{:?}", back[0].error);
    assert_eq!(fs::read(&file).unwrap(), b"hello bin");
    assert!(log.list().unwrap()[0].restored_at.is_some());
}

#[test]
#[ignore = "reads the real registry and runs git"]
fn cleanup_system_environment_reads_real_settings() {
    let info = SystemEnvironment.recycle_bin(Path::new("C:\\"));
    eprintln!("C: Recycle Bin: {info:?}");
    assert!(matches!(
        info,
        RecycleBinInfo::Enabled { .. } | RecycleBinInfo::Disabled | RecycleBinInfo::Unknown
    ));

    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap()
    };
    let outside = dir.path().join("untracked.txt");
    fs::write(&outside, "x").unwrap();
    // Not a repository: a definite "nothing here is tracked", not a failed check, so the
    // preview does not warn on every project that simply has no git.
    assert_eq!(SystemEnvironment.git_tracked(&outside), Some(false));
    assert!(git(&["init", "-q"]).status.success());
    fs::write(dir.path().join("tracked.txt"), "x").unwrap();
    assert!(git(&["add", "tracked.txt"]).status.success());
    assert!(git(&["commit", "-q", "-m", "c"]).status.success());
    assert_eq!(
        SystemEnvironment.git_tracked(&dir.path().join("tracked.txt")),
        Some(true)
    );
    assert_eq!(SystemEnvironment.git_tracked(&outside), Some(false));
}
