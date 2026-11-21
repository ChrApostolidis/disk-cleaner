//! Admin fast mode: builds the folder tree by reading the NTFS Master File Table directly
//! instead of listing every folder. Needs a raw read handle on the volume, which Windows only
//! gives to administrators.
//!
//! Compared with `WalkScanner`: every file is seen (no access-denied folders), hard links
//! still appear once per name (like the walk), and size on disk is the real allocation, so
//! tiny files stored inside their MFT record count as 0 bytes, as Explorer shows them.

mod record;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::index::{NodeId, TreeBuilder};
use crate::scanner::{
    IssueKind, Progress, ProgressSink, ScanError, ScanIssue, ScanOptions, ScanResult, Scanner,
};
use record::{AttrList, Boot, Run, apply_fixups, data_extent_records, parse_boot, parse_record};

/// Reads raw bytes of a volume. Offsets and lengths are always multiples of the cluster size.
pub trait VolumeReader {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
}

/// Record 5 is the root folder of every NTFS volume.
const ROOT_RECORD: u64 = 5;
/// Records below 16 are NTFS's own metadata files ($MFT, $Bitmap, $Extend, ...).
const FIRST_USER_RECORD: u64 = 16;
const CHUNK: usize = 4 << 20;

const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;

/// FILETIME (100 ns ticks since 1601) to SystemTime.
fn filetime(ft: Option<u64>) -> Option<SystemTime> {
    const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;
    let ft = ft?;
    if ft >= UNIX_EPOCH_TICKS {
        let d = ft - UNIX_EPOCH_TICKS;
        UNIX_EPOCH.checked_add(Duration::new(d / 10_000_000, (d % 10_000_000) as u32 * 100))
    } else {
        let d = UNIX_EPOCH_TICKS - ft;
        UNIX_EPOCH.checked_sub(Duration::new(d / 10_000_000, (d % 10_000_000) as u32 * 100))
    }
}

fn bad(why: impl Into<String>) -> ScanError {
    ScanError::BadVolume(why.into())
}

fn is_cancelled(cancel: &Option<Arc<AtomicBool>>) -> bool {
    cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed))
}

/// The $MFT file itself, mapped through its data runs.
struct Mft<'a> {
    reader: &'a mut dyn VolumeReader,
    boot: Boot,
    /// (first VCN, run)
    runs: Vec<(u64, Run)>,
}

impl Mft<'_> {
    fn set_runs(&mut self, runs: &[Run]) {
        let mut vcn = 0;
        self.runs = runs
            .iter()
            .map(|r| {
                let at = vcn;
                vcn += r.clusters;
                (at, *r)
            })
            .collect();
    }

    /// Reads `buf.len()` bytes at `offset` within the MFT. Both are cluster multiples.
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), ScanError> {
        let cs = self.boot.cluster_size;
        let mut done = 0usize;
        while done < buf.len() {
            let pos = offset + done as u64;
            let vcn = pos / cs;
            let (start, run) = self
                .runs
                .iter()
                .copied()
                .find(|(s, r)| vcn >= *s && vcn < s + r.clusters)
                .ok_or_else(|| bad("the file table is shorter than it says"))?;
            let run_end = (start + run.clusters) * cs;
            let take = ((run_end - pos) as usize).min(buf.len() - done);
            let piece = &mut buf[done..done + take];
            match run.lcn {
                Some(lcn) => self
                    .reader
                    .read_at((lcn + vcn - start) * cs + pos % cs, piece)?,
                None => piece.fill(0),
            }
            done += take;
        }
        Ok(())
    }

    /// One record with fixups applied; None when it is damaged or not a FILE record.
    fn record(&mut self, n: u64) -> Result<Option<record::Record>, ScanError> {
        let cs = self.boot.cluster_size;
        let rs = self.boot.record_size as u64;
        let at = n * rs;
        let start = at - at % cs;
        let len = (at + rs).div_ceil(cs) * cs - start;
        let mut buf = vec![0u8; len as usize];
        self.read(start, &mut buf)?;
        let rec = &mut buf[(at - start) as usize..(at - start + rs) as usize];
        Ok(if apply_fixups(rec) {
            parse_record(rec)
        } else {
            None
        })
    }
}

