//! Linux hardening tests covering `vfork`/`posix_spawn` memory reclamation,
//! `memfd_create` + `execveat(AT_EMPTY_PATH)`, `execve("/proc/self/fd/N")`,
//! and deep `execve` chain FD/env/filter hygiene.
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
async fn execve_proc_self_fd_of_cloexec_descriptor_runs_and_tracks() -> anyhow::Result<()> {
    // `execve("/proc/self/fd/N")` on an `O_CLOEXEC` descriptor (glibc's
    // `fexecve` fallback, memfd launchers): the kernel opens the image before
    // closing CLOEXEC descriptors, so the launcher must hand the image over by
    // descriptor rather than reopen the path after exec.
    let probe_path = Path::new("/fspy-proc-self-fd-exec-probe");
    let cmd = vt_subprocess_test::command_for_fn!(
        (test_bin_path().to_str().unwrap().to_owned(), probe_path.to_str().unwrap().to_owned()),
        |(static_bin, probe_path): (String, String)| {
            use std::{ffi::CString, os::fd::AsRawFd as _};

            // Rust opens files with `O_CLOEXEC`.
            let file = std::fs::File::open(&static_bin).unwrap();
            let path = CString::new(format!("/proc/self/fd/{}", file.as_raw_fd())).unwrap();
            let arg0 = CString::new("fd-bin").unwrap();
            let arg1 = CString::new("open_read").unwrap();
            let arg2 = CString::new(probe_path).unwrap();
            let argv = [arg0.as_ptr(), arg1.as_ptr(), arg2.as_ptr(), std::ptr::null()];
            let envp = [std::ptr::null::<libc::c_char>()];

            // SAFETY: `fork()` followed by async-signal-safe `execve` + `_exit`.
            let pid = unsafe { libc::fork() };
            assert!(pid >= 0, "fork failed");
            if pid == 0 {
                // SAFETY: `path`, `argv`, and `envp` are valid NUL-terminated arrays.
                unsafe {
                    libc::execve(path.as_ptr(), argv.as_ptr(), envp.as_ptr());
                    libc::_exit(127);
                }
            }

            let mut status: libc::c_int = 0;
            // SAFETY: `pid` is our child and `status` is writable.
            let waited = unsafe { libc::waitpid(pid, &raw mut status, 0) };
            assert_eq!(waited, pid);
            assert!(
                libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
                "/proc/self/fd execve child failed with status {status}"
            );
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

#[test(tokio::test)]
async fn e2big_on_execve_and_posix_spawn_cleans_up_scratch_without_leaking() -> anyhow::Result<()> {
    let cmd = vt_subprocess_test::command_for_fn!((), |()| {
        use std::ffi::CString;

        fn count_open_fds() -> usize {
            std::fs::read_dir("/proc/self/fd")
                .unwrap()
                .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok())
                .filter(|&fd| {
                    // SAFETY: `F_GETFD` only queries flags and returns `-1` for
                    // the already-closed `read_dir` descriptor.
                    unsafe { libc::fcntl(fd, libc::F_GETFD) >= 0 }
                })
                .count()
        }

        let prog = CString::new("/bin/true").unwrap();
        let arg0 = CString::new("true").unwrap();
        let argv = [arg0.as_ptr(), std::ptr::null()];

        // Exceeds Linux `MAX_ARG_STRLEN` (32 * PAGE_SIZE) on both 4 KiB and
        // 64 KiB page kernels so `SYS_execve` fails cleanly with `E2BIG`.
        let mut big = Vec::with_capacity(4 + 3 * 1024 * 1024 + 1);
        big.extend_from_slice(b"BIG=");
        big.resize(4 + 3 * 1024 * 1024, b'x');
        let big_cstr = CString::new(big).unwrap();
        let huge_envp = [big_cstr.as_ptr(), std::ptr::null()];

        let fds_before = count_open_fds();

        // 1. Direct `execve` returns `-1` / `E2BIG` and runs `cleanup_exec_scratch`.
        // SAFETY: `prog`, `argv`, and `huge_envp` are valid NUL/NULL-terminated arrays.
        let rc = unsafe { libc::execve(prog.as_ptr(), argv.as_ptr(), huge_envp.as_ptr()) };
        assert_eq!(rc, -1, "oversized execve unexpectedly succeeded");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::E2BIG),
            "expected E2BIG from oversized execve"
        );
        assert_eq!(count_open_fds(), fds_before, "execve E2BIG leaked an open FD");

        // 2. `posix_spawn` (`CLONE_VM | CLONE_VFORK`) returns `E2BIG` cleanly
        //    and releases its parked `ExecScratch` and duped FD without crashing.
        let mut pid: libc::pid_t = 0;
        let argv_mut = [arg0.as_ptr().cast_mut(), std::ptr::null_mut()];
        let huge_envp_mut = [big_cstr.as_ptr().cast_mut(), std::ptr::null_mut()];
        // SAFETY: `prog`, `argv_mut`, and `huge_envp_mut` are valid arrays.
        let spawn_rc = unsafe {
            libc::posix_spawn(
                &raw mut pid,
                prog.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                argv_mut.as_ptr(),
                huge_envp_mut.as_ptr(),
            )
        };
        assert_eq!(spawn_rc, libc::E2BIG, "expected E2BIG from oversized posix_spawn");
        assert_eq!(count_open_fds(), fds_before, "posix_spawn E2BIG leaked an open FD");

        // 3. Process remains healthy: a normal `posix_spawn` still succeeds.
        let empty_envp = [std::ptr::null_mut::<libc::c_char>()];
        let mut ok_pid: libc::pid_t = 0;
        // SAFETY: `prog`, `argv_mut`, and `empty_envp` are valid arrays.
        let ok_rc = unsafe {
            libc::posix_spawn(
                &raw mut ok_pid,
                prog.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                argv_mut.as_ptr(),
                empty_envp.as_ptr(),
            )
        };
        assert_eq!(ok_rc, 0, "follow-up posix_spawn failed with errno {ok_rc}");
        let mut status: libc::c_int = 0;
        // SAFETY: `ok_pid` is our child and `status` is writable.
        assert_eq!(unsafe { libc::waitpid(ok_pid, &raw mut status, 0) }, ok_pid);
        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
    });

    let _accesses = test_utils::spawn_command(cmd).await?;
    Ok(())
}

