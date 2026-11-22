//! Safe cleanup: preview checks, Recycle Bin moves, the cleanup log and restore ("Put back").
//!
//! This is the only code in the app that removes anything. Every removal goes to the
//! Recycle Bin, is logged, can be restored, and is refused beforehand when it looks risky.

mod env;
mod log;

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::index::{NodeId, NodeKind, Tree};
use crate::projects::Project;
use crate::rules::Rules;

pub use env::{Environment, RecycleBinInfo, SystemEnvironment};
pub(crate) use log::long_form;
pub use log::{BinAccess, BinError, CleanupLog, HistoryEntry, RestoreOutcome, SystemBin, restore};

#[derive(Debug, thiserror::Error)]
pub enum CleanupError {
    #[error("not part of the scan: {0}")]
    NotInScan(PathBuf),
    #[error("cleanup log: {0}")]
    Db(#[from] rusqlite::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Ok,
    Warn,
    Refuse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    File,
    Dir,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewItem {
    pub path: PathBuf,
    pub name: String,
    pub kind: ItemKind,
    pub allocated: u64,
    pub regenerable: bool,
    pub project_path: Option<PathBuf>,
    pub project_label: Option<String>,
    pub verdict: Verdict,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub items: Vec<PreviewItem>,
    pub total_freed: u64,
}

pub struct CleanupContext<'a> {
    pub tree: &'a Tree,
    pub projects: &'a [Project],
    pub rules: &'a Rules,
    /// Extra paths that must never be removed (e.g. the app's own data folder).
    pub protected: &'a [PathBuf],
}

/// Path components for comparison: lowercased, `\\?\` prefix dropped, `.` removed.
/// Windows paths are case-insensitive, and a verbatim path must not slip past a plain one.
fn comps(path: &Path) -> Vec<String> {
    path.components()
        .filter(|c| *c != Component::CurDir)
        .map(|c| {
            let s = c.as_os_str().to_string_lossy().to_ascii_lowercase();
            match c {
                Component::Prefix(_) => s.strip_prefix(r"\\?\").unwrap_or(&s).to_owned(),
                _ => s,
            }
        })
        .collect()
}

fn is_within(path: &Path, base: &Path) -> bool {
    let (p, b) = (comps(path), comps(base));
    !b.is_empty() && p.len() >= b.len() && p[..b.len()] == b[..]
}

/// Drops duplicates, and paths inside another requested path (the folder covers them).
fn collapse(paths: &[PathBuf]) -> Vec<&PathBuf> {
    let keys: Vec<Vec<String>> = paths.iter().map(|p| comps(p)).collect();
    let set: HashSet<&Vec<String>> = keys.iter().collect();
    let mut seen = HashSet::new();
    paths
        .iter()
        .zip(&keys)
        .filter(|(_, k)| {
            let covered = (1..k.len()).any(|n| set.contains(&k[..n].to_vec()));
            !covered && seen.insert(*k)
        })
        .map(|(p, _)| p)
        .collect()
}

pub fn preview(
    ctx: &CleanupContext,
    paths: &[PathBuf],
    env: &dyn Environment,
) -> Result<Preview, CleanupError> {
    let ignore = ctx.rules.ignore_paths();
    let profile = env.user_profile();
    let project_at: HashMap<NodeId, usize> = ctx
        .projects
        .iter()
        .enumerate()
        .map(|(i, p)| (p.node, i))
        .collect();

    let requested = collapse(paths);
    // Resolve everything first: a path missing from the scan is a caller bug, not a verdict,
    // and nothing should be half-processed when it happens.
    let mut resolved = Vec::with_capacity(requested.len());
    for path in requested {
        let node = if is_within(path, ctx.tree.root_path()) {
            let id = ctx
                .tree
                .find(path)
                .ok_or_else(|| CleanupError::NotInScan(path.clone()))?;
            Some(id)
        } else {
            None
        };
        resolved.push((path, node));
    }

    let mut items = Vec::with_capacity(resolved.len());
    for (path, node) in resolved {
        let mut reasons: Vec<(Verdict, String)> = Vec::new();
        let refuse = |why: String| (Verdict::Refuse, why);

        if path.parent().is_none() {
            reasons.push(refuse("this is a drive root".into()));
        }
        if let Some(ig) = ignore.iter().find(|ig| is_within(path, ig)) {
            reasons.push(refuse(format!(
                "inside a protected system folder ({})",
                ig.display()
            )));
        }
        if profile.as_deref().is_some_and(|pr| is_within(pr, path)) {
            reasons.push(refuse(
                "this is your user profile folder or contains it".into(),
            ));
        }
        if ctx.protected.iter().any(|pr| is_within(path, pr)) {
            reasons.push(refuse("this folder is used by Ballast itself".into()));
        }
        if node.is_none() {
            reasons.push(refuse("outside the scanned folder".into()));
        }
        if node.is_some_and(|id| !ctx.tree.is_live(id)) {
            reasons.push(refuse("no longer in the scan".into()));
        }
        if comps(path).iter().any(|c| c == ".git") {
            reasons.push(refuse(
                "part of a .git folder: deleting it would destroy the repository history".into(),
            ));
        }

        let mut project: Option<usize> = None;
        let mut allocated = 0;
        let mut kind = None;
        let mut regenerable = false;
        let mut git = None;
        let mut bin = RecycleBinInfo::Unknown;
        let mut dependency_without_lock = false;
        if let Some(id) = node {
            let n = ctx.tree.node(id);
            allocated = n.allocated;
            kind = Some(n.kind);
            let mut cur = Some(id);
            while let Some(c) = cur {
                if let Some(&i) = project_at.get(&c) {
                    project = Some(i);
                    break;
                }
                cur = ctx.tree.node(c).parent;
            }
            // Regenerable / dependency status only applies to a direct child of a project root.
            if let Some(&pi) = n.parent.and_then(|p| project_at.get(&p)) {
                let info = &ctx.projects[pi].info;
                regenerable = info
                    .regenerable
                    .iter()
                    .any(|r| r.eq_ignore_ascii_case(&n.name));
                dependency_without_lock =
                    !info.has_lockfile && ctx.rules.is_dependency_dir(&info.ecosystems, &n.name);
            }
            git = env.git_tracked(path);
            bin = env.recycle_bin(path);
        }

        if git == Some(true) {
            reasons.push(refuse(
                "tracked by git: deleting would lose committed files".into(),
            ));
        }
        match bin {
            RecycleBinInfo::Disabled => reasons.push(refuse(
                "Recycle Bin is turned off for this drive, so this would be permanent".into(),
            )),
            RecycleBinInfo::Enabled { max_bytes } if allocated > max_bytes => {
                reasons.push(refuse(format!(
                    "too big for the Recycle Bin ({} > {}), Windows would delete it permanently",
                    human(allocated),
                    human(max_bytes)
                )))
            }
            _ => {}
        }

        let warn = |why: &str| (Verdict::Warn, why.to_owned());
        if dependency_without_lock {
            reasons.push(warn(
                "no lockfile: reinstalling may install different versions",
            ));
        }
        if node.is_some() && bin == RecycleBinInfo::Unknown {
            reasons.push(warn("could not check the Recycle Bin size for this drive"));
        }
        if git.is_none() && project.is_some() {
            reasons.push(warn("could not check git"));
        }
        // Everything goes to the Recycle Bin, so this is a warning, not a refusal: the point is
        // that a project folder holds source code that no tool can regenerate.
        if let Some(id) = node.filter(|&id| ctx.tree.is_live(id)) {
            if project_at.contains_key(&id) {
                reasons.push(warn(
                    "whole project folder: includes its source code, not only files that can be regenerated",
                ));
            }
            let inside = ctx
                .projects
                .iter()
                .filter(|p| {
                    p.node != id && ctx.tree.is_live(p.node) && is_below(ctx.tree, p.node, id)
                })
                .count();
            if inside > 0 {
                reasons.push((
                    Verdict::Warn,
                    format!("contains {inside} projects, including their source code"),
                ));
            }
        }

        let verdict = if reasons.iter().any(|(v, _)| *v == Verdict::Refuse) {
            Verdict::Refuse
        } else if reasons.is_empty() {
            Verdict::Ok
        } else {
            Verdict::Warn
        };
        let proj = project.map(|i| &ctx.projects[i]);
        items.push(PreviewItem {
            path: path.clone(),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            kind: match kind {
                Some(NodeKind::File) => ItemKind::File,
                Some(NodeKind::Dir) => ItemKind::Dir,
                None if path.is_dir() => ItemKind::Dir,
                None => ItemKind::File,
            },
            allocated,
            regenerable,
            project_path: proj.map(|p| ctx.tree.path(p.node)),
            project_label: proj.map(|p| p.info.label.clone()),
            verdict,
            reasons: reasons.into_iter().map(|(_, r)| r).collect(),
        });
    }

    let total_freed = items
        .iter()
        .filter(|i| i.verdict != Verdict::Refuse)
        .map(|i| i.allocated)
        .sum();
    Ok(Preview { items, total_freed })
}

/// True when `node` lies somewhere under `ancestor`.
fn is_below(tree: &Tree, node: NodeId, ancestor: NodeId) -> bool {
    let mut cur = tree.node(node).parent;
    while let Some(c) = cur {
        if c == ancestor {
            return true;
        }
        cur = tree.node(c).parent;
    }
    false
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub trait Mover {
    fn to_recycle_bin(&self, path: &Path) -> Result<(), String>;
}

/// Moves to the real Recycle Bin. Windows skips (and `trash` reports) anything it would
/// delete permanently, so a failure here never means data loss.
pub struct TrashMover;

impl Mover for TrashMover {
    fn to_recycle_bin(&self, path: &Path) -> Result<(), String> {
        trash::delete(path).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct RecycleOutcome {
    pub path: PathBuf,
    pub allocated: u64,
    pub ok: bool,
    pub error: Option<String>,
    pub entry_id: Option<i64>,
}

/// Moves each non-refused item to the Recycle Bin, one at a time, logging each success.
/// The checks are re-run here so no caller can bypass them. `progress(done, total, current)`
/// is called before each move.
pub fn recycle(
    ctx: &CleanupContext,
    paths: &[PathBuf],
    env: &dyn Environment,
    mover: &dyn Mover,
    log: &CleanupLog,
    batch_id: &str,
    progress: &mut dyn FnMut(usize, usize, &Path),
) -> Result<Vec<RecycleOutcome>, CleanupError> {
    let pre = preview(ctx, paths, env)?;
    let total = pre
        .items
        .iter()
        .filter(|i| i.verdict != Verdict::Refuse)
        .count();
    let mut done = 0;
    let mut out = Vec::with_capacity(pre.items.len());
    for item in pre.items {
        let mut outcome = RecycleOutcome {
            path: item.path.clone(),
            allocated: item.allocated,
            ok: false,
            error: None,
            entry_id: None,
        };
        if item.verdict == Verdict::Refuse {
            outcome.error = Some(item.reasons.join("; "));
            out.push(outcome);
            continue;
        }
        progress(done, total, &item.path);
        done += 1;
        // Expanded while the item still exists, so the log matches what the bin records.
        let logged_path = log::long_form(&item.path);
        match mover.to_recycle_bin(&item.path) {
            Err(e) => outcome.error = Some(e),
            Ok(()) => {
                outcome.ok = true;
                let entry = log::NewEntry {
                    batch_id,
                    deleted_at: log::now_ms(),
                    path: &logged_path,
                    allocated: item.allocated,
                    project_path: item.project_path.as_deref(),
                    project_label: item.project_label.as_deref(),
                };
                match log.insert(&entry) {
                    Ok(id) => outcome.entry_id = Some(id),
                    // It is in the bin already; say so rather than claim it failed.
                    Err(e) => outcome.error = Some(format!("moved, but could not be logged: {e}")),
                }
            }
        }
        out.push(outcome);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
