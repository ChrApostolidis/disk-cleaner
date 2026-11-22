//! Saving the last scan to disk and loading it back, so the app reopens with it.
//!
//! Format, little-endian throughout:
//!
//! ```text
//! "DDSNAP" u32 version(1)
//! i64 scanned_at  u64 elapsed_ms  u64 issues
//! u32 root_path_len, root path (UTF-8)
//! u64 node_count                       (root included)
//! root:   modified
//! others: u32 parent, u8 kind (0 file, 1 dir), u16 name_len, name,
//!         files only: u64 size, u64 allocated,
//!         modified
//! modified = u8 present, then i64 unix nanoseconds when present
//! ```
//!
//! Nodes are written in pre-order, so a parent index always points backwards. Aggregates
//! (dir sizes, file counts, newest mtime) and child order are not stored: `TreeBuilder::finish`
//! recomputes them, which keeps the file small and impossible to leave inconsistent.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::index::{NodeKind, Tree, TreeBuilder};

const MAGIC: &[u8; 6] = b"DDSNAP";
const VERSION: u32 = 1;
/// Longest root path accepted on load; Windows paths top out far below this.
const MAX_ROOT_PATH: u32 = 1 << 16;
/// Parent indexes are u32, so no valid file has more nodes than that.
const MAX_NODES: u64 = u32::MAX as u64;
const IO_BUF: usize = 1 << 20;

pub struct Snapshot {
    pub tree: Tree,
    pub issues: u64,
    pub elapsed_ms: u64,
    pub scanned_at: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("snapshot file: {0}")]
    Io(#[from] io::Error),
    #[error("not a Ballast snapshot")]
    NotASnapshot,
    #[error("snapshot version {0} is not supported")]
    UnsupportedVersion(u32),
    #[error("snapshot is damaged: {0}")]
    Corrupt(String),
}

fn corrupt(why: &str) -> SnapshotError {
    SnapshotError::Corrupt(why.to_owned())
}

fn to_nanos(t: Option<SystemTime>) -> Option<i64> {
    let t = t?;
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).ok(),
        Err(e) => i64::try_from(e.duration().as_nanos()).ok().map(|n| -n),
    }
}

fn from_nanos(n: i64) -> Option<SystemTime> {
    let d = Duration::from_nanos(n.unsigned_abs());
    if n >= 0 {
        UNIX_EPOCH.checked_add(d)
    } else {
        UNIX_EPOCH.checked_sub(d)
    }
}

/// Writes to `<path>.tmp` and renames over `path`, so a crash mid-write never leaves a half
/// file where the last good snapshot was. Detached nodes are skipped.
pub fn save(
    path: &Path,
    tree: &Tree,
    issues: u64,
    elapsed_ms: u64,
    scanned_at: i64,
) -> Result<(), SnapshotError> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let result = write_file(&tmp, tree, issues, elapsed_ms, scanned_at)
        .and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

fn write_file(
    path: &Path,
    tree: &Tree,
    issues: u64,
    elapsed_ms: u64,
    scanned_at: i64,
) -> io::Result<()> {
    // Pre-order over live nodes; `index[id]` is where each node lands in the file.
    let mut order = Vec::with_capacity(tree.len());
    let mut index = vec![u32::MAX; tree.len()];
    let mut stack = vec![tree.root()];
    while let Some(id) = stack.pop() {
        if !tree.is_live(id) {
            continue;
        }
        index[id] = order.len() as u32;
        order.push(id);
        stack.extend(tree.children(id).iter().rev());
    }

    let file = File::create(path)?;
    let mut w = BufWriter::with_capacity(IO_BUF, file);
    w.write_all(MAGIC)?;
    w.write_all(&VERSION.to_le_bytes())?;
    w.write_all(&scanned_at.to_le_bytes())?;
    w.write_all(&elapsed_ms.to_le_bytes())?;
    w.write_all(&issues.to_le_bytes())?;
    let root = tree.root_path().to_string_lossy();
    w.write_all(&(root.len() as u32).to_le_bytes())?;
    w.write_all(root.as_bytes())?;
    w.write_all(&(order.len() as u64).to_le_bytes())?;

    for &id in &order {
        let n = tree.node(id);
        if let Some(parent) = n.parent {
            w.write_all(&index[parent].to_le_bytes())?;
            let is_dir = n.kind == NodeKind::Dir;
            w.write_all(&[is_dir as u8])?;
            let name = n.name.as_bytes();
            let len = u16::try_from(name.len())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "file name too long"))?;
            w.write_all(&len.to_le_bytes())?;
            w.write_all(name)?;
            if !is_dir {
                w.write_all(&n.size.to_le_bytes())?;
                w.write_all(&n.allocated.to_le_bytes())?;
            }
        }
        match to_nanos(n.modified) {
            Some(ns) => {
                w.write_all(&[1])?;
                w.write_all(&ns.to_le_bytes())?;
            }
            None => w.write_all(&[0])?,
        }
    }
    let file = w.into_inner().map_err(|e| e.into_error())?;
    // Without this the rename can land before the data does, and a power cut leaves an empty file.
    file.sync_all()
}

