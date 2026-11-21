//! Tests run against small NTFS images built in memory, so no admin rights are needed.
//! The one test that reads a real drive is `#[ignore]`d; run it from an elevated terminal.

use super::record::{Run, decode_runs};
use super::*;
use crate::index::{NodeKind, Tree};
use crate::scanner::NoProgress;
use std::sync::Mutex;

const CS: u64 = 4096;
const RS: usize = 1024;
const SECTOR: u64 = 512;
/// Some time in 2022, as a FILETIME.
const T0: u64 = 133_000_000_000_000_000;

// ---------- record builder ----------

fn align8(n: usize) -> usize {
    n.div_ceil(8) * 8
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn resident(kind: u32, stream: &str, value: &[u8]) -> Vec<u8> {
    let name = utf16le(stream);
    let value_off = align8(0x18 + name.len());
    let len = align8(value_off + value.len());
    let mut a = vec![0u8; len];
    a[0..4].copy_from_slice(&kind.to_le_bytes());
    a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    a[9] = (name.len() / 2) as u8;
    a[0x0A..0x0C].copy_from_slice(&0x18u16.to_le_bytes());
    a[0x10..0x14].copy_from_slice(&(value.len() as u32).to_le_bytes());
    a[0x14..0x16].copy_from_slice(&(value_off as u16).to_le_bytes());
    a[0x18..0x18 + name.len()].copy_from_slice(&name);
    a[value_off..value_off + value.len()].copy_from_slice(value);
    a
}

struct NonRes<'a> {
    kind: u32,
    stream: &'a str,
    lowest_vcn: u64,
    runs: Vec<u8>,
    allocated: u64,
    size: u64,
    /// Compressed or sparse: (attribute flags, total allocated).
    packed: Option<(u16, u64)>,
}

fn non_resident(n: NonRes) -> Vec<u8> {
    let name = utf16le(n.stream);
    let head = if n.packed.is_some() { 0x48 } else { 0x40 };
    let runs_off = align8(head + name.len());
    let len = align8(runs_off + n.runs.len() + 1);
    let mut a = vec![0u8; len];
    a[0..4].copy_from_slice(&n.kind.to_le_bytes());
    a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    a[8] = 1;
    a[9] = (name.len() / 2) as u8;
    a[0x0A..0x0C].copy_from_slice(&(head as u16).to_le_bytes());
    if let Some((flags, total)) = n.packed {
        a[0x0C..0x0E].copy_from_slice(&flags.to_le_bytes());
        a[0x40..0x48].copy_from_slice(&total.to_le_bytes());
    }
    a[0x10..0x18].copy_from_slice(&n.lowest_vcn.to_le_bytes());
    let clusters = n.allocated.div_ceil(CS).max(1);
    a[0x18..0x20].copy_from_slice(&(n.lowest_vcn + clusters - 1).to_le_bytes());
    a[0x20..0x22].copy_from_slice(&(runs_off as u16).to_le_bytes());
    a[0x28..0x30].copy_from_slice(&n.allocated.to_le_bytes());
    a[0x30..0x38].copy_from_slice(&n.size.to_le_bytes());
    a[0x38..0x40].copy_from_slice(&n.size.to_le_bytes());
    a[head..head + name.len()].copy_from_slice(&name);
    a[runs_off..runs_off + n.runs.len()].copy_from_slice(&n.runs);
    a
}

/// Data runs from (absolute LCN or None for sparse, clusters).
fn runs(list: &[(Option<u64>, u64)]) -> Vec<u8> {
    fn min_bytes(v: i64, signed: bool) -> Vec<u8> {
        let b = v.to_le_bytes();
        for n in 1..=8 {
            let fits = if signed {
                let shift = 64 - 8 * n as u32;
                n == 8 || (v << shift) >> shift == v
            } else {
                n == 8 || (v as u64) >> (8 * n) == 0
            };
            if fits {
                return b[..n].to_vec();
            }
        }
        b.to_vec()
    }
    let mut out = Vec::new();
    let mut prev = 0i64;
    for &(lcn, clusters) in list {
        let len = min_bytes(clusters as i64, false);
        let off = match lcn {
            Some(l) => {
                let d = l as i64 - prev;
                prev = l as i64;
                min_bytes(d, true)
            }
            None => Vec::new(),
        };
        out.push(((off.len() as u8) << 4) | len.len() as u8);
        out.extend(len);
        out.extend(off);
    }
    out.push(0);
    out
}