#[test(tokio::test)]
async fn inherited_sigsys_ign_and_blocked_mask_survive_execve() -> anyhow::Result<()> {
    let probe_path = Path::new("/fspy-inherited-sigsys-probe");
    let cmd = vt_subprocess_test::command_for_fn!(
        probe_path.to_str().unwrap().to_owned(),
        |probe_path: String| {
            use std::{ffi::CString, os::unix::ffi::OsStrExt as _};

            if std::env::var_os("FSPY_TEST_SIGSYS_STAGE").is_some() {
                // Verify `SIGSYS` is still virtually `SIG_IGN`.
                // SAFETY: `libc::sigaction` is a plain C struct valid when zeroed.
                let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
                // SAFETY: querying current disposition for `SIGSYS`.
                let rc = unsafe { libc::sigaction(libc::SIGSYS, std::ptr::null(), &raw mut old) };
                assert_eq!(rc, 0);
                assert_eq!(
                    old.sa_sigaction,
                    libc::SIG_IGN,
                    "inherited SIGSYS SIG_IGN was not preserved across execve"
                );

                // Verify both `SIGSYS` and `SIGUSR1` remain blocked in the signal mask.
                // SAFETY: `sigset_t` is a plain C struct valid when zeroed.
                let mut cur_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
                // SAFETY: querying current thread signal mask.
                let mask_rc = unsafe {
                    libc::sigprocmask(libc::SIG_SETMASK, std::ptr::null(), &raw mut cur_mask)
                };
                assert_eq!(mask_rc, 0);
                // SAFETY: `cur_mask` was initialized by `sigprocmask`.
                let sigsys_blocked =
                    unsafe { libc::sigismember(&raw const cur_mask, libc::SIGSYS) };
                assert_eq!(sigsys_blocked, 1, "inherited blocked SIGSYS was lost across execve");
                // SAFETY: `cur_mask` was initialized by `sigprocmask`.
                let sigusr1_blocked =
                    unsafe { libc::sigismember(&raw const cur_mask, libc::SIGUSR1) };
                assert_eq!(sigusr1_blocked, 1, "inherited blocked SIGUSR1 was lost across execve");

                // Verify syscall tracking still works with virtualized SIGSYS ignored+blocked.
                let _ = std::fs::File::open(&probe_path);
                return;
            }

            // Stage 0: set `SIGSYS` to `SIG_IGN` and block `SIGSYS` + `SIGUSR1`.
            // SAFETY: `libc::sigaction` is a plain C struct valid when zeroed.
            let mut ign: libc::sigaction = unsafe { std::mem::zeroed() };
            ign.sa_sigaction = libc::SIG_IGN;
            // SAFETY: installing `SIG_IGN` for `SIGSYS`.
            let ign_rc =
                unsafe { libc::sigaction(libc::SIGSYS, &raw const ign, std::ptr::null_mut()) };
            assert_eq!(ign_rc, 0);

            // SAFETY: `sigset_t` is initialized via `sigemptyset`/`sigaddset`.
            let mut block_set: libc::sigset_t = unsafe { std::mem::zeroed() };
            // SAFETY: valid stack-local `sigset_t`.
            unsafe {
                libc::sigemptyset(&raw mut block_set);
                libc::sigaddset(&raw mut block_set, libc::SIGSYS);
                libc::sigaddset(&raw mut block_set, libc::SIGUSR1);
                assert_eq!(
                    libc::sigprocmask(libc::SIG_BLOCK, &raw const block_set, std::ptr::null_mut()),
                    0
                );
            }

            let exe = std::fs::read_link("/proc/self/exe").unwrap();
            let prog = CString::new(exe.as_os_str().as_bytes()).unwrap();
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
                .map(|(k, v)| {
                    let mut bytes = k.as_bytes().to_vec();
                    bytes.push(b'=');
                    bytes.extend_from_slice(v.as_bytes());
                    CString::new(bytes).unwrap()
                })
                .collect();
            env_cstrs.push(CString::new("FSPY_TEST_SIGSYS_STAGE=1").unwrap());
            let mut envp: Vec<*const libc::c_char> = env_cstrs.iter().map(|s| s.as_ptr()).collect();
            envp.push(std::ptr::null());

            // SAFETY: `prog`, `argv`, and `envp` are valid NUL/NULL-terminated arrays.
            unsafe { libc::execve(prog.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
            panic!("execve failed: {}", std::io::Error::last_os_error());
        }
    );

    let accesses = test_utils::spawn_command(cmd).await?;
    assert_contains(&accesses, probe_path, fspy::PathAccess::Read);
    Ok(())
}

#[test(tokio::test)]
async fn proc_self_environ_does_not_expose_fspy_bootstrap() -> anyhow::Result<()> {
    let cmd = vt_subprocess_test::command_for_fn!((), |()| {
        fn assert_bootstrap_scrubbed(label: &str) {
            assert!(
                std::env::var_os("__FSPY_BOOTSTRAP").is_none(),
                "{label}: __FSPY_BOOTSTRAP present in process env"
            );
            let raw = std::fs::read("/proc/self/environ").unwrap();
            let needle = b"__FSPY_BOOTSTRAP";
            assert!(
                !raw.windows(needle.len()).any(|w| w == needle),
                "{label}: __FSPY_BOOTSTRAP found in /proc/self/environ"
            );
        }

        // Check initial spawn (`Target::Unresolved`).
        assert_bootstrap_scrubbed("initial spawn");

        // Also check a child spawned via `execve` (`Target::ResolvedFd`).
        let output = std::process::Command::new("/bin/cat")
            .arg("/proc/self/environ")
            .output()
            .expect("failed to run /bin/cat /proc/self/environ");
        assert!(output.status.success());
        let needle = b"__FSPY_BOOTSTRAP";
        assert!(
            !output.stdout.windows(needle.len()).any(|w| w == needle),
            "child execve: __FSPY_BOOTSTRAP found in /proc/self/environ"
        );
    });

    let _accesses = test_utils::spawn_command(cmd).await?;
    Ok(())
}

#[test(tokio::test)]
async fn child_execve_with_stale_bootstrap_env_succeeds() -> anyhow::Result<()> {
    let probe_path = Path::new("/fspy-stale-bootstrap-probe");
    let cmd = vt_subprocess_test::command_for_fn!(
        (test_bin_path().to_str().unwrap().to_owned(), probe_path.to_str().unwrap().to_owned()),
        |(static_bin, probe_path): (String, String)| {
            use std::ffi::CString;

            // 1. Exec static test binary with duplicate stale `__FSPY_BOOTSTRAP`
            //    entries in `envp` — must overwrite/deduplicate cleanly instead
            //    of failing with `EINVAL`.
            let prog = CString::new(static_bin).unwrap();
            let arg0 = CString::new("test_bin").unwrap();
            let arg1 = CString::new("open_read").unwrap();
            let arg2 = CString::new(probe_path).unwrap();
            let argv = [arg0.as_ptr(), arg1.as_ptr(), arg2.as_ptr(), std::ptr::null()];
            let envp = [
                c"__FSPY_BOOTSTRAP=stale".as_ptr(),
                c"__FSPY_BOOTSTRAP=stale2".as_ptr(),
                c"KEEP=yes".as_ptr(),
                std::ptr::null(),
            ];

            // SAFETY: `fork()` followed by async-signal-safe `execve` + `_exit`.
            let pid = unsafe { libc::fork() };
            assert!(pid >= 0, "fork failed");
            if pid == 0 {
                // SAFETY: `prog`, `argv`, and `envp` are valid arrays.
                unsafe {
                    libc::execve(prog.as_ptr(), argv.as_ptr(), envp.as_ptr());
                    libc::_exit(127);
                }
            }
            let mut status: libc::c_int = 0;
            // SAFETY: `pid` is our child and `status` is writable.
            assert_eq!(unsafe { libc::waitpid(pid, &raw mut status, 0) }, pid);
            assert!(
                libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
                "child execve with stale __FSPY_BOOTSTRAP failed with status {status}"
            );

            // 2. Exec `/bin/sh` with stale `__FSPY_BOOTSTRAP=stale` and verify
            //    the child sees `KEEP=yes` and no `__FSPY_BOOTSTRAP`.
            let sh_argv = [
                c"/bin/sh".as_ptr(),
                c"-c".as_ptr(),
                c"test -z \"${__FSPY_BOOTSTRAP+x}\" && test \"$KEEP\" = \"yes\"".as_ptr(),
                std::ptr::null(),
            ];
            // SAFETY: `fork()` followed by async-signal-safe `execve` + `_exit`.
            let pid2 = unsafe { libc::fork() };
            assert!(pid2 >= 0, "fork failed");
            if pid2 == 0 {
                // SAFETY: `sh_argv` and `envp` are valid arrays.
                unsafe {
                    libc::execve(c"/bin/sh".as_ptr(), sh_argv.as_ptr(), envp.as_ptr());
                    libc::_exit(127);
                }
            }
            let mut status2: libc::c_int = 0;
            // SAFETY: `pid2` is our child and `status2` is writable.
            assert_eq!(unsafe { libc::waitpid(pid2, &raw mut status2, 0) }, pid2);
            assert!(
                libc::WIFEXITED(status2) && libc::WEXITSTATUS(status2) == 0,
                "sh execve with stale __FSPY_BOOTSTRAP failed with status {status2}"
            );
        }
    );

    let accesses = test_utils::spawn_command(cmd).await?;
    assert_contains(&accesses, probe_path, fspy::PathAccess::Read);
    Ok(())
}

#[test(tokio::test)]
async fn posix_spawn_setsigdef_resets_child_sigsys_without_mutating_parent() -> anyhow::Result<()> {
    let probe_path = Path::new("/fspy-posix-spawn-sigdef-probe");
    let cmd = vt_subprocess_test::command_for_fn!(
        probe_path.to_str().unwrap().to_owned(),
        |probe_path: String| {
            use std::{ffi::CString, os::unix::ffi::OsStrExt as _};

            if std::env::var_os("FSPY_TEST_SPAWN_SIGDEF_CHILD").is_some() {
                // Child requested `POSIX_SPAWN_SETSIGDEF` with `SIGSYS` +
                // `POSIX_SPAWN_SETSIGMASK` with `SIGSYS | SIGUSR1`, so it must
                // observe `SIGSYS == SIG_DFL` (not parent's `SIG_IGN`) and both
                // signals blocked.
                // SAFETY: `libc::sigaction` is a plain C struct valid when zeroed.
                let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
                // SAFETY: querying current disposition for `SIGSYS`.
                let rc = unsafe { libc::sigaction(libc::SIGSYS, std::ptr::null(), &raw mut old) };
                assert_eq!(rc, 0);
                assert_eq!(
                    old.sa_sigaction,
                    libc::SIG_DFL,
                    "posix_spawn POSIX_SPAWN_SETSIGDEF did not reset child SIGSYS to SIG_DFL"
                );

                // SAFETY: `sigset_t` is a plain C struct valid when zeroed.
                let mut cur_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
                // SAFETY: querying current thread signal mask.
                let mask_rc = unsafe {
                    libc::sigprocmask(libc::SIG_SETMASK, std::ptr::null(), &raw mut cur_mask)
                };
                assert_eq!(mask_rc, 0);
                // SAFETY: `cur_mask` was initialized by `sigprocmask`.
                let sigsys_blocked =
                    unsafe { libc::sigismember(&raw const cur_mask, libc::SIGSYS) };
                assert_eq!(
                    sigsys_blocked, 1,
                    "posix_spawn POSIX_SPAWN_SETSIGMASK did not block SIGSYS"
                );
                // SAFETY: `cur_mask` was initialized by `sigprocmask`.
                let sigusr1_blocked =
                    unsafe { libc::sigismember(&raw const cur_mask, libc::SIGUSR1) };
                assert_eq!(
                    sigusr1_blocked, 1,
                    "posix_spawn POSIX_SPAWN_SETSIGMASK did not block SIGUSR1"
                );

                // Syscall tracking must keep working inside the spawned child.
                let _ = std::fs::File::open(&probe_path);
                return;
            }

            // Parent sets `SIGSYS` to `SIG_IGN`.
            // SAFETY: `libc::sigaction` is a plain C struct valid when zeroed.
            let mut ign: libc::sigaction = unsafe { std::mem::zeroed() };
            ign.sa_sigaction = libc::SIG_IGN;
            // SAFETY: installing `SIG_IGN` for `SIGSYS`.
            let ign_rc =
                unsafe { libc::sigaction(libc::SIGSYS, &raw const ign, std::ptr::null_mut()) };
            assert_eq!(ign_rc, 0);

            let exe = std::fs::read_link("/proc/self/exe").unwrap();
            let prog = CString::new(exe.as_os_str().as_bytes()).unwrap();
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
            let mut argv: Vec<*mut libc::c_char> =
                args.iter().map(|s| s.as_ptr().cast_mut()).collect();
            argv.push(std::ptr::null_mut());

            let mut env_cstrs: Vec<CString> = std::env::vars_os()
                .map(|(k, v)| {
                    let mut bytes = k.as_bytes().to_vec();
                    bytes.push(b'=');
                    bytes.extend_from_slice(v.as_bytes());
                    CString::new(bytes).unwrap()
                })
                .collect();
            env_cstrs.push(CString::new("FSPY_TEST_SPAWN_SIGDEF_CHILD=1").unwrap());
            let mut envp: Vec<*mut libc::c_char> =
                env_cstrs.iter().map(|s| s.as_ptr().cast_mut()).collect();
            envp.push(std::ptr::null_mut());

            // Configure `posix_spawnattr_t` with `POSIX_SPAWN_SETSIGDEF | POSIX_SPAWN_SETSIGMASK`.
            // SAFETY: `posix_spawnattr_t` and `sigset_t` are plain C structs valid when zeroed.
            let mut attr: libc::posix_spawnattr_t = unsafe { std::mem::zeroed() };
            // SAFETY: `sigset_t` is valid when zeroed.
            let mut sigdef_set: libc::sigset_t = unsafe { std::mem::zeroed() };
            // SAFETY: `sigset_t` is valid when zeroed.
            let mut sigmask_set: libc::sigset_t = unsafe { std::mem::zeroed() };
            let mut child_pid: libc::pid_t = 0;
            let spawn_flags =
                libc::c_short::try_from(libc::POSIX_SPAWN_SETSIGDEF | libc::POSIX_SPAWN_SETSIGMASK)
                    .unwrap();
            // SAFETY: valid stack-local `posix_spawnattr_t`, `sigset_t`, and C arrays.
            let spawn_rc = unsafe {
                assert_eq!(libc::posix_spawnattr_init(&raw mut attr), 0);
                libc::sigemptyset(&raw mut sigdef_set);
                libc::sigaddset(&raw mut sigdef_set, libc::SIGSYS);
                libc::sigemptyset(&raw mut sigmask_set);
                libc::sigaddset(&raw mut sigmask_set, libc::SIGSYS);
                libc::sigaddset(&raw mut sigmask_set, libc::SIGUSR1);
                assert_eq!(libc::posix_spawnattr_setflags(&raw mut attr, spawn_flags), 0);
                assert_eq!(
                    libc::posix_spawnattr_setsigdefault(&raw mut attr, &raw const sigdef_set),
                    0
                );
                assert_eq!(
                    libc::posix_spawnattr_setsigmask(&raw mut attr, &raw const sigmask_set),
                    0
                );
                let rc = libc::posix_spawn(
                    &raw mut child_pid,
                    prog.as_ptr(),
                    std::ptr::null(),
                    &raw const attr,
                    argv.as_ptr(),
                    envp.as_ptr(),
                );
                libc::posix_spawnattr_destroy(&raw mut attr);
                rc
            };
            assert_eq!(spawn_rc, 0, "posix_spawn failed with errno {spawn_rc}");

            let mut status: libc::c_int = 0;
            // SAFETY: `child_pid` is our child and `status` is writable.
            let waited = unsafe { libc::waitpid(child_pid, &raw mut status, 0) };
            assert_eq!(waited, child_pid);
            assert!(
                libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
                "posix_spawn child failed with status {status}"
            );

            // Parent must retain `SIGSYS == SIG_IGN`!
            // SAFETY: `libc::sigaction` is a plain C struct valid when zeroed.
            let mut parent_old: libc::sigaction = unsafe { std::mem::zeroed() };
            // SAFETY: querying parent's `SIGSYS` disposition.
            let parent_rc =
                unsafe { libc::sigaction(libc::SIGSYS, std::ptr::null(), &raw mut parent_old) };
            assert_eq!(parent_rc, 0);
            assert_eq!(
                parent_old.sa_sigaction,
                libc::SIG_IGN,
                "posix_spawn child clobbered parent's SIGSYS SIG_IGN"
            );
        }
    );

    let accesses = test_utils::spawn_command(cmd).await?;
    assert_contains(&accesses, probe_path, fspy::PathAccess::Read);
    Ok(())
}

#[test(tokio::test)]
async fn nested_fspy_falls_back_cleanly_while_outer_tracks_subtree() -> anyhow::Result<()> {
    let probe_path = Path::new("/fspy-nested-inner-child-probe");
    let cmd = vt_subprocess_test::command_for_fn!(
        (test_bin_path().to_str().unwrap().to_owned(), probe_path.to_str().unwrap().to_owned()),
        |(static_bin, probe_path): (String, String)| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async move {
                let mut inner_cmd = fspy::Command::new(&static_bin);
                inner_cmd.args(["open_read", &probe_path]);
                let termination = inner_cmd
                    .spawn(tokio_util::sync::CancellationToken::new())
                    .await
                    .expect("nested fspy spawn must succeed via untracked fallback")
                    .wait_handle
                    .await
                    .expect("nested fspy child wait failed");
                assert!(
                    termination.status.success(),
                    "nested child exited unsuccessfully: {:?}",
                    termination.status
                );
                assert!(
                    termination.path_accesses.is_err(),
                    "nested fspy must report TrackingIncomplete while delegating to outer tracker"
                );
            });
        }
    );

    let accesses = test_utils::spawn_command(cmd).await?;
    assert_contains(&accesses, probe_path, fspy::PathAccess::Read);
    Ok(())
}

