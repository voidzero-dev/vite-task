//! Allocating filesystem wrappers.

#[cfg(target_os = "macos")]
use allocator_api2::boxed::Box;
use allocator_api2::{alloc::Allocator, vec::Vec};
use fspy_nostd::{BorrowedFd, CStr, CWD, Error, Fat, Result, Thin};

use crate::CString;

/// Returns the absolute pathname of the current working directory, allocated
/// with `allocator`.
///
/// # Errors
///
/// Returns [`Error::NOMEM`] if storage cannot be allocated, or the error from
/// [`fspy_nostd::fs::getcwd`].
pub fn getcwd<A: Allocator>(allocator: A) -> Result<CString<Fat, A>> {
    let mut bytes = Vec::new_in(allocator);
    bytes.try_reserve_exact(fspy_nostd::fs::PATH_MAX).map_err(|_| Error::NOMEM)?;
    let initialized =
        fspy_nostd::fs::getcwd(&mut bytes.spare_capacity_mut()[..fspy_nostd::fs::PATH_MAX])?
            .len_with_nul();

    // SAFETY: `getcwd` initialized this prefix.
    unsafe { bytes.set_len(initialized) };

    // SAFETY: `getcwd` returned one C string including its terminating NUL.
    Ok(unsafe { CString::from_vec_with_nul_unchecked(bytes) })
}

/// Returns the target of `path` relative to `dirfd`, allocated with
/// `allocator`.
///
/// The buffer grows and retries when `readlinkat` fills it, because the
/// underlying operation does not otherwise distinguish an exact fit from a
/// truncated result.
///
/// # Errors
///
/// Returns [`Error::NOMEM`] if storage cannot be allocated, or the error from
/// [`fspy_nostd::fs::readlinkat`].
#[cfg(any(target_os = "linux", target_os = "none"))]
pub fn readlinkat<A: Allocator>(
    allocator: A,
    dirfd: BorrowedFd<'_>,
    path: CStr<'_, Thin>,
) -> Result<Vec<u8, A>> {
    let mut bytes = Vec::new_in(allocator);
    bytes.try_reserve_exact(fspy_nostd::fs::PATH_MAX).map_err(|_| Error::NOMEM)?;

    loop {
        let capacity = bytes.capacity();
        let initialized =
            fspy_nostd::fs::readlinkat(dirfd, path, bytes.spare_capacity_mut())?.len();
        if initialized < capacity {
            // SAFETY: `readlinkat` initialized this prefix.
            unsafe { bytes.set_len(initialized) };
            return Ok(bytes);
        }

        // The length remains zero, so reserve the desired total capacity.
        let next_capacity = capacity.checked_mul(2).ok_or(Error::NOMEM)?;
        bytes.try_reserve_exact(next_capacity).map_err(|_| Error::NOMEM)?;
    }
}

/// Returns the path associated with `fd`, allocated with `allocator`.
///
/// The returned C string remains thin because `F_GETPATH` reports no length.
///
/// # Errors
///
/// Returns [`Error::NOMEM`] if storage cannot be allocated, or the error from
/// [`fspy_nostd::fs::fcntl_getpath`].
#[cfg(target_os = "macos")]
pub fn fcntl_getpath<A: Allocator>(
    allocator: A,
    fd: fspy_nostd::BorrowedFd<'_>,
) -> Result<CString<Thin, A>> {
    let mut bytes = path_buffer(allocator)?;
    fspy_nostd::fs::fcntl_getpath(fd, &mut bytes)?;

    // SAFETY: `fcntl_getpath` initialized a NUL-terminated string at the start
    // of `bytes`.
    Ok(unsafe { CString::from_boxed_with_nul_unchecked(Box::slice(bytes)) })
}

#[cfg(target_os = "macos")]
fn path_buffer<A: Allocator>(
    allocator: A,
) -> Result<Box<[core::mem::MaybeUninit<u8>; fspy_nostd::fs::PATH_MAX], A>> {
    let bytes =
        Box::<[core::mem::MaybeUninit<u8>; fspy_nostd::fs::PATH_MAX], A>::try_new_uninit_in(
            allocator,
        )
        .map_err(|_| Error::NOMEM)?;

    // SAFETY: an array of `MaybeUninit<u8>` requires no initialization.
    Ok(unsafe { bytes.assume_init() })
}