/// Raw volume bytes of a non-resident attribute (used for a non-resident attribute list).
fn read_runs(
    reader: &mut dyn VolumeReader,
    cs: u64,
    runs: &[Run],
    size: u64,
) -> Result<Vec<u8>, ScanError> {
    let total: u64 = runs.iter().map(|r| r.clusters * cs).sum();
    let mut out = vec![0u8; total as usize];
    let mut pos = 0usize;
    for r in runs {
        let len = (r.clusters * cs) as usize;
        if let Some(lcn) = r.lcn {
            reader.read_at(lcn * cs, &mut out[pos..pos + len])?;
        }
        pos += len;
    }
    out.truncate(size as usize);
    Ok(out)
}

/// Finds the MFT's own data runs and size from record 0, following its attribute list when
/// the MFT is fragmented enough to need extension records.
fn open_mft<'a>(reader: &'a mut dyn VolumeReader, boot: Boot) -> Result<(Mft<'a>, u64), ScanError> {
    let mut mft = Mft {
        reader,
        boot,
        runs: Vec::new(),
    };
    // Record 0 sits at the very start of the MFT, so one fake run is enough to read it.
    let first = (boot.record_size as u64).div_ceil(boot.cluster_size);
    mft.set_runs(&[Run {
        lcn: Some(boot.mft_lcn),
        clusters: first,
    }]);
    let rec0 = mft
        .record(0)
        .map_err(|e| bad(format!("record 0: {e}")))?
        .ok_or_else(|| bad("record 0 of the file table is damaged"))?;
    let size = rec0
        .data
        .filter(|d| !d.resident)
        .map(|d| d.size)
        .ok_or_else(|| bad("the file table has no data"))?;
    let mut extents = rec0.data_extents.clone();
    mft.set_runs(
        &extents
            .iter()
            .flat_map(|e| e.runs.clone())
            .collect::<Vec<_>>(),
    );

    if let Some(list) = &rec0.attr_list {
        let bytes = match list {
            AttrList::Resident(b) => b.clone(),
            AttrList::NonResident { runs, size } => {
                read_runs(mft.reader, boot.cluster_size, runs, *size)?
            }
        };
        for (_, n) in data_extent_records(&bytes)
            .into_iter()
            .filter(|(_, n)| *n != 0)
        {
            if let Some(ext) = mft.record(n)? {
                extents.extend(ext.data_extents);
            }
        }
        extents.sort_by_key(|e| e.lowest_vcn);
        extents.dedup_by_key(|e| e.lowest_vcn);
        mft.set_runs(
            &extents
                .iter()
                .flat_map(|e| e.runs.clone())
                .collect::<Vec<_>>(),
        );
    }
    Ok((mft, size))
}

#[derive(Debug, Clone, Default)]
struct Entry {
    in_use: bool,
    is_dir: bool,
    seq: u16,
    modified: Option<u64>,
    attributes: u32,
    size: u64,
    allocated: u64,
    reparse: Option<u32>,
}

impl Entry {
    fn is_link(&self) -> bool {
        matches!(
            self.reparse,
            Some(IO_REPARSE_TAG_MOUNT_POINT | IO_REPARSE_TAG_SYMLINK)
        )
    }

    fn on_disk(&self) -> u64 {
        // Cloud-only placeholders occupy no local space, same rule as the walk scanner.
        if self.attributes & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0 {
            0
        } else {
            self.allocated
        }
    }
}

struct Table {
    entries: Vec<Entry>,
    /// (record, parent record, parent sequence, name)
    names: Vec<(u32, u32, u16, String)>,
    damaged: u64,
}

