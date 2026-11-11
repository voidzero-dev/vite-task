#[expect(clippy::unimplemented, reason = "fspy_test_bin is a test-only binary for Linux")]
#[cfg(not(target_os = "linux"))]
fn main() {
    unimplemented!("fspy_test_bin is only for Linux");
}

#[cfg(target_os = "linux")]
fn main() {
    use std::fs::File;

    use nix::fcntl::{AT_FDCWD, OFlag, OpenHow, openat2};
    let args = std::env::args().collect::<Vec<_>>();
    assert!(
        args.len() == 3 || args.len() == 4,
        "expected 2 or 3 arguments: <action> <file_path> [<second_path>]"
    );
    let action = args[1].as_str();
    let path = args[2].as_str();
    let second_path =
        || args.get(3).unwrap_or_else(|| panic!("action {action} takes a second path")).as_str();

    match action {
        "open_read" => {
            let _ = File::open(path);
        }
        "open_write" => {
            let _ = File::options().write(true).open(path);
        }
        "open_readwrite" => {
            let _ = File::options().read(true).write(true).open(path);
        }
        "open_folder_only" => {
            use std::os::unix::fs::OpenOptionsExt as _;
            let _ = File::options().read(true).custom_flags(nix::libc::O_DIRECTORY).open(path);
        }
        "openat2_read" => {
            let _ = openat2(AT_FDCWD, path, OpenHow::new().flags(OFlag::O_RDONLY));
        }
        "openat2_write" => {
            let _ = openat2(AT_FDCWD, path, OpenHow::new().flags(OFlag::O_WRONLY));
        }
        "openat2_readwrite" => {
            let _ = openat2(AT_FDCWD, path, OpenHow::new().flags(OFlag::O_RDWR));
        }
        "readdir" => {
            let mut entries = std::fs::read_dir(path).unwrap();
            let _ = entries.next();
        }
        "stat" => {
            let _ = std::fs::metadata(path);
        }
        "execve" => {
            let _ = std::process::Command::new(path).spawn();
        }
        "mkdir" => {
            let _ = std::fs::create_dir(path);
        }
        "rename" => {
            let _ = std::fs::rename(path, second_path());
        }
        "renameat2" => {
            // Renames `source` in the folder `path` to the second path, through
            // a descriptor for the folder.
            use std::os::fd::AsRawFd as _;
            let dir = File::open(path).unwrap();
            let destination = std::ffi::CString::new(second_path()).unwrap();
            // SAFETY: the descriptor is open and the names are valid C strings.
            let _ = unsafe {
                nix::libc::syscall(
                    nix::libc::SYS_renameat2,
                    dir.as_raw_fd(),
                    c"source".as_ptr(),
                    dir.as_raw_fd(),
                    destination.as_ptr(),
                    0,
                )
            };
        }
        "hard_link" => {
            let _ = std::fs::hard_link(path, second_path());
        }
        "symlink" => {
            let _ = std::os::unix::fs::symlink(path, second_path());
        }
        _ => panic!("unknown action: {action}"),
    }
}
