//! The Windows Recycle Bin as a whole: total size, its contents, and emptying it.
//!
//! Folder sizes: the shell only reports an entry count for a folder in the bin, so the bytes
//! are measured by walking the item's real storage (`C:\$Recycle.Bin\<sid>\$R…`), which is
//! what `TrashItem.id` holds on Windows (the shell's absolute parsing name).

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Serialize;

use crate::cleanup::{ItemKind, long_form};

/// A deletion this close to a logged one, at the same path, is taken to be the same event.
const SAME_DELETION_MS: i64 = 2 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BinSummary {
    pub items: u64,
    /// Bytes, as Windows reports for the whole bin.
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BinItem {
    pub id: String,
    pub name: String,
    pub original_path: PathBuf,
    /// Unix milliseconds.
    pub deleted_at: i64,
    pub kind: ItemKind,
    pub size: Option<u64>,
    pub from_dev_disk: bool,
}

/// One bin entry as the system reports it, before it is matched against the cleanup log.
#[derive(Debug, Clone, PartialEq)]
pub struct RawBinItem {
    pub id: String,
    pub name: String,
    pub original_path: PathBuf,
    pub deleted_at_ms: i64,
    pub is_dir: bool,
    /// None when it could not be measured.
    pub size: Option<u64>,
}

pub trait RecycleBin {
    fn summary(&self) -> Result<BinSummary, String>;
    fn items(&self) -> Result<Vec<RawBinItem>, String>;
    /// Permanently deletes everything in the bin, on all drives.
    fn empty(&self) -> Result<(), String>;
}

pub struct SystemRecycleBin;

/// Same path in the sense Windows uses: case-insensitive, either slash, no `\\?\` prefix.
fn key(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_start_matches(r"\\?\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

/// Biggest first (unmeasured last, ties by name). `from_dev_disk` is true when a logged
/// `(path, deleted_at_ms)` has the same path and a deletion time within two minutes.
pub fn list_items(bin: &dyn RecycleBin, logged: &[(PathBuf, i64)]) -> Result<Vec<BinItem>, String> {
    let raw = bin.items()?;
    // Indexed once: a big bin times a long history would otherwise be a quadratic compare.
    let mut by_path: HashMap<String, Vec<i64>> = HashMap::new();
    for (path, at) in logged {
        by_path.entry(key(&long_form(path))).or_default().push(*at);
    }
    let mut out: Vec<BinItem> = raw
        .into_iter()
        .map(|r| {
            let from_dev_disk = by_path.get(&key(&r.original_path)).is_some_and(|times| {
                times
                    .iter()
                    .any(|t| (t - r.deleted_at_ms).abs() <= SAME_DELETION_MS)
            });
            BinItem {
                id: r.id,
                name: r.name,
                original_path: r.original_path,
                deleted_at: r.deleted_at_ms,
                kind: if r.is_dir {
                    ItemKind::Dir
                } else {
                    ItemKind::File
                },
                size: r.size,
                from_dev_disk,
            }
        })
        .collect();
    // `Option` orders None first, so compare the reversed pair to get None last.
    out.sort_by(|a, b| {
        b.size
            .is_some()
            .cmp(&a.size.is_some())
            .then(b.size.cmp(&a.size))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

#[cfg(windows)]
mod sys {
    use std::path::{Path, PathBuf};
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use windows_sys::Win32::UI::Shell::{
        SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND, SHEmptyRecycleBinW, SHQUERYRBINFO,
        SHQueryRecycleBinW,
    };

    use super::{BinSummary, RawBinItem, RecycleBin, SystemRecycleBin};
    use crate::scanner::{NoProgress, ScanOptions, Scanner, WalkScanner};

    /// What Windows returns for emptying a bin that is already empty (E_UNEXPECTED).
    const E_UNEXPECTED: i32 = 0x8000_FFFF_u32 as i32;
    const UNMEASURED: u64 = u64::MAX;
    const WORKERS: usize = 4;

    impl RecycleBin for SystemRecycleBin {
        fn summary(&self) -> Result<BinSummary, String> {
            let mut info = SHQUERYRBINFO {
                cbSize: size_of::<SHQUERYRBINFO>() as u32,
                i64Size: 0,
                i64NumItems: 0,
            };
            // SAFETY: a null root means all drives; `info` is valid with `cbSize` set.
            let hr = unsafe { SHQueryRecycleBinW(null(), &mut info) };
            if hr < 0 {
                return Err(format!("could not read the Recycle Bin (error {hr:#010x})"));
            }
            Ok(BinSummary {
                items: info.i64NumItems.max(0) as u64,
                size: info.i64Size.max(0) as u64,
            })
        }

        fn items(&self) -> Result<Vec<RawBinItem>, String> {
            let listed = trash::os_limited::list().map_err(|e| e.to_string())?;
            let mut out = Vec::with_capacity(listed.len());
            // Folders to walk later: (index in `out`, storage path).
            let mut folders: Vec<(usize, PathBuf)> = Vec::new();
            for item in &listed {
                let storage = PathBuf::from(&item.id);
                let (is_dir, size) = match std::fs::symlink_metadata(&storage) {
                    Ok(m) if m.is_dir() => (true, None),
                    Ok(m) => (false, Some(m.len())),
                    // Not a real path we can stat: ask the shell instead.
                    Err(_) => match trash::os_limited::metadata(item) {
                        Ok(m) => match m.size {
                            trash::TrashItemSize::Bytes(b) => (false, Some(b)),
                            trash::TrashItemSize::Entries(_) => (true, None),
                        },
                        Err(_) => (false, None),
                    },
                };
                if is_dir && storage.is_dir() {
                    folders.push((out.len(), storage));
                }
                out.push(RawBinItem {
                    id: item.id.to_string_lossy().into_owned(),
                    name: item.name.to_string_lossy().into_owned(),
                    original_path: item.original_path(),
                    deleted_at_ms: item.time_deleted.saturating_mul(1000),
                    is_dir,
                    size,
                });
            }
            for ((idx, _), size) in folders.iter().zip(measure_all(&folders)) {
                out[*idx].size = size;
            }
            Ok(out)
        }

        fn empty(&self) -> Result<(), String> {
            let flags = SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND;
            // SAFETY: null window and root are allowed; the call only reads its arguments.
            let hr = unsafe { SHEmptyRecycleBinW(null_mut(), null(), flags) };
            if hr == 0 || hr == E_UNEXPECTED {
                Ok(())
            } else {
                Err(format!(
                    "could not empty the Recycle Bin (error {hr:#010x})"
                ))
            }
        }
    }

    fn measure(path: &Path) -> Option<u64> {
        let res = WalkScanner
            .scan(path, &ScanOptions::default(), &NoProgress)
            .ok()?;
        Some(res.tree.node(res.tree.root()).allocated)
    }

    /// Sizes for each folder, in order; None where a walk failed.
    fn measure_all(folders: &[(usize, PathBuf)]) -> Vec<Option<u64>> {
        let sizes: Vec<AtomicU64> = folders.iter().map(|_| AtomicU64::new(UNMEASURED)).collect();
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..WORKERS.min(folders.len()) {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some((_, path)) = folders.get(i) else {
                            break;
                        };
                        if let Some(n) = measure(path) {
                            sizes[i].store(n, Ordering::Relaxed);
                        }
                    }
                });
            }
        });
        sizes
            .iter()
            .map(|a| Some(a.load(Ordering::Relaxed)).filter(|&n| n != UNMEASURED))
            .collect()
    }
}

#[cfg(not(windows))]
impl RecycleBin for SystemRecycleBin {
    fn summary(&self) -> Result<BinSummary, String> {
        Err("unsupported".into())
    }
    fn items(&self) -> Result<Vec<RawBinItem>, String> {
        Err("unsupported".into())
    }
    fn empty(&self) -> Result<(), String> {
        Err("unsupported".into())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    struct Fake {
        items: Result<Vec<RawBinItem>, String>,
        emptied: Cell<bool>,
    }

    impl Fake {
        fn with(items: Vec<RawBinItem>) -> Self {
            Self {
                items: Ok(items),
                emptied: Cell::new(false),
            }
        }
    }

    impl RecycleBin for Fake {
        fn summary(&self) -> Result<BinSummary, String> {
            Ok(BinSummary { items: 2, size: 9 })
        }
        fn items(&self) -> Result<Vec<RawBinItem>, String> {
            self.items.clone()
        }
        fn empty(&self) -> Result<(), String> {
            self.emptied.set(true);
            Ok(())
        }
    }

    const MIN: i64 = 60_000;
    const T0: i64 = 1_700_000_000_000;

    fn raw(name: &str, path: &str, at: i64, size: Option<u64>) -> RawBinItem {
        RawBinItem {
            id: format!("id-{name}"),
            name: name.to_owned(),
            original_path: PathBuf::from(path),
            deleted_at_ms: at,
            is_dir: true,
            size,
        }
    }

    #[test]
    fn biggest_first_with_unmeasured_last_and_ties_by_name() {
        let bin = Fake::with(vec![
            raw("none-b", "C:\\x\\none-b", T0, None),
            raw("small", "C:\\x\\small", T0, Some(10)),
            raw("tie-b", "C:\\x\\tie-b", T0, Some(500)),
            raw("none-a", "C:\\x\\none-a", T0, None),
            raw("big", "C:\\x\\big", T0, Some(9000)),
            raw("tie-a", "C:\\x\\tie-a", T0, Some(500)),
        ]);
        let names: Vec<_> = list_items(&bin, &[])
            .unwrap()
            .into_iter()
            .map(|i| i.name)
            .collect();
        assert_eq!(
            names,
            ["big", "tie-a", "tie-b", "small", "none-a", "none-b"]
        );
    }

    #[test]
    fn from_dev_disk_needs_same_path_and_a_close_deletion_time() {
        let bin = Fake::with(vec![
            raw("a", "C:\\Proj\\node_modules", T0, Some(1)),
            raw("b", "C:\\Proj\\node_modules", T0 + 10 * MIN, Some(2)),
            raw("c", "C:\\Proj\\other", T0, Some(3)),
            raw("d", "C:\\Proj\\node_modules", T0 + 2 * MIN, Some(4)),
            raw("e", "C:\\Proj\\node_modules", T0 - 2 * MIN - 1, Some(5)),
        ]);
        // Different case and slash style, as the log may hold them.
        let logged = vec![(PathBuf::from("c:/proj/NODE_MODULES"), T0)];
        let got: Vec<_> = list_items(&bin, &logged)
            .unwrap()
            .into_iter()
            .map(|i| (i.name, i.from_dev_disk))
            .collect();
        let flag = |n: &str| got.iter().find(|(x, _)| x == n).unwrap().1;
        assert!(flag("a"), "same path and time");
        assert!(!flag("b"), "10 minutes off");
        assert!(!flag("c"), "different path");
        assert!(flag("d"), "exactly 2 minutes is still within");
        assert!(!flag("e"), "just over 2 minutes");
    }

    #[test]
    fn items_error_is_propagated() {
        let bin = Fake {
            items: Err("shell said no".into()),
            emptied: Cell::new(false),
        };
        assert_eq!(list_items(&bin, &[]).unwrap_err(), "shell said no");
    }

    #[test]
    fn fields_map_over_and_kind_follows_is_dir() {
        let mut file = raw("f.txt", "C:\\x\\f.txt", T0, Some(7));
        file.is_dir = false;
        let out = list_items(&Fake::with(vec![file]), &[]).unwrap();
        assert_eq!(out[0].kind, ItemKind::File);
        assert_eq!(out[0].deleted_at, T0);
        assert_eq!(out[0].original_path, PathBuf::from("C:\\x\\f.txt"));
        assert_eq!(out[0].id, "id-f.txt");
    }

    #[test]
    fn json_keys_match_the_ui_types() {
        let keys = |v: serde_json::Value| {
            let mut k: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        let s = serde_json::to_value(BinSummary { items: 1, size: 2 }).unwrap();
        assert_eq!(keys(s), ["items", "size"]);
        let item = list_items(&Fake::with(vec![raw("a", "C:\\a", T0, None)]), &[]).unwrap();
        let v = serde_json::to_value(&item[0]).unwrap();
        assert_eq!(v["kind"], "dir");
        assert!(v["size"].is_null());
        assert_eq!(
            keys(v),
            [
                "deletedAt",
                "fromDevDisk",
                "id",
                "kind",
                "name",
                "originalPath",
                "size"
            ]
        );
    }

    // Read-only against the real bin: no empty, purge or restore anywhere in here.
    #[test]
    #[ignore = "reads the real Recycle Bin"]
    fn real_bin_can_be_summarised_and_listed() {
        let bin = SystemRecycleBin;
        let s = bin.summary().unwrap();
        println!("summary: {} items, {} bytes", s.items, s.size);
        let t = std::time::Instant::now();
        let items = bin.items().unwrap();
        let measured = items.iter().filter(|i| i.size.is_some()).count();
        let dirs = items.iter().filter(|i| i.is_dir).count();
        println!(
            "items: {} ({dirs} dirs, {measured} measured) in {:?}",
            items.len(),
            t.elapsed()
        );
        if let Some(first) = items.first() {
            println!("sample id: {}", first.id);
        }
        // The bin can change while this runs (other apps, a running DevDisk), so counts are only printed.
        let listed = list_items(&bin, &[]).unwrap();
        println!("list_items: {} items", listed.len());
    }
}