/// Reads every record of the MFT into a flat table.
fn read_table(
    mft: &mut Mft,
    size: u64,
    options: &ScanOptions,
    progress: &dyn ProgressSink,
) -> Result<Table, ScanError> {
    let rs = mft.boot.record_size;
    let total = (size / rs as u64) as usize;
    let mut t = Table {
        entries: vec![Entry::default(); total],
        names: Vec::new(),
        damaged: 0,
    };
    let chunk = CHUNK.max(rs).max(mft.boot.cluster_size as usize);
    let mut buf = vec![0u8; chunk];
    let (mut files, mut dirs, mut bytes) = (0u64, 0u64, 0u64);
    let mut last_report = Instant::now();
    let mut offset = 0u64;
    while offset < size {
        if is_cancelled(&options.cancel) {
            return Err(ScanError::Cancelled);
        }
        let want = ((size - offset) as usize).min(chunk);
        // Reads stay cluster-aligned; the tail past `size` is ignored.
        let aligned =
            want.div_ceil(mft.boot.cluster_size as usize) * mft.boot.cluster_size as usize;
        let buf = &mut buf[..aligned.min(chunk)];
        mft.read(offset, buf)?;
        for (i, rec) in buf[..want].chunks_exact_mut(rs).enumerate() {
            let n = (offset as usize / rs) + i;
            if rec[..4] != *b"FILE" {
                continue;
            }
            let Some(r) = apply_fixups(rec).then(|| parse_record(rec)).flatten() else {
                t.damaged += 1;
                continue;
            };
            if !r.in_use {
                continue;
            }
            // Extension records add attributes to their base record.
            let target = if r.base != 0 && r.base != n as u64 {
                if r.base as usize >= total {
                    continue;
                }
                r.base as usize
            } else {
                let e = &mut t.entries[n];
                e.in_use = true;
                e.is_dir = r.is_dir;
                e.seq = r.seq;
                e.modified = r.modified;
                e.attributes = r.attributes.unwrap_or(0);
                if r.is_dir {
                    dirs += 1;
                } else {
                    files += 1;
                }
                n
            };
            let e = &mut t.entries[target];
            if let Some(d) = r.data {
                e.size = d.size;
                e.allocated = d.allocated;
                bytes += d.allocated;
            }
            if r.reparse_tag.is_some() {
                e.reparse = r.reparse_tag;
            }
            for name in r.names {
                if name.parent < total as u64 {
                    t.names.push((
                        target as u32,
                        name.parent as u32,
                        name.parent_seq,
                        name.name,
                    ));
                }
            }
        }
        offset += want as u64;
        if last_report.elapsed() >= options.progress_interval {
            last_report = Instant::now();
            progress.report(&Progress {
                files,
                dirs,
                bytes,
                current_path: PathBuf::from(format!(
                    "Reading the drive's file table… {}%",
                    offset * 100 / size.max(1)
                )),
            });
        }
    }
    Ok(t)
}

/// Child lists per folder, as offsets into one flat array of (child record, name index).
struct Children {
    offsets: Vec<u32>,
    items: Vec<(u32, u32)>,
}

impl Children {
    fn new(t: &Table) -> Self {
        let n = t.entries.len();
        let valid = |&(child, parent, pseq, _): &(u32, u32, u16, String)| {
            let p = &t.entries[parent as usize];
            child != parent
                && t.entries[child as usize].in_use
                && p.in_use
                && p.is_dir
                // A name whose parent record was reused for another file is stale.
                && (pseq == 0 || pseq == p.seq)
        };
        let mut counts = vec![0u32; n + 1];
        for name in t.names.iter().filter(|x| valid(x)) {
            counts[name.1 as usize + 1] += 1;
        }
        for i in 0..n {
            counts[i + 1] += counts[i];
        }
        let mut fill = counts.clone();
        let mut items = vec![(0u32, 0u32); counts[n] as usize];
        for (i, name) in t.names.iter().enumerate() {
            if valid(name) {
                let slot = &mut fill[name.1 as usize];
                items[*slot as usize] = (name.0, i as u32);
                *slot += 1;
            }
        }
        Self {
            offsets: counts,
            items,
        }
    }

    fn of(&self, record: u64) -> &[(u32, u32)] {
        let r = record as usize;
        &self.items[self.offsets[r] as usize..self.offsets[r + 1] as usize]
    }
}

