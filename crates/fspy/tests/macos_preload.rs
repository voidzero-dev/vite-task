#![cfg(target_os = "macos")]
mod test_utils;

use std::{
    ffi::CString,
    fs::{self, Permissions},
    os::unix::fs::PermissionsExt as _,
    path::Path,
    ptr::null,
};

use fspy::PathAccess;
use test_log::test;
use test_utils::{assert_contains, assert_not_contains};

fn write_executable(path: &Path, content: &str) -> anyhow::Result<()> {
    fs::write(path, content)?;
    fs::set_permissions(path, Permissions::from_mode(0o755))?;
    Ok(())
}

/// Runs `f` in a forked child process and asserts it exits cleanly with code 0.
fn run_in_fork(f: impl FnOnce()) {
    // SAFETY: `fork()` is followed immediately by `exec*` or `_exit`.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed: {}", std::io::Error::last_os_error());
    if pid == 0 {
        f();
        // SAFETY: terminate child immediately without running parent destructors.
        unsafe { libc::_exit(127) };
    }
    let mut status: libc::c_int = 0;
    // SAFETY: `pid` is our child and `status` is valid writable storage.
    let waited = unsafe { libc::waitpid(pid, &raw mut status, 0) };
    assert_eq!(waited, pid);
    assert!(
        libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
        "child failed with raw status {status:#x}"
    );
}