fn si(modified: u64, attributes: u32) -> Vec<u8> {
    let mut v = vec![0u8; 0x48];
    v[0x08..0x10].copy_from_slice(&modified.to_le_bytes());
    v[0x20..0x24].copy_from_slice(&attributes.to_le_bytes());
    resident(0x10, "", &v)
}

fn file_name(parent: u64, parent_seq: u16, name: &str, namespace: u8) -> Vec<u8> {
    let n = utf16le(name);
    let mut v = vec![0u8; 0x42 + n.len()];
    let r = parent | (u64::from(parent_seq) << 48);
    v[0..8].copy_from_slice(&r.to_le_bytes());
    v[0x40] = (n.len() / 2) as u8;
    v[0x41] = namespace;
    v[0x42..].copy_from_slice(&n);
    resident(0x30, "", &v)
}

#[derive(Clone)]
struct R {
    flags: u16,
    seq: u16,
    base: u64,
    attrs: Vec<Vec<u8>>,
    break_fixup: bool,
}

fn rec() -> R {
    R {
        flags: 0x01,
        seq: 1,
        base: 0,
        attrs: Vec::new(),
        break_fixup: false,
    }
}

impl R {
    fn dir(mut self) -> Self {
        self.flags |= 0x02;
        self
    }
    fn unused(mut self) -> Self {
        self.flags &= !0x01;
        self
    }
    fn seq(mut self, s: u16) -> Self {
        self.seq = s;
        self
    }
    fn base(mut self, b: u64) -> Self {
        self.base = b;
        self
    }
    fn attr(mut self, a: Vec<u8>) -> Self {
        self.attrs.push(a);
        self
    }
    fn si(self, modified: u64, attributes: u32) -> Self {
        self.attr(si(modified, attributes))
    }
    fn name(self, parent: u64, name: &str) -> Self {
        self.attr(file_name(parent, 1, name, 1))
    }
    fn data(self, size: u64, allocated: u64) -> Self {
        self.attr(non_resident(NonRes {
            kind: 0x80,
            stream: "",
            lowest_vcn: 0,
            runs: runs(&[(Some(900), allocated.div_ceil(CS).max(1))]),
            allocated,
            size,
            packed: None,
        }))
    }
    fn small(self, len: usize) -> Self {
        self.attr(resident(0x80, "", &vec![b'x'; len]))
    }
    fn reparse(self, tag: u32) -> Self {
        self.attr(resident(0xC0, "", &tag.to_le_bytes()))
    }
    fn torn(mut self) -> Self {
        self.break_fixup = true;
        self
    }

