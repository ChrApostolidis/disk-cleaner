//! Running a program with administrator rights: checking whether this process already has
//! them, and starting another process through the UAC prompt.

#[cfg(not(windows))]
use std::path::Path;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ElevateError {
    #[error("administrator permission was not given")]
    Declined,
    #[error("could not start the elevated process: {0}")]
    Failed(String),
}

/// Quotes one argument so Windows' command-line parser gives it back unchanged.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                // Backslashes before a quote are doubled, and the quote is escaped.
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    // Trailing backslashes would escape the closing quote.
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// A process started with administrator rights.
pub trait ElevatedChild: Send {
    /// Waits up to `timeout`; Some(exit code) once the process has ended.
    fn wait(&mut self, timeout: Duration) -> Option<i32>;
}

#[cfg(windows)]
pub use win::{is_elevated, spawn_elevated};

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    false
}

#[cfg(not(windows))]
pub fn spawn_elevated(
    _exe: &Path,
    _args: &[String],
) -> Result<Box<dyn ElevatedChild>, ElevateError> {
    Err(ElevateError::Failed("only supported on Windows".into()))
}

#[cfg(windows)]
mod win {
    use super::{ElevateError, ElevatedChild, quote_arg};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, GetLastError, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };

    const WAIT_OBJECT_0: u32 = 0;
    const SW_HIDE: i32 = 0;

    pub fn is_elevated() -> bool {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: the pseudo handle of the current process is always valid; `token` is an
        // out pointer.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        // SAFETY: the buffer is a TOKEN_ELEVATION of the size passed; `token` is open.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                (&mut elevation as *mut TOKEN_ELEVATION).cast(),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            )
        };
        // SAFETY: `token` was opened above and is closed once.
        unsafe { CloseHandle(token) };
        ok != 0 && elevation.TokenIsElevated != 0
    }

    struct Child(HANDLE);

    // SAFETY: a process handle can be waited on and closed from any thread.
    unsafe impl Send for Child {}

    impl ElevatedChild for Child {
        fn wait(&mut self, timeout: Duration) -> Option<i32> {
            let ms = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
            // SAFETY: the handle stays open until drop.
            if unsafe { WaitForSingleObject(self.0, ms) } != WAIT_OBJECT_0 {
                return None;
            }
            let mut code = 0u32;
            // SAFETY: as above; `code` is an out pointer.
            if unsafe { GetExitCodeProcess(self.0, &mut code) } == 0 {
                return Some(-1);
            }
            Some(code as i32)
        }
    }

    impl Drop for Child {
        fn drop(&mut self) {
            // SAFETY: closed exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain([0]).collect()
    }

    /// Starts `exe` through the UAC prompt. `Declined` when the user says no.
    pub fn spawn_elevated(
        exe: &Path,
        args: &[String],
    ) -> Result<Box<dyn ElevatedChild>, ElevateError> {
        let verb = wide("runas".as_ref());
        let file = wide(exe.as_os_str());
        let params: Vec<String> = args.iter().map(|a| quote_arg(a)).collect();
        let params = wide(params.join(" ").as_ref());
        // SAFETY: all-zero is valid for this plain C struct; the fields used are set below.
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_HIDE;
        // SAFETY: the strings outlive the call and `info` is fully initialised.
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            // SAFETY: reads the calling thread's last error.
            let err = unsafe { GetLastError() };
            return Err(if err == ERROR_CANCELLED {
                ElevateError::Declined
            } else {
                ElevateError::Failed(std::io::Error::from_raw_os_error(err as i32).to_string())
            });
        }
        if info.hProcess.is_null() {
            return Err(ElevateError::Failed("no process handle".into()));
        }
        Ok(Box::new(Child(info.hProcess)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_arguments_are_not_quoted() {
        assert_eq!(quote_arg("--flag"), "--flag");
        assert_eq!(quote_arg(r"C:\data"), r"C:\data");
    }

    #[test]
    fn spaces_quotes_and_trailing_backslashes_survive() {
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg(r"C:\My Files"), r#""C:\My Files""#);
        // A trailing backslash inside quotes must be doubled or it escapes the quote.
        assert_eq!(quote_arg(r"C:\My Files\"), r#""C:\My Files\\""#);
        assert_eq!(quote_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_arg(r#"a\"b c"#), r#""a\\\"b c""#);
    }

    #[cfg(windows)]
    #[test]
    fn is_elevated_answers_without_failing() {
        // Either answer is fine; this only checks the token query works.
        let _ = is_elevated();
    }
}