#[test(tokio::test)]
async fn exec_family_variants_succeed_and_preserve_tracking() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let root = tmp_dir.path().canonicalize()?;
    let bin_dir = root.join("bin");
    fs::create_dir(&bin_dir)?;

    let execve_marker = root.join("marker_execve");
    let execv_marker = root.join("marker_execv");
    let execvp_marker = root.join("marker_execvp");
    let execlp_marker = root.join("marker_execlp");
    let execle_marker = root.join("marker_execle");
    for marker in [&execve_marker, &execv_marker, &execvp_marker, &execlp_marker, &execle_marker] {
        fs::write(marker, "ok")?;
    }

    let script_execve = root.join("s_execve.sh");
    write_executable(
        &script_execve,
        &format!(
            "#!/bin/sh\n[ \"$CUSTOM_ENV\" = \"from_execve\" ] || exit 2\ncat \"{}\" >/dev/null\n",
            execve_marker.display()
        ),
    )?;
    let script_execv = root.join("s_execv.sh");
    write_executable(
        &script_execv,
        &format!("#!/bin/sh\ncat \"{}\" >/dev/null\n", execv_marker.display()),
    )?;
    let script_execvp = bin_dir.join("fspy_test_execvp_cmd");
    write_executable(
        &script_execvp,
        &format!("#!/bin/sh\ncat \"{}\" >/dev/null\n", execvp_marker.display()),
    )?;
    let script_execlp = bin_dir.join("fspy_test_execlp_cmd");
    write_executable(
        &script_execlp,
        &format!("#!/bin/sh\ncat \"{}\" >/dev/null\n", execlp_marker.display()),
    )?;
    let script_execle = root.join("s_execle.sh");
    write_executable(
        &script_execle,
        &format!(
            "#!/bin/sh\n[ \"$CUSTOM_ENV\" = \"from_execle\" ] || exit 2\ncat \"{}\" >/dev/null\n",
            execle_marker.display()
        ),
    )?;

    let args = (
        script_execve.to_str().unwrap().to_owned(),
        script_execv.to_str().unwrap().to_owned(),
        script_execle.to_str().unwrap().to_owned(),
        bin_dir.to_str().unwrap().to_owned(),
    );

    let accesses = track_fn!(args, |(script_execve, script_execv, script_execle, bin_dir): (
        String,
        String,
        String,
        String
    )| {
        // 1. execve with custom envp (fspy must inject tracking env into it)
        run_in_fork(|| {
            let prog = CString::new(script_execve.as_str()).unwrap();
            let argv = [prog.as_ptr(), null()];
            let env0 = CString::new("CUSTOM_ENV=from_execve").unwrap();
            let env1 = CString::new("PATH=/bin:/usr/bin").unwrap();
            let envp = [env0.as_ptr(), env1.as_ptr(), null()];
            // SAFETY: all pointers are valid NUL-terminated strings / arrays.
            unsafe { libc::execve(prog.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        });

        // 2. execv
        run_in_fork(|| {
            let prog = CString::new(script_execv.as_str()).unwrap();
            let argv = [prog.as_ptr(), null()];
            // SAFETY: `prog` and `argv` are valid.
            unsafe { libc::execv(prog.as_ptr(), argv.as_ptr()) };
        });

        // 3. execvp (searches PATH)
        run_in_fork(|| {
            let new_path = format!("{bin_dir}:/bin:/usr/bin");
            // SAFETY: running in single-threaded forked child.
            unsafe { std::env::set_var("PATH", new_path) };
            let prog = c"fspy_test_execvp_cmd";
            let argv = [prog.as_ptr(), null()];
            // SAFETY: `prog` and `argv` are valid.
            unsafe { libc::execvp(prog.as_ptr(), argv.as_ptr()) };
        });

        // 4. execlp (searches PATH, variadic argv)
        run_in_fork(|| {
            let new_path = format!("{bin_dir}:/bin:/usr/bin");
            // SAFETY: running in single-threaded forked child.
            unsafe { std::env::set_var("PATH", new_path) };
            let prog = c"fspy_test_execlp_cmd";
            // SAFETY: variadic args terminated by NULL.
            unsafe { libc::execlp(prog.as_ptr(), prog.as_ptr(), null::<libc::c_char>()) };
        });

        // 5. execle (variadic argv + trailing envp)
        run_in_fork(|| {
            let prog = CString::new(script_execle.as_str()).unwrap();
            let env0 = CString::new("CUSTOM_ENV=from_execle").unwrap();
            let env1 = CString::new("PATH=/bin:/usr/bin").unwrap();
            let envp = [env0.as_ptr(), env1.as_ptr(), null()];
            // SAFETY: variadic args terminated by NULL followed by valid `envp`.
            unsafe {
                libc::execle(prog.as_ptr(), prog.as_ptr(), null::<libc::c_char>(), envp.as_ptr())
            };
        });
    })
    .await?;

    for marker in [&execve_marker, &execv_marker, &execvp_marker, &execlp_marker, &execle_marker] {
        assert_contains(&accesses, marker, PathAccess::Read);
    }

    Ok(())
}

#[test(tokio::test)]
async fn which_path_search_handles_unset_empty_and_directory_entries() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let root = tmp_dir.path().canonicalize()?;

    let dir_with_subdir = root.join("dir_with_subdir");
    let dir_with_real = root.join("dir_with_real");
    let empty_entry_cwd = root.join("empty_entry_cwd");
    fs::create_dir(&dir_with_subdir)?;
    fs::create_dir(&dir_with_real)?;
    fs::create_dir(&empty_entry_cwd)?;

    // Place a directory named `same_name` in the first PATH entry, and an executable
    // script with the same name in the second PATH entry.
    fs::create_dir(dir_with_subdir.join("same_name"))?;
    let skipped_marker = root.join("marker_dir_skip");
    fs::write(&skipped_marker, "ok")?;
    write_executable(
        &dir_with_real.join("same_name"),
        &format!("#!/bin/sh\ncat \"{}\" >/dev/null\n", skipped_marker.display()),
    )?;

    // Place an executable in `empty_entry_cwd` to be found via an empty PATH component.
    let empty_marker = root.join("marker_empty_path");
    fs::write(&empty_marker, "ok")?;
    write_executable(
        &empty_entry_cwd.join("local_cmd"),
        &format!("#!/bin/sh\ncat \"{}\" >/dev/null\n", empty_marker.display()),
    )?;

    let args = (
        dir_with_subdir.to_str().unwrap().to_owned(),
        dir_with_real.to_str().unwrap().to_owned(),
        empty_entry_cwd.to_str().unwrap().to_owned(),
    );

    let accesses = track_fn!(args, |(dir_with_subdir, dir_with_real, empty_entry_cwd): (
        String,
        String,
        String
    )| {
        // 1. Directory on PATH is skipped in favor of a later regular executable.
        run_in_fork(|| {
            let path = format!("{dir_with_subdir}:{dir_with_real}:/bin:/usr/bin");
            // SAFETY: single-threaded forked child.
            unsafe { std::env::set_var("PATH", path) };
            let prog = c"same_name";
            let argv = [prog.as_ptr(), null()];
            // SAFETY: `prog` and `argv` are valid.
            unsafe { libc::execvp(prog.as_ptr(), argv.as_ptr()) };
        });

        // 2. Directory on PATH with no later match fails with EACCES.
        run_in_fork(|| {
            // SAFETY: single-threaded forked child.
            unsafe { std::env::set_var("PATH", &dir_with_subdir) };
            let prog = c"same_name";
            let argv = [prog.as_ptr(), null()];
            // SAFETY: `prog` and `argv` are valid.
            let ret = unsafe { libc::execvp(prog.as_ptr(), argv.as_ptr()) };
            assert_eq!(ret, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EACCES),
                "directory-only PATH match must fail with EACCES"
            );
            // SAFETY: exit child cleanly after checking errno.
            unsafe { libc::_exit(0) };
        });

        // 3. Unset PATH falls back to default PATH (`/usr/local/bin:/bin:/usr/bin`).
        run_in_fork(|| {
            // SAFETY: single-threaded forked child.
            unsafe { std::env::remove_var("PATH") };
            let prog = c"true";
            let argv = [prog.as_ptr(), null()];
            // SAFETY: `prog` and `argv` are valid.
            unsafe { libc::execvp(prog.as_ptr(), argv.as_ptr()) };
        });

        // 4. Empty PATH component searches the current working directory.
        run_in_fork(|| {
            std::env::set_current_dir(&empty_entry_cwd).unwrap();
            // Leading colon creates an empty first PATH entry (= CWD).
            // SAFETY: single-threaded forked child.
            unsafe { std::env::set_var("PATH", ":/bin:/usr/bin") };
            let prog = c"local_cmd";
            let argv = [prog.as_ptr(), null()];
            // SAFETY: `prog` and `argv` are valid.
            unsafe { libc::execvp(prog.as_ptr(), argv.as_ptr()) };
        });
    })
    .await?;

    assert_contains(&accesses, &skipped_marker, PathAccess::Read);
    assert_contains(&accesses, &empty_marker, PathAccess::Read);

    Ok(())
}