/// Scans the folder `rel` (path components below the volume root) of the volume behind
/// `reader`. `root_path` is the folder's full path, used for the tree and for messages.
pub fn scan_volume(
    reader: &mut dyn VolumeReader,
    rel: &[String],
    root_path: &Path,
    options: &ScanOptions,
    progress: &dyn ProgressSink,
) -> Result<ScanResult, ScanError> {
    let started = Instant::now();
    if is_cancelled(&options.cancel) {
        return Err(ScanError::Cancelled);
    }
    let mut boot_buf = vec![0u8; 4096];
    reader.read_at(0, &mut boot_buf)?;
    let boot = parse_boot(&boot_buf).map_err(bad)?;
    let (mut mft, size) = open_mft(reader, boot)?;
    let table = read_table(&mut mft, size, options, progress)?;
    if (ROOT_RECORD as usize) >= table.entries.len() || !table.entries[ROOT_RECORD as usize].in_use
    {
        return Err(bad(
            "the drive's root folder is missing from the file table",
        ));
    }
    let children = Children::new(&table);
    let name_of = |i: u32| table.names[i as usize].3.as_str();

    // Find the scanned folder by name, one component at a time.
    let mut root = ROOT_RECORD;
    for comp in rel {
        let hit = children
            .of(root)
            .iter()
            .find(|&&(_, ni)| name_of(ni).eq_ignore_ascii_case(comp))
            .map(|&(c, _)| c as u64)
            .ok_or_else(|| ScanError::RootNotFound(root_path.to_path_buf()))?;
        let e = &table.entries[hit as usize];
        if e.is_link() {
            return Err(ScanError::RootNotFound(root_path.to_path_buf()));
        }
        if !e.is_dir {
            return Err(ScanError::RootNotADirectory(root_path.to_path_buf()));
        }
        root = hit;
    }

    let mut builder = TreeBuilder::new(root_path, filetime(table.entries[root as usize].modified));
    let mut issues = Vec::new();
    if table.damaged > 0 {
        issues.push(ScanIssue {
            path: root_path.to_path_buf(),
            kind: IssueKind::Other,
            message: format!(
                "{} file table records could not be read and were left out",
                table.damaged
            ),
        });
    }
    let mut visited = vec![false; table.entries.len()];
    visited[root as usize] = true;
    let mut stack: Vec<(u64, NodeId, PathBuf)> = vec![(root, builder.root(), root_path.into())];
    let (mut files, mut dirs, mut bytes) = (0u64, 0u64, 0u64);
    let mut since_check = 0u32;
    while let Some((record, node, path)) = stack.pop() {
        for &(child, ni) in children.of(record) {
            let c = child as u64;
            if c < FIRST_USER_RECORD {
                continue;
            }
            let e = &table.entries[child as usize];
            let name = name_of(ni);
            if e.is_link() {
                issues.push(ScanIssue {
                    path: path.join(name),
                    kind: IssueKind::SkippedLink,
                    message: "symlink or junction not followed".into(),
                });
            } else if e.is_dir {
                // A folder reached twice means the table points in a loop.
                if std::mem::replace(&mut visited[child as usize], true) {
                    continue;
                }
                let id = builder.add_dir(node, name, filetime(e.modified));
                stack.push((c, id, path.join(name)));
                dirs += 1;
            } else {
                builder.add_file(node, name, e.size, e.on_disk(), filetime(e.modified));
                files += 1;
                bytes += e.on_disk();
            }
            since_check += 1;
            if since_check >= 4096 {
                since_check = 0;
                if is_cancelled(&options.cancel) {
                    return Err(ScanError::Cancelled);
                }
            }
        }
    }
    progress.report(&Progress {
        files,
        dirs,
        bytes,
        current_path: root_path.to_path_buf(),
    });
    Ok(ScanResult {
        tree: builder.finish(),
        issues,
        elapsed: started.elapsed(),
    })
}

/// Reads NTFS volumes directly. Fails with `ScanError::NeedsAdmin` when the process is not
/// elevated and `ScanError::NotNtfs` on other file systems.
#[derive(Debug, Default)]
pub struct MftScanner;

