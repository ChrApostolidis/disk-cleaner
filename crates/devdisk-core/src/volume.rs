//! Total and free space of the volume that holds a path.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeInfo {
    pub mount: PathBuf,
    pub total: u64,
    pub free: u64,
}

/// Total and free bytes of the volume holding `path`. Free counts the whole volume,
/// not the caller's quota, so it matches what Explorer shows.
#[cfg(windows)]
pub fn volume_info(path: &Path) -> std::io::Result<VolumeInfo> {
    use std::ffi::OsString;
    use std::io;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetVolumePathNameW};

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // Volume paths can be long (\\?\Volume{guid}\), so don't rely on MAX_PATH.
    let mut buf = vec![0u16; 1024];
    // SAFETY: `wide` is NUL-terminated and `buf` is valid for `buf.len()` u16s.
    if unsafe { GetVolumePathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    buf.truncate(len);
    let mount = PathBuf::from(OsString::from_wide(&buf));

    let mut mount_z = buf;
    mount_z.push(0);
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: `mount_z` is NUL-terminated and the out pointers are valid u64s.
    if unsafe { GetDiskFreeSpaceExW(mount_z.as_ptr(), &mut avail, &mut total, &mut free) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(VolumeInfo { mount, total, free })
}

#[cfg(not(windows))]
pub fn volume_info(_path: &Path) -> std::io::Result<VolumeInfo> {
    Err(std::io::ErrorKind::Unsupported.into())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn reports_the_volume_of_a_tempdir() {
        let dir = tempfile::tempdir().unwrap();
        let v = volume_info(dir.path()).unwrap();
        assert!(v.total > 0);
        assert!(v.free <= v.total);
        assert!(v.mount.to_string_lossy().ends_with('\\'), "{:?}", v.mount);
    }
}
