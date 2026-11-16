//! In-memory folder tree with aggregated sizes. Filled by a scanner, queried by path.

use std::cmp::Reverse;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

pub type NodeId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Dir,
}

#[derive(Debug, Clone)]
pub struct Node {
    /// File or folder name. For the root: the root path as given, as a string.
    pub name: String,
    pub kind: NodeKind,
    pub parent: Option<NodeId>,
    /// Logical size in bytes. Dirs: sum of all descendant files.
    pub size: u64,
    /// Size on disk in bytes. Dirs: sum of all descendant files.
    pub allocated: u64,
    /// The entry's own last-modified time.
    pub modified: Option<SystemTime>,
    /// Files: same as `modified`. Dirs: newest `modified` of any descendant file, None if no files.
    pub newest_modified: Option<SystemTime>,
    /// Files: 1. Dirs: number of descendant files.
    pub file_count: u64,
}

pub struct TreeBuilder {
    root_path: PathBuf,
    nodes: Vec<Node>,
}

impl TreeBuilder {
    pub fn new(root_path: &Path, root_modified: Option<SystemTime>) -> Self {
        let root = Node {
            name: root_path.to_string_lossy().into_owned(),
            kind: NodeKind::Dir,
            parent: None,
            size: 0,
            allocated: 0,
            modified: root_modified,
            newest_modified: None,
            file_count: 0,
        };
        Self {
            root_path: root_path.to_path_buf(),
            nodes: vec![root],
        }
    }

    pub fn root(&self) -> NodeId {
        0
    }

    pub fn add_dir(&mut self, parent: NodeId, name: &str, modified: Option<SystemTime>) -> NodeId {
        self.push(Node {
            name: name.to_owned(),
            kind: NodeKind::Dir,
            parent: Some(parent),
            size: 0,
            allocated: 0,
            modified,
            newest_modified: None,
            file_count: 0,
        })
    }

    pub fn add_file(
        &mut self,
        parent: NodeId,
        name: &str,
        size: u64,
        allocated: u64,
        modified: Option<SystemTime>,
    ) -> NodeId {
        self.push(Node {
            name: name.to_owned(),
            kind: NodeKind::File,
            parent: Some(parent),
            size,
            allocated,
            modified,
            newest_modified: modified,
            file_count: 1,
        })
    }

    fn push(&mut self, node: Node) -> NodeId {
        debug_assert!(node.parent.is_some_and(|p| p < self.nodes.len()));
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    /// Computes all dir aggregates bottom-up and sorts every child list by `allocated`
    /// descending (ties: name ascending).
    pub fn finish(mut self) -> Tree {
        let n = self.nodes.len();
        // A parent is always added before its children, so a reverse pass sees every
        // child fully aggregated before it is folded into its parent.
        for i in (1..n).rev() {
            let Some(p) = self.nodes[i].parent else {
                continue;
            };
            let (size, alloc, count, newest) = {
                let c = &self.nodes[i];
                (c.size, c.allocated, c.file_count, c.newest_modified)
            };
            let parent = &mut self.nodes[p];
            parent.size += size;
            parent.allocated += alloc;
            parent.file_count += count;
            parent.newest_modified = parent.newest_modified.max(newest);
        }

        // Compressed child lists: one flat array plus offsets, so files cost no Vec each.
        let mut offsets = vec![0usize; n + 1];
        for node in &self.nodes[1..] {
            offsets[node.parent.unwrap() + 1] += 1;
        }
        for i in 0..n {
            offsets[i + 1] += offsets[i];
        }
        let mut fill = offsets.clone();
        let mut flat = vec![0; n.saturating_sub(1)];
        for (i, node) in self.nodes.iter().enumerate().skip(1) {
            let p = node.parent.unwrap();
            flat[fill[p]] = i;
            fill[p] += 1;
        }
        for p in 0..n {
            flat[offsets[p]..offsets[p + 1]].sort_unstable_by(|&a, &b| {
                let (a, b) = (&self.nodes[a], &self.nodes[b]);
                b.allocated
                    .cmp(&a.allocated)
                    .then_with(|| a.name.cmp(&b.name))
            });
        }

        let ends = offsets[1..].to_vec();
        let dead = vec![false; n];
        Tree {
            root_path: self.root_path,
            nodes: self.nodes,
            offsets,
            ends,
            dead,
            children: flat,
        }
    }
}

#[derive(Clone)]
pub struct Tree {
    root_path: PathBuf,
    nodes: Vec<Node>,
    /// Start of each node's child slice in `children`.
    offsets: Vec<usize>,
    /// End of each node's child slice. Separate from `offsets` so `detach` can shrink a
    /// slice without renumbering every later node.
    ends: Vec<usize>,
    /// Detached nodes stay in `nodes` (ids must stay stable) but are invisible.
    dead: Vec<bool>,
    children: Vec<NodeId>,
}

impl Tree {
    pub fn root(&self) -> NodeId {
        0
    }

    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    /// Sorted by `allocated` descending.
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.children[self.offsets[id]..self.ends[id]]
    }