    fn build(&self) -> Vec<u8> {
        let mut r = vec![0u8; RS];
        r[0..4].copy_from_slice(b"FILE");
        r[4..6].copy_from_slice(&0x30u16.to_le_bytes());
        let usa_count = (RS / 512 + 1) as u16;
        r[6..8].copy_from_slice(&usa_count.to_le_bytes());
        r[0x10..0x12].copy_from_slice(&self.seq.to_le_bytes());
        r[0x14..0x16].copy_from_slice(&0x38u16.to_le_bytes());
        r[0x16..0x18].copy_from_slice(&self.flags.to_le_bytes());
        r[0x1C..0x20].copy_from_slice(&(RS as u32).to_le_bytes());
        r[0x20..0x28].copy_from_slice(&self.base.to_le_bytes());
        let mut pos = 0x38;
        for a in &self.attrs {
            r[pos..pos + a.len()].copy_from_slice(a);
            pos += a.len();
        }
        r[pos..pos + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        r[0x18..0x1C].copy_from_slice(&((pos + 8) as u32).to_le_bytes());
        // Update sequence: save each block's last two bytes, then stamp the sequence number.
        let usn = [0x42u8, 0x00];
        r[0x30..0x32].copy_from_slice(&usn);
        for i in 1..usa_count as usize {
            let end = i * 512;
            let saved = [r[end - 2], r[end - 1]];
            r[0x30 + i * 2..0x30 + i * 2 + 2].copy_from_slice(&saved);
            r[end - 2..end].copy_from_slice(&usn);
        }
        if self.break_fixup {
            r[510] ^= 0xFF;
        }
        r
    }
}

fn file(parent: u64, name: &str) -> R {
    rec().si(T0, 0).name(parent, name)
}

fn folder(parent: u64, name: &str) -> R {
    rec().dir().si(T0, 0).name(parent, name)
}

// ---------- volume image ----------

/// An NTFS image: boot sector, MFT at `mft_runs`, records placed through those runs.
struct Image {
    bytes: Vec<u8>,
    mft_runs: Vec<(u64, u64)>,
    reads: u64,
}

impl VolumeReader for Image {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        // A raw volume only accepts sector-aligned reads.
        assert!(
            offset.is_multiple_of(SECTOR) && (buf.len() as u64).is_multiple_of(SECTOR),
            "unaligned read {offset} +{}",
            buf.len()
        );
        let end = offset as usize + buf.len();
        if end > self.bytes.len() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf.copy_from_slice(&self.bytes[offset as usize..end]);
        self.reads += 1;
        Ok(())
    }
}

fn boot_sector(mft_lcn: u64) -> Vec<u8> {
    let mut b = vec![0u8; 512];
    b[3..11].copy_from_slice(b"NTFS    ");
    b[0x0B..0x0D].copy_from_slice(&512u16.to_le_bytes());
    b[0x0D] = (CS / 512) as u8;
    b[0x30..0x38].copy_from_slice(&mft_lcn.to_le_bytes());
    // 2^10 = 1024-byte records, written as a negative number.
    b[0x40] = (-10i8) as u8;
    b
}

impl Image {
    fn new(mft_runs: &[(u64, u64)], total_clusters: u64) -> Self {
        let mut bytes = vec![0u8; (total_clusters * CS) as usize];
        bytes[..512].copy_from_slice(&boot_sector(mft_runs[0].0));
        Self {
            bytes,
            mft_runs: mft_runs.to_vec(),
            reads: 0,
        }
    }

    fn records(&self) -> u64 {
        self.mft_runs.iter().map(|r| r.1).sum::<u64>() * CS / RS as u64
    }

    fn put(&mut self, n: u64, r: &R) {
        let at = n * RS as u64;
        let mut vcn = at / CS;
        for &(lcn, clusters) in &self.mft_runs {
            if vcn < clusters {
                let phys = ((lcn + vcn) * CS + at % CS) as usize;
                self.bytes[phys..phys + RS].copy_from_slice(&r.build());
                return;
            }
            vcn -= clusters;
        }
        panic!("record {n} is outside the MFT");
    }

    /// Record 0 ($MFT) describing all runs in one extent.
    fn put_mft_record(&mut self) {
        let list: Vec<(Option<u64>, u64)> =
            self.mft_runs.iter().map(|&(l, c)| (Some(l), c)).collect();
        let clusters: u64 = self.mft_runs.iter().map(|r| r.1).sum();
        let r = file(ROOT_RECORD, "$MFT").attr(non_resident(NonRes {
            kind: 0x80,
            stream: "",
            lowest_vcn: 0,
            runs: runs(&list),
            allocated: clusters * CS,
            size: self.records() * RS as u64,
            packed: None,
        }));
        self.put(0, &r);
    }
}

/// root/{docs/{a.txt (100 B, resident), big.bin (10000 B)}, empty/, top.dat (5000 B)} plus
/// the metadata files NTFS keeps in its root.
fn sample(mft_runs: &[(u64, u64)]) -> Image {
    let mut img = Image::new(mft_runs, 64);
    img.put_mft_record();
    for (n, name) in [
        (1, "$MFTMirr"),
        (2, "$LogFile"),
        (3, "$Volume"),
        (6, "$Bitmap"),
    ] {
        img.put(n, &file(ROOT_RECORD, name).data(4096, 4096));
    }
    img.put(11, &folder(ROOT_RECORD, "$Extend"));
    img.put(ROOT_RECORD, &folder(ROOT_RECORD, "."));
    img.put(16, &folder(ROOT_RECORD, "docs"));
    img.put(17, &file(16, "a.txt").small(100));
    img.put(18, &file(16, "big.bin").data(10_000, 12_288));
    img.put(19, &folder(ROOT_RECORD, "empty"));
    img.put(20, &file(ROOT_RECORD, "top.dat").data(5000, 8192));
    img
}

