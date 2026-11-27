use allocator_api2::{alloc::Allocator, vec::Vec};
use fspy_nostd::{
    BorrowedFd, CStr, Error, OwnedFd, Result, Thin,
    fs::{AccessFlags, Mode, OFlags, faccessat, fstat, openat},
    io::pread,
};

/// Opens an executable regular file relative to `dirfd` and checks `X_OK`.
///
/// Opens with `O_RDONLY | O_CLOEXEC | O_NONBLOCK` (plus `O_NOFOLLOW` when
/// `nofollow` is `true`) so FIFOs do not block and symlinks fail with `ELOOP`
/// under `AT_SYMLINK_NOFOLLOW`. Non-regular files (such as directories or
/// FIFOs) are rejected with `EACCES`, matching kernel `execve`.
pub fn open_executable(
    dirfd: BorrowedFd<'_>,
    path: CStr<'_, Thin>,
    nofollow: bool,
) -> Result<OwnedFd> {
    let mut flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK;
    if nofollow {
        flags |= OFlags::NOFOLLOW;
    }
    let fd = openat(dirfd, path, flags, Mode::empty())?;
    if !fstat(fd.as_fd())?.is_regular_file() {
        return Err(Error::ACCES);
    }
    faccessat(dirfd, path, AccessFlags::X_OK)?;
    Ok(fd)
}

/// Reads up to `buf.len()` bytes starting at offset 0 of `fd` without mutating
/// the descriptor's file offset.
pub fn pread_header(fd: BorrowedFd<'_>, buf: &mut [u8]) -> Result<usize> {
    let mut total = 0usize;
    while total < buf.len() {
        match pread(fd, &mut buf[total..], total as u64) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(Error::INTR) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(total)
}

/// Makes a relative `path` absolute against `base_dir` (which must be
/// absolute), matching `std::path::absolute` without touching `std` or libc
/// malloc.
#[cfg_attr(
    not(target_os = "macos"),
    expect(dead_code, reason = "used by macOS posix_spawn chdir path normalization")
)]
pub fn join_relative_to_base<A: Allocator>(
    base_dir: &[u8],
    path: &[u8],
    allocator: A,
) -> Result<Vec<u8, A>> {
    debug_assert!(!path.starts_with(b"/"));
    if path.is_empty() {
        return Err(Error::INVAL);
    }
    let mut abs = Vec::with_capacity_in(base_dir.len() + 1 + path.len(), allocator);
    abs.extend_from_slice(base_dir);
    append_relative_segments(&mut abs, path);
    Ok(abs)
}

/// Makes a relative `path` absolute against the current working directory,
/// matching `std::path::absolute` without touching `std` or libc malloc.
#[cfg_attr(
    not(any(target_os = "macos", test)),
    expect(dead_code, reason = "used by macOS path normalization and unit tests")
)]
pub fn absolute_relative<A: Allocator>(path: &[u8], allocator: A) -> Result<Vec<u8, A>> {
    debug_assert!(!path.starts_with(b"/"));
    if path.is_empty() {
        return Err(Error::INVAL);
    }
    let mut abs = fspy_nostd_alloc::fs::getcwd(allocator)?.into_units();
    append_relative_segments(&mut abs, path);
    Ok(abs)
}

#[cfg_attr(
    not(any(target_os = "macos", test)),
    expect(dead_code, reason = "used by macOS path normalization and unit tests")
)]
fn append_relative_segments<A: Allocator>(abs: &mut Vec<u8, A>, path: &[u8]) {
    for segment in path.split(|&b| b == b'/') {
        if segment.is_empty() || segment == b"." {
            continue;
        }
        if abs.last() != Some(&b'/') {
            abs.push(b'/');
        }
        abs.extend_from_slice(segment);
    }
    if path.ends_with(b"/") && abs.last() != Some(&b'/') {
        abs.push(b'/');
    }
}

#[cfg(test)]
mod tests {
    use std::{os::unix::ffi::OsStrExt as _, path::Path};

    use allocator_api2::alloc::Global;

    use super::absolute_relative;

    #[test]
    fn absolute_relative_matches_std() {
        for path in ["foo", "./foo", "foo/./bar", "foo//bar", "../foo", "foo/", ".", "./", ""] {
            let std_path = Path::new(path);
            assert_eq!(
                absolute_relative(path.as_bytes(), Global).ok().as_deref(),
                std::path::absolute(std_path).ok().as_ref().map(|p| p.as_os_str().as_bytes()),
                "{path}"
            );
        }
    }
}
