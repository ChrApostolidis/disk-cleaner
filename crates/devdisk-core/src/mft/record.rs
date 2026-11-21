//! NTFS on-disk structures: the boot sector, FILE records (with their update-sequence
//! fixups), the attributes the scanner needs, and data runs. Pure functions over byte slices.

/// What the scanner needs from the boot sector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Boot {
    pub cluster_size: u64,
    pub mft_lcn: u64,
    pub record_size: usize,
}

/// Update sequence fixups protect every 512-byte block of a record, whatever the sector size.
const FIXUP_STRIDE: usize = 512;

pub(crate) const ATTR_STANDARD_INFORMATION: u32 = 0x10;
pub(crate) const ATTR_ATTRIBUTE_LIST: u32 = 0x20;
pub(crate) const ATTR_FILE_NAME: u32 = 0x30;
pub(crate) const ATTR_DATA: u32 = 0x80;
pub(crate) const ATTR_REPARSE_POINT: u32 = 0xC0;
const ATTR_END: u32 = 0xFFFF_FFFF;

const FLAG_IN_USE: u16 = 0x01;
const FLAG_DIRECTORY: u16 = 0x02;

const ATTR_FLAG_COMPRESSED: u16 = 0x0001;
const ATTR_FLAG_SPARSE: u16 = 0x8000;

/// DOS-only 8.3 names duplicate a long name in the same folder.
const NAMESPACE_DOS: u8 = 2;

/// Record numbers are the low 48 bits of a file reference; the top 16 are the sequence number.
pub(crate) fn split_ref(r: u64) -> (u64, u16) {
    (r & 0x0000_FFFF_FFFF_FFFF, (r >> 48) as u16)
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

pub(crate) fn parse_boot(b: &[u8]) -> Result<Boot, String> {
    if b.get(3..11) != Some(b"NTFS    ") {
        return Err("not an NTFS boot sector".into());
    }
    let bytes_per_sector = u64::from(u16_at(b, 0x0B).unwrap_or(0));
    let spc_raw = b[0x0D];
    // Values above 0x80 are negative powers of two, used for clusters of 64 KB and more.
    let sectors_per_cluster: u64 = if spc_raw > 0x80 {
        1u64.checked_shl(256 - u32::from(spc_raw)).unwrap_or(0)
    } else {
        u64::from(spc_raw)
    };
    let cluster_size = bytes_per_sector * sectors_per_cluster;
    let mft_lcn = u64_at(b, 0x30).unwrap_or(0);
    let per_record = b[0x40] as i8;
    let record_size: u64 = if per_record < 0 {
        1u64.checked_shl(u32::from(per_record.unsigned_abs()))
            .unwrap_or(0)
    } else {
        cluster_size * per_record as u64
    };
    if !bytes_per_sector.is_power_of_two()
        || bytes_per_sector < 256
        || !cluster_size.is_power_of_two()
        || !record_size.is_power_of_two()
        || !(512..=65536).contains(&record_size)
        || mft_lcn == 0
    {
        return Err("boot sector has impossible values".into());
    }
    Ok(Boot {
        cluster_size,
        mft_lcn,
        record_size: record_size as usize,
    })
}

/// Checks and undoes the update sequence: the last two bytes of each 512-byte block were
/// replaced by the sequence number on write. False for a torn or damaged record.
pub(crate) fn apply_fixups(rec: &mut [u8]) -> bool {
    let (Some(off), Some(count)) = (u16_at(rec, 4), u16_at(rec, 6)) else {
        return false;
    };
    let (off, count) = (off as usize, count as usize);
    if count < 2 || (count - 1) * FIXUP_STRIDE > rec.len() || off + count * 2 > rec.len() {
        return false;
    }
    let usn = [rec[off], rec[off + 1]];
    for i in 1..count {
        let end = i * FIXUP_STRIDE;
        if rec[end - 2..end] != usn {
            return false;
        }
        let saved = [rec[off + i * 2], rec[off + i * 2 + 1]];
        rec[end - 2..end].copy_from_slice(&saved);
    }
    true
}

/// One contiguous piece of a non-resident attribute. `lcn` is None for a sparse hole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Run {
    pub lcn: Option<u64>,
    pub clusters: u64,
}

