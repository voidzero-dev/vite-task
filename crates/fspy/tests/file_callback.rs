//! Tests for the optional blocking open/close callbacks.
//!
//! These exercise the preload backend (`LD_PRELOAD` on Linux glibc, `DYLD` on
//! macOS, Detours on Windows): the traced process is a dynamically-linked
//! re-exec of this test binary. The seccomp backend is covered separately in
//! `static_executable.rs`.

mod test_utils;

use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use fspy::{AccessMode, FileEvent, FileEventKind, TrackedChild};
use test_utils::{assert_contains, command_for_fn};
use tokio_util::sync::CancellationToken;

/// A one-shot latch: many waiters block until `set` is called once.
#[derive(Default)]
struct Latch {
    raised: Mutex<bool>,
    cv: Condvar,
}

impl Latch {
    fn set(&self) {
        *self.raised.lock().unwrap() = true;
        self.cv.notify_all();
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "the mutex guard must be held across the condvar wait loop"
    )]
    fn wait(&self) {
        let mut raised = self.raised.lock().unwrap();
        while !*raised {
            raised = self.cv.wait(raised).unwrap();
        }
    }
}

/// Reads up to 256 bytes at offset 0 without disturbing the descriptor's
/// current file offset (which is shared with the traced process).
fn read_fd_content(file: &File) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; 256];
    #[cfg(unix)]
    let read = {
        use std::os::unix::fs::FileExt as _;
        file.read_at(&mut buf, 0)?
    };
    #[cfg(windows)]
    let read = {
        use std::os::windows::fs::FileExt as _;
        file.seek_read(&mut buf, 0)?
    };
    buf.truncate(read);
    Ok(buf)
}

/// Spawns a traced subprocess with a blocking open/close callback registered.
async fn spawn_with_callback<F>(
    cmd: subprocess_test::Command,
    mask: AccessMode,
    callback: F,
) -> anyhow::Result<TrackedChild>
where
    F: Fn(FileEvent<'_>) + Send + Sync + 'static,
{
    let mut command = fspy::Command::from(cmd);
    command.on_file_event(mask, callback);
    Ok(command.spawn(CancellationToken::new()).await?)
}

/// The callback blocks the traced process: while it is running, the target
/// cannot make progress past the open it is blocked in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn callback_blocks_until_supervisor_resumes() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("file_a"), b"a")?;
    std::fs::write(dir.path().join("file_b"), b"b")?;
    let dir_path = std::fs::canonicalize(dir.path())?;

    let opened: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Latch::default());
    let release = Arc::new(Latch::default());
    let blocked_once = Arc::new(AtomicBool::new(false));

    let callback = {
        let (dir_path, opened, entered, release, blocked_once) = (
            dir_path.clone(),
            Arc::clone(&opened),
            Arc::clone(&entered),
            Arc::clone(&release),
            Arc::clone(&blocked_once),
        );
        move |event: FileEvent<'_>| {
            if event.kind != FileEventKind::Opened {
                return;
            }
            let Some(path) = event.path.get() else {
                return;
            };
            if !path.starts_with(&dir_path) {
                return; // ignore unrelated startup I/O
            }
            opened.lock().unwrap().push(path.to_path_buf());
            // Block the traced process on the very first matching open.
            if !blocked_once.swap(true, Ordering::SeqCst) {
                entered.set();
                release.wait();
            }
        }
    };

    let cmd = command_for_fn!(dir_path.to_str().unwrap().to_owned(), |dir: String| {
        let dir = std::path::Path::new(&dir);
        let _ = std::fs::File::open(dir.join("file_a")).unwrap();
        let _ = std::fs::File::open(dir.join("file_b")).unwrap();
    });
    let child = spawn_with_callback(cmd, AccessMode::READ, callback).await?;

    // Wait until the callback for `file_a` is blocking the traced process.
    {
        let entered = Arc::clone(&entered);
        tokio::task::spawn_blocking(move || entered.wait()).await?;
    }
    // The target is blocked inside `file_a`'s open hook awaiting the ACK, so it
    // cannot have reached the `file_b` open yet.
    assert_eq!(
        opened.lock().unwrap().len(),
        1,
        "traced process progressed past the open while the callback was still running"
    );

    release.set();
    let termination = child.wait_handle.await?;
    assert!(termination.status.success());
    assert_eq!(
        opened.lock().unwrap().len(),
        2,
        "expected an Opened event for each of the two files"
    );
    Ok(())
}

