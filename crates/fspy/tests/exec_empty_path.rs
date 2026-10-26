//! An exec of an empty path fails with `ENOENT` before the kernel looks
//! anything up. Under fspy, every exec entry point must fail the same way it
//! does untraced, without aborting the program or recording an access.
#![cfg(unix)]

mod test_utils;

use std::{env::current_dir, io::Error, ptr::null};

use libc::{c_char, c_int};
use test_log::test;
use test_utils::assert_not_contains;

/// Calls each exec entry point with an empty path and returns the error
/// each reports: `errno` from the exec family, and the return value of
/// `posix_spawn` and `posix_spawnp`.
fn exec_empty_path() -> Vec<c_int> {
    let argv = [c"".as_ptr(), null()];
    let envp = [null::<c_char>()];
    let errno = |ret: c_int| {
        assert_eq!(ret, -1);
        Error::last_os_error().raw_os_error().unwrap()
    };
    let mut pid = 0;
    // SAFETY: every pointer is a valid NUL-terminated C string or a
    // NULL-terminated array of them, and an exec of an empty path returns.
    unsafe {
        vec![
            errno(libc::execve(c"".as_ptr(), argv.as_ptr(), envp.as_ptr())),
            errno(libc::execv(c"".as_ptr(), argv.as_ptr())),
            errno(libc::execvp(c"".as_ptr(), argv.as_ptr())),
            errno(libc::execl(c"".as_ptr(), argv[0], null::<c_char>())),
            errno(libc::execle(c"".as_ptr(), argv[0], null::<c_char>(), envp.as_ptr())),
            errno(libc::execlp(c"".as_ptr(), argv[0], null::<c_char>())),
            libc::posix_spawn(
                &raw mut pid,
                c"".as_ptr(),
                null(),
                null(),
                argv.as_ptr().cast(),
                envp.as_ptr().cast(),
            ),
            libc::posix_spawnp(
                &raw mut pid,
                c"".as_ptr(),
                null(),
                null(),
                argv.as_ptr().cast(),
                envp.as_ptr().cast(),
            ),
            #[cfg(target_os = "linux")]
            errno(libc::execvpe(c"".as_ptr(), argv.as_ptr(), envp.as_ptr())),
            #[cfg(all(target_os = "linux", target_env = "gnu"))]
            errno(libc::execveat(
                libc::AT_FDCWD,
                c"".as_ptr(),
                argv.as_ptr().cast(),
                envp.as_ptr().cast(),
                0,
            )),
        ]
    }
}

#[test(tokio::test)]
async fn exec_empty_path_fails_like_untraced() -> anyhow::Result<()> {
    let untraced = exec_empty_path();
    let accesses = track_fn!(untraced, |untraced: Vec<c_int>| {
        assert_eq!(exec_empty_path(), untraced);
    })
    .await?;
    // Seccomp tracking, used on musl, reports the directory an empty path
    // is relative to.
    if cfg!(not(target_env = "musl")) {
        assert_not_contains(&accesses, &current_dir()?);
    }
    Ok(())
}