const ROOT: &str = r"C:\";

fn scan(img: &mut Image, rel: &[&str]) -> Result<ScanResult, ScanError> {
    let rel: Vec<String> = rel.iter().map(|s| s.to_string()).collect();
    let mut path = PathBuf::from(ROOT);
    path.extend(&rel);
    scan_volume(img, &rel, &path, &ScanOptions::default(), &NoProgress)
}

fn names(tree: &Tree, id: NodeId) -> Vec<String> {
    let mut v: Vec<String> = tree
        .children(id)
        .iter()
        .map(|&c| tree.node(c).name.clone())
        .collect();
    v.sort();
    v
}

fn node<'a>(tree: &'a Tree, path: &str) -> &'a crate::index::Node {
    tree.node(tree.find(&Path::new(ROOT).join(path)).unwrap())
}

// ---------- tests ----------

#[test]
fn data_runs_decode_with_relative_and_sparse_offsets() {
    // len 0x18 at 0x5634; then 5 clusters 5 back; then a 7-cluster hole.
    let r = decode_runs(&[0x21, 0x18, 0x34, 0x56, 0x11, 0x05, 0xFB, 0x01, 0x07, 0x00]).unwrap();
    assert_eq!(
        r,
        [
            Run {
                lcn: Some(0x5634),
                clusters: 0x18
            },
            Run {
                lcn: Some(0x5634 - 5),
                clusters: 5
            },
            Run {
                lcn: None,
                clusters: 7
            },
        ]
    );
    assert!(decode_runs(&[0x21, 0x18]).is_err());
    assert!(
        decode_runs(&[0x11, 0x01, 0x80, 0x00]).is_err(),
        "runs before LCN 0"
    );
    // The builder used by the other tests round-trips.
    let built = runs(&[(Some(300), 2), (Some(10), 70_000), (None, 3)]);
    let back = decode_runs(&built).unwrap();
    assert_eq!(
        back[1],
        Run {
            lcn: Some(10),
            clusters: 70_000
        }
    );
    assert_eq!(back[2].lcn, None);
}

#[test]
fn builds_the_tree_with_sizes_and_times() {
    let mut img = sample(&[(4, 6)]);
    let res = scan(&mut img, &[]).unwrap();
    let t = &res.tree;
    assert_eq!(names(t, t.root()), ["docs", "empty", "top.dat"]);
    let root = t.node(t.root());
    assert_eq!(root.size, 15_100);
    // The resident a.txt lives inside its record: no clusters of its own.
    assert_eq!(root.allocated, 12_288 + 8192);
    assert_eq!(root.file_count, 3);
    assert_eq!(node(t, r"docs\a.txt").allocated, 0);
    assert_eq!(node(t, r"docs\a.txt").size, 100);
    assert_eq!(node(t, r"docs\big.bin").allocated, 12_288);
    assert_eq!(node(t, "docs").kind, NodeKind::Dir);
    assert_eq!(node(t, "empty").file_count, 0);
    assert_eq!(node(t, "top.dat").modified, filetime(Some(T0)));
    assert!(t.node(t.root()).newest_modified.is_some());
    assert!(res.issues.is_empty(), "{:?}", res.issues);
}

#[test]
fn filetime_converts_to_unix_time() {
    let t = filetime(Some(116_444_736_000_000_000 + 15_000_000)).unwrap();
    assert_eq!(t, UNIX_EPOCH + Duration::from_millis(1500));
    assert!(filetime(Some(1)).unwrap() < UNIX_EPOCH);
    assert_eq!(filetime(None), None);
}

