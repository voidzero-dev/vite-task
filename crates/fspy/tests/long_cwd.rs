//! fspy can't read a current directory longer than `PATH_MAX` without
//! allocating, so it can't resolve paths relative to it. Opens and execs of
//! such paths must still behave as they do untraced; they just go unreported.
#![cfg(unix)]

mod test_utils;

use std::{
    env::{current_exe, set_current_dir, temp_dir},
    fs,
    io::{self, Error, Write as _},
    os::unix::fs::PermissionsExt,
    process::Command,
    ptr::null,
};

use libc::c_int;
use test_log::test;

/// Enters a new directory under `root` whose path is longer than `PATH_MAX`,
/// writes a script there whose interpreter is a copy of `program`, and
/// execs it by relative path in a child. Returns the child's wait status.
fn exec_from_long_cwd(root: &str, program: &str) -> c_int {
    // Descend with relative `chdir`s until the current directory is longer
    // than `PATH_MAX`.
    fs::create_dir(root).unwrap();
    set_current_dir(root).unwrap();
    let name = "d".repeat(255);
    let mut len = root.len();
    while len <= usize::try_from(libc::PATH_MAX).unwrap() {
        fs::create_dir(&name).unwrap();
        set_current_dir(&name).unwrap();
        len += 1 + name.len();
    }

    // The interpreter is relative too, so resolving the exec reads two
    // relative paths. It is this test binary, which exits successfully when
    // no test matches the script path it is given.
    fs::copy(program, "program").unwrap();
    fs::write("script", "#!./program\n").unwrap();
    fs::set_permissions("script", fs::Permissions::from_mode(0o755)).unwrap();

    // SAFETY: the subprocess runs single-threaded in a constructor.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed: {}", Error::last_os_error());
    if pid == 0 {
        let argv = [c"./script".as_ptr(), null()];
        // SAFETY: every pointer is a valid C string or a NULL-terminated
        // array of them, and stdout is open.
        unsafe {
            libc::dup2(libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY), 1);
            libc::execv(argv[0], argv.as_ptr());
            libc::_exit(127);
        }
    }
    let mut status = 0;
    // SAFETY: `pid` is the child forked above.
    assert_eq!(unsafe { libc::waitpid(pid, &raw mut status, 0) }, pid);
    status
}

#[test(tokio::test)]
async fn exec_from_cwd_longer_than_path_max_like_untraced() -> anyhow::Result<()> {
    let program = current_exe()?.to_str().unwrap().to_owned();
    let root = temp_dir().join(format!("fspy-long-cwd-{}", std::process::id()));
    fs::create_dir(&root)?;
    let untraced_root = root.join("untraced").to_str().unwrap().to_owned();
    let traced_root = root.join("traced").to_str().unwrap().to_owned();

    let arg = (untraced_root, program.clone());
    let untraced =
        Command::from(test_utils::command_for_fn!(arg, |(root, program): (String, String)| {
            let status = exec_from_long_cwd(&root, &program).to_string();
            io::stdout().write_all(status.as_bytes()).unwrap();
        }))
        .output()?;
    assert!(untraced.status.success(), "{untraced:?}");
    let untraced: c_int = String::from_utf8(untraced.stdout)?.parse()?;

    let arg = (traced_root, program, untraced);
    track_fn!(arg, |(root, program, untraced): (String, String, c_int)| {
        assert_eq!(exec_from_long_cwd(&root, &program), untraced);
    })
    .await?;
    fs::remove_dir_all(&root)?;
    Ok(())
}