#[test(tokio::test)]
async fn execve_of_et_exec_overlapping_launcher_region_returns_eexist_cleanly() -> anyhow::Result<()>
{
    #[cfg(target_arch = "x86_64")]
    const MACHINE: u16 = elf::abi::EM_X86_64;
    #[cfg(target_arch = "aarch64")]
    const MACHINE: u16 = elf::abi::EM_AARCH64;

    let survived_probe = Path::new("/fspy-overlap-et-exec-survived-probe");
    let tmp_dir = tempfile::tempdir()?;
    let overlap_path = tmp_dir.path().canonicalize()?.join("overlap_et_exec");

    // Construct a minimal valid native ELF64 `ET_EXEC` whose `PT_LOAD` overlaps
    // `fspy_elf_loader::LAUNCHER_REGION`.
    // SAFETY: `_SC_PAGESIZE` is a standard `sysconf` query.
    let page = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap();
    let vaddr = u64::try_from(fspy_elf_loader::LAUNCHER_REGION.0 + 4 * page).unwrap();

    let mut bytes = vec![0_u8; 2 * page];
    bytes[0..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&elf::abi::ET_EXEC.to_le_bytes());
    bytes[18..20].copy_from_slice(&MACHINE.to_le_bytes());
    bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
    bytes[24..32].copy_from_slice(&(vaddr + 0x200).to_le_bytes());
    bytes[32..40].copy_from_slice(&64_u64.to_le_bytes()); // e_phoff
    bytes[52..54].copy_from_slice(&64_u16.to_le_bytes()); // e_ehsize
    bytes[54..56].copy_from_slice(&56_u16.to_le_bytes()); // e_phentsize
    bytes[56..58].copy_from_slice(&1_u16.to_le_bytes()); // e_phnum

    let ph = &mut bytes[64..64 + 56];
    ph[0..4].copy_from_slice(&elf::abi::PT_LOAD.to_le_bytes());
    ph[4..8].copy_from_slice(&(elf::abi::PF_R | elf::abi::PF_X).to_le_bytes());
    ph[8..16].copy_from_slice(&0_u64.to_le_bytes()); // p_offset
    ph[16..24].copy_from_slice(&vaddr.to_le_bytes()); // p_vaddr
    ph[32..40].copy_from_slice(&u64::try_from(page).unwrap().to_le_bytes()); // p_filesz
    ph[40..48].copy_from_slice(&u64::try_from(page).unwrap().to_le_bytes()); // p_memsz
    ph[48..56].copy_from_slice(&u64::try_from(page).unwrap().to_le_bytes()); // p_align

    std::fs::write(&overlap_path, &bytes)?;
    std::fs::set_permissions(&overlap_path, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;

    let cmd = vt_subprocess_test::command_for_fn!(
        (overlap_path.to_str().unwrap().to_owned(), survived_probe.to_str().unwrap().to_owned()),
        |(overlap_path, survived_probe): (String, String)| {
            use std::ffi::CString;

            let prog = CString::new(overlap_path).unwrap();
            let argv = [prog.as_ptr(), std::ptr::null()];
            let envp = [std::ptr::null::<libc::c_char>()];
            // SAFETY: valid NUL/NULL-terminated arrays; `execve` must fail with `EEXIST`.
            let rc = unsafe { libc::execve(prog.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
            assert_eq!(rc, -1, "overlapping ET_EXEC execve unexpectedly succeeded");
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EEXIST),
                "expected EEXIST (not ENOEXEC) for ET_EXEC overlapping LAUNCHER_REGION"
            );

            // Caller survived and syscall tracking remains healthy.
            let _ = std::fs::File::open(&survived_probe);
        }
    );

    let accesses = test_utils::spawn_command(cmd).await?;
    assert_contains(&accesses, survived_probe, fspy::PathAccess::Read);
    Ok(())
}