#[test]
fn a_fragmented_mft_is_read_through_all_its_runs() {
    // 24 records in three pieces, the middle one before the first.
    let mut img = sample(&[(10, 2), (4, 2), (20, 2)]);
    img.put(23, &file(16, "last.bin").data(1, 4096));
    let res = scan(&mut img, &[]).unwrap();
    assert_eq!(
        names(&res.tree, res.tree.find(Path::new(r"C:\docs")).unwrap()),
        ["a.txt", "big.bin", "last.bin"]
    );
}

#[test]
fn a_subfolder_can_be_the_root_and_names_match_case_insensitively() {
    let mut img = sample(&[(4, 6)]);
    let res = scan(&mut img, &["DOCS"]).unwrap();
    let t = &res.tree;
    assert_eq!(t.root_path(), Path::new(r"C:\DOCS"));
    assert_eq!(names(t, t.root()), ["a.txt", "big.bin"]);
    assert_eq!(t.node(t.root()).size, 10_100);
}

#[test]
fn a_missing_root_or_a_file_root_is_an_error() {
    let mut img = sample(&[(4, 6)]);
    assert!(matches!(
        scan(&mut img, &["nope"]),
        Err(ScanError::RootNotFound(_))
    ));
    assert!(matches!(
        scan(&mut img, &["top.dat"]),
        Err(ScanError::RootNotADirectory(_))
    ));
}

#[test]
fn ntfs_metadata_files_are_hidden() {
    let mut img = sample(&[(4, 6)]);
    let res = scan(&mut img, &[]).unwrap();
    let all = names(&res.tree, res.tree.root());
    assert!(
        all.iter().all(|n| !n.starts_with('$') && n != "."),
        "{all:?}"
    );
}

#[test]
fn dos_names_are_dropped_and_hard_links_appear_under_each_name() {
    let mut img = sample(&[(4, 6)]);
    img.put(
        21,
        &rec()
            .si(T0, 0)
            .attr(file_name(16, 1, "LONGNA~1.TXT", 2))
            .attr(file_name(16, 1, "long name.txt", 1))
            .attr(file_name(19, 1, "link.txt", 0))
            .data(3000, 4096),
    );
    let res = scan(&mut img, &[]).unwrap();
    let t = &res.tree;
    assert_eq!(
        names(t, t.find(Path::new(r"C:\docs")).unwrap()),
        ["a.txt", "big.bin", "long name.txt"]
    );
    assert_eq!(
        names(t, t.find(Path::new(r"C:\empty")).unwrap()),
        ["link.txt"]
    );
    // Every link counts in full, like the walk scanner.
    assert_eq!(node(t, r"empty\link.txt").allocated, 4096);
}

#[test]
fn unused_records_and_names_pointing_at_a_reused_folder_are_ignored() {
    let mut img = sample(&[(4, 6)]);
    img.put(21, &file(16, "deleted.txt").data(1, 4096).unused());
    img.put(
        22,
        &rec()
            .si(T0, 0)
            .attr(file_name(16, 9, "stale.txt", 1))
            .data(1, 4096),
    );
    let res = scan(&mut img, &[]).unwrap();
    assert_eq!(
        names(&res.tree, res.tree.find(Path::new(r"C:\docs")).unwrap()),
        ["a.txt", "big.bin"]
    );
}

#[test]
fn a_folder_with_another_sequence_number_still_owns_names_that_match_it() {
    let mut img = sample(&[(4, 6)]);
    img.put(16, &folder(ROOT_RECORD, "docs").seq(9));
    img.put(
        17,
        &rec()
            .si(T0, 0)
            .attr(file_name(16, 9, "a.txt", 1))
            .small(100),
    );
    let res = scan(&mut img, &[]).unwrap();
    assert_eq!(
        names(&res.tree, res.tree.find(Path::new(r"C:\docs")).unwrap()),
        ["a.txt"]
    );
}

#[test]
fn attributes_in_an_extension_record_belong_to_the_base_record() {
    let mut img = sample(&[(4, 6)]);
    img.put(21, &file(ROOT_RECORD, "frag.bin"));
    img.put(22, &rec().base(21).data(777_777, 790_528));
    let res = scan(&mut img, &[]).unwrap();
    let f = node(&res.tree, "frag.bin");
    assert_eq!((f.size, f.allocated), (777_777, 790_528));
    assert_eq!(res.tree.node(res.tree.root()).file_count, 4);
}

