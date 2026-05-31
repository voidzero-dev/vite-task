//! Tests for fspy tracing of statically-linked executables (seccomp path).
//! Skipped on musl: the test binary is an artifact dep targeting musl, and when
//! the CI builds with `-crt-static` the binary becomes dynamically linked,
//! defeating the purpose of these tests.
#![cfg(all(target_os = "linux", not(target_env = "musl")))]
use std::{
    fs::{self, Permissions},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use fspy::PathAccessIterable;
use fspy_shared_unix::is_dynamically_linked_to_libc;
use test_log::test;

use crate::test_utils::assert_contains;

mod test_utils;

const TEST_BIN_CONTENT: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_FSPY_TEST_BIN"));

fn test_bin_path() -> &'static Path {
    static TEST_BIN_PATH: LazyLock<PathBuf> = LazyLock::new(|| {
        assert_eq!(
            is_dynamically_linked_to_libc(TEST_BIN_CONTENT),
            Ok(false),
            "Test binary is not a static executable"
        );

        let tmp_dir = env!("CARGO_TARGET_TMPDIR");
        let test_bin_path = PathBuf::from(tmp_dir).join("fspy-test-bin");
        fs::write(&test_bin_path, TEST_BIN_CONTENT).expect("failed to write test binary");
        fs::set_permissions(&test_bin_path, Permissions::from_mode(0o755))
            .expect("failed to set permissions on test binary");

        test_bin_path
    });
    TEST_BIN_PATH.as_path()
}

async fn track_test_bin(args: &[&str], cwd: Option<&str>) -> PathAccessIterable {
    let mut cmd = fspy::Command::new(test_bin_path());
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    cmd.args(args);
    let tracked_child = cmd.spawn(tokio_util::sync::CancellationToken::new()).await.unwrap();

    let termination = tracked_child.wait_handle.await.unwrap();
    assert!(termination.status.success());

    termination.path_accesses
}

#[test(tokio::test)]
async fn open_read() {
    let accesses = track_test_bin(&["open_read", "/hello"], None).await;
    assert_contains(&accesses, Path::new("/hello"), fspy::AccessMode::READ);
}

#[test(tokio::test)]
async fn open_write() {
    let accesses = track_test_bin(&["open_write", "/hello"], None).await;
    assert_contains(&accesses, Path::new("/hello"), fspy::AccessMode::WRITE);
}

#[test(tokio::test)]
async fn open_readwrite() {
    let accesses = track_test_bin(&["open_readwrite", "/hello"], None).await;
    assert_contains(
        &accesses,
        Path::new("/hello"),
        fspy::AccessMode::READ | fspy::AccessMode::WRITE,
    );
}

#[test(tokio::test)]
async fn openat2_read() {
    let accesses = track_test_bin(&["openat2_read", "/hello"], None).await;
    assert_contains(&accesses, Path::new("/hello"), fspy::AccessMode::READ);
}

#[test(tokio::test)]
async fn openat2_write() {
    let accesses = track_test_bin(&["openat2_write", "/hello"], None).await;
    assert_contains(&accesses, Path::new("/hello"), fspy::AccessMode::WRITE);
}

#[test(tokio::test)]
async fn openat2_readwrite() {
    let accesses = track_test_bin(&["openat2_readwrite", "/hello"], None).await;
    assert_contains(
        &accesses,
        Path::new("/hello"),
        fspy::AccessMode::READ | fspy::AccessMode::WRITE,
    );
}

#[test(tokio::test)]
async fn open_relative() {
    let accesses = track_test_bin(&["open_read", "hello"], Some("/home")).await;
    assert_contains(&accesses, Path::new("/home/hello"), fspy::AccessMode::READ);
}

#[test(tokio::test)]
async fn readdir() {
    let accesses = track_test_bin(&["readdir", "/home"], None).await;
    assert_contains(&accesses, Path::new("/home"), fspy::AccessMode::READ_DIR);
}

#[test(tokio::test)]
async fn stat() {
    let accesses = track_test_bin(&["stat", "/hello"], None).await;
    assert_contains(&accesses, Path::new("/hello"), fspy::AccessMode::READ);
}

