//! Linux hardening tests covering `vfork`/`posix_spawn` memory reclamation,
//! `memfd_create` + `execveat(AT_EMPTY_PATH)`, and deep `execve` chain
//! FD/env/filter hygiene.
#![cfg(target_os = "linux")]

mod test_utils;

use std::path::Path;

use test_log::test;

use crate::test_utils::{assert_contains, test_bin_path};

#[test(tokio::test)]
async fn posix_spawn_parent_rss_stays_bounded() -> anyhow::Result<()> {
    let cmd = vt_subprocess_test::command_for_fn!((), |()| {
        use std::ffi::CString;

        fn vm_rss_kib() -> usize {
            let status = std::fs::read_to_string("/proc/self/status").unwrap();
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    return rest.split_whitespace().next().unwrap().parse::<usize>().unwrap();
                }
            }
            panic!("VmRSS not found in /proc/self/status");
        }

        let prog = CString::new("/bin/true").unwrap();
        let arg0 = CString::new("true").unwrap();
        let argv = [arg0.as_ptr().cast_mut(), std::ptr::null_mut()];

        let mut env_cstrs = Vec::new();
        for (k, v) in std::env::vars() {
            env_cstrs.push(CString::new(format!("{k}={v}")).unwrap());
        }
        let mut envp: Vec<*mut libc::c_char> =
            env_cstrs.iter().map(|s| s.as_ptr().cast_mut()).collect();
        envp.push(std::ptr::null_mut());

        let mut baseline_rss = 0usize;
        for i in 0..200 {
            let mut pid: libc::pid_t = 0;
            // SAFETY: `prog`, `argv`, and `envp` are valid NUL-terminated arrays.
            let rc = unsafe {
                libc::posix_spawn(
                    &raw mut pid,
                    prog.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    argv.as_ptr(),
                    envp.as_ptr(),
                )
            };
            assert_eq!(rc, 0, "posix_spawn iteration {i} failed with errno {rc}");
            let mut status: libc::c_int = 0;
            // SAFETY: `pid` is our child and `status` is valid writable storage.
            let waited = unsafe { libc::waitpid(pid, &raw mut status, 0) };
            assert_eq!(waited, pid);
            assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
            if i == 10 {
                baseline_rss = vm_rss_kib();
            }
        }

        let final_rss = vm_rss_kib();
        let growth = final_rss.saturating_sub(baseline_rss);
        assert!(
            growth < 4096,
            "parent VmRSS grew by {growth} KiB across 190 posix_spawns \
             (baseline={baseline_rss} KiB, final={final_rss} KiB)"
        );
    });

    let _accesses = test_utils::spawn_command(cmd).await?;
    Ok(())
}

#[test(tokio::test)]
async fn memfd_execveat_empty_path_tracks_child_accesses() -> anyhow::Result<()> {
    let probe_path = Path::new("/fspy-memfd-execveat-probe");
    let cmd = vt_subprocess_test::command_for_fn!(
        (test_bin_path().to_str().unwrap().to_owned(), probe_path.to_str().unwrap().to_owned()),
        |(static_bin, probe_path): (String, String)| {
            use std::ffi::CString;

            let bin_bytes = std::fs::read(&static_bin).unwrap();
            let name = CString::new("fspy-memfd-test").unwrap();
            // SAFETY: `memfd_create` takes a valid NUL-terminated name and flags.
            let fd = libc::c_int::try_from(unsafe {
                libc::syscall(libc::SYS_memfd_create, name.as_ptr(), libc::MFD_CLOEXEC)
            })
            .unwrap();
            assert!(fd >= 0, "memfd_create failed: {}", std::io::Error::last_os_error());

            let mut written = 0usize;
            while written < bin_bytes.len() {
                // SAFETY: `fd` is our open memfd and slice is valid.
                let n = unsafe {
                    libc::write(fd, bin_bytes[written..].as_ptr().cast(), bin_bytes.len() - written)
                };
                assert!(n > 0, "write memfd failed: {}", std::io::Error::last_os_error());
                written += usize::try_from(n).unwrap();
            }

            let empty = CString::new("").unwrap();
            let arg0 = CString::new("memfd-bin").unwrap();
            let arg1 = CString::new("open_read").unwrap();
            let arg2 = CString::new(probe_path).unwrap();
            let argv = [arg0.as_ptr(), arg1.as_ptr(), arg2.as_ptr(), std::ptr::null()];
            let envp = [std::ptr::null::<libc::c_char>()];

            // SAFETY: `fork()` followed by async-signal-safe `execveat` + `_exit`.
            let pid = unsafe { libc::fork() };
            assert!(pid >= 0, "fork failed");
            if pid == 0 {
                // SAFETY: `fd` is open and arguments are valid NUL-terminated arrays.
                unsafe {
                    libc::syscall(
                        libc::SYS_execveat,
                        fd,
                        empty.as_ptr(),
                        argv.as_ptr(),
                        envp.as_ptr(),
                        libc::AT_EMPTY_PATH,
                    );
                    libc::_exit(127);
                }
            }

            let mut status: libc::c_int = 0;
            // SAFETY: `pid` is our child and `status` is writable.
            let waited = unsafe { libc::waitpid(pid, &raw mut status, 0) };
            assert_eq!(waited, pid);
            assert!(
                libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
                "memfd execveat child failed with status {status}"
            );
            // SAFETY: closing our owned memfd.
            unsafe { libc::close(fd) };
        }
    );

    let accesses = test_utils::spawn_command(cmd).await?;
    assert_contains(&accesses, probe_path, fspy::PathAccess::Read);
    Ok(())
}

