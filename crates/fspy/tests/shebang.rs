#![cfg(unix)]
mod test_utils;

use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
};

use fspy::PathAccess;
use test_log::test;
use test_utils::assert_contains;
use tokio::fs;

#[test(tokio::test)]
async fn spawn_sh_shebang() -> anyhow::Result<()> {
    let tmp_dir = tempfile::TempDir::new()?;

    let shebang_script_path = tmp_dir.path().join("fspy_test_shebang_script.sh");
    let shebang_script_path = shebang_script_path.into_os_string().into_string().unwrap();

    fs::write(&shebang_script_path, "#!/bin/sh\ncat hello\n").await?;

    let mut perms = fs::metadata(&shebang_script_path).await?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&shebang_script_path, perms).await?;

    let accesses = track_fn!(shebang_script_path.clone(), |shebang_script_path: String| {
        let _ignored = Command::new(&shebang_script_path)
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .status()
            .expect("Failed to execute shebang script");
    })
    .await?;

    assert_contains(&accesses, Path::new(&shebang_script_path), PathAccess::Read);
    assert_contains(&accesses, Path::new("/hello"), PathAccess::Read);

    Ok(())
}

#[test(tokio::test)]
async fn spawn_shebang_with_long_interpreter_path() -> anyhow::Result<()> {
    let tmp_dir = tempfile::TempDir::new()?;

    // Kernels read 256 (Linux) or 512 (macOS) bytes of the shebang line, so a
    // 200-byte interpreter path must be kept whole. Keep the symlink basename
    // `sh` so multi-call binaries like Alpine's BusyBox `/bin/sh` dispatch to
    // the `sh` applet.
    let mut dir = tmp_dir.path().to_path_buf();
    while dir.as_os_str().len() < 150 {
        dir.push("d".repeat(40));
    }
    dir.push("d".repeat(200 - "/sh".len() - dir.as_os_str().len() - 1));
    fs::create_dir_all(&dir).await?;
    let interpreter = dir.join("sh");
    assert_eq!(interpreter.as_os_str().len(), 200);
    fs::symlink("/bin/sh", &interpreter).await?;

    let script_path = tmp_dir.path().join("long_shebang.sh");
    fs::write(&script_path, format!("#!{}\necho ok\n", interpreter.display())).await?;
    fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).await?;
    let script_path = script_path.into_os_string().into_string().unwrap();

    let accesses = track_fn!(script_path, |script_path: String| {
        let output = Command::new(&script_path).stdin(Stdio::null()).output();
        // On macOS, fspy only swaps the arm64e `/bin/sh` for its own shell when
        // it's run by that path. Through the symlink, dyld refuses to load the
        // arm64 preload into it where SIP is off (as on CI runners), so only
        // check there that the whole interpreter path was read.
        if cfg!(target_os = "linux") {
            let output = output.expect("Failed to execute shebang script");
            assert_eq!(
                output.stdout,
                b"ok\n",
                "stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    })
    .await?;

    assert_contains(&accesses, &interpreter, PathAccess::Read);

    Ok(())
}

/// macOS splits shebang arguments on whitespace; Linux passes everything after
/// the interpreter as a single argument.
#[test(tokio::test)]
async fn shebang_argument_splitting_matches_platform() -> anyhow::Result<()> {
    let tmp_dir = tempfile::TempDir::new()?;
    let script_path = tmp_dir.path().canonicalize()?.join("split_args.sh");
    fs::write(&script_path, "#!/bin/echo   foo   bar\n").await?;
    fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).await?;
    let script_str = script_path.to_str().unwrap().to_owned();

    let expected = if cfg!(target_os = "macos") {
        format!("foo bar {script_str}\n")
    } else {
        format!("foo   bar {script_str}\n")
    };
    let untraced = Command::new(&script_str).stdin(Stdio::null()).output()?;
    assert!(untraced.status.success());
    assert_eq!(String::from_utf8_lossy(&untraced.stdout), expected);

    let args = (script_str, expected);
    let accesses = track_fn!(args, |(script_str, expected): (String, String)| {
        let output = Command::new(&script_str)
            .stdin(Stdio::null())
            .output()
            .expect("Failed to execute shebang script");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    })
    .await?;

    assert_contains(&accesses, &script_path, PathAccess::Read);

    Ok(())
}

/// Chained shebang depth limits: Linux allows 5 hops and rejects the 6th with
/// `ELOOP`; macOS allows 1 hop and rejects the 2nd with `ENOEXEC`.
#[test(tokio::test)]
async fn chained_shebang_depth_matches_platform() -> anyhow::Result<()> {
    let tmp_dir = tempfile::TempDir::new()?;
    let root = tmp_dir.path().canonicalize()?;

    let mut scripts = Vec::new();
    let base = root.join("hop1.sh");
    fs::write(&base, "#!/bin/sh\necho chained-ok\n").await?;
    fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).await?;
    scripts.push(base);

    let max_hops = if cfg!(target_os = "macos") { 2 } else { 6 };
    for i in 2..=max_hops {
        let prev = scripts.last().unwrap();
        let next = root.join(format!("hop{i}.sh"));
        fs::write(&next, format!("#!{}\n", prev.display())).await?;
        fs::set_permissions(&next, std::fs::Permissions::from_mode(0o755)).await?;
        scripts.push(next);
    }

    let max_ok = scripts[scripts.len() - 2].to_str().unwrap().to_owned();
    let too_deep = scripts[scripts.len() - 1].to_str().unwrap().to_owned();
    let expected_errno = if cfg!(target_os = "macos") { libc::ENOEXEC } else { libc::ELOOP };

    let args = (max_ok, too_deep, expected_errno);
    let accesses =
        track_fn!(args, |(max_ok, too_deep, expected_errno): (String, String, libc::c_int)| {
            let output = Command::new(&max_ok)
                .stdin(Stdio::null())
                .output()
                .expect("max allowed shebang depth should succeed");
            assert!(output.status.success());
            assert_eq!(output.stdout, b"chained-ok\n");

            let err = Command::new(&too_deep)
                .stdin(Stdio::null())
                .status()
                .expect_err("exceeding shebang recursion depth should fail");
            assert_eq!(err.raw_os_error(), Some(expected_errno));
        })
        .await?;

    for script in &scripts {
        assert_contains(&accesses, script, PathAccess::Read);
    }

    Ok(())
}

/// A shebang line exceeding the kernel's buffer limit must fail with `ENOEXEC`.
#[test(tokio::test)]
async fn overlong_shebang_is_rejected_with_enoexec() -> anyhow::Result<()> {
    fn posix_spawn_errno(path: &std::ffi::CStr) -> libc::c_int {
        let argv = [path.as_ptr().cast_mut(), std::ptr::null_mut()];
        let envp = [std::ptr::null_mut::<libc::c_char>()];
        let mut pid: libc::pid_t = 0;
        // SAFETY: `path`, `argv`, and `envp` are valid NUL/NULL-terminated.
        let rc = unsafe {
            libc::posix_spawn(
                &raw mut pid,
                path.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                argv.as_ptr(),
                envp.as_ptr(),
            )
        };
        if rc == 0 {
            let mut status = 0;
            // SAFETY: reap unexpected child.
            unsafe { libc::waitpid(pid, &raw mut status, 0) };
        }
        rc
    }

    let tmp_dir = tempfile::TempDir::new()?;
    let script_path = tmp_dir.path().canonicalize()?.join("overlong.sh");
    // Longer than both Linux's 256-byte and macOS's 512-byte kernel buffers.
    let content = format!("#!/{}\n", "a".repeat(520));
    fs::write(&script_path, content).await?;
    fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).await?;
    let script_str = script_path.to_str().unwrap().to_owned();

    // Use `posix_spawn` directly: `posix_spawnp` (used by `std::process::Command`)
    // falls back to `/bin/sh` on `ENOEXEC` on macOS.
    let c_script = std::ffi::CString::new(script_str.as_str())?;
    assert_eq!(posix_spawn_errno(&c_script), libc::ENOEXEC);

    let accesses = track_fn!(script_str, |script_str: String| {
        let c_script = std::ffi::CString::new(script_str.as_str()).unwrap();
        assert_eq!(posix_spawn_errno(&c_script), libc::ENOEXEC);
    })
    .await?;

    assert_contains(&accesses, &script_path, PathAccess::Read);

    Ok(())
}

/// Executing a FIFO must not block (opened with `O_NONBLOCK`) and must fail
/// with `EACCES`, matching the kernel's `execve`.
#[test(tokio::test)]
async fn exec_fifo_does_not_block_and_returns_eacces() -> anyhow::Result<()> {
    let tmp_dir = tempfile::TempDir::new()?;
    let fifo_path = tmp_dir.path().canonicalize()?.join("exec_fifo");
    let fifo_cstr = std::ffi::CString::new(fifo_path.to_str().unwrap())?;
    // SAFETY: `fifo_cstr` is a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo_cstr.as_ptr(), 0o755) }, 0);
    let fifo_str = fifo_path.to_str().unwrap().to_owned();

    let untraced_err = Command::new(&fifo_str).stdin(Stdio::null()).status().unwrap_err();
    assert_eq!(untraced_err.raw_os_error(), Some(libc::EACCES));

    let accesses = track_fn!(fifo_str, |fifo_str: String| {
        let err = Command::new(&fifo_str).stdin(Stdio::null()).status().unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EACCES));
    })
    .await?;

    assert_contains(&accesses, &fifo_path, PathAccess::Read);

    Ok(())
}