#[test(tokio::test)]
async fn execve() {
    let accesses = track_test_bin(&["execve", "/hello"], None).await;
    assert_contains(&accesses, Path::new("/hello"), fspy::AccessMode::READ);
}

/// Reads up to 64 bytes from `file` at offset 0 without moving its (shared)
/// file offset.
fn read_at_start(file: &std::fs::File) -> Vec<u8> {
    use std::os::unix::fs::FileExt as _;
    let mut buf = vec![0u8; 64];
    let read = file.read_at(&mut buf, 0).unwrap_or(0);
    buf.truncate(read);
    buf
}

/// The blocking callback fires for a statically-linked (seccomp-traced)
/// target: the supervisor opens the file, the callback reads it, and the
/// descriptor is installed into the target via `ADDFD` so its own read works.
#[test(tokio::test)]
async fn callback_open_read() {
    use std::sync::{Arc, Mutex};

    use fspy::{AccessMode, FileEvent, FileEventKind};

    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("seccomp_callback_file");
    let content = b"fspy-seccomp-callback-content";
    fs::write(&file_path, content).unwrap();
    let dir_path = dir.path().to_path_buf();

    let opened_bytes: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let kinds: Arc<Mutex<Vec<FileEventKind>>> = Arc::new(Mutex::new(Vec::new()));
    let callback = {
        let (dir_path, opened_bytes, kinds) =
            (dir_path.clone(), Arc::clone(&opened_bytes), Arc::clone(&kinds));
        move |event: FileEvent<'_>| {
            let Some(path) = event.path.get() else {
                return;
            };
            if !path.starts_with(&dir_path) {
                return;
            }
            kinds.lock().unwrap().push(event.kind);
            if event.kind == FileEventKind::Opened {
                *opened_bytes.lock().unwrap() = Some(read_at_start(&event.fd.as_file()));
            }
        }
    };

    let mut cmd = fspy::Command::new(test_bin_path());
    cmd.args(["read_verify", file_path.to_str().unwrap()]);
    cmd.on_file_event(AccessMode::READ, callback);
    let child = cmd.spawn(tokio_util::sync::CancellationToken::new()).await.unwrap();
    let termination = child.wait_handle.await.unwrap();

    // A successful exit proves the `ADDFD`-installed descriptor worked.
    assert!(termination.status.success(), "target failed: ADDFD descriptor did not work");
    assert_eq!(
        opened_bytes.lock().unwrap().as_deref(),
        Some(content.as_slice()),
        "callback should read the file content through the passed descriptor"
    );
    let kinds = kinds.lock().unwrap().clone();
    assert!(kinds.contains(&FileEventKind::Opened), "expected an Opened event, got {kinds:?}");
    assert!(kinds.contains(&FileEventKind::Closing), "expected a Closing event, got {kinds:?}");
}

/// The blocking callback handles concurrent opens from several target threads
/// without deadlocking.
#[test(tokio::test)]
async fn callback_multi_threaded() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use fspy::{AccessMode, FileEvent, FileEventKind};

    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("seccomp_threads_file");
    fs::write(&file_path, b"threaded-callback-content").unwrap();
    let dir_path = dir.path().to_path_buf();

    let opened = Arc::new(AtomicUsize::new(0));
    let callback = {
        let (dir_path, opened) = (dir_path.clone(), Arc::clone(&opened));
        move |event: FileEvent<'_>| {
            let Some(path) = event.path.get() else {
                return;
            };
            if path.starts_with(&dir_path) && event.kind == FileEventKind::Opened {
                opened.fetch_add(1, Ordering::SeqCst);
            }
        }
    };

    let mut cmd = fspy::Command::new(test_bin_path());
    cmd.args(["read_verify_threads", file_path.to_str().unwrap()]);
    cmd.on_file_event(AccessMode::READ, callback);
    let child = cmd.spawn(tokio_util::sync::CancellationToken::new()).await.unwrap();
    let termination = child.wait_handle.await.unwrap();

    assert!(termination.status.success(), "target failed under concurrent callbacks");
    assert_eq!(opened.load(Ordering::SeqCst), 4, "expected one Opened event per worker thread");
}