#[test(tokio::test)]
async fn sip_stripping_and_dyld_insert_libraries_merging() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let root = tmp_dir.path().canonicalize()?;
    let sed_input = root.join("input.txt");
    fs::write(&sed_input, "hello world\n")?;
    let awk_link = root.join("awk_link");
    std::os::unix::fs::symlink("/usr/bin/awk", &awk_link)?;

    let args = (sed_input.to_str().unwrap().to_owned(), awk_link.to_str().unwrap().to_owned());

    let accesses = track_fn!(args, |(sed_input, awk_link): (String, String)| {
        // 1. Non-injectable SIP binary (`/usr/bin/sed`) executes cleanly.
        let out = std::process::Command::new("/usr/bin/sed")
            .args(["s/world/fspy/", &sed_input])
            .output()
            .unwrap();
        assert!(out.status.success(), "sed failed: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(out.stdout, b"hello fspy\n");

        // 2. Non-injectable SIP binary invoked via symlink strips FSPY_PAYLOAD.
        let out = std::process::Command::new(&awk_link)
            .arg("BEGIN { printf \"%s\", ENVIRON[\"FSPY_PAYLOAD\"] }")
            .output()
            .unwrap();
        assert!(out.status.success(), "awk failed: {}", String::from_utf8_lossy(&out.stderr));
        assert!(
            out.stdout.is_empty(),
            "FSPY_PAYLOAD must be stripped for symlinked SIP binary, got {:?}",
            String::from_utf8_lossy(&out.stdout)
        );

        // 3. Pre-existing DYLD_INSERT_LIBRARIES on an injectable binary is preserved
        //    with fspy's preload appended rather than clobbered.
        let user_lib = "/usr/lib/libSystem.B.dylib";
        let out = std::process::Command::new("/bin/sh")
            .env("DYLD_INSERT_LIBRARIES", user_lib)
            .args(["-c", "printf '%s' \"$DYLD_INSERT_LIBRARIES\""])
            .output()
            .unwrap();
        assert!(out.status.success(), "sh failed: {}", String::from_utf8_lossy(&out.stderr));
        let dyld = String::from_utf8(out.stdout).unwrap();
        let parts: Vec<&str> = dyld.split(':').collect();
        assert!(
            parts.len() >= 2
                && parts[0] == user_lib
                && Path::new(parts.last().unwrap())
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("dylib")),
            "expected user preload preserved before fspy preload, got {dyld:?}"
        );
    })
    .await?;

    assert_contains(&accesses, Path::new("/usr/bin/sed"), PathAccess::Read);
    assert_contains(&accesses, &awk_link, PathAccess::Read);

    Ok(())
}