/// Decodes a mapping-pairs array (data runs).
pub(crate) fn decode_runs(b: &[u8]) -> Result<Vec<Run>, String> {
    let mut runs = Vec::new();
    let mut pos = 0;
    let mut lcn: i64 = 0;
    while let Some(&head) = b.get(pos) {
        if head == 0 {
            return Ok(runs);
        }
        let len_size = (head & 0x0F) as usize;
        let off_size = (head >> 4) as usize;
        pos += 1;
        if len_size == 0 || len_size > 8 || off_size > 8 || pos + len_size + off_size > b.len() {
            return Err("bad data run".into());
        }
        let mut len = [0u8; 8];
        len[..len_size].copy_from_slice(&b[pos..pos + len_size]);
        let clusters = u64::from_le_bytes(len);
        pos += len_size;
        let run_lcn = if off_size == 0 {
            None
        } else {
            // Sign-extend the relative offset from its top byte.
            let fill = if b[pos + off_size - 1] & 0x80 != 0 {
                0xFF
            } else {
                0
            };
            let mut off = [fill; 8];
            off[..off_size].copy_from_slice(&b[pos..pos + off_size]);
            lcn = lcn
                .checked_add(i64::from_le_bytes(off))
                .ok_or("bad data run")?;
            if lcn < 0 {
                return Err("data run before the start of the volume".into());
            }
            Some(lcn as u64)
        };
        pos += off_size;
        runs.push(Run {
            lcn: run_lcn,
            clusters,
        });
    }
    Err("data runs are not terminated".into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileName {
    pub parent: u64,
    pub parent_seq: u16,
    pub name: String,
}

/// Sizes of the unnamed data stream, from its first extent (only that one carries them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct DataSize {
    pub size: u64,
    pub allocated: u64,
    pub resident: bool,
}

/// One extent of the unnamed data stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DataExtent {
    pub lowest_vcn: u64,
    pub runs: Vec<Run>,
}

/// An `$ATTRIBUTE_LIST` that names where the other parts of a record's attributes live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttrList {
    Resident(Vec<u8>),
    NonResident { runs: Vec<Run>, size: u64 },
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Record {
    pub in_use: bool,
    pub is_dir: bool,
    pub seq: u16,
    /// Record number of the base record; 0 for a base record itself.
    pub base: u64,
    /// Every name except DOS-only 8.3 names. More than one means hard links.
    pub names: Vec<FileName>,
    /// FILETIME of the last content change, from `$STANDARD_INFORMATION`.
    pub modified: Option<u64>,
    pub attributes: Option<u32>,
    pub data: Option<DataSize>,
    pub data_extents: Vec<DataExtent>,
    pub reparse_tag: Option<u32>,
    pub attr_list: Option<AttrList>,
}

fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&units)
}