#[test(tokio::test)]
async fn deep_exec_chain_preserves_fd_env_and_seccomp_filter_hygiene() -> anyhow::Result<()> {
    // Capture any ambient non-CLOEXEC FDs inherited by the test runner itself
    // (e.g. CI runner or jobserver descriptors) so we strictly assert zero
    // leaked FDs introduced by `fspy` across the exec chain.
    let mut inherited_fds: Vec<i32> = std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .collect();
    inherited_fds.retain(|&fd| {
        if fd <= 2 {
            return false;
        }
        // SAFETY: `F_GETFD` only queries flags and returns `-1` for the
        // already-closed `read_dir("/proc/self/fd")` descriptor.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        flags >= 0 && (flags & libc::FD_CLOEXEC) == 0
    });
    inherited_fds.sort_unstable();

    let cmd = vt_subprocess_test::command_for_fn!(inherited_fds, |inherited_fds: Vec<i32>| {
        use std::{ffi::CString, os::unix::ffi::OsStrExt as _};

        fn seccomp_filters() -> usize {
            let status = std::fs::read_to_string("/proc/self/status").unwrap();
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("Seccomp_filters:") {
                    return rest.trim().parse::<usize>().unwrap();
                }
            }
            panic!("Seccomp_filters not found");
        }

        // Verify `__FSPY_BOOTSTRAP` was stripped before entering the target.
        assert!(
            std::env::var_os("__FSPY_BOOTSTRAP").is_none(),
            "__FSPY_BOOTSTRAP leaked into child environment"
        );

        let depth: usize =
            std::env::var("FSPY_TEST_EXEC_DEPTH").ok().and_then(|s| s.parse().ok()).unwrap_or(0);

        let cur_filters = seccomp_filters();
        assert!(cur_filters >= 1, "expected at least one seccomp filter");

        if let Ok(expected) = std::env::var("FSPY_TEST_EXPECTED_FILTERS") {
            let expected: usize = expected.parse().unwrap();
            assert_eq!(
                cur_filters, expected,
                "Seccomp_filters grew across exec chain ({expected} -> {cur_filters})"
            );
        }

        // Verify no FDs leaked from the launcher handoff or across execs
        // (ignoring stdin/stdout/stderr, the already-closed `/proc/self/fd`
        // directory handle, and ambient non-CLOEXEC FDs inherited from the
        // parent test runner).
        let open_fds: Vec<i32> = std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok())
            .collect();
        let leaked_fds: Vec<i32> = open_fds
            .into_iter()
            .filter(|&fd| fd > 2 && !inherited_fds.contains(&fd))
            .filter(|&fd| {
                // SAFETY: `F_GETFD` only queries descriptor flags and returns
                // `-1` for the now-closed `read_dir("/proc/self/fd")` handle.
                unsafe { libc::fcntl(fd, libc::F_GETFD) >= 0 }
            })
            .collect();
        assert!(
            leaked_fds.is_empty(),
            "unexpected leaked open FDs at depth {depth} (inherited={inherited_fds:?}): {leaked_fds:?}"
        );

        if depth >= 5 {
            return;
        }

        let exe = std::fs::read_link("/proc/self/exe").unwrap();
        let prog = CString::new(exe.as_os_str().as_bytes()).unwrap();
        // `vt_subprocess_test` runs inside `.init_array`, where `std::env::args_os()`
        // is empty on musl; fall back to `/proc/self/cmdline` just like `vt_subprocess_test`.
        let mut args: Vec<CString> =
            std::env::args_os().map(|a| CString::new(a.as_bytes()).unwrap()).collect();
        if args.is_empty() {
            let cmdline = std::fs::read("/proc/self/cmdline").unwrap();
            let mut parts: Vec<&[u8]> = cmdline.split(|&b| b == 0).collect();
            if parts.last().is_some_and(|s| s.is_empty()) {
                parts.pop();
            }
            args = parts.into_iter().map(|p| CString::new(p).unwrap()).collect();
        }
        let mut argv: Vec<*const libc::c_char> = args.iter().map(|s| s.as_ptr()).collect();
        argv.push(std::ptr::null());

        let mut env_cstrs: Vec<CString> = std::env::vars_os()
            .filter(|(k, _)| k != "FSPY_TEST_EXEC_DEPTH" && k != "FSPY_TEST_EXPECTED_FILTERS")
            .map(|(k, v)| {
                let mut bytes = k.as_bytes().to_vec();
                bytes.push(b'=');
                bytes.extend_from_slice(v.as_bytes());
                CString::new(bytes).unwrap()
            })
            .collect();
        env_cstrs.push(CString::new(format!("FSPY_TEST_EXEC_DEPTH={}", depth + 1)).unwrap());
        env_cstrs.push(CString::new(format!("FSPY_TEST_EXPECTED_FILTERS={cur_filters}")).unwrap());
        let mut envp: Vec<*const libc::c_char> = env_cstrs.iter().map(|s| s.as_ptr()).collect();
        envp.push(std::ptr::null());

        // SAFETY: `prog`, `argv`, and `envp` are valid NUL-terminated arrays.
        unsafe { libc::execve(prog.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        panic!("execve failed at depth {depth}: {}", std::io::Error::last_os_error());
    });

    let _accesses = test_utils::spawn_command(cmd).await?;
    Ok(())
}
