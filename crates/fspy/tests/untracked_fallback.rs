//! Tests for the untracked-exec fallback: when the preload cannot install
//! the seccomp filter, the exec must proceed untracked and the run must be
//! reported as incompletely tracked (so it is not cached), rather than
//! every spawn failing. Skipped on musl: no preload library exists there.
#![cfg(all(target_os = "linux", not(target_env = "musl")))]

use std::{
    ffi::OsStr,
    fs::{self, Permissions},
    os::unix::{ffi::OsStrExt as _, fs::PermissionsExt as _},
    path::{Path, PathBuf},
    process::Command,
};

use allocator_api2::alloc::Global;
use fspy_seccomp_unotify::payload::SeccompPayload;
use fspy_shared::ipc::{
    IpcStr,
    channel::{Receiver, RecordsLost, channel},
};
use fspy_shared_unix::payload::{Payload, encode_payload};

use crate::test_utils::test_bin_path;

mod test_utils;

/// The preload cdylib, built as a dependency of this crate.
const PRELOAD_CDYLIB: &str = env!("CARGO_CDYLIB_FILE_FSPY_PRELOAD_UNIX");

/// Clears `command`'s environment and preloads fspy into it with a seccomp
/// payload whose install always fails. Returns the channel the preload
/// reports to.
fn preload_with_failing_seccomp(command: &mut Command) -> Receiver<Global> {
    let receiver = channel(1 << 30, Global).unwrap();
    {
        let preload_path: &IpcStr = Path::new(PRELOAD_CDYLIB).into();
        let payload = Payload {
            ipc_channel_conf: receiver.conf(),
            preload_path,
            seccomp_payload: SeccompPayload::unreachable(
                b"/nonexistent/fspy-unreachable-supervisor".to_vec(),
            ),
        };
        let bump = bumpalo::Bump::new();
        let encoded = encode_payload(payload, &bump);
        command
            .env_clear()
            .env("LD_PRELOAD", PRELOAD_CDYLIB)
            .env("FSPY_PAYLOAD", OsStr::from_bytes(encoded.encoded_string.as_ref()));
    }
    receiver
}

/// A static binary exec'd from a traced process needs the preload's inline
/// seccomp install. When that install fails (here: the payload's empty
/// filter, which the kernel refuses), the binary must still run — untracked
/// — and the channel must report the loss.
#[test]
fn static_binary_runs_untracked_when_injection_fails() {
    let mut command = Command::new("/bin/sh");
    let receiver = preload_with_failing_seccomp(&mut command);
    let output = command
        .arg("-c")
        .arg(format!("exec {} stat /hello", test_bin_path().display()))
        .output()
        .expect("failed to spawn the shell");
    assert!(
        output.status.success(),
        "the static binary did not run: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let Err(RecordsLost) = receiver.close() else {
        panic!("the channel did not report the untracked exec");
    };
}

/// The untracked fallback must replay the interposed call as it was made,
/// not as `execve`. GNU `env` runs its command through `execvp`, so a bare
/// program name found only on `PATH` still runs when injection fails;
/// forwarding it to `execve` would fail with `ENOENT` instead.
#[test]
fn execvp_fallback_keeps_path_search() {
    let test_bin = test_bin_path();
    let mut command = Command::new("/usr/bin/env");
    let receiver = preload_with_failing_seccomp(&mut command);
    let output = command
        .arg(test_bin.file_name().unwrap())
        .arg("stat")
        .arg("/hello")
        .env("PATH", test_bin.parent().unwrap())
        .current_dir("/")
        .output()
        .expect("failed to spawn env");
    assert!(
        output.status.success(),
        "the execvp fallback lost its PATH search: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let Err(RecordsLost) = receiver.close() else {
        panic!("the channel did not report the untracked exec");
    };
}

/// Only a failed seccomp install takes the untracked path. An exec that
/// fails the way the real exec would, here `ENOEXEC` for a script without a
/// `#!` line, keeps its errno and leaves the trace complete. `sh` then runs
/// the script itself, as it does without tracking.
#[test]
fn exec_errors_are_not_reported_as_lost() {
    let script = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fspy-script-without-shebang");
    fs::write(&script, "exit 0\n").expect("failed to write the script");
    fs::set_permissions(&script, Permissions::from_mode(0o755))
        .expect("failed to set permissions on the script");

    let mut command = Command::new("/bin/sh");
    let receiver = preload_with_failing_seccomp(&mut command);
    let output =
        command.arg("-c").arg(script.as_os_str()).output().expect("failed to spawn the shell");
    assert!(
        output.status.success(),
        "the script without a shebang did not run: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(receiver.close().is_ok(), "an authentic exec error was reported as an untracked exec");
}