#[test]
fn junctions_and_symlinks_are_skipped_but_cloud_files_are_kept() {
    let mut img = sample(&[(4, 7)]);
    img.put(
        21,
        &folder(ROOT_RECORD, "junction").reparse(IO_REPARSE_TAG_MOUNT_POINT),
    );
    img.put(
        22,
        &file(ROOT_RECORD, "link.txt").reparse(IO_REPARSE_TAG_SYMLINK),
    );
    img.put(23, &folder(21, "inside"));
    img.put(
        24,
        &rec()
            .si(T0, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
            .name(ROOT_RECORD, "cloud.docx")
            .reparse(0x9000_601A)
            .attr(non_resident(NonRes {
                kind: 0x80,
                stream: "",
                lowest_vcn: 0,
                runs: runs(&[(None, 13)]),
                allocated: 53_248,
                size: 50_000,
                packed: Some((0x8000, 0)),
            })),
    );
    let res = scan(&mut img, &[]).unwrap();
    let t = &res.tree;
    assert_eq!(
        names(t, t.root()),
        ["cloud.docx", "docs", "empty", "top.dat"]
    );
    let cloud = node(t, "cloud.docx");
    assert_eq!((cloud.size, cloud.allocated), (50_000, 0));
    let mut skipped: Vec<_> = res
        .issues
        .iter()
        .filter(|i| i.kind == IssueKind::SkippedLink)
        .map(|i| i.path.clone())
        .collect();
    skipped.sort();
    assert_eq!(
        skipped,
        [PathBuf::from(r"C:\junction"), PathBuf::from(r"C:\link.txt")]
    );
}

#[test]
fn compressed_files_count_their_compressed_size_and_named_streams_are_ignored() {
    let mut img = sample(&[(4, 6)]);
    img.put(
        21,
        &file(ROOT_RECORD, "log.txt").attr(non_resident(NonRes {
            kind: 0x80,
            stream: "",
            lowest_vcn: 0,
            runs: runs(&[(Some(40), 10), (None, 15)]),
            allocated: 102_400,
            size: 100_000,
            packed: Some((0x0001, 40_960)),
        })),
    );
    img.put(
        22,
        &file(ROOT_RECORD, "tagged.jpg").small(10).attr(resident(
            0x80,
            "Zone.Identifier",
            b"[ZoneTransfer]",
        )),
    );
    let res = scan(&mut img, &[]).unwrap();
    assert_eq!(node(&res.tree, "log.txt").allocated, 40_960);
    assert_eq!(node(&res.tree, "tagged.jpg").size, 10);
}

#[test]
fn damaged_records_are_reported_and_the_rest_is_read() {
    let mut img = sample(&[(4, 6)]);
    img.put(21, &file(ROOT_RECORD, "torn.bin").data(1, 4096).torn());
    let res = scan(&mut img, &[]).unwrap();
    assert_eq!(
        names(&res.tree, res.tree.root()),
        ["docs", "empty", "top.dat"]
    );
    assert_eq!(res.issues.len(), 1);
    assert!(
        res.issues[0].message.starts_with("1 file table records"),
        "{}",
        res.issues[0].message
    );
}

#[test]
fn an_mft_split_over_extension_records_is_followed_through_its_attribute_list() {
    // The first extent (4 records) holds record 0 and the extension record 3; the rest of
    // the MFT is only described by record 3.
    let mut img = Image::new(&[(4, 1), (30, 5)], 64);
    let total = img.records();
    let mut list = Vec::new();
    for (vcn, record) in [(0u64, 0u64), (1, 3)] {
        let mut e = vec![0u8; 0x20];
        e[0..4].copy_from_slice(&0x80u32.to_le_bytes());
        e[4..6].copy_from_slice(&0x20u16.to_le_bytes());
        e[7] = 0x1A;
        e[8..16].copy_from_slice(&vcn.to_le_bytes());
        e[0x10..0x18].copy_from_slice(&record.to_le_bytes());
        list.extend(e);
    }
    let rec0 = file(ROOT_RECORD, "$MFT")
        .attr(resident(0x20, "", &list))
        .attr(non_resident(NonRes {
            kind: 0x80,
            stream: "",
            lowest_vcn: 0,
            runs: runs(&[(Some(4), 1)]),
            allocated: 6 * CS,
            size: total * RS as u64,
            packed: None,
        }));
    img.put(0, &rec0);
    img.put(
        3,
        &rec().base(0).attr(non_resident(NonRes {
            kind: 0x80,
            stream: "",
            lowest_vcn: 1,
            runs: runs(&[(Some(30), 5)]),
            allocated: 5 * CS,
            size: 0,
            packed: None,
        })),
    );
    img.put(ROOT_RECORD, &folder(ROOT_RECORD, "."));
    img.put(16, &folder(ROOT_RECORD, "far"));
    img.put(23, &file(16, "end.bin").data(9, 4096));
    let res = scan(&mut img, &[]).unwrap();
    assert_eq!(
        names(&res.tree, res.tree.find(Path::new(r"C:\far")).unwrap()),
        ["end.bin"]
    );
}

#[test]
fn other_file_systems_are_rejected() {
    let mut img = sample(&[(4, 6)]);
    img.bytes[3..11].copy_from_slice(b"EXFAT   ");
    assert!(matches!(scan(&mut img, &[]), Err(ScanError::BadVolume(_))));
}

#[test]
fn cancel_stops_the_scan() {
    let mut img = sample(&[(4, 6)]);
    let opts = ScanOptions {
        cancel: Some(Arc::new(AtomicBool::new(true))),
        ..ScanOptions::default()
    };
    let err = scan_volume(&mut img, &[], Path::new(ROOT), &opts, &NoProgress).err();
    assert!(matches!(err, Some(ScanError::Cancelled)));
}

#[test]
fn progress_ends_with_the_totals_of_the_scanned_folder() {
    struct Collect(Mutex<Vec<Progress>>);
    impl ProgressSink for Collect {
        fn report(&self, p: &Progress) {
            self.0.lock().unwrap().push(p.clone());
        }
    }
    let mut img = sample(&[(4, 6)]);
    let sink = Collect(Mutex::new(Vec::new()));
    scan_volume(
        &mut img,
        &[],
        Path::new(ROOT),
        &ScanOptions::default(),
        &sink,
    )
    .unwrap();
    let all = sink.0.lock().unwrap();
    let last = all.last().unwrap();
    assert_eq!((last.files, last.dirs, last.bytes), (3, 2, 20_480));
}

#[test]
fn the_mft_is_read_in_large_chunks_not_record_by_record() {
    let mut img = sample(&[(4, 6)]);
    scan(&mut img, &[]).unwrap();
    // Boot sector, record 0, then one read for the whole (small) MFT.
    assert_eq!(img.reads, 3);
}

/// Needs an elevated terminal: `cargo test -p devdisk-core mft -- --ignored`.
/// Scans a temp folder both ways and compares names and logical sizes.
#[cfg(windows)]
#[test]
#[ignore]
fn real_drive_matches_the_walk_scanner() {
    use crate::scanner::WalkScanner;
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    fs::create_dir_all(r.join("a/b")).unwrap();
    fs::write(r.join("a/b/big.bin"), vec![1u8; 300_000]).unwrap();
    fs::write(r.join("a/small.txt"), b"hello").unwrap();
    fs::write(r.join("top.dat"), vec![2u8; 70_000]).unwrap();
    let walk = WalkScanner
        .scan(r, &ScanOptions::default(), &NoProgress)
        .unwrap();
    let mft = MftScanner
        .scan(r, &ScanOptions::default(), &NoProgress)
        .unwrap();
    let list = |t: &Tree| {
        let mut v: Vec<(PathBuf, u64)> =
            (1..t.len()).map(|i| (t.path(i), t.node(i).size)).collect();
        v.sort();
        v
    };
    assert_eq!(list(&mft.tree), list(&walk.tree));
    assert_eq!(mft.tree.node(mft.tree.root()).size, 370_005);
}