    /// ASCII case-insensitive, because Windows paths are.
    pub fn child_by_name(&self, id: NodeId, name: &str) -> Option<NodeId> {
        self.children(id)
            .iter()
            .copied()
            .find(|&c| self.nodes[c].name.eq_ignore_ascii_case(name))
    }

    /// Absolute path: root_path joined with the names down to `id`.
    pub fn path(&self, id: NodeId) -> PathBuf {
        let mut names = Vec::new();
        let mut cur = id;
        while let Some(p) = self.nodes[cur].parent {
            names.push(self.nodes[cur].name.as_str());
            cur = p;
        }
        let mut path = self.root_path.clone();
        path.extend(names.iter().rev());
        path
    }

    /// Absolute path under the root, case-insensitive. None if outside the root or not scanned.
    pub fn find(&self, path: &Path) -> Option<NodeId> {
        let mut root_comps = self
            .root_path
            .components()
            .filter(|c| *c != Component::CurDir);
        let mut comps = path.components().filter(|c| *c != Component::CurDir);
        for rc in root_comps.by_ref() {
            let c = comps.next()?;
            let (a, b) = (
                rc.as_os_str().to_string_lossy(),
                c.as_os_str().to_string_lossy(),
            );
            if !a.eq_ignore_ascii_case(&b) {
                return None;
            }
        }
        let mut cur = self.root();
        for c in comps {
            match c {
                Component::Normal(name) => {
                    cur = self.child_by_name(cur, &name.to_string_lossy())?
                }
                _ => return None,
            }
        }
        Some(cur)
    }

    /// False once the node (or an ancestor) was detached.
    pub fn is_live(&self, id: NodeId) -> bool {
        !self.dead[id]
    }

