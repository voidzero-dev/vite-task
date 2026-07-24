#![cfg(all(any(target_os = "linux", target_os = "macos"), not(target_env = "musl")))]

mod test_utils;

use std::{ffi::CString, fs, os::unix::ffi::OsStrExt as _, path::Path, time::Duration};

use fspy::AccessMode;
use test_log::test;
use test_utils::assert_contains;

#[test(tokio::test)]
async fn captures_access_after_fork_without_exec() -> anyhow::Result<()> {
    let tempdir = tempfile::tempdir()?;
    let prime_path = tempdir.path().join("prime");
    let after_fork_path = tempdir.path().join("after-fork");
    fs::write(&prime_path, [])?;
    fs::write(&after_fork_path, [])?;

    let accesses = track_fn!(
        (
            prime_path.as_os_str().as_bytes().to_vec(),
            after_fork_path.as_os_str().as_bytes().to_vec(),
        ),
        |(prime_path, after_fork_path): (Vec<u8>, Vec<u8>)| {
            let prime_path = CString::new(prime_path).unwrap();
            let after_fork_path = CString::new(after_fork_path).unwrap();
            let inherited_fds = inherited_open_fds();
            let ready = new_pipe();
            let parent_alive = new_pipe();

            // SAFETY: The process is single-threaded at this point in the test
            // function, and both branches restrict post-fork work to libc calls.
            let pid = unsafe { libc::fork() };
            assert_ne!(pid, -1);

            if pid == 0 {
                // SAFETY: All descriptors and C strings were prepared before
                // fork and are valid in the child.
                unsafe {
                    libc::close(ready.read);
                    libc::close(parent_alive.write);
                    for fd in inherited_fds {
                        libc::close(fd);
                    }

                    // The first child access must acquire a sender backed by
                    // this process's own lock-file description.
                    open_and_close(&prime_path);
                    write_byte(ready.write);
                    libc::close(ready.write);

                    wait_for_eof(parent_alive.read);
                    libc::close(parent_alive.read);
                    // Without a child-owned sender, the receiver can finish
                    // as soon as the direct parent exits. Give it time to do
                    // so before making the access this test cares about.
                    sleep(Duration::from_millis(250));

                    open_and_close(&after_fork_path);
                    libc::_exit(0);
                }
            }

            // SAFETY: These are the parent's valid pipe endpoints.
            unsafe {
                libc::close(ready.write);
                libc::close(parent_alive.read);
                read_byte(ready.read);
                libc::close(ready.read);
            }

            // Keep `parent_alive.write` open. subprocess_test exits this direct
            // parent as soon as the closure returns, which releases the child.
        }
    )
    .await?;

    assert_contains(&accesses, Path::new(&after_fork_path), AccessMode::READ);
    Ok(())
}

#[derive(Clone, Copy)]
struct Pipe {
    read: libc::c_int,
    write: libc::c_int,
}

fn new_pipe() -> Pipe {
    let mut fds = [-1; 2];
    // SAFETY: `fds` points to two writable c_int values.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    Pipe { read: fds[0], write: fds[1] }
}

fn inherited_open_fds() -> Vec<libc::c_int> {
    #[cfg(target_os = "linux")]
    const FD_DIRECTORY: &str = "/proc/self/fd";
    #[cfg(target_os = "macos")]
    const FD_DIRECTORY: &str = "/dev/fd";

    let mut fds = fs::read_dir(FD_DIRECTORY)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().parse::<libc::c_int>().unwrap())
        .filter(|fd| *fd > libc::STDERR_FILENO)
        .collect::<Vec<_>>();

    // Reading the directory temporarily opens an fd that may appear in its own
    // listing. Remove descriptors that closed with the iterator before pipes
    // are created and can reuse their numbers.
    fds.retain(|fd| {
        // SAFETY: F_GETFD only examines the numeric descriptor.
        (unsafe { libc::fcntl(*fd, libc::F_GETFD) }) != -1
    });
    fds
}

unsafe fn open_and_close(path: &CString) {
    // SAFETY: `path` is a valid NUL-terminated string.
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };
    if fd == -1 {
        // SAFETY: Immediate process termination is valid in the fork child.
        unsafe { libc::_exit(10) };
    }
    // SAFETY: `fd` was returned by open above.
    unsafe { libc::close(fd) };
}

unsafe fn write_byte(fd: libc::c_int) {
    let byte = [1_u8];
    // SAFETY: `byte` is readable for one byte and `fd` is the pipe write end.
    if unsafe { libc::write(fd, byte.as_ptr().cast(), byte.len()) } != 1 {
        // SAFETY: Immediate process termination is valid in the fork child.
        unsafe { libc::_exit(11) };
    }
}

unsafe fn read_byte(fd: libc::c_int) {
    let mut byte = [0_u8];
    // SAFETY: `byte` is writable for one byte and `fd` is the pipe read end.
    assert_eq!(unsafe { libc::read(fd, byte.as_mut_ptr().cast(), byte.len()) }, 1);
}

unsafe fn wait_for_eof(fd: libc::c_int) {
    let mut byte = [0_u8];
    // SAFETY: `byte` is writable for one byte and `fd` is the pipe read end.
    if unsafe { libc::read(fd, byte.as_mut_ptr().cast(), byte.len()) } != 0 {
        // SAFETY: Immediate process termination is valid in the fork child.
        unsafe { libc::_exit(12) };
    }
}

unsafe fn sleep(duration: Duration) {
    let request = libc::timespec {
        tv_sec: libc::time_t::try_from(duration.as_secs()).unwrap(),
        tv_nsec: libc::c_long::from(duration.subsec_nanos()),
    };
    // SAFETY: `request` is a valid timespec and no remainder is requested.
    if unsafe { libc::nanosleep(&raw const request, std::ptr::null_mut()) } != 0 {
        // SAFETY: Immediate process termination is valid in the fork child.
        unsafe { libc::_exit(13) };
    }
}
