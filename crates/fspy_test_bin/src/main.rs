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
    assert!(args.len() == 3, "expected 2 arguments: <action> <file_path>");
    let action = args[1].as_str();
    let path = args[2].as_str();

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
        "read_verify" => {
            // Open and read the file, then drop it (closing it). Used by the
            // seccomp blocking-callback test: under seccomp the supervisor
            // opens the file itself and installs the descriptor into this
            // process via `ADDFD`, so a successful non-empty read proves the
            // installed descriptor works.
            use std::io::Read as _;
            let mut file = File::open(path).expect("read_verify: open failed");
            let mut content = Vec::new();
            file.read_to_end(&mut content).expect("read_verify: read failed");
            assert!(!content.is_empty(), "read_verify: file content was empty");
        }
        "read_verify_threads" => {
            // Like `read_verify`, but from several threads concurrently, so the
            // seccomp blocking-callback path is exercised under concurrency.
            use std::io::Read as _;
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let path = path.to_owned();
                    std::thread::spawn(move || {
                        let mut file = File::open(&path).expect("open failed");
                        let mut content = Vec::new();
                        file.read_to_end(&mut content).expect("read failed");
                        assert!(!content.is_empty(), "file content was empty");
                    })
                })
                .collect();
            for handle in handles {
                handle.join().expect("worker thread panicked");
            }
        }
        "execve" => {
            let _ = std::process::Command::new(path).spawn();
        }
        _ => panic!("unknown action: {action}"),
    }
}
