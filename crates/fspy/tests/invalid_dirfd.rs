//! A tracked program may pass `-1` as a directory descriptor, typically a
//! failed `open` result that it does not check. With a relative path the call
//! fails with `EBADF`; with an absolute path the descriptor is ignored. The
//! interposers must forward both cases instead of aborting the program.
//!
//! procps-ng's `ps -p <pid>` does this when the process has exited: it opens
//! `/proc/<pid>`, then calls `openat(-1, "status", ...)`.
//!
//! Directory descriptors exist only on Unix; Windows has no equivalent calls.
#![cfg(unix)]

mod test_utils;

use fspy::PathAccess;
use test_log::test;
use test_utils::{assert_contains, assert_not_contains};

fn errno() -> libc::c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap()
}

/// `STATX_BASIC_STATS`. The libc crate does not define it for every Linux
/// target, such as musl without `musl_v1_2_3`.
#[cfg(target_os = "linux")]
const STATX_BASIC_STATS: libc::c_uint = 0x7ff;

/// Storage for the kernel's 256-byte `struct statx`. The libc crate does not
/// define the struct for every Linux target.
#[cfg(target_os = "linux")]
#[repr(C, align(8))]
struct StatxBuf([u8; 256]);

/// Calls `statx` through `syscall`, which fspy intercepts on glibc.
///
/// The libc `statx` wrapper is not called directly: glibc added it in 2.28,
/// so a test built for an older glibc fails to link.
#[cfg(target_os = "linux")]
fn syscall_statx(
    dirfd: libc::c_int,
    path: *const libc::c_char,
    flags: libc::c_int,
) -> libc::c_long {
    let mut buf = StatxBuf([0; 256]);
    // SAFETY: `path` is null or a valid C string, and `buf` is writable
    // storage of the size that the kernel writes.
    unsafe {
        libc::syscall(
            libc::SYS_statx,
            dirfd,
            path,
            flags,
            STATX_BASIC_STATS,
            core::ptr::from_mut(&mut buf),
        )
    }
}

/// Calls the glibc `statx` wrapper, resolved at run time. With `LD_PRELOAD`,
/// the lookup finds the fspy interposer first.
///
/// # Panics
///
/// Panics if glibc has no `statx` (added in 2.28).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn glibc_statx(dirfd: libc::c_int, path: *const libc::c_char, flags: libc::c_int) -> libc::c_int {
    type Statx = unsafe extern "C" fn(
        libc::c_int,
        *const libc::c_char,
        libc::c_int,
        libc::c_uint,
        *mut StatxBuf,
    ) -> libc::c_int;
    // SAFETY: the name is a valid C string.
    let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"statx".as_ptr()) };
    assert!(!symbol.is_null(), "glibc has no statx");
    // SAFETY: glibc's `statx` has this signature.
    let statx = unsafe { core::mem::transmute::<*mut libc::c_void, Statx>(symbol) };
    let mut buf = StatxBuf([0; 256]);
    // SAFETY: `path` is null or a valid C string, and `buf` is writable
    // storage of the size that the kernel writes.
    unsafe { statx(dirfd, path, flags, STATX_BASIC_STATS, core::ptr::from_mut(&mut buf)) }
}

/// Calls the glibc `execveat` wrapper, resolved at run time for the same
/// reason as [`glibc_statx`].
///
/// # Panics
///
/// Panics if glibc has no `execveat` (added in 2.34).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn glibc_execveat(dirfd: libc::c_int, path: &core::ffi::CStr) -> libc::c_int {
    type Execveat = unsafe extern "C" fn(
        libc::c_int,
        *const libc::c_char,
        *const *const libc::c_char,
        *const *const libc::c_char,
        libc::c_int,
    ) -> libc::c_int;
    // SAFETY: the name is a valid C string.
    let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"execveat".as_ptr()) };
    assert!(!symbol.is_null(), "glibc has no execveat");
    // SAFETY: glibc's `execveat` has this signature.
    let execveat = unsafe { core::mem::transmute::<*mut libc::c_void, Execveat>(symbol) };
    let argv = [path.as_ptr(), core::ptr::null()];
    let envp = [core::ptr::null()];
    // SAFETY: `path` is a valid C string, and `argv` and `envp` are
    // null-terminated arrays of valid C strings.
    unsafe { execveat(dirfd, path.as_ptr(), argv.as_ptr(), envp.as_ptr(), 0) }
}