#[test(tokio::test)]
async fn fopen_and_freopen_plus_modes_record_read_write() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let root = tmp_dir.path().canonicalize()?;

    let p_r = root.join("mode_r");
    let p_w = root.join("mode_w");
    let p_a = root.join("mode_a");
    let p_rplus = root.join("mode_rplus");
    let p_wplus = root.join("mode_wplus");
    let p_aplus = root.join("mode_aplus");
    let p_freopen = root.join("mode_freopen_rplus");
    fs::write(&p_r, "r")?;
    fs::write(&p_rplus, "r+")?;
    fs::write(&p_freopen, "freopen")?;

    let root_str = root.to_str().unwrap().to_owned();
    let accesses = track_fn!(root_str, |root_str: String| {
        let open_and_close = |name: &str, mode: &core::ffi::CStr| {
            let p = CString::new(format!("{root_str}/{name}")).unwrap();
            // SAFETY: `p` and `mode` are valid NUL-terminated C strings.
            let fp = unsafe { libc::fopen(p.as_ptr(), mode.as_ptr()) };
            assert!(!fp.is_null(), "fopen({name}, {mode:?}) failed");
            // SAFETY: `fp` was returned by `fopen` above.
            unsafe { libc::fclose(fp) };
        };

        open_and_close("mode_r", c"r");
        open_and_close("mode_w", c"w");
        open_and_close("mode_a", c"a");
        open_and_close("mode_rplus", c"r+");
        open_and_close("mode_wplus", c"w+");
        open_and_close("mode_aplus", c"a+");

        let p_tmp = CString::new(format!("{root_str}/mode_r")).unwrap();
        let p_re = CString::new(format!("{root_str}/mode_freopen_rplus")).unwrap();
        // SAFETY: valid C strings and FILE stream from `fopen`.
        unsafe {
            let fp = libc::fopen(p_tmp.as_ptr(), c"r".as_ptr());
            assert!(!fp.is_null());
            let fp2 = libc::freopen(p_re.as_ptr(), c"r+".as_ptr(), fp);
            assert!(!fp2.is_null());
            libc::fclose(fp2);
        }
    })
    .await?;

    assert_contains(&accesses, &p_r, PathAccess::Read);
    assert_contains(&accesses, &p_w, PathAccess::Write);
    assert_contains(&accesses, &p_a, PathAccess::Write);
    assert_contains(&accesses, &p_rplus, PathAccess::ReadWrite);
    assert_contains(&accesses, &p_wplus, PathAccess::ReadWrite);
    assert_contains(&accesses, &p_aplus, PathAccess::ReadWrite);
    assert_contains(&accesses, &p_freopen, PathAccess::ReadWrite);

    Ok(())
}

