//! No-malloc `PATH` lookup for `execvp`, `execlp`, and `posix_spawnp`.

use allocator_api2::{alloc::Allocator, vec::Vec};
use bstr::BStr;
use fspy_nostd::{
    CStr, CWD, Error, Result, Thin,
    fs::{AccessFlags, faccessat},
};
use fspy_nostd_alloc::fs::resolve_path_at;

const NAME_MAX: usize = 255;
const DEFAULT_PATH: &[u8] = b"/usr/local/bin:/bin:/usr/bin";

fn getenv_path() -> &'static [u8] {
    // SAFETY: `c"PATH"` is a static NUL-terminated C string.
    let value = unsafe { ::libc::getenv(c"PATH".as_ptr()) };
    if value.is_null() {
        DEFAULT_PATH
    } else {
        // SAFETY: `getenv` returns a pointer to a NUL-terminated env value.
        unsafe { CStr::<Thin>::from_ptr(value.cast()) }.count().as_units()
    }
}

/// Resolves `file` against `PATH` using `allocator` (no libc malloc).
///
/// Calls `on_path_read` with the absolute path of every candidate checked.
///
/// # Errors
///
/// Returns `ENOENT`, `EACCES`, `ENOTDIR`, or `ENAMETOOLONG`, matching POSIX
/// `execvp` / musl `execvp`.
pub fn resolve_in_path<A: Allocator + Clone>(
    file: &[u8],
    cwd: Option<&[u8]>,
    allocator: A,
    mut on_path_read: impl FnMut(&BStr),
) -> Result<Vec<u8, A>> {
    if file.is_empty() {
        return Err(Error::NOENT);
    }
    if file.contains(&b'/') {
        let mut out = Vec::with_capacity_in(file.len(), allocator);
        out.extend_from_slice(file);
        return Ok(out);
    }
    if file.len() > NAME_MAX {
        return Err(Error::NAMETOOLONG);
    }

    let path_env = getenv_path();
    let mut seen_eacces = false;
    let mut last_err = Error::NOENT;
    let mut candidate = Vec::new_in(allocator.clone());

    for dir in path_env.split(|&ch| ch == b':') {
        candidate.clear();
        if let Some(base) = cwd.filter(|_| !dir.starts_with(b"/")) {
            candidate.extend_from_slice(base);
            if candidate.last() != Some(&b'/') {
                candidate.push(b'/');
            }
        }
        candidate.extend_from_slice(dir);
        if !dir.is_empty() && candidate.last() != Some(&b'/') {
            candidate.push(b'/');
        }
        candidate.extend_from_slice(file);
        candidate.push(0);

        let c_candidate = CStr::from_units_with_nul(&candidate).ok_or(Error::NOENT)?;
        if let Ok(Some(abs)) = resolve_path_at(&allocator, Some(CWD), c_candidate.as_thin()) {
            on_path_read(BStr::new(abs.as_units()));
        }

        match faccessat(CWD, c_candidate.as_thin(), AccessFlags::X_OK) {
            Ok(()) => {
                let mut st = core::mem::MaybeUninit::<::libc::stat>::zeroed();
                // SAFETY: `c_candidate` is NUL-terminated and `st` is writable storage.
                if unsafe { ::libc::stat(c_candidate.as_ptr().cast(), st.as_mut_ptr()) } == 0 {
                    // SAFETY: `stat` succeeded and initialized `st`.
                    let st = unsafe { st.assume_init() };
                    if (st.st_mode & ::libc::S_IFMT) == ::libc::S_IFREG {
                        candidate.pop(); // strip trailing NUL
                        return Ok(candidate);
                    }
                    seen_eacces = true;
                    last_err = Error::ACCES;
                } else {
                    match Error::last_os_error() {
                        err @ (Error::ACCES | Error::NOENT | Error::NOTDIR) => {
                            if err == Error::ACCES {
                                seen_eacces = true;
                            }
                            last_err = err;
                        }
                        other => return Err(other),
                    }
                }
            }
            Err(err @ (Error::ACCES | Error::NOENT | Error::NOTDIR)) => {
                if err == Error::ACCES {
                    seen_eacces = true;
                }
                last_err = err;
            }
            Err(other) => return Err(other),
        }
    }

    Err(if seen_eacces { Error::ACCES } else { last_err })
}