/// Parses a FILE record whose fixups were already applied. None if it is not a FILE record
/// or its layout is broken.
pub(crate) fn parse_record(rec: &[u8]) -> Option<Record> {
    if rec.get(0..4)? != b"FILE" {
        return None;
    }
    let flags = u16_at(rec, 0x16)?;
    let mut out = Record {
        in_use: flags & FLAG_IN_USE != 0,
        is_dir: flags & FLAG_DIRECTORY != 0,
        seq: u16_at(rec, 0x10)?,
        base: split_ref(u64_at(rec, 0x20)?).0,
        ..Record::default()
    };
    let used = (u32_at(rec, 0x18)? as usize).min(rec.len());
    let mut pos = u16_at(rec, 0x14)? as usize;
    while pos + 8 <= used {
        let kind = u32_at(rec, pos)?;
        if kind == ATTR_END {
            break;
        }
        let len = u32_at(rec, pos + 4)? as usize;
        if len < 0x18 || pos + len > used {
            return None;
        }
        let a = &rec[pos..pos + len];
        pos += len;
        let non_resident = a[8] != 0;
        let named = a[9] != 0;
        let aflags = u16_at(a, 0x0C)?;
        let value = if non_resident {
            None
        } else {
            let vlen = u32_at(a, 0x10)? as usize;
            let voff = u16_at(a, 0x14)? as usize;
            Some(a.get(voff..voff.checked_add(vlen)?)?)
        };
        match (kind, value) {
            (ATTR_STANDARD_INFORMATION, Some(v)) => {
                out.modified = u64_at(v, 0x08).filter(|&t| t != 0);
                out.attributes = u32_at(v, 0x20);
            }
            (ATTR_FILE_NAME, Some(v)) => {
                let (parent, parent_seq) = split_ref(u64_at(v, 0)?);
                let chars = *v.get(0x40)? as usize;
                let namespace = *v.get(0x41)?;
                if namespace != NAMESPACE_DOS {
                    out.names.push(FileName {
                        parent,
                        parent_seq,
                        name: utf16(v.get(0x42..0x42 + chars * 2)?),
                    });
                }
            }
            (ATTR_REPARSE_POINT, Some(v)) => out.reparse_tag = u32_at(v, 0),
            // A non-resident reparse point is too big to be a symlink or junction.
            (ATTR_REPARSE_POINT, None) => {}
            (ATTR_ATTRIBUTE_LIST, Some(v)) => out.attr_list = Some(AttrList::Resident(v.to_vec())),
            (ATTR_ATTRIBUTE_LIST, None) => {
                let runs_off = u16_at(a, 0x20)? as usize;
                out.attr_list = Some(AttrList::NonResident {
                    runs: decode_runs(a.get(runs_off..)?).ok()?,
                    size: u64_at(a, 0x30)?,
                });
            }
            // Named data streams (alternate data streams) are not part of the file's size.
            (ATTR_DATA, _) if named => {}
            (ATTR_DATA, Some(v)) => {
                out.data = Some(DataSize {
                    size: v.len() as u64,
                    allocated: 0,
                    resident: true,
                });
            }
            (ATTR_DATA, None) => {
                let lowest_vcn = u64_at(a, 0x10)?;
                let runs_off = u16_at(a, 0x20)? as usize;
                if lowest_vcn == 0 {
                    let packed = aflags & (ATTR_FLAG_COMPRESSED | ATTR_FLAG_SPARSE) != 0;
                    let allocated = if packed {
                        u64_at(a, 0x40)?
                    } else {
                        u64_at(a, 0x28)?
                    };
                    out.data = Some(DataSize {
                        size: u64_at(a, 0x30)?,
                        allocated,
                        resident: false,
                    });
                }
                if let Ok(runs) = decode_runs(a.get(runs_off..)?) {
                    out.data_extents.push(DataExtent { lowest_vcn, runs });
                }
            }
            _ => {}
        }
    }
    Some(out)
}

/// Entries of an attribute list that point at unnamed `$DATA` extents in other records:
/// (lowest VCN, record number).
pub(crate) fn data_extent_records(list: &[u8]) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 0x1A <= list.len() {
        let (Some(kind), Some(len)) = (u32_at(list, pos), u16_at(list, pos + 4)) else {
            break;
        };
        let len = len as usize;
        if len < 0x1A || pos + len > list.len() {
            break;
        }
        let name_len = list[pos + 6];
        if kind == ATTR_DATA && name_len == 0 {
            let vcn = u64_at(list, pos + 8).unwrap_or(0);
            let (record, _) = split_ref(u64_at(list, pos + 0x10).unwrap_or(0));
            out.push((vcn, record));
        }
        pos += len;
    }
    out
}
