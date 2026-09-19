//! Tests for the untracked-exec fallback: when the preload cannot install
//! its injection machinery, the exec must proceed untracked and the run must
//! be reported as incompletely tracked (so it is not cached), rather than
//! every spawn failing. Skipped on musl: no preload library exists there.
#![cfg(all(target_os = "linux", not(target_env = "musl")))]

use std::{
    ffi::OsStr,
    fs::{self, Permissions},
    os::unix::{ffi::OsStrExt as _, fs::PermissionsExt as _},
    path::{Path, PathBuf},
    process::Command,
    sync::LazyLock,
};

use allocator_api2::alloc::Global;
use fspy_seccomp_unotify::payload::SeccompPayload;
use fspy_shared::ipc::{
    IpcStr,
    channel::{RecordsLost, channel},
};
use fspy_shared_unix::payload::{Payload, encode_payload};

/// The preload cdylib, built as a dependency of this crate.
const PRELOAD_CDYLIB: &str = env!("CARGO_CDYLIB_FILE_FSPY_PRELOAD_UNIX");

const TEST_BIN_CONTENT: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_FSPY_TEST_BIN"));

fn test_bin_path() -> &'static Path {
    static TEST_BIN_PATH: LazyLock<PathBuf> = LazyLock::new(|| {
        let test_bin_path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fspy-test-bin");
        fs::write(&test_bin_path, TEST_BIN_CONTENT).expect("failed to write test binary");
        fs::set_permissions(&test_bin_path, Permissions::from_mode(0o755))
            .expect("failed to set permissions on test binary");
        test_bin_path
    });
    TEST_BIN_PATH.as_path()
}

/// A static binary exec'd from a traced process needs the preload's inline
/// seccomp install. When that install fails (here: the payload's supervisor
/// IPC path is bogus, simulating a sandbox that denies it), the binary must
/// still run — untracked — and the channel must report the loss.
#[test]
fn static_binary_runs_untracked_when_injection_fails() {
    let receiver = channel(1 << 30, Global).unwrap();
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

    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("exec {} stat /hello", test_bin_path().display()))
        .env_clear()
        .env("LD_PRELOAD", PRELOAD_CDYLIB)
        .env("FSPY_PAYLOAD", OsStr::from_bytes(encoded.encoded_string.as_ref()))
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

/// A preload loaded without a payload (e.g. a leaked LD_PRELOAD in an
/// env-scrubbed sandbox) must not abort its host process: the constructor
/// degrades and every exec forwards to the original.
#[test]
fn preload_without_payload_runs_untracked() {
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg("exec /bin/true")
        .env_clear()
        .env("LD_PRELOAD", PRELOAD_CDYLIB)
        .output()
        .expect("failed to spawn the shell");
    assert!(
        output.status.success(),
        "the preload aborted its host process: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
