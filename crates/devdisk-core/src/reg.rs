//! Small read-only wrapper over the Windows registry API.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, KEY_READ, REG_SAM_FLAGS, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW,
    RegGetValueW, RegOpenKeyExW,
};

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// An open registry key, closed on drop.
pub(crate) struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful RegOpenKeyExW and is closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

impl Key {
    pub(crate) fn open(parent: HKEY, path: &str, view: REG_SAM_FLAGS) -> Option<Key> {
        let path = wide(path);
        let mut out: HKEY = null_mut();
        // SAFETY: `path` is NUL-terminated and `out` is a valid out pointer.
        let status = unsafe { RegOpenKeyExW(parent, path.as_ptr(), 0, KEY_READ | view, &mut out) };
        (status == ERROR_SUCCESS).then_some(Key(out))
    }

    pub(crate) fn sub(&self, name: &str) -> Option<Key> {
        Key::open(self.0, name, 0)
    }

    /// Names of the direct subkeys. Stops at the first error (normally ERROR_NO_MORE_ITEMS).
    pub(crate) fn subkeys(&self) -> Vec<String> {
        let mut out = Vec::new();
        // Key names are at most 255 characters.
        let mut buf = [0u16; 256];
        for index in 0.. {
            let mut len = buf.len() as u32;
            // SAFETY: `buf` is valid for `len` u16s; the optional out pointers are null.
            let status = unsafe {
                RegEnumKeyExW(
                    self.0,
                    index,
                    buf.as_mut_ptr(),
                    &mut len,
                    null(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                )
            };
            if status != ERROR_SUCCESS {
                break;
            }
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
        out
    }

    /// A string value; REG_EXPAND_SZ values come back expanded.
    pub(crate) fn string(&self, name: &str) -> Option<String> {
        let name = wide(name);
        let mut bytes = 0u32;
        // SAFETY: a null data pointer asks only for the size.
        let status = unsafe {
            RegGetValueW(
                self.0,
                null(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                null_mut(),
                &mut bytes,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        // Expansion can need more room than the size query reported, so retry a few times.
        for _ in 0..3 {
            let mut buf = vec![0u16; (bytes as usize).div_ceil(2) + 1];
            let mut size = (buf.len() * 2) as u32;
            // SAFETY: `buf` is valid for `size` bytes.
            let status = unsafe {
                RegGetValueW(
                    self.0,
                    null(),
                    name.as_ptr(),
                    RRF_RT_REG_SZ,
                    null_mut(),
                    buf.as_mut_ptr().cast(),
                    &mut size,
                )
            };
            match status {
                ERROR_SUCCESS => {
                    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                    return Some(String::from_utf16_lossy(&buf[..len]));
                }
                ERROR_MORE_DATA => bytes = size.max(bytes.saturating_mul(2)),
                _ => return None,
            }
        }
        None
    }

    pub(crate) fn dword(&self, name: &str) -> Option<u32> {
        let name = wide(name);
        let mut data = 0u32;
        let mut size = size_of::<u32>() as u32;
        // SAFETY: `data`/`size` describe a valid 4-byte buffer.
        let status = unsafe {
            RegGetValueW(
                self.0,
                null(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                null_mut(),
                (&mut data as *mut u32).cast(),
                &mut size,
            )
        };
        (status == ERROR_SUCCESS).then_some(data)
    }
}
