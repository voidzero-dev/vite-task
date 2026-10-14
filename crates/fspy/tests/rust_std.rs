mod test_utils;

use std::{
    env::current_dir,
    fs::{self, File, OpenOptions},
    process::Stdio,
};

use fspy::PathAccess;
use test_log::test;
use test_utils::assert_contains;

#[test(tokio::test)]
async fn open_read() -> anyhow::Result<()> {
    let accesses = track_fn!((), |(): ()| {
        let _ = File::open("hello");
    })
    .await?;
    assert_contains(&accesses, current_dir().unwrap().join("hello").as_path(), PathAccess::Read);

    Ok(())
}

#[test(tokio::test)]
async fn open_write() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let tmp_path = tmp_dir.path().join("hello");
    let tmp_path_str = tmp_path.to_str().unwrap().to_owned();
    let accesses = track_fn!(tmp_path_str, |tmp_path_str: String| {
        let _ = OpenOptions::new().write(true).open(tmp_path_str);
    })
    .await?;
    assert_contains(&accesses, tmp_path.as_path(), PathAccess::Write);

    Ok(())
}

#[test(tokio::test)]
async fn metadata() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let tmp_path = tmp_dir.path().join("hello");
    File::create(&tmp_path)?;
    let tmp_path_str = tmp_path.to_str().unwrap().to_owned();

    let accesses = track_fn!(tmp_path_str, |tmp_path_str: String| {
        let _ = fs::metadata(tmp_path_str);
    })
    .await?;
    assert_contains(&accesses, tmp_path.as_path(), PathAccess::Read);

    Ok(())
}

#[test(tokio::test)]
async fn concurrent_metadata() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let tmp_path = tmp_dir.path().join("hello");
    fs::write(&tmp_path, b"hello")?;
    let tmp_path_str = tmp_path.to_str().unwrap().to_owned();

    let accesses = track_fn!(tmp_path_str, |tmp_path_str: String| {
        // Release the workers together without calling futex before the file
        // accesses. On Linux, concurrent first calls to the syscall interposer
        // used to recurse through LazyLock's own futex wait and overflow.
        let start = std::sync::atomic::AtomicBool::new(false);
        let ready = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..32 {
                scope.spawn(|| {
                    ready.fetch_add(1, std::sync::atomic::Ordering::Release);
                    while !start.load(std::sync::atomic::Ordering::Acquire) {
                        std::hint::spin_loop();
                    }
                    assert_eq!(fs::metadata(&tmp_path_str).unwrap().len(), 5);
                });
            }
            while ready.load(std::sync::atomic::Ordering::Acquire) != 32 {
                std::hint::spin_loop();
            }
            start.store(true, std::sync::atomic::Ordering::Release);
        });
    })
    .await?;
    assert_contains(&accesses, tmp_path.as_path(), PathAccess::Read);

    Ok(())
}

#[test(tokio::test)]
async fn readdir() -> anyhow::Result<()> {
    let tmpdir = tempfile::tempdir()?;
    let tmpdir_path = std::fs::canonicalize(tmpdir.path())?;
    // Reading a non-existent directory results in different tracked accesses on different platforms:
    // - Windows: READ, because the NT APIs open the directory as handle just like files (NtCreateFile/NtOpenFile),
    //   and if that fails, not read dir call (NtQueryDirectoryFile/NtQueryDirectoryFileEx) is made.
    // - macOS/Linux:
    //   - opendir results in a read_dir access. This call is directly made without trying to open the directory as a fd first.
    //   - open + fopendir results in READ access, because open would fail with ENOENT, and fopendir is not called.
    //
    // This difference is acceptable because both will result in a "not found" fingerprint in vite-task.
    // To keep the test consistent across platforms, we create the directory first.
    std::fs::create_dir(tmpdir.path().join("hello_dir"))?;

    let accesses = track_fn!(tmpdir_path.to_str().unwrap().to_owned(), |tmpdir_path: String| {
        std::env::set_current_dir(tmpdir_path).unwrap();
        // Consume at least one entry so that getdents64 fires on seccomp targets.
        let _ = std::fs::read_dir("hello_dir").unwrap().next();
    })
    .await?;
    assert_contains(&accesses, tmpdir_path.join("hello_dir").as_path(), PathAccess::ReadDir);

    Ok(())
}

#[test(tokio::test)]
async fn read_in_subprocess() -> anyhow::Result<()> {
    let accesses = track_fn!((), |(): ()| {
        let mut command = if cfg!(windows) {
            let mut command = std::process::Command::new("cmd");
            command.arg("/c").arg("type hello");
            command
        } else {
            let mut command = std::process::Command::new("/bin/sh");
            command.arg("-c").arg("cat hello");
            command
        };
        command
            .stdout(Stdio::null())
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
            .wait()
            .unwrap();
    })
    .await?;
    assert_contains(&accesses, current_dir().unwrap().join("hello").as_path(), PathAccess::Read);

    Ok(())
}

#[test(tokio::test)]
async fn read_program() -> anyhow::Result<()> {
    let accesses = track_fn!((), |(): ()| {
        let _ = std::process::Command::new("./not_exist.exe").spawn();
    })
    .await?;
    assert_contains(
        &accesses,
        current_dir().unwrap().join("not_exist.exe").as_path(),
        PathAccess::Read,
    );

    Ok(())
}
