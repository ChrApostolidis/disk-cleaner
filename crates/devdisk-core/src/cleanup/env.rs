//! The outside world the cleanup checks depend on (git, the Recycle Bin settings, the user
//! profile), behind a trait so tests can fake it.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecycleBinInfo {
    /// Items up to `max_bytes` can be recycled; bigger ones are deleted permanently by Windows.
    Enabled {
        max_bytes: u64,
    },
    /// "Don't move files to the Recycle Bin" is set for the drive.
    Disabled,
    Unknown,
}

pub trait Environment {
    /// Some(true/false) from `git`, None when git is missing or the folder is not in a repo.
    fn git_tracked(&self, path: &Path) -> Option<bool>;
    /// The Recycle Bin settings of the volume holding `path`.
    fn recycle_bin(&self, path: &Path) -> RecycleBinInfo;
    /// The folder that must never be removed, nor any folder above it.
    fn user_profile(&self) -> Option<PathBuf> {
        std::env::var_os("USERPROFILE")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }
}

pub struct SystemEnvironment;

impl Environment for SystemEnvironment {
    fn git_tracked(&self, path: &Path) -> Option<bool> {
        git_tracked(path)
    }

    fn recycle_bin(&self, path: &Path) -> RecycleBinInfo {
        recycle_bin(path)
    }
}

fn git_tracked(path: &Path) -> Option<bool> {
    use std::process::Command;
    let parent = path.parent()?;
    let name = path.file_name()?;
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(parent)
        .args(["ls-files", "-z", "--"])
        .arg(name);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashes when called from the GUI app.
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().ok()?;
    if out.status.success() {
        return Some(!out.stdout.is_empty());
    }
    // Outside any repository nothing can be tracked: a definite answer, not a failed check.
    // Other failures (e.g. git refusing a repo with a different owner) stay unknown.
    String::from_utf8_lossy(&out.stderr)
        .contains("not a git repository")
        .then_some(false)
}

/// Pure decision from the two registry values, split out so it is testable without a registry.
pub(crate) fn bin_info_from(nuke_on_delete: Option<u32>, max_mb: Option<u32>) -> RecycleBinInfo {
    if nuke_on_delete == Some(1) {
        return RecycleBinInfo::Disabled;
    }
    match max_mb {
        Some(mb) => RecycleBinInfo::Enabled {
            max_bytes: u64::from(mb) * 1024 * 1024,
        },
        None => RecycleBinInfo::Unknown,
    }
}

/// `\?\Volume{guid}\` -> `{guid}`.
pub(crate) fn volume_guid(volume_name: &str) -> Option<&str> {
    let start = volume_name.find('{')?;
    let end = volume_name[start..].find('}')? + start;
    Some(&volume_name[start..=end])
}

#[cfg(not(windows))]
fn recycle_bin(_path: &Path) -> RecycleBinInfo {
    RecycleBinInfo::Unknown
}

#[cfg(windows)]
fn recycle_bin(path: &Path) -> RecycleBinInfo {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }
    fn to_string(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    let mut mount = [0u16; 1024];
    let mut name = [0u16; 128];
    // SAFETY: both buffers outlive the calls and the lengths passed match their sizes; the
    // input strings are NUL-terminated.
    let guid = unsafe {
        if GetVolumePathNameW(wide(path.as_os_str()).as_ptr(), mount.as_mut_ptr(), 1024) == 0 {
            return RecycleBinInfo::Unknown;
        }
        if GetVolumeNameForVolumeMountPointW(mount.as_ptr(), name.as_mut_ptr(), 128) == 0 {
            return RecycleBinInfo::Unknown;
        }
        to_string(&name)
    };
    let Some(guid) = volume_guid(&guid) else {
        return RecycleBinInfo::Unknown;
    };
    let key =
        format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{guid}");
    bin_info_from(
        read_dword(&key, "NukeOnDelete"),
        read_dword(&key, "MaxCapacity"),
    )
}

#[cfg(windows)]
fn read_dword(subkey: &str, value: &str) -> Option<u32> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let (subkey, value) = (wide(subkey), wide(value));
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: `data`/`size` describe a valid 4-byte buffer; strings are NUL-terminated.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    (status == ERROR_SUCCESS).then_some(data)
}
