//! `execl`, `execlp`, and `execle` take a variable number of arguments. The
//! interposer stores them in a stack array for fewer than 32 arguments and a
//! heap allocation otherwise. A call with 32 or more arguments used to write
//! the NULL terminator past the end of that allocation, aborting the program.
//!
//! `exec` replaces the process, so the call runs in a forked child; the parent
//! waits for the child and checks that the program ran successfully.
#![cfg(unix)]

mod test_utils;

use std::{ffi::CString, io::Error};

use test_log::test;

#[test(tokio::test)]
async fn execl_with_many_arguments() -> anyhow::Result<()> {
    let program = which::which("true")?;
    track_fn!(program.to_str().unwrap().to_owned(), |program: String| {
        let program = CString::new(program).unwrap();
        let args: Vec<CString> = (0..40)
            .map(
                |i| if i == 0 { program.clone() } else { CString::new(format!("arg{i}")).unwrap() },
            )
            .collect();

        // SAFETY: the tracked test process is single-threaded here, and the
        // child and parent only call async-signal-safe functions below.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed: {}", Error::last_os_error());

        if pid == 0 {
            // SAFETY: every pointer is a valid NUL-terminated C string and the
            // argument list is NULL-terminated.
            unsafe {
                libc::execl(
                    program.as_ptr(),
                    args[0].as_ptr(),
                    args[1].as_ptr(),
                    args[2].as_ptr(),
                    args[3].as_ptr(),
                    args[4].as_ptr(),
                    args[5].as_ptr(),
                    args[6].as_ptr(),
                    args[7].as_ptr(),
                    args[8].as_ptr(),
                    args[9].as_ptr(),
                    args[10].as_ptr(),
                    args[11].as_ptr(),
                    args[12].as_ptr(),
                    args[13].as_ptr(),
                    args[14].as_ptr(),
                    args[15].as_ptr(),
                    args[16].as_ptr(),
                    args[17].as_ptr(),
                    args[18].as_ptr(),
                    args[19].as_ptr(),
                    args[20].as_ptr(),
                    args[21].as_ptr(),
                    args[22].as_ptr(),
                    args[23].as_ptr(),
                    args[24].as_ptr(),
                    args[25].as_ptr(),
                    args[26].as_ptr(),
                    args[27].as_ptr(),
                    args[28].as_ptr(),
                    args[29].as_ptr(),
                    args[30].as_ptr(),
                    args[31].as_ptr(),
                    args[32].as_ptr(),
                    args[33].as_ptr(),
                    args[34].as_ptr(),
                    args[35].as_ptr(),
                    args[36].as_ptr(),
                    args[37].as_ptr(),
                    args[38].as_ptr(),
                    args[39].as_ptr(),
                    core::ptr::null::<libc::c_char>(),
                );
                libc::_exit(127);
            }
        }

        let mut status = 0;
        // SAFETY: `pid` is the child process created above, and `status` is
        // writable storage for the wait status.
        let waited = unsafe { libc::waitpid(pid, &raw mut status, 0) };
        assert_eq!(waited, pid, "waitpid failed: {}", Error::last_os_error());
        assert!(libc::WIFEXITED(status), "child did not exit normally: {status}");
        assert_eq!(libc::WEXITSTATUS(status), 0);
    })
    .await?;

    Ok(())
}