struct Reader<R: Read>(R);

impl<R: Read> Reader<R> {
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], SnapshotError> {
        let mut b = [0u8; N];
        self.fill(&mut b)?;
        Ok(b)
    }

    fn fill(&mut self, buf: &mut [u8]) -> Result<(), SnapshotError> {
        self.0.read_exact(buf).map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                corrupt("file ends too early")
            } else {
                SnapshotError::Io(e)
            }
        })
    }

    fn u8(&mut self) -> Result<u8, SnapshotError> {
        Ok(self.bytes::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, SnapshotError> {
        Ok(u16::from_le_bytes(self.bytes()?))
    }
    fn u32(&mut self) -> Result<u32, SnapshotError> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }
    fn u64(&mut self) -> Result<u64, SnapshotError> {
        Ok(u64::from_le_bytes(self.bytes()?))
    }
    fn i64(&mut self) -> Result<i64, SnapshotError> {
        Ok(i64::from_le_bytes(self.bytes()?))
    }

    fn modified(&mut self) -> Result<Option<SystemTime>, SnapshotError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(from_nanos(self.i64()?)),
            _ => Err(corrupt("bad time flag")),
        }
    }
}

pub fn load(path: &Path) -> Result<Snapshot, SnapshotError> {
    let mut r = Reader(BufReader::with_capacity(IO_BUF, File::open(path)?));

    // A short or different header means some other file, not a damaged snapshot.
    let mut magic = [0u8; MAGIC.len()];
    match r.0.read_exact(&mut magic) {
        Ok(()) if &magic == MAGIC => {}
        Ok(()) => return Err(SnapshotError::NotASnapshot),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
            return Err(SnapshotError::NotASnapshot);
        }
        Err(e) => return Err(e.into()),
    }
    let version = r.u32()?;
    if version != VERSION {
        return Err(SnapshotError::UnsupportedVersion(version));
    }
    let scanned_at = r.i64()?;
    let elapsed_ms = r.u64()?;
    let issues = r.u64()?;

    let root_len = r.u32()?;
    if root_len > MAX_ROOT_PATH {
        return Err(corrupt("root path too long"));
    }
    let mut buf = vec![0u8; root_len as usize];
    r.fill(&mut buf)?;
    let root_path = std::str::from_utf8(&buf).map_err(|_| corrupt("root path is not UTF-8"))?;
    let count = r.u64()?;
    if !(1..=MAX_NODES).contains(&count) {
        return Err(corrupt("bad node count"));
    }
    let mut builder = TreeBuilder::new(Path::new(root_path), r.modified()?);

    // Growing as nodes arrive rather than reserving `count`: the count is untrusted.
    let mut is_dir = vec![true];
    for i in 1..count as usize {
        let parent = r.u32()? as usize;
        // Pre-order: the parent was already read, and only a dir can have children.
        if parent >= i || !is_dir[parent] {
            return Err(corrupt("bad parent index"));
        }
        let dir = match r.u8()? {
            0 => false,
            1 => true,
            _ => return Err(corrupt("bad node kind")),
        };
        let len = r.u16()? as usize;
        buf.resize(len, 0);
        r.fill(&mut buf)?;
        let name = std::str::from_utf8(&buf).map_err(|_| corrupt("name is not UTF-8"))?;
        let id = if dir {
            builder.add_dir(parent, name, r.modified()?)
        } else {
            let size = r.u64()?;
            let allocated = r.u64()?;
            builder.add_file(parent, name, size, allocated, r.modified()?)
        };
        debug_assert_eq!(id, i);
        is_dir.push(dir);
    }
    let mut extra = [0u8; 1];
    if r.0.read(&mut extra)? != 0 {
        return Err(corrupt("unexpected data after the last node"));
    }
    Ok(Snapshot {
        tree: builder.finish(),
        issues,
        elapsed_ms,
        scanned_at,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::index::{NodeId, NodeKind, TreeBuilder};

    fn at(secs: u64) -> Option<SystemTime> {
        Some(UNIX_EPOCH + Duration::new(secs, 123_456_789))
    }

    /// Nested dirs, files, missing mtimes and non-ASCII names.
    fn sample() -> Tree {
        let mut b = TreeBuilder::new(Path::new("C:\\proj"), at(5));
        let r = b.root();
        let src = b.add_dir(r, "src", at(10));
        b.add_file(src, "main.rs", 100, 4096, at(20));
        b.add_file(src, "ünï-文件.txt", 7, 4096, None);
        b.add_dir(src, "empty-dir", None);
        let nm = b.add_dir(r, "node_modules", None);
        let a = b.add_dir(nm, "a", None);
        b.add_file(a, "index.js", 9000, 12288, at(30));
        b.add_file(r, "Cargo.toml", 20, 4096, at(1));
        b.finish()
    }

    fn live_ids(t: &Tree) -> Vec<NodeId> {
        let mut out = vec![];
        let mut stack = vec![t.root()];
        while let Some(n) = stack.pop() {
            out.push(n);
            stack.extend_from_slice(t.children(n));
        }
        out
    }

    fn assert_same(a: &Tree, b: &Tree) {
        assert_eq!(a.root_path(), b.root_path());
        let ids = live_ids(a);
        assert_eq!(ids.len(), live_ids(b).len());
        for id in ids {
            let p = a.path(id);
            let other = b.find(&p).unwrap_or_else(|| panic!("missing {p:?}"));
            let (x, y) = (a.node(id), b.node(other));
            assert_eq!(x.name, y.name, "{p:?}");
            assert_eq!(x.kind, y.kind, "{p:?}");
            assert_eq!(x.size, y.size, "{p:?}");
            assert_eq!(x.allocated, y.allocated, "{p:?}");
            assert_eq!(x.modified, y.modified, "{p:?}");
            assert_eq!(x.newest_modified, y.newest_modified, "{p:?}");
            assert_eq!(x.file_count, y.file_count, "{p:?}");
        }
    }

    fn tmp() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("scan.snap");
        (d, p)
    }

    #[test]
    fn round_trip_keeps_paths_values_and_aggregates() {
        let (_d, p) = tmp();
        let t = sample();
        save(&p, &t, 3, 4567, 1_700_000_000_123).unwrap();
        let s = load(&p).unwrap();
        assert_eq!(
            (s.issues, s.elapsed_ms, s.scanned_at),
            (3, 4567, 1_700_000_000_123)
        );
        assert_same(&t, &s.tree);
        let root = s.tree.node(s.tree.root());
        assert_eq!(root.allocated, 4096 + 4096 + 12288 + 4096);
        assert_eq!(root.file_count, 4);
        assert_eq!(root.kind, NodeKind::Dir);
    }

    #[test]
    fn detached_nodes_are_not_saved() {
        let (_d, p) = tmp();
        let mut t = sample();
        let nm = t.find(Path::new("C:\\proj\\node_modules")).unwrap();
        t.detach(nm);
        save(&p, &t, 0, 0, 0).unwrap();
        let s = load(&p).unwrap();
        assert!(s.tree.find(Path::new("C:\\proj\\node_modules")).is_none());
        assert!(
            s.tree
                .find(Path::new("C:\\proj\\node_modules\\a\\index.js"))
                .is_none()
        );
        assert_eq!(s.tree.len(), t.len() - 3);
        assert_eq!(
            s.tree.node(s.tree.root()).allocated,
            t.node(t.root()).allocated
        );
    }

    #[test]
    fn truncated_file_is_corrupt() {
        let (_d, p) = tmp();
        save(&p, &sample(), 0, 0, 0).unwrap();
        let full = fs::read(&p).unwrap();
        // Every cut past the magic and version must fail cleanly, never panic.
        for cut in 10..full.len() {
            fs::write(&p, &full[..cut]).unwrap();
            assert!(
                matches!(load(&p), Err(SnapshotError::Corrupt(_))),
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn random_bytes_are_not_a_snapshot() {
        let (_d, p) = tmp();
        let junk: Vec<u8> = (0..4096u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        fs::write(&p, junk).unwrap();
        assert!(matches!(load(&p), Err(SnapshotError::NotASnapshot)));
        fs::write(&p, b"").unwrap();
        assert!(matches!(load(&p), Err(SnapshotError::NotASnapshot)));
    }

    #[test]
    fn missing_file_is_an_io_error() {
        let (_d, p) = tmp();
        assert!(matches!(load(&p), Err(SnapshotError::Io(_))));
    }

    #[test]
    fn wrong_version_is_unsupported() {
        let (_d, p) = tmp();
        save(&p, &sample(), 0, 0, 0).unwrap();
        let mut bytes = fs::read(&p).unwrap();
        bytes[6..10].copy_from_slice(&2u32.to_le_bytes());
        fs::write(&p, bytes).unwrap();
        assert!(matches!(
            load(&p),
            Err(SnapshotError::UnsupportedVersion(2))
        ));
    }

    #[test]
    fn absurd_lengths_are_corrupt_not_a_panic() {
        let (_d, p) = tmp();
        save(&p, &sample(), 0, 0, 0).unwrap();
        let mut bytes = fs::read(&p).unwrap();
        for b in &mut bytes[10..] {
            *b = 0xFF;
        }
        fs::write(&p, bytes).unwrap();
        assert!(matches!(load(&p), Err(SnapshotError::Corrupt(_))));
    }

    #[test]
    fn save_is_atomic_and_replaces_an_existing_file() {
        let (d, p) = tmp();
        fs::write(&p, b"old contents").unwrap();
        save(&p, &sample(), 0, 0, 0).unwrap();
        assert!(load(&p).is_ok());
        let names: Vec<_> = fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["scan.snap"], "no .tmp left behind");
    }

    #[test]
    #[ignore = "performance: run with --release -- --ignored"]
    fn a_million_files_save_and_load_fast() {
        let mut b = TreeBuilder::new(Path::new("C:\\big"), None);
        let r = b.root();
        for d in 0..1000 {
            let dir = b.add_dir(r, &format!("folder-{d:04}"), at(d));
            for f in 0..1000u64 {
                b.add_file(dir, &format!("file-{f:05}.dat"), f * 3, f * 4096, at(f));
            }
        }
        let tree = b.finish();
        assert_eq!(tree.len(), 1_001_001);
        let (_d, p) = tmp();
        let t0 = Instant::now();
        save(&p, &tree, 0, 0, 0).unwrap();
        let save_t = t0.elapsed();
        let t1 = Instant::now();
        let s = load(&p).unwrap();
        let load_t = t1.elapsed();
        let size = fs::metadata(&p).unwrap().len();
        println!(
            "save {save_t:?}, load {load_t:?}, file {} MB ({size} bytes)",
            size / 1_000_000
        );
        assert_eq!(s.tree.len(), tree.len());
        assert_eq!(
            s.tree.node(s.tree.root()).allocated,
            tree.node(tree.root()).allocated
        );
        if !cfg!(debug_assertions) {
            assert!(save_t < Duration::from_secs(2), "save {save_t:?}");
            assert!(load_t < Duration::from_secs(2), "load {load_t:?}");
        }
    }
}
