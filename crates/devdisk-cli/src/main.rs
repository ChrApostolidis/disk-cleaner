mod format;

use clap::{Parser, Subcommand};
use devdisk_core::index::Tree;
use devdisk_core::mft::MftScanner;
use devdisk_core::projects::{Project, find_projects};
use devdisk_core::rules::Rules;
use devdisk_core::scanner::{Progress, ProgressSink, ScanOptions, Scanner, WalkScanner};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Parser)]
#[command(name = "devdisk", about = "Find what is eating your disk")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan a folder and list the biggest folders and the projects in it
    Scan {
        path: PathBuf,
        /// How many of the biggest folders to list
        #[arg(long, default_value_t = 15)]
        top: usize,
        /// Read the NTFS file table directly (run from an administrator terminal)
        #[arg(long)]
        mft: bool,
    },
}

/// One updating line on stderr. The previous line's length is remembered so a
/// shorter line fully overwrites it.
struct StderrProgress {
    last_len: Mutex<usize>,
}

impl ProgressSink for StderrProgress {
    fn report(&self, p: &Progress) {
        let line = format!(
            "{} files, {} dirs, {}  {}",
            p.files,
            p.dirs,
            format::size(p.bytes),
            format::shorten(&p.current_path.to_string_lossy(), 60)
        );
        let mut last = self.last_len.lock().unwrap();
        let pad = last.saturating_sub(line.chars().count());
        eprint!("\r{line}{}", " ".repeat(pad));
        let _ = std::io::stderr().flush();
        *last = line.chars().count();
    }
}

impl StderrProgress {
    fn clear(&self) {
        let last = *self.last_len.lock().unwrap();
        eprint!("\r{}\r", " ".repeat(last));
        let _ = std::io::stderr().flush();
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Scan { path, top, mft } => match scan(path, top, mft) {
            Ok(()) => ExitCode::SUCCESS,
            Err(msg) => {
                eprintln!("error: {msg}");
                ExitCode::FAILURE
            }
        },
    }
}

fn scan(path: PathBuf, top: usize, mft: bool) -> Result<(), String> {
    // `canonicalize` would prefix every printed path with the verbatim marker on Windows.
    let root = std::path::absolute(&path).map_err(|e| e.to_string())?;
    let progress = StderrProgress {
        last_len: Mutex::new(0),
    };
    let scanner: &dyn Scanner = if mft { &MftScanner } else { &WalkScanner };
    let result = scanner.scan(&root, &ScanOptions::default(), &progress);
    progress.clear();
    let result = result.map_err(|e| e.to_string())?;

    let rules = Rules::builtin();
    let projects = find_projects(&result.tree, &rules);
    let tree = &result.tree;
    let root_node = tree.node(tree.root());

    println!(
        "{}: {} files, {} dirs, {} on disk, scanned in {:.1}s, {} issues",
        root.display(),
        root_node.file_count,
        tree.len() as u64 - root_node.file_count - 1,
        format::size(root_node.allocated),
        result.elapsed.as_secs_f64(),
        result.issues.len()
    );

    println!("\nBiggest folders (on disk / logical / files)");
    for id in tree.biggest_dirs(top) {
        let n = tree.node(id);
        println!(
            "  {:>10} {:>10} {:>9}  {}",
            format::size(n.allocated),
            format::size(n.size),
            n.file_count,
            rel_path(tree, id)
        );
    }

    print_projects(tree, &projects);

    if !result.issues.is_empty() {
        println!("\nIssues ({}, first 5)", result.issues.len());
        for issue in result.issues.iter().take(5) {
            println!(
                "  {:?}: {} ({})",
                issue.kind,
                issue.path.display(),
                issue.message
            );
        }
    }
    Ok(())
}

fn rel_path(tree: &Tree, id: usize) -> String {
    let full = tree.path(id);
    match full.strip_prefix(tree.root_path()) {
        Ok(r) if r.as_os_str().is_empty() => ".".to_owned(),
        Ok(r) => r.display().to_string(),
        Err(_) => full.display().to_string(),
    }
}

fn depth(projects: &[Project], mut i: usize) -> usize {
    let mut d = 0;
    while let Some(p) = projects[i].parent {
        d += 1;
        i = p;
    }
    d
}

/// Projects whose parent is `parent`, biggest reclaimable first.
fn sorted_children(projects: &[Project], parent: Option<usize>) -> Vec<usize> {
    let mut ids: Vec<usize> = (0..projects.len())
        .filter(|&i| projects[i].parent == parent)
        .collect();
    ids.sort_by_key(|&i| std::cmp::Reverse(projects[i].reclaimable_allocated));
    ids
}

fn print_projects(tree: &Tree, projects: &[Project]) {
    println!("\nProjects ({}), by reclaimable space", projects.len());
    if projects.is_empty() {
        return;
    }
    // Children are printed right under their parent, so sorting is per sibling group.
    let now = SystemTime::now();
    let mut stack: Vec<usize> = sorted_children(projects, None);
    stack.reverse();
    while let Some(i) = stack.pop() {
        let mut kids = sorted_children(projects, Some(i));
        kids.reverse();
        stack.extend(kids);
        let p = &projects[i];
        let indent = "  ".repeat(depth(projects, i));
        let version = p
            .info
            .version
            .as_deref()
            .map(|v| format!(" v{v}"))
            .unwrap_or_default();
        let manager = p.info.package_manager.as_deref().unwrap_or("no lockfile");
        println!(
            "  {indent}{}{version}  {} on disk, {} reclaimable, {}, {manager}",
            p.info.display_name(),
            format::size(p.total_allocated),
            format::size(p.reclaimable_allocated),
            format::days_ago(p.last_activity, now),
        );
        println!("  {indent}  {}", rel_path(tree, p.node));
    }
    // A nested project's regenerable dirs are never inside its parent's (the walk does not
    // descend into regenerable dirs), so plain summing cannot count anything twice.
    let total: u64 = projects.iter().map(|p| p.reclaimable_allocated).sum();
    println!(
        "\nTotal reclaimable across {} projects: {}",
        projects.len(),
        format::size(total)
    );
}
