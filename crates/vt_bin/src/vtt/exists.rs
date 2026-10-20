//! Prints whether each path exists, without opening or statting it.
//!
//! Usage: `vtt exists <path>...`
//!
//! The check makes the call Node's `fs.existsSync` makes, `access` on Unix
//! and `GetFileAttributesW` on Windows, so tests can make the existence
//! probes tools like Vite make while looking for the workspace root.
//! `vtt stat-file` stats instead, which reads metadata such as the size.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    for path in args {
        if exists(path)? {
            println!("{path}: exists");
        } else {
            println!("{path}: missing");
        }
    }
    Ok(())
}

#[cfg(unix)]
fn exists(path: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let path = std::ffi::CString::new(path)?;
    // SAFETY: `path` is a valid NUL-terminated C string that outlives the call.
    Ok(unsafe { libc::access(path.as_ptr(), libc::F_OK) } == 0)
}

#[cfg(windows)]
fn exists(path: &str) -> Result<bool, Box<dyn std::error::Error>> {
    use std::os::windows::ffi::OsStrExt as _;

    use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES};

    let path: Vec<u16> = std::ffi::OsStr::new(path).encode_wide().chain(Some(0)).collect();
    // SAFETY: `path` is a valid NUL-terminated wide string that outlives the call.
    Ok(unsafe { GetFileAttributesW(path.as_ptr()) } != INVALID_FILE_ATTRIBUTES)
}