impl Scanner for MftScanner {
    fn scan(
        &self,
        root: &Path,
        options: &ScanOptions,
        progress: &dyn ProgressSink,
    ) -> Result<ScanResult, ScanError> {
        if is_cancelled(&options.cancel) {
            return Err(ScanError::Cancelled);
        }
        let meta = std::fs::metadata(root).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => ScanError::RootNotFound(root.to_path_buf()),
            _ => ScanError::Io(e),
        })?;
        if !meta.is_dir() {
            return Err(ScanError::RootNotADirectory(root.to_path_buf()));
        }
        scan_real(root, options, progress)
    }
}

#[cfg(windows)]
fn scan_real(
    root: &Path,
    options: &ScanOptions,
    progress: &dyn ProgressSink,
) -> Result<ScanResult, ScanError> {
    let abs = std::path::absolute(root)?;
    let vol = win::volume_of(&abs)?;
    if !vol.file_system.eq_ignore_ascii_case("NTFS") {
        return Err(ScanError::NotNtfs(vol.file_system));
    }
    let skip = vol.mount.components().count();
    let rel: Vec<String> = abs
        .components()
        .skip(skip)
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let file = std::fs::File::open(&vol.device).map_err(|e| match e.kind() {
        io::ErrorKind::PermissionDenied => ScanError::NeedsAdmin,
        _ => ScanError::Io(e),
    })?;
    let mut reader = win::FileReader(file);
    scan_volume(&mut reader, &rel, root, options, progress)
}

#[cfg(not(windows))]
fn scan_real(
    _root: &Path,
    _options: &ScanOptions,
    _progress: &dyn ProgressSink,
) -> Result<ScanResult, ScanError> {
    Err(ScanError::NotNtfs("not Windows".into()))
}

#[cfg(windows)]
mod win {
    use super::VolumeReader;
    use crate::scanner::ScanError;
    use std::ffi::OsString;
    use std::io;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::FileExt;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Storage::FileSystem::{
        GetVolumeInformationW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };

    pub struct FileReader(pub std::fs::File);

    impl VolumeReader for FileReader {
        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
            let mut done = 0;
            while done < buf.len() {
                let n = self.0.seek_read(&mut buf[done..], offset + done as u64)?;
                if n == 0 {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                done += n;
            }
            Ok(())
        }
    }

    pub struct Volume {
        /// e.g. `C:\`
        pub mount: PathBuf,
        /// e.g. `\\?\Volume{guid}`, openable for raw reads.
        pub device: PathBuf,
        pub file_system: String,
    }

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain([0]).collect()
    }

    fn until_nul(buf: &[u16]) -> OsString {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        OsString::from_wide(&buf[..len])
    }

    pub fn volume_of(path: &Path) -> Result<Volume, ScanError> {
        let mut buf = vec![0u16; 1024];
        // SAFETY: the input is NUL-terminated and `buf` is valid for its length.
        if unsafe {
            GetVolumePathNameW(
                wide(path.as_os_str()).as_ptr(),
                buf.as_mut_ptr(),
                buf.len() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        let mount = until_nul(&buf);
        let mount_w = wide(&mount);
        let mut name = vec![0u16; 1024];
        // SAFETY: `mount_w` ends with a backslash and NUL as the API requires; `name` is valid.
        if unsafe {
            GetVolumeNameForVolumeMountPointW(
                mount_w.as_ptr(),
                name.as_mut_ptr(),
                name.len() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        // The volume name ends with a backslash, which would open the root folder instead
        // of the volume itself.
        let device = until_nul(&name)
            .to_string_lossy()
            .trim_end_matches('\\')
            .to_owned();
        let mut fs = vec![0u16; 64];
        // SAFETY: optional outputs are null; `fs` is valid for its length.
        if unsafe {
            GetVolumeInformationW(
                mount_w.as_ptr(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Volume {
            mount: PathBuf::from(mount),
            device: PathBuf::from(device),
            file_system: until_nul(&fs).to_string_lossy().into_owned(),
        })
    }
}

#[cfg(test)]
mod tests;