#[test(tokio::test)]
async fn empty_path_calls_do_not_record_cwd_or_dirfd() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let root = tmp_dir.path().canonicalize()?;
    let sub_dir = root.join("sub");
    fs::create_dir(&sub_dir)?;

    let args = (root.to_str().unwrap().to_owned(), sub_dir.to_str().unwrap().to_owned());
    let accesses = track_fn!(args, |(root_str, sub_str): (String, String)| {
        std::env::set_current_dir(&root_str).unwrap();
        let sub_c = CString::new(sub_str).unwrap();
        // SAFETY: `sub_c` and `c""` are valid NUL-terminated C strings.
        unsafe {
            assert_eq!(libc::open(c"".as_ptr(), libc::O_RDONLY), -1);
            let mut st = std::mem::MaybeUninit::<libc::stat>::zeroed();
            assert_eq!(libc::stat(c"".as_ptr(), st.as_mut_ptr()), -1);
            assert_eq!(libc::lstat(c"".as_ptr(), st.as_mut_ptr()), -1);
            assert_eq!(libc::access(c"".as_ptr(), libc::F_OK), -1);

            let dirfd = libc::open(sub_c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY);
            assert!(dirfd >= 0);
            assert_eq!(libc::openat(dirfd, c"".as_ptr(), libc::O_RDONLY), -1);
            assert_eq!(libc::fstatat(dirfd, c"".as_ptr(), st.as_mut_ptr(), 0), -1);
            libc::close(dirfd);
        }
    })
    .await?;

    assert_not_contains(&accesses, &root);
    // `sub_dir` itself was opened with `O_RDONLY`, not written or re-recorded via `openat("", ...)`.
    assert_contains(&accesses, &sub_dir, PathAccess::Read);

    Ok(())
}

#[test(tokio::test)]
async fn open_rdonly_with_creat_or_trunc_and_rename_swap() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let root = tmp_dir.path().canonicalize()?;

    let p_creat = root.join("rdonly_creat");
    let p_trunc = root.join("rdonly_trunc");
    fs::write(&p_trunc, "content")?;
    let swap_a = root.join("swap_a");
    let swap_b = root.join("swap_b");
    fs::write(&swap_a, "aaa")?;
    fs::write(&swap_b, "bbb")?;

    let root_str = root.to_str().unwrap().to_owned();
    let accesses = track_fn!(root_str, |root_str: String| {
        let c_creat = CString::new(format!("{root_str}/rdonly_creat")).unwrap();
        let c_trunc = CString::new(format!("{root_str}/rdonly_trunc")).unwrap();
        let c_a = CString::new(format!("{root_str}/swap_a")).unwrap();
        let c_b = CString::new(format!("{root_str}/swap_b")).unwrap();
        // SAFETY: all paths are valid NUL-terminated C strings.
        unsafe {
            let fd = libc::open(c_creat.as_ptr(), libc::O_RDONLY | libc::O_CREAT, 0o644);
            assert!(fd >= 0, "open(O_RDONLY|O_CREAT) failed: {}", std::io::Error::last_os_error());
            libc::close(fd);

            let fd = libc::open(c_trunc.as_ptr(), libc::O_RDONLY | libc::O_TRUNC);
            if fd >= 0 {
                libc::close(fd);
            }

            assert_eq!(
                libc::renamex_np(c_a.as_ptr(), c_b.as_ptr(), libc::RENAME_SWAP),
                0,
                "renamex_np(RENAME_SWAP) failed: {}",
                std::io::Error::last_os_error()
            );
        }
    })
    .await?;

    assert_contains(&accesses, &p_creat, PathAccess::ReadWrite);
    assert_contains(&accesses, &p_trunc, PathAccess::ReadWrite);
    assert_contains(&accesses, &swap_a, PathAccess::RenameTarget);
    assert_contains(&accesses, &swap_b, PathAccess::RenameTarget);

    Ok(())
}

#[test(tokio::test)]
async fn posix_spawn_addchdir_np_resolves_relative_program() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let root = tmp_dir.path().canonicalize()?;
    let sub = root.join("child_cwd");
    fs::create_dir(&sub)?;
    let marker = sub.join("marker_chdir");
    fs::write(&marker, "ok")?;
    let script = sub.join("run.sh");
    write_executable(&script, "#!/bin/sh\ncat marker_chdir >/dev/null\n")?;

    let sub_str = sub.to_str().unwrap().to_owned();
    let accesses = track_fn!(sub_str, |sub_str: String| {
        // `std::process::Command::current_dir` uses `posix_spawn_file_actions_addchdir_np`
        // on macOS.
        let status = std::process::Command::new("./run.sh")
            .current_dir(&sub_str)
            .status()
            .expect("posix_spawn with addchdir_np failed");
        assert!(status.success());
    })
    .await?;

    assert_contains(&accesses, &script, PathAccess::Read);
    assert_contains(&accesses, &marker, PathAccess::Read);

    Ok(())
}