/// Resolves the pathname associated with an open file descriptor (or [`CWD`]),
/// allocated with `allocator`.
///
/// Returns `Ok(None)` when the descriptor is invalid or has no resolvable path
/// (`EBADF` / `ENOENT`).
///
/// # Errors
///
/// Returns [`Error::NOMEM`] if storage cannot be allocated, or any other OS
/// error reported while resolving the descriptor.
pub fn fd_path<A: Allocator>(allocator: A, fd: BorrowedFd<'_>) -> Result<Option<CString<Fat, A>>> {
    if fd.as_raw_fd() == CWD.as_raw_fd() {
        return getcwd(allocator).map(Some);
    }

    #[cfg(any(target_os = "linux", target_os = "none"))]
    {
        let Ok(raw_fd) = u32::try_from(fd.as_raw_fd()) else {
            return Ok(None);
        };
        let mut proc_buf = [0u8; 32];
        let proc_path = format_proc_self_fd(&mut proc_buf, raw_fd);
        match readlinkat(allocator, CWD, proc_path) {
            Ok(mut bytes) => {
                bytes.try_reserve(1).map_err(|_| Error::NOMEM)?;
                bytes.push(0);
                Ok(CString::from_vec_with_nul(bytes))
            }
            Err(Error::BADF | Error::NOENT) => Ok(None),
            Err(err) => Err(err),
        }
    }

    #[cfg(target_os = "macos")]
    {
        match fcntl_getpath(allocator, fd) {
            Ok(path) => Ok(Some(path.count())),
            Err(Error::BADF | Error::NOENT) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

/// Formats `/proc/self/fd/<fd>\0` on the stack without allocating.
#[cfg(any(target_os = "linux", target_os = "none"))]
fn format_proc_self_fd(buf: &mut [u8; 32], mut fd: u32) -> CStr<'_, Thin> {
    const PREFIX: &[u8] = b"/proc/self/fd/";
    let mut digits = [0u8; 10];
    let mut num_digits = 0;
    loop {
        digits[num_digits] = b'0' + (fd % 10) as u8;
        num_digits += 1;
        fd /= 10;
        if fd == 0 {
            break;
        }
    }
    let mut len = PREFIX.len();
    buf[..len].copy_from_slice(PREFIX);
    for i in (0..num_digits).rev() {
        buf[len] = digits[i];
        len += 1;
    }
    buf[len] = 0;
    // SAFETY: `buf` begins with a NUL-terminated ASCII string and remains
    // borrowed for the returned lifetime.
    unsafe { CStr::from_ptr(buf.as_ptr()) }
}

/// Resolves `path` relative to `dirfd`, leaking any newly allocated joined
/// pathname into `allocator`.
///
/// Absolute paths are returned directly without allocating. Relative paths
/// (including the empty path) return `Ok(None)` when `dirfd` is `None` or
/// cannot be resolved to a path.
///
/// # Errors
///
/// Returns [`Error::NOMEM`] if storage cannot be allocated, or any other OS
/// error from [`fd_path`].
pub fn resolve_path_at<'a, A: Allocator>(
    allocator: &'a A,
    dirfd: Option<BorrowedFd<'_>>,
    path: CStr<'a, Thin>,
) -> Result<Option<CStr<'a, Fat>>> {
    let counted = path.count();
    let rel = counted.as_units();
    if rel.first() == Some(&b'/') {
        return Ok(Some(counted));
    }

    let Some(dirfd) = dirfd else {
        return Ok(None);
    };

    let Some(base) = fd_path(allocator, dirfd)? else {
        return Ok(None);
    };

    let bytes = if rel.is_empty() {
        base.into_units_with_nul()
    } else {
        let mut joined = base.into_units();
        let needs_sep = joined.last() != Some(&b'/');
        let extra = usize::from(needs_sep) + rel.len() + 1;
        joined.try_reserve(extra).map_err(|_| Error::NOMEM)?;
        if needs_sep {
            joined.push(b'/');
        }
        joined.extend_from_slice(rel);
        joined.push(0);
        joined
    };

    let leaked: &'a [u8] = bytes.leak();
    // SAFETY: `base` and `rel` contain no interior NULs, and `bytes` ends with
    // exactly one NUL terminator.
    Ok(Some(unsafe { CStr::from_units_with_nul_unchecked(leaked) }))
}

#[cfg(test)]
mod tests {
    use std::{ffi::CString as StdCString, fs::File, os::fd::AsRawFd as _};

    use allocator_api2::alloc::Global;
    use fspy_nostd::{BorrowedFd, CStr, CWD, Thin};

