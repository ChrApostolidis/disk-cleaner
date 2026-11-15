//! Treemap slices of the scanned tree: a folder with its children to a limited depth, tagged with project labels.

use crate::index::{NodeId, NodeKind, Tree};
use crate::projects::Project;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TreemapKind {
    Dir,
    File,
    Others,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreemapNode {
    pub path: PathBuf,
    pub name: String,
    pub kind: TreemapKind,
    pub allocated: u64,
    pub file_count: u64,
    pub label: Option<String>,
    pub project_root: bool,
    pub regenerable: bool,
    pub children: Option<Vec<TreemapNode>>,
}

/// Precomputed once per scan so a treemap request never searches the project list.
pub struct ProjectIndex {
    /// Project root node -> index into the projects slice.
    roots: HashMap<NodeId, usize>,
    /// Dirs that are a regenerable folder of the project directly above them.
    regenerable: HashSet<NodeId>,
}

impl ProjectIndex {
    pub fn new(tree: &Tree, projects: &[Project]) -> Self {
        let mut roots = HashMap::new();
        let mut regenerable = HashSet::new();
        for (i, p) in projects.iter().enumerate() {
            roots.insert(p.node, i);
            for &c in tree.children(p.node) {
                let n = tree.node(c);
                if n.kind == NodeKind::Dir
                    && p.info
                        .regenerable
                        .iter()
                        .any(|r| r.eq_ignore_ascii_case(&n.name))
                {
                    regenerable.insert(c);
                }
            }
        }
        Self { roots, regenerable }
    }
}

/// `node` with children `depth` levels deep (0 loads none), at most `max_children`
/// non-empty entries per folder; the rest is folded into one `Others` entry.
pub fn treemap(
    tree: &Tree,
    projects: &[Project],
    index: &ProjectIndex,
    node: NodeId,
    depth: u32,
    max_children: usize,
) -> TreemapNode {
    // Walk up once for the inherited label; deeper levels receive it from their parent.
    let mut label: Option<&str> = None;
    let mut cur = Some(node);
    while let Some(id) = cur {
        if let Some(&pi) = index.roots.get(&id) {
            label = Some(projects[pi].info.label.as_str());
            break;
        }
        cur = tree.node(id).parent;
    }
    build(
        tree,
        projects,
        index,
        node,
        label,
        tree.path(node),
        depth,
        max_children,
    )
}

#[allow(clippy::too_many_arguments)]
fn build(
    tree: &Tree,
    projects: &[Project],
    index: &ProjectIndex,
    id: NodeId,
    inherited: Option<&str>,
    path: PathBuf,
    depth: u32,
    max_children: usize,
) -> TreemapNode {
    let n = tree.node(id);
    let root = index.roots.get(&id).map(|&pi| &projects[pi]);
    // A project root's own label wins over whatever enclosed it.
    let label = root.map(|p| p.info.label.as_str()).or(inherited);
    let is_dir = n.kind == NodeKind::Dir;

    let children = if is_dir && depth > 0 {
        let mut out = Vec::new();
        let (mut rest_alloc, mut rest_files, mut rest_count) = (0u64, 0u64, 0usize);
        for &c in tree.children(id) {
            let cn = tree.node(c);
            if out.len() < max_children && cn.allocated > 0 {
                out.push(build(
                    tree,
                    projects,
                    index,
                    c,
                    label,
                    path.join(&cn.name),
                    depth - 1,
                    max_children,
                ));
            } else {
                rest_alloc += cn.allocated;
                rest_files += cn.file_count;
                rest_count += 1;
            }
        }
        if rest_count > 0 {
            out.push(TreemapNode {
                path: path.clone(),
                name: format!("{} smaller items", with_commas(rest_count)),
                kind: TreemapKind::Others,
                allocated: rest_alloc,
                file_count: rest_files,
                label: label.map(str::to_owned),
                project_root: false,
                regenerable: false,
                children: None,
            });
        }
        Some(out)
    } else {
        None
    };

    TreemapNode {
        path,
        name: n.name.clone(),
        kind: if is_dir {
            TreemapKind::Dir
        } else {
            TreemapKind::File
        },
        allocated: n.allocated,
        file_count: n.file_count,
        label: label.map(str::to_owned),
        project_root: root.is_some(),
        regenerable: index.regenerable.contains(&id),
        children,
    }
}

fn with_commas(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::DetectedProject;
    use crate::index::TreeBuilder;
    use std::path::Path;

    fn project(tree: &Tree, node: NodeId, label: &str, regen: &[&str]) -> Project {
        Project {
            node,
            info: DetectedProject {
                root: tree.path(node),
                ecosystems: vec![],
                framework: None,
                label: label.into(),
                version: None,
                package_manager: None,
                has_lockfile: false,
                regenerable: regen.iter().map(|s| s.to_string()).collect(),
                reinstall_command: None,
            },
            parent: None,
            total_allocated: 0,
            reclaimable_allocated: 0,
            last_activity: None,
        }
    }

    /// root/{app/{node_modules/{dep.js 500}, src/{a.js 50}, .env 0}, docs/{big.bin 900}, empty/}
    fn sample() -> (Tree, Vec<Project>) {
        let mut b = TreeBuilder::new(Path::new(r"C:\scan"), None);
        let r = b.root();
        let app = b.add_dir(r, "app", None);
        let nm = b.add_dir(app, "node_modules", None);
        b.add_file(nm, "dep.js", 500, 500, None);
        let src = b.add_dir(app, "src", None);
        b.add_file(src, "a.js", 50, 50, None);
        b.add_file(app, ".env", 0, 0, None);
        let docs = b.add_dir(r, "docs", None);
        b.add_file(docs, "big.bin", 900, 900, None);
        b.add_dir(r, "empty", None);
        let tree = b.finish();
        let app = tree.find(Path::new(r"C:\scan\app")).unwrap();
        let p = project(&tree, app, "Next.js", &["Node_Modules"]);
        (tree, vec![p])
    }

    fn run(tree: &Tree, ps: &[Project], at: &str, depth: u32, max: usize) -> TreemapNode {
        let idx = ProjectIndex::new(tree, ps);
        let node = tree.find(Path::new(at)).unwrap();
        treemap(tree, ps, &idx, node, depth, max)
    }

    fn child<'a>(n: &'a TreemapNode, name: &str) -> &'a TreemapNode {
        n.children
            .as_ref()
            .unwrap()
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("no child {name}"))
    }

    #[test]
    fn depth_one_loads_children_without_grandchildren() {
        let (t, ps) = sample();
        let n = run(&t, &ps, r"C:\scan", 1, 60);
        assert_eq!(n.name, r"C:\scan");
        assert_eq!(n.kind, TreemapKind::Dir);
        assert_eq!(child(&n, "app").children, None);
        assert_eq!(child(&n, "docs").children, None);
    }

    #[test]
    fn depth_two_goes_one_level_further_and_files_have_none() {
        let (t, ps) = sample();
        let n = run(&t, &ps, r"C:\scan", 2, 60);
        let big = child(child(&n, "docs"), "big.bin");
        assert_eq!(big.kind, TreemapKind::File);
        assert_eq!(big.children, None);
        assert_eq!(child(child(&n, "app"), "src").children, None);
    }

    #[test]
    fn depth_zero_loads_no_children() {
        let (t, ps) = sample();
        assert_eq!(run(&t, &ps, r"C:\scan", 0, 60).children, None);
    }

    #[test]
    fn empty_dir_within_depth_has_empty_children() {
        let (t, ps) = sample();
        let n = run(&t, &ps, r"C:\scan", 2, 60);
        // The zero-size dir is folded into "others" at the parent level.
        let others = n.children.as_ref().unwrap().last().unwrap();
        assert_eq!(others.kind, TreemapKind::Others);
        assert_eq!(others.name, "1 smaller items");
        let e = run(&t, &ps, r"C:\scan\empty", 1, 60);
        assert_eq!(e.children, Some(vec![]));
    }

    #[test]
    fn max_children_cut_goes_into_others() {
        let mut b = TreeBuilder::new(Path::new(r"C:\x"), None);
        let r = b.root();
        for i in 0..1300u64 {
            b.add_file(r, &format!("f{i}"), 10 + i, 10 + i, None);
        }
        let t = b.finish();
        let n = run(&t, &[], r"C:\x", 1, 100);
        let kids = n.children.unwrap();
        assert_eq!(kids.len(), 101);
        let others = kids.last().unwrap();
        assert_eq!(others.kind, TreemapKind::Others);
        assert_eq!(others.name, "1,200 smaller items");
        assert_eq!(others.path, PathBuf::from(r"C:\x"));
        assert_eq!(others.file_count, 1200);
        let kept: u64 = kids[..100].iter().map(|k| k.allocated).sum();
        assert_eq!(kept + others.allocated, n.allocated);
        assert_eq!(others.children, None);
        assert!(kids[0].allocated >= kids[1].allocated);
    }

    #[test]
    fn zero_size_entries_go_into_others() {
        let (t, ps) = sample();
        let n = run(&t, &ps, r"C:\scan\app", 1, 60);
        let kids = n.children.unwrap();
        assert_eq!(kids.len(), 3);
        let others = &kids[2];
        assert_eq!(others.kind, TreemapKind::Others);
        assert_eq!(others.name, "1 smaller items");
        assert_eq!(others.allocated, 0);
        assert_eq!(others.label.as_deref(), Some("Next.js"));
    }

    #[test]
    fn no_others_when_nothing_is_left_over() {
        let (t, ps) = sample();
        let n = run(&t, &ps, r"C:\scan\docs", 1, 60);
        assert_eq!(n.children.unwrap().len(), 1);
    }

    #[test]
    fn labels_are_inherited_and_roots_flagged() {
        let (t, ps) = sample();
        let n = run(&t, &ps, r"C:\scan", 3, 60);
        assert_eq!(n.label, None);
        assert!(!n.project_root);
        let app = child(&n, "app");
        assert!(app.project_root);
        assert_eq!(app.label.as_deref(), Some("Next.js"));
        let src = child(app, "src");
        assert_eq!(src.label.as_deref(), Some("Next.js"));
        assert_eq!(child(src, "a.js").label.as_deref(), Some("Next.js"));
        assert_eq!(child(&n, "docs").label, None);
    }

    #[test]
    fn label_found_when_starting_below_a_project_root() {
        let (t, ps) = sample();
        let n = run(&t, &ps, r"C:\scan\app\src", 1, 60);
        assert_eq!(n.label.as_deref(), Some("Next.js"));
        assert!(!n.project_root);
        assert_eq!(n.children.unwrap()[0].label.as_deref(), Some("Next.js"));
    }

    #[test]
    fn nested_project_overrides_the_label() {
        let (t, mut ps) = sample();
        let src = t.find(Path::new(r"C:\scan\app\src")).unwrap();
        ps.push(project(&t, src, "Rust", &[]));
        let n = run(&t, &ps, r"C:\scan", 3, 60);
        let src = child(child(&n, "app"), "src");
        assert_eq!(src.label.as_deref(), Some("Rust"));
        assert!(src.project_root);
        assert_eq!(child(src, "a.js").label.as_deref(), Some("Rust"));
        assert_eq!(child(&n, "app").label.as_deref(), Some("Next.js"));
        // Starting inside the nested project picks the nearest one.
        let m = run(&t, &ps, r"C:\scan\app\src", 1, 60);
        assert_eq!(m.label.as_deref(), Some("Rust"));
    }

    #[test]
    fn regenerable_only_on_direct_children_case_insensitive() {
        let (t, ps) = sample();
        let n = run(&t, &ps, r"C:\scan", 3, 60);
        let app = child(&n, "app");
        assert!(child(app, "node_modules").regenerable);
        assert!(!child(app, "src").regenerable);
        assert!(!app.regenerable);

        // Same name deeper down, or outside a project, is not flagged.
        let mut b = TreeBuilder::new(Path::new(r"C:\y"), None);
        let r = b.root();
        let p = b.add_dir(r, "p", None);
        let sub = b.add_dir(p, "sub", None);
        let deep = b.add_dir(sub, "node_modules", None);
        b.add_file(deep, "f", 5, 5, None);
        let loose = b.add_dir(r, "node_modules", None);
        b.add_file(loose, "f", 5, 5, None);
        let t = b.finish();
        let p = t.find(Path::new(r"C:\y\p")).unwrap();
        let ps = vec![project(&t, p, "Node.js", &["node_modules"])];
        let n = run(&t, &ps, r"C:\y", 3, 60);
        assert!(!child(&n, "node_modules").regenerable);
        assert!(!child(child(child(&n, "p"), "sub"), "node_modules").regenerable);
    }

    #[test]
    fn child_paths_are_real_paths() {
        let (t, ps) = sample();
        let n = run(&t, &ps, r"C:\scan", 1, 60);
        assert_eq!(child(&n, "docs").path, PathBuf::from(r"C:\scan\docs"));
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(with_commas(7), "7");
        assert_eq!(with_commas(1000), "1,000");
        assert_eq!(with_commas(1_234_567), "1,234,567");
    }
}