#[test(tokio::test)]
async fn relative_path_at_invalid_dirfd() -> anyhow::Result<()> {
    let accesses = track_fn!((), |(): ()| {
        let path = c"hello";
        // SAFETY: every call below receives a valid C string and writable
        // output storage; the kernel rejects the descriptor.
        unsafe {
            assert_eq!(libc::openat(-1, path.as_ptr(), libc::O_RDONLY), -1);
            assert_eq!(errno(), libc::EBADF);

            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            assert_eq!(libc::fstatat(-1, path.as_ptr(), stat.as_mut_ptr(), 0), -1);
            assert_eq!(errno(), libc::EBADF);

            assert_eq!(libc::faccessat(-1, path.as_ptr(), libc::F_OK, 0), -1);
            assert_eq!(errno(), libc::EBADF);

            assert!(libc::fdopendir(-1).is_null());
            assert_eq!(errno(), libc::EBADF);
        }
        #[cfg(target_os = "linux")]
        {
            assert_eq!(syscall_statx(-1, path.as_ptr(), 0), -1);
            assert_eq!(errno(), libc::EBADF);
        }
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        {
            assert_eq!(glibc_statx(-1, path.as_ptr(), 0), -1);
            assert_eq!(errno(), libc::EBADF);

            assert_eq!(glibc_execveat(-1, path), -1);
            assert_eq!(errno(), libc::EBADF);
        }
    })
    .await?;
    // `-1` is not the current directory. The child inherits the current
    // directory of the test.
    assert_not_contains(&accesses, &std::env::current_dir()?.join("hello"));
    Ok(())
}

/// `statx` with a null path and `AT_EMPTY_PATH` uses only the descriptor.
#[cfg(target_os = "linux")]
#[test(tokio::test)]
async fn empty_path_at_invalid_dirfd() -> anyhow::Result<()> {
    track_fn!((), |(): ()| {
        // Linux 6.11 and later accept a null path with `AT_EMPTY_PATH` and
        // reject the descriptor. Earlier kernels reject the null path.
        let rejected = [libc::EBADF, libc::EFAULT];

        assert_eq!(syscall_statx(-1, core::ptr::null(), libc::AT_EMPTY_PATH), -1);
        assert!(rejected.contains(&errno()), "unexpected errno {}", errno());

        #[cfg(target_env = "gnu")]
        {
            assert_eq!(glibc_statx(-1, core::ptr::null(), libc::AT_EMPTY_PATH), -1);
            assert!(rejected.contains(&errno()), "unexpected errno {}", errno());
        }
    })
    .await?;
    Ok(())
}

#[test(tokio::test)]
async fn absolute_path_at_invalid_dirfd() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let tmp_path = std::fs::canonicalize(tmp_dir.path())?.join("hello");
    std::fs::write(&tmp_path, "")?;

    let accesses = track_fn!(tmp_path.to_str().unwrap().to_owned(), |tmp_path: String| {
        let path = std::ffi::CString::new(tmp_path).unwrap();
        // SAFETY: `path` is a valid C string; an absolute path makes the
        // kernel ignore the descriptor.
        let fd = unsafe { libc::openat(-1, path.as_ptr(), libc::O_RDONLY) };
        assert!(fd >= 0, "openat failed with errno {}", errno());
        // SAFETY: `fd` was opened above and is closed once.
        unsafe { libc::close(fd) };
    })
    .await?;
    assert_contains(&accesses, &tmp_path, PathAccess::Read);

    Ok(())
}