    #[test]
    fn getcwd_allocates_a_c_string() {
        let path = super::getcwd(Global).unwrap();
        let mut buffer = [core::mem::MaybeUninit::uninit(); fspy_nostd::fs::PATH_MAX];
        let expected = fspy_nostd::fs::getcwd(&mut buffer).unwrap();
        let expected = expected.as_units_with_nul();

        assert_eq!(path.as_c_str().as_units_with_nul(), expected);
        assert_eq!(path.as_units(), &expected[..expected.len() - 1]);
        assert_eq!(path.as_units_with_nul(), expected);
        assert_eq!(path.into_units().as_slice(), &expected[..expected.len() - 1]);

        let path = super::getcwd(Global).unwrap();
        assert_eq!(path.into_units_with_nul().as_slice(), expected);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn fcntl_getpath_allocates_a_thin_c_string() {
        let root = File::open("/").unwrap();
        // SAFETY: `root` remains open for the call.
        let root = unsafe { BorrowedFd::borrow_raw(root.as_raw_fd()) };
        let path = super::fcntl_getpath(Global, root).unwrap();

        assert_eq!(path.as_c_str().count().as_units_with_nul(), b"/\0");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn readlinkat_allocates_the_complete_target() {
        // SAFETY: the literal is NUL-terminated and lives for the call.
        let path = unsafe { CStr::<Thin>::from_ptr(c"/proc/self/exe".as_ptr().cast()) };
        let target = super::readlinkat(Global, CWD, path).unwrap();

        assert_eq!(
            target.as_slice(),
            std::fs::read_link("/proc/self/exe").unwrap().as_os_str().as_encoded_bytes()
        );
    }

    #[test]
    fn fd_path_resolves_cwd_regular_fd_and_invalid_fd() {
        let cwd_path = super::fd_path(Global, CWD).unwrap().unwrap();
        let expected_cwd = super::getcwd(Global).unwrap();
        assert_eq!(cwd_path.as_units_with_nul(), expected_cwd.as_units_with_nul());

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("sample.txt");
        std::fs::write(&file_path, b"hi").unwrap();
        let canonical = std::fs::canonicalize(&file_path).unwrap();
        let file = File::open(&file_path).unwrap();
        // SAFETY: `file` remains open for the call.
        let file_fd = unsafe { BorrowedFd::borrow_raw(file.as_raw_fd()) };
        let resolved = super::fd_path(Global, file_fd).unwrap().unwrap();
        assert_eq!(resolved.as_units(), canonical.as_os_str().as_encoded_bytes());

        // A high descriptor number that is not open returns `Ok(None)`.
        // SAFETY: passing an unopened descriptor only triggers EBADF/ENOENT.
        let invalid_fd = unsafe { BorrowedFd::borrow_raw(999_999) };
        assert!(super::fd_path(Global, invalid_fd).unwrap().is_none());

        // `try_borrow_raw(-1)` yields `None`, matching invalid caller fds.
        // SAFETY: `-1` is explicitly handled by `try_borrow_raw`.
        assert!(unsafe { BorrowedFd::try_borrow_raw(-1) }.is_none());
    }

    #[test]
    fn resolve_path_at_handles_absolute_relative_empty_and_missing_dirfd() {
        let bump = crate::pooled_bump();

        // Absolute path returns directly without allocating.
        let abs_raw = c"/usr/bin/env";
        // SAFETY: static NUL-terminated literal.
        let abs = unsafe { CStr::<Thin>::from_ptr(abs_raw.as_ptr().cast()) };
        let resolved_abs = super::resolve_path_at(&bump, None, abs).unwrap().unwrap();
        assert_eq!(resolved_abs.as_units(), b"/usr/bin/env");
        assert_eq!(resolved_abs.as_ptr(), abs.as_ptr());

        // Relative path with `None` dirfd returns `Ok(None)`.
        // SAFETY: static NUL-terminated literal.
        let rel = unsafe { CStr::<Thin>::from_ptr(c"sub/file".as_ptr().cast()) };
        assert!(super::resolve_path_at(&bump, None, rel).unwrap().is_none());

        // Relative path with `Some(CWD)` joins against `getcwd`.
        let expected_cwd = super::getcwd(Global).unwrap();
        let resolved_cwd = super::resolve_path_at(&bump, Some(CWD), rel).unwrap().unwrap();
        let mut expected = expected_cwd.as_units().to_vec();
        if expected.last() != Some(&b'/') {
            expected.push(b'/');
        }
        expected.extend_from_slice(b"sub/file");
        assert_eq!(resolved_cwd.as_units(), expected.as_slice());

        // Relative path with a real directory fd.
        let dir = tempfile::tempdir().unwrap();
        let canonical_dir = std::fs::canonicalize(dir.path()).unwrap();
        let dir_file = File::open(dir.path()).unwrap();
        // SAFETY: `dir_file` stays open for the calls below.
        let dir_fd = unsafe { BorrowedFd::borrow_raw(dir_file.as_raw_fd()) };

        let resolved_dir = super::resolve_path_at(&bump, Some(dir_fd), rel).unwrap().unwrap();
        let mut expected_dir = canonical_dir.as_os_str().as_encoded_bytes().to_vec();
        expected_dir.push(b'/');
        expected_dir.extend_from_slice(b"sub/file");
        assert_eq!(resolved_dir.as_units(), expected_dir.as_slice());

        // Empty path with a directory fd returns the directory path itself.
        // SAFETY: static NUL-terminated literal.
        let empty = unsafe { CStr::<Thin>::from_ptr(c"".as_ptr().cast()) };
        let resolved_empty = super::resolve_path_at(&bump, Some(dir_fd), empty).unwrap().unwrap();
        assert_eq!(resolved_empty.as_units(), canonical_dir.as_os_str().as_encoded_bytes());

        // Joining against `/` does not double the leading slash.
        let root_file = File::open("/").unwrap();
        // SAFETY: `root_file` stays open for the call.
        let root_fd = unsafe { BorrowedFd::borrow_raw(root_file.as_raw_fd()) };
        let child = StdCString::new("etc").unwrap();
        // SAFETY: `child` is NUL-terminated for the call.
        let child_cstr = unsafe { CStr::<Thin>::from_ptr(child.as_ptr().cast()) };
        let resolved_root =
            super::resolve_path_at(&bump, Some(root_fd), child_cstr).unwrap().unwrap();
        assert_eq!(resolved_root.as_units(), b"/etc");
    }
}