    /// Removes `id` and its whole subtree from the live tree, e.g. after it was moved to the
    /// Recycle Bin. Ancestors lose its `size`, `allocated` and `file_count`; the node leaves
    /// its parent's `children()` (order of the others is kept) and it and all its descendants
    /// are skipped by `find`, `child_by_name`, `biggest_files` and `biggest_dirs`.
    /// Ancestors' `newest_modified` is NOT recomputed (that would need a rescan), so it may
    /// still show a time from the removed files. No-op for the root or an already dead node.
    pub fn detach(&mut self, id: NodeId) {
        let Some(parent) = self.nodes[id].parent else {
            return;
        };
        if self.dead[id] {
            return;
        }
        let (size, allocated, files) = {
            let n = &self.nodes[id];
            (n.size, n.allocated, n.file_count)
        };
        let mut cur = Some(parent);
        while let Some(a) = cur {
            let n = &mut self.nodes[a];
            n.size = n.size.saturating_sub(size);
            n.allocated = n.allocated.saturating_sub(allocated);
            n.file_count = n.file_count.saturating_sub(files);
            cur = n.parent;
        }
        let (start, end) = (self.offsets[parent], self.ends[parent]);
        if let Some(pos) = self.children[start..end].iter().position(|&c| c == id) {
            self.children.copy_within(start + pos + 1..end, start + pos);
            self.ends[parent] -= 1;
        }
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            self.dead[n] = true;
            stack.extend_from_slice(&self.children[self.offsets[n]..self.ends[n]]);
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Files only, by `allocated` descending.
    pub fn biggest_files(&self, limit: usize) -> Vec<NodeId> {
        self.top(limit, |n| n.kind == NodeKind::File)
    }

    /// Dirs only, excluding the root, by `allocated` descending.
    pub fn biggest_dirs(&self, limit: usize) -> Vec<NodeId> {
        self.top(limit, |n| n.kind == NodeKind::Dir && n.parent.is_some())
    }

    fn top(&self, limit: usize, keep: impl Fn(&Node) -> bool) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = (0..self.nodes.len())
            .filter(|&i| !self.dead[i] && keep(&self.nodes[i]))
            .collect();
        let key = |&i: &NodeId| (Reverse(self.nodes[i].allocated), i);
        if limit < ids.len() {
            // Partial selection avoids sorting millions of entries for a top-N list.
            ids.select_nth_unstable_by_key(limit, key);
            ids.truncate(limit);
        }
        ids.sort_unstable_by_key(key);
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn t(secs: u64) -> Option<SystemTime> {
        Some(UNIX_EPOCH + Duration::from_secs(secs))
    }

    /// root/ { a/ { x(10,16,t5), y(20,32,t9) }, B/ { z(5,8,t7) }, empty/, top(100,128,t1) }
    fn sample() -> Tree {
        let mut b = TreeBuilder::new(Path::new("C:\\data"), t(0));
        let root = b.root();
        let a = b.add_dir(root, "a", t(2));
        b.add_file(a, "x", 10, 16, t(5));
        b.add_file(a, "y", 20, 32, t(9));
        let bd = b.add_dir(root, "B", t(3));
        b.add_file(bd, "z", 5, 8, t(7));
        b.add_dir(root, "empty", t(4));
        b.add_file(root, "top", 100, 128, t(1));
        b.finish()
    }

    #[test]
    fn aggregates_dirs() {
        let tree = sample();
        let root = tree.node(tree.root());
        assert_eq!(root.size, 135);
        assert_eq!(root.allocated, 184);
        assert_eq!(root.file_count, 4);
        assert_eq!(root.newest_modified, t(9));
        assert_eq!(root.modified, t(0));
        assert_eq!(root.name, "C:\\data");
        let a = tree.child_by_name(tree.root(), "a").unwrap();
        assert_eq!(tree.node(a).size, 30);
        assert_eq!(tree.node(a).file_count, 2);
        assert_eq!(tree.node(a).newest_modified, t(9));
        let e = tree.child_by_name(tree.root(), "empty").unwrap();
        assert_eq!(tree.node(e).file_count, 0);
        assert_eq!(tree.node(e).newest_modified, None);
        let top = tree.child_by_name(tree.root(), "top").unwrap();
        assert_eq!(tree.node(top).file_count, 1);
        assert_eq!(tree.node(top).newest_modified, t(1));
    }

    #[test]
    fn children_sorted_by_allocated_then_name() {
        let mut b = TreeBuilder::new(Path::new("r"), None);
        let r = b.root();
        b.add_file(r, "b", 1, 8, None);
        b.add_file(r, "a", 1, 8, None);
        b.add_file(r, "big", 1, 64, None);
        let tree = b.finish();
        let names: Vec<_> = tree
            .children(r)
            .iter()
            .map(|&c| tree.node(c).name.as_str())
            .collect();
        assert_eq!(names, ["big", "a", "b"]);
    }

    #[test]
    fn path_and_find() {
        let tree = sample();
        let x = tree.find(Path::new("C:\\data\\a\\x")).unwrap();
        assert_eq!(tree.node(x).name, "x");
        assert_eq!(tree.path(x), Path::new("C:\\data").join("a").join("x"));
        assert_eq!(tree.find(Path::new("c:\\DATA\\A\\X")), Some(x));
        assert_eq!(tree.find(Path::new("C:\\data")), Some(tree.root()));
        assert_eq!(tree.find(Path::new("C:\\other\\a")), None);
        assert_eq!(tree.find(Path::new("C:\\data\\nope")), None);
        assert_eq!(tree.path(tree.root()), Path::new("C:\\data"));
    }

    #[test]
    fn child_by_name_is_case_insensitive() {
        let tree = sample();
        assert!(tree.child_by_name(tree.root(), "b").is_some());
        assert!(tree.child_by_name(tree.root(), "zzz").is_none());
    }

    #[test]
    fn biggest_files_and_dirs() {
        let tree = sample();
        let files: Vec<_> = tree
            .biggest_files(2)
            .iter()
            .map(|&i| tree.node(i).name.clone())
            .collect();
        assert_eq!(files, ["top", "y"]);
        assert_eq!(tree.biggest_files(100).len(), 4);
        let dirs: Vec<_> = tree
            .biggest_dirs(10)
            .iter()
            .map(|&i| tree.node(i).name.clone())
            .collect();
        assert_eq!(dirs, ["a", "B", "empty"]);
        assert_eq!(tree.len(), 8);
        assert!(!tree.is_empty());
    }

    #[test]
    fn detach_updates_ancestors_and_hides_subtree() {
        let mut tree = sample();
        let a = tree.child_by_name(tree.root(), "a").unwrap();
        let x = tree.child_by_name(a, "x").unwrap();
        let before: Vec<NodeId> = tree.children(tree.root()).to_vec();
        tree.detach(a);
        let root = tree.node(tree.root());
        assert_eq!((root.size, root.allocated, root.file_count), (105, 136, 2));
        // Newest times of ancestors are deliberately not recomputed.
        assert_eq!(root.newest_modified, t(9));
        assert!(!tree.is_live(a) && !tree.is_live(x));
        assert!(tree.is_live(tree.root()));
        let expect: Vec<NodeId> = before.into_iter().filter(|&c| c != a).collect();
        assert_eq!(tree.children(tree.root()), expect.as_slice());
        assert_eq!(tree.find(Path::new("C:\\data\\a")), None);
        assert_eq!(tree.find(Path::new("C:\\data\\a\\x")), None);
        assert!(!tree.biggest_files(10).contains(&x));
        assert!(!tree.biggest_dirs(10).contains(&a));
    }

    #[test]
    fn detach_twice_or_root_is_noop() {
        let mut tree = sample();
        let b = tree.child_by_name(tree.root(), "B").unwrap();
        tree.detach(b);
        let snapshot = tree.node(tree.root()).allocated;
        tree.detach(b);
        tree.detach(tree.root());
        assert_eq!(tree.node(tree.root()).allocated, snapshot);
        assert!(tree.is_live(tree.root()));
    }

    #[test]
    fn detach_nested_subtracts_from_every_ancestor() {
        let mut b = TreeBuilder::new(Path::new("C:\\r"), None);
        let root = b.root();
        let d1 = b.add_dir(root, "d1", None);
        let d2 = b.add_dir(d1, "d2", None);
        b.add_file(d2, "f", 10, 16, None);
        b.add_file(d1, "g", 1, 4, None);
        let mut tree = b.finish();
        tree.detach(d2);
        assert_eq!(tree.node(d1).allocated, 4);
        assert_eq!(tree.node(d1).file_count, 1);
        assert_eq!(tree.node(root).size, 1);
    }
}
