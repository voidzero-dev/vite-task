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
async fn rename_destination() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    File::create(tmp_dir.path().join("source"))?;
    let tmp_dir_str = tmp_dir.path().to_str().unwrap().to_owned();
    let accesses = track_fn!(tmp_dir_str, |tmp_dir_str: String| {
        let tmp_dir = std::path::Path::new(&tmp_dir_str);
        fs::rename(tmp_dir.join("source"), tmp_dir.join("destination")).unwrap();
    })
    .await?;
    assert_contains(
        &accesses,
        tmp_dir.path().join("destination").as_path(),
        PathAccess::RenameTarget,
    );

    Ok(())
}

#[test(tokio::test)]
async fn hard_link_destination() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    File::create(tmp_dir.path().join("original"))?;
    let tmp_dir_str = tmp_dir.path().to_str().unwrap().to_owned();
    let accesses = track_fn!(tmp_dir_str, |tmp_dir_str: String| {
        let tmp_dir = std::path::Path::new(&tmp_dir_str);
        fs::hard_link(tmp_dir.join("original"), tmp_dir.join("link")).unwrap();
    })
    .await?;
    assert_contains(&accesses, tmp_dir.path().join("link").as_path(), PathAccess::Write);

    Ok(())
}

/// glibc's `renameat2`, looked up at run time, since glibc added it in 2.28,
/// with the destination relative to a descriptor for its folder.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[test(tokio::test)]
async fn renameat2_destination() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let tmp_path = tmp_dir.path().canonicalize()?;
    File::create(tmp_path.join("source"))?;
    let tmp_path_str = tmp_path.to_str().unwrap().to_owned();
    let accesses = track_fn!(tmp_path_str, |tmp_path_str: String| {
        use std::os::fd::AsRawFd as _;
        type Renameat2 = unsafe extern "C" fn(
            libc::c_int,
            *const libc::c_char,
            libc::c_int,
            *const libc::c_char,
            libc::c_uint,
        ) -> libc::c_int;
        // SAFETY: the name is a valid C string.
        let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"renameat2".as_ptr()) };
        assert!(!symbol.is_null(), "glibc has no renameat2");
        // SAFETY: glibc's `renameat2` has this signature.
        let renameat2 = unsafe { std::mem::transmute::<*mut libc::c_void, Renameat2>(symbol) };
        let dir = File::open(tmp_path_str).unwrap();
        let dir_fd = dir.as_raw_fd();
        // SAFETY: the descriptor is open and the names are valid C strings.
        let result =
            unsafe { renameat2(dir_fd, c"source".as_ptr(), dir_fd, c"destination".as_ptr(), 0) };
        assert_eq!(result, 0);
    })
    .await?;
    assert_contains(&accesses, &tmp_path.join("destination"), PathAccess::RenameTarget);

    Ok(())
}

/// macOS's `renamex_np`, and `renameatx_np` with the destination relative to
/// a descriptor for its folder.
#[cfg(target_os = "macos")]
#[test(tokio::test)]
async fn renamex_np_destination() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    // A descriptor's path is its real path, and `TMPDIR` is under the `/var`
    // link.
    let tmp_path = tmp_dir.path().canonicalize()?;
    File::create(tmp_path.join("source"))?;
    let tmp_path_str = tmp_path.to_str().unwrap().to_owned();
    let accesses = track_fn!(tmp_path_str, |tmp_path_str: String| {
        use std::{ffi::CString, os::fd::AsRawFd as _};
        let source = CString::new(format!("{tmp_path_str}/source")).unwrap();
        let moved = CString::new(format!("{tmp_path_str}/moved")).unwrap();
        // SAFETY: the paths are valid C strings.
        assert_eq!(unsafe { libc::renamex_np(source.as_ptr(), moved.as_ptr(), 0) }, 0);
        let dir = File::open(tmp_path_str).unwrap();
        let dir_fd = dir.as_raw_fd();
        // SAFETY: the descriptor is open and the names are valid C strings.
        let result = unsafe {
            libc::renameatx_np(dir_fd, c"moved".as_ptr(), dir_fd, c"destination".as_ptr(), 0)
        };
        assert_eq!(result, 0);
    })
    .await?;
    for name in ["moved", "destination"] {
        assert_contains(&accesses, &tmp_path.join(name), PathAccess::RenameTarget);
    }

    Ok(())
}

