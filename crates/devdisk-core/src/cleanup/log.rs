//! The cleanup log (SQLite) and restore ("Put back") from the Recycle Bin.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::CleanupError;

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: i64,
    pub batch_id: String,
    /// Unix milliseconds.
    pub deleted_at: i64,
    pub path: PathBuf,
    pub allocated: u64,
    pub project_path: Option<PathBuf>,
    pub project_label: Option<String>,
    pub restored_at: Option<i64>,
}

pub struct CleanupLog {
    conn: Connection,
}

/// A new log row; the id is assigned by the database.
pub(crate) struct NewEntry<'a> {
    pub batch_id: &'a str,
    pub deleted_at: i64,
    pub path: &'a Path,
    pub allocated: u64,
    pub project_path: Option<&'a Path>,
    pub project_label: Option<&'a str>,
}

const COLUMNS: &str =
    "id, batch_id, deleted_at, path, allocated, project_path, project_label, restored_at";

fn row_to_entry(r: &rusqlite::Row) -> rusqlite::Result<HistoryEntry> {
    Ok(HistoryEntry {
        id: r.get(0)?,
        batch_id: r.get(1)?,
        deleted_at: r.get(2)?,
        path: PathBuf::from(r.get::<_, String>(3)?),
        allocated: r.get::<_, i64>(4)? as u64,
        project_path: r.get::<_, Option<String>>(5)?.map(PathBuf::from),
        project_label: r.get(6)?,
        restored_at: r.get(7)?,
    })
}

impl CleanupLog {
    /// Opens (creating if needed) the log database.
    pub fn open(db_path: &Path) -> Result<Self, CleanupError> {
        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cleanup_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                batch_id TEXT NOT NULL,
                deleted_at INTEGER NOT NULL,
                path TEXT NOT NULL,
                allocated INTEGER NOT NULL,
                project_path TEXT,
                project_label TEXT,
                restored_at INTEGER
            );",
        )?;
        Ok(Self { conn })
    }

    /// Newest first.
    pub fn list(&self) -> Result<Vec<HistoryEntry>, CleanupError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM cleanup_log ORDER BY deleted_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map([], row_to_entry)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub(crate) fn insert(&self, e: &NewEntry) -> Result<i64, CleanupError> {
        self.conn.execute(
            "INSERT INTO cleanup_log (batch_id, deleted_at, path, allocated, project_path, project_label)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                e.batch_id,
                e.deleted_at,
                e.path.to_string_lossy(),
                e.allocated as i64,
                e.project_path.map(|p| p.to_string_lossy().into_owned()),
                e.project_label,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub(crate) fn get(&self, id: i64) -> Result<Option<HistoryEntry>, CleanupError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM cleanup_log WHERE id = ?1"),
                [id],
                row_to_entry,
            )
            .optional()?)
    }

    fn mark_restored(&self, id: i64, at: i64) -> Result<(), CleanupError> {
        self.conn.execute(
            "UPDATE cleanup_log SET restored_at = ?2 WHERE id = ?1",
            params![id, at],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinError {
    /// Something already exists at the original path.
    Collision,
    Other(String),
}

/// The Recycle Bin, behind a trait so tests never touch the real one.
pub trait BinAccess {
    fn list(&self) -> Result<Vec<trash::TrashItem>, String>;
    fn restore(&self, item: trash::TrashItem) -> Result<(), BinError>;
}

pub struct SystemBin;

impl BinAccess for SystemBin {
    fn list(&self) -> Result<Vec<trash::TrashItem>, String> {
        trash::os_limited::list().map_err(|e| e.to_string())
    }

    fn restore(&self, item: trash::TrashItem) -> Result<(), BinError> {
        match trash::os_limited::restore_all([item]) {
            Ok(()) => Ok(()),
            Err(trash::Error::RestoreCollision { .. }) => Err(BinError::Collision),
            Err(e) => Err(BinError::Other(e.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreOutcome {
    pub id: i64,
    pub ok: bool,
    pub error: Option<String>,
}

/// Windows can spell one folder two ways (8.3 `APOSTO~1` vs `Apostolidis`), and the Recycle Bin
/// records the long form, so logged paths must use it too or "Put back" cannot find them.
pub(crate) fn long_form(path: &Path) -> PathBuf {
    if let Some(p) = expand_long(path) {
        return p;
    }
    // A recycled item no longer exists, so only its folder can be expanded.
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => expand_long(parent)
            .map(|p| p.join(name))
            .unwrap_or_else(|| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

#[cfg(windows)]
fn expand_long(path: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut buf = vec![0u16; 512];
    loop {
        // SAFETY: `wide` is NUL-terminated; `buf` is valid for `buf.len()` u16s.
        let n =
            unsafe { GetLongPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            buf.truncate(n);
            return Some(PathBuf::from(std::ffi::OsString::from_wide(&buf)));
        }
        // Too small: `n` is the size needed, including the NUL.
        buf.resize(n, 0);
    }
}

#[cfg(not(windows))]
fn expand_long(path: &Path) -> Option<PathBuf> {
    path.exists().then(|| path.to_path_buf())
}

/// Same path in the sense Windows uses: ASCII case-insensitive, either slash.
fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_start_matches(r"\\?\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    norm(a) == norm(b)
}

/// Puts entries back. Finds the trash item whose original path matches and whose deletion
/// time is closest to the logged one (the same path may have been recycled more than once).
pub fn restore(log: &CleanupLog, ids: &[i64], bin: &dyn BinAccess) -> Vec<RestoreOutcome> {
    let mut listing: Option<Result<Vec<trash::TrashItem>, String>> = None;
    ids.iter()
        .map(|&id| {
            let result = restore_one(log, id, bin, &mut listing);
            RestoreOutcome {
                id,
                ok: result.is_ok(),
                error: result.err(),
            }
        })
        .collect()
}

fn restore_one(
    log: &CleanupLog,
    id: i64,
    bin: &dyn BinAccess,
    listing: &mut Option<Result<Vec<trash::TrashItem>, String>>,
) -> Result<(), String> {
    let entry = log
        .get(id)
        .map_err(|e| e.to_string())?
        .ok_or("unknown entry")?;
    if entry.restored_at.is_some() {
        return Err("already put back".into());
    }
    // Listed lazily and once: listing a big bin is slow, and a batch restore shares it.
    let items = listing.get_or_insert_with(|| bin.list());
    let items = items.as_ref().map_err(|e| e.clone())?;
    // Entries are logged in long form, but expand again in case the log came from elsewhere.
    let wanted = long_form(&entry.path);
    let item = items
        .iter()
        .filter(|i| {
            let original = i.original_path();
            same_path(&original, &wanted) || same_path(&original, &entry.path)
        })
        .min_by_key(|i| (i.time_deleted.saturating_mul(1000) - entry.deleted_at).abs())
        .cloned()
        .ok_or("no longer in the Recycle Bin")?;
    let item_id = item.id.clone();
    match bin.restore(item) {
        Ok(()) => {}
        Err(BinError::Collision) => return Err("something already exists at that path".into()),
        Err(BinError::Other(e)) => return Err(e),
    }
    // The file is back already; a failed bookkeeping write must not report the restore as failed.
    let _ = log.mark_restored(id, now_ms());
    // The bin no longer holds it, so a later id for the same path must not pick it again.
    if let Some(Ok(v)) = listing {
        v.retain(|i| i.id != item_id);
    }
    Ok(())
}