/// The callback receives a descriptor it can read inside the supervisor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn callback_can_read_fd() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let content = b"hello fspy callback";
    std::fs::write(dir.path().join("content"), content)?;
    let dir_path = std::fs::canonicalize(dir.path())?;

    let seen: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let callback = {
        let (dir_path, seen) = (dir_path.clone(), Arc::clone(&seen));
        move |event: FileEvent<'_>| {
            if event.kind != FileEventKind::Opened {
                return;
            }
            let Some(path) = event.path.get() else {
                return;
            };
            if !path.starts_with(&dir_path) {
                return;
            }
            let file = event.fd.as_file();
            if let Ok(bytes) = read_fd_content(&file) {
                *seen.lock().unwrap() = Some(bytes);
            }
        }
    };

    let cmd = command_for_fn!(dir_path.to_str().unwrap().to_owned(), |dir: String| {
        let path = std::path::Path::new(&dir).join("content");
        let _ = std::fs::read(path).unwrap();
    });
    let child = spawn_with_callback(cmd, AccessMode::READ, callback).await?;
    let termination = child.wait_handle.await?;
    assert!(termination.status.success());

    assert_eq!(
        seen.lock().unwrap().as_deref(),
        Some(content.as_slice()),
        "callback should read the file content through the passed descriptor"
    );
    Ok(())
}

/// A `Closing` event fires before the close, with a still-readable descriptor,
/// and after the matching `Opened` event.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_callback_fires_before_close() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("closeme"), b"data")?;
    let dir_path = std::fs::canonicalize(dir.path())?;

    let kinds: Arc<Mutex<Vec<FileEventKind>>> = Arc::new(Mutex::new(Vec::new()));
    let close_readable = Arc::new(AtomicBool::new(false));
    let callback = {
        let (dir_path, kinds, close_readable) =
            (dir_path.clone(), Arc::clone(&kinds), Arc::clone(&close_readable));
        move |event: FileEvent<'_>| {
            let Some(path) = event.path.get() else {
                return;
            };
            if !path.starts_with(&dir_path) {
                return;
            }
            kinds.lock().unwrap().push(event.kind);
            if event.kind == FileEventKind::Closing
                && read_fd_content(&event.fd.as_file()).is_ok_and(|bytes| bytes == b"data")
            {
                close_readable.store(true, Ordering::SeqCst);
            }
        }
    };

    let cmd = command_for_fn!(dir_path.to_str().unwrap().to_owned(), |dir: String| {
        let path = std::path::Path::new(&dir).join("closeme");
        let file = std::fs::File::open(path).unwrap();
        drop(file);
    });
    let child = spawn_with_callback(cmd, AccessMode::READ, callback).await?;
    let termination = child.wait_handle.await?;
    assert!(termination.status.success());

    let kinds = kinds.lock().unwrap().clone();
    let open_idx = kinds.iter().position(|kind| *kind == FileEventKind::Opened);
    let close_idx = kinds.iter().position(|kind| *kind == FileEventKind::Closing);
    assert!(open_idx.is_some(), "expected an Opened event, got {kinds:?}");
    assert!(close_idx.is_some(), "expected a Closing event, got {kinds:?}");
    assert!(open_idx < close_idx, "Opened must come before Closing, got {kinds:?}");
    assert!(
        close_readable.load(Ordering::SeqCst),
        "the descriptor must still be readable inside the close callback"
    );
    Ok(())
}

/// The access-mode mask filters which events reach the callback.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mask_filters_events() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("read_file"), b"r")?;
    std::fs::write(dir.path().join("write_file"), b"w")?;
    let dir_path = std::fs::canonicalize(dir.path())?;

    let modes: Arc<Mutex<Vec<AccessMode>>> = Arc::new(Mutex::new(Vec::new()));
    let callback = {
        let (dir_path, modes) = (dir_path.clone(), Arc::clone(&modes));
        move |event: FileEvent<'_>| {
            if event.kind != FileEventKind::Opened {
                return;
            }
            let Some(path) = event.path.get() else {
                return;
            };
            if !path.starts_with(&dir_path) {
                return;
            }
            modes.lock().unwrap().push(event.mode);
        }
    };

    let cmd = command_for_fn!(dir_path.to_str().unwrap().to_owned(), |dir: String| {
        let dir = std::path::Path::new(&dir);
        let _ = std::fs::File::open(dir.join("read_file")).unwrap();
        let _ = std::fs::OpenOptions::new().write(true).open(dir.join("write_file")).unwrap();
    });
    let child = spawn_with_callback(cmd, AccessMode::WRITE, callback).await?;
    let termination = child.wait_handle.await?;
    assert!(termination.status.success());

    let modes = modes.lock().unwrap().clone();
    assert_eq!(modes.len(), 1, "a WRITE mask should yield exactly the write open, got {modes:?}");
    assert!(modes[0].contains(AccessMode::WRITE));
    Ok(())
}

/// Without a registered callback, access tracking is unaffected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_callback_is_zero_overhead() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("plain");
    std::fs::write(&path, b"x")?;

    let cmd = command_for_fn!(path.to_str().unwrap().to_owned(), |path: String| {
        let _ = std::fs::File::open(path).unwrap();
    });
    let termination =
        fspy::Command::from(cmd).spawn(CancellationToken::new()).await?.wait_handle.await?;
    assert!(termination.status.success());
    assert_contains(&termination.path_accesses, Path::new(&path), AccessMode::READ);
    Ok(())
}