#[test(tokio::test)]
async fn symlink_destination() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    // On Windows the link is recorded from its handle, by its long name, while
    // `TEMP` may hold an 8.3 short name, so name the links by the long name.
    let tmp_path = tmp_dir.path().canonicalize()?;
    fs::create_dir(tmp_path.join("folder"))?;
    let tmp_path_str = tmp_path.to_str().unwrap().to_owned();
    let accesses = track_fn!(tmp_path_str, |tmp_path_str: String| {
        let tmp_dir = std::path::Path::new(&tmp_path_str);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("file", tmp_dir.join("file_link")).unwrap();
            std::os::unix::fs::symlink("folder", tmp_dir.join("folder_link")).unwrap();
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file("file", tmp_dir.join("file_link")).unwrap();
            std::os::windows::fs::symlink_dir("folder", tmp_dir.join("folder_link")).unwrap();
        }
    })
    .await?;
    assert_contains(&accesses, &tmp_path.join("file_link"), PathAccess::Write);
    let folder_link = tmp_path.join("folder_link");
    #[cfg(unix)]
    assert_contains(&accesses, &folder_link, PathAccess::Write);
    // `CreateSymbolicLinkW` makes a folder link as a folder, recorded as a
    // mkdir, before it sets the reparse point.
    #[cfg(windows)]
    test_utils::assert_includes(&accesses, &folder_link, PathAccess::Write);

    Ok(())
}

#[test(tokio::test)]
async fn mkdir() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    fs::create_dir(tmp_dir.path().join("existing"))?;
    let tmp_dir_str = tmp_dir.path().to_str().unwrap().to_owned();
    let accesses = track_fn!(tmp_dir_str, |tmp_dir_str: String| {
        let tmp_dir = std::path::Path::new(&tmp_dir_str);
        fs::create_dir(tmp_dir.join("new")).unwrap();
        // Fails, since the folder is there already; recorded anyway.
        fs::create_dir(tmp_dir.join("existing")).unwrap_err();
    })
    .await?;
    assert_contains(&accesses, tmp_dir.path().join("new").as_path(), PathAccess::Mkdir);
    assert_contains(&accesses, tmp_dir.path().join("existing").as_path(), PathAccess::Mkdir);

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

#[cfg(all(target_os = "linux", not(target_env = "musl")))]
#[test(tokio::test)]
async fn syscall_during_dlopen() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let fifo_path = tmp_dir.path().join("fifo").to_str().unwrap().to_owned();
    nix::unistd::mkfifo(fifo_path.as_str(), nix::sys::stat::Mode::S_IRWXU).unwrap();

    track_fn!(fifo_path, |fifo_path: String| {
        // The interposer used to resolve libc's syscall with dlsym, which waits for dlopen,
        // so a second thread calling syscall waited for the first through futex, re-entering
        // the interposer until its stack overflowed.
        fn call_syscall_during_dlopen(fifo_path: &str) {
            // This waits for dlopen to open the FIFO, which it reads until writers close.
            let _writer = OpenOptions::new().write(true).open(fifo_path).unwrap();
            // SAFETY: getpid takes no arguments.
            unsafe { libc::syscall(libc::SYS_getpid) };
        }
        // Opening files and spawning threads can also wait for dlopen, so do both first.
        File::open("/").unwrap();
        let path = fifo_path.clone();
        std::thread::spawn(move || {
            let c = std::ffi::CString::new(path.as_str()).unwrap();
            // SAFETY: dlopen fails on the FIFO's empty contents.
            std::thread::spawn(move || unsafe {
                libc::dlopen(c.as_ptr(), libc::RTLD_NOW);
            });
            call_syscall_during_dlopen(&path);
        });
        call_syscall_during_dlopen(&fifo_path);
    })
    .await
    .unwrap();
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
