//! Tests for Linux untracked fallback and authentic exec errors:
//! - When `seccomp` is blocked by an outer sandbox, `fspy` runs commands
//!   untracked and reports `Err(UntrackedReason::Incomplete)` rather than failing every
//!   spawn.
//! - Authentic exec errors (such as `ENOEXEC` on a script without a `#!` line)
//!   propagate normally and keep the trace complete.
#![cfg(target_os = "linux")]

use std::{
    fs::{self, Permissions},
    mem::offset_of,
    os::unix::fs::PermissionsExt as _,
    process::Stdio,
};

use fspy::UntrackedReason;
use linux_raw_sys::{
    errno::EPERM,
    prctl::PR_SET_NO_NEW_PRIVS,
    ptrace::{
        BPF_ABS, BPF_JEQ, BPF_JMP, BPF_K, BPF_LD, BPF_RET, BPF_W, SECCOMP_RET_ALLOW,
        SECCOMP_RET_DATA, SECCOMP_RET_ERRNO, SECCOMP_SET_MODE_FILTER, seccomp_data, sock_filter,
        sock_fprog,
    },
};
use syscalls::Sysno;
use test_log::test;
use tokio::io::AsyncReadExt as _;
use tokio_util::sync::CancellationToken;
use vt_subprocess_test::command_for_fn;

/// An authentic exec error (`ENOEXEC` for a script without a `#!` line) keeps
/// its errno and leaves the trace complete so `/bin/sh` runs the script itself,
/// as it does without tracking.
#[test(tokio::test)]
async fn exec_errors_are_not_reported_as_lost() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let script = dir.path().join("fspy-script-without-shebang");
    fs::write(&script, "exit 0\n").expect("failed to write the script");
    fs::set_permissions(&script, Permissions::from_mode(0o755))
        .expect("failed to set permissions on the script");

    let mut command = fspy::Command::new("/bin/sh");
    command.arg("-c").arg(script.as_os_str()).stdin(Stdio::null());
    let tracked = command.spawn(CancellationToken::new()).await.expect("failed to spawn /bin/sh");
    let termination = tracked.wait_handle.await.expect("failed to wait for /bin/sh");
    assert!(termination.status.success(), "the script without a shebang did not run");
    assert!(
        termination.path_accesses.is_ok(),
        "an authentic exec error was reported as an untracked exec"
    );
}

/// When an outer sandbox blocks `SYS_seccomp` with `EPERM`, `fspy` detects
/// this during initialization, runs spawned commands untracked, and reports
/// `Err(UntrackedReason::Incomplete)` so callers do not cache partial traces.
#[test]
fn seccomp_blocked_sandbox_falls_back_to_untracked() {
    let cmd = command_for_fn!((), |()| {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "cBPF opcodes and offsets fit in u16/u32"
        )]
        let filter = [
            sock_filter {
                code: (BPF_LD | BPF_W | BPF_ABS) as u16,
                jt: 0,
                jf: 0,
                k: offset_of!(seccomp_data, nr) as u32,
            },
            sock_filter {
                code: (BPF_JMP | BPF_JEQ | BPF_K) as u16,
                jt: 0,
                jf: 1,
                k: Sysno::seccomp as u32,
            },
            sock_filter {
                code: (BPF_RET | BPF_K) as u16,
                jt: 0,
                jf: 0,
                k: SECCOMP_RET_ERRNO | (EPERM & SECCOMP_RET_DATA),
            },
            sock_filter { code: (BPF_RET | BPF_K) as u16, jt: 0, jf: 0, k: SECCOMP_RET_ALLOW },
        ];
        let prog = sock_fprog { len: 4, filter: filter.as_ptr().cast_mut() };
        // SAFETY: installs a valid 4-instruction seccomp filter on this test subprocess.
        unsafe {
            syscalls::syscall!(Sysno::prctl, PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0).unwrap();
            syscalls::syscall!(Sysno::seccomp, SECCOMP_SET_MODE_FILTER, 0, &raw const prog)
                .unwrap();
        }

        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let mut command = fspy::Command::new("/bin/echo");
            command.arg("sandbox-ok").stdout(Stdio::piped()).stdin(Stdio::null());
            let mut tracked = command
                .spawn(CancellationToken::new())
                .await
                .expect("sandboxed spawn should fall back untracked");
            let mut stdout = Vec::new();
            tracked.stdout.take().unwrap().read_to_end(&mut stdout).await.unwrap();
            let termination = tracked.wait_handle.await.unwrap();
            assert!(termination.status.success());
            assert_eq!(stdout, b"sandbox-ok\n");
            assert_eq!(termination.path_accesses.err(), Some(UntrackedReason::Incomplete));
        });
    });

    let output = std::process::Command::from(cmd).output().expect("failed to run sandbox test");
    assert!(
        output.status.success(),
        "sandboxed subprocess failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
