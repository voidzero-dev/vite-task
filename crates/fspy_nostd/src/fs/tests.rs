use core::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt as _;

use super::getcwd;
use crate::Error;

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "std::env::current_dir is the reference implementation under test"
)]
fn getcwd_returns_current_directory() {
    let expected = std::env::current_dir().unwrap();
    let mut buf = vec![MaybeUninit::uninit(); expected.as_os_str().as_bytes().len() + 1];
    let buf_ptr = buf.as_ptr().cast::<u8>();

    let actual = getcwd(&mut buf).unwrap();

    assert_eq!(actual.as_ptr().cast::<u8>(), buf_ptr);
    assert_eq!(
        actual.as_units_with_nul()[..actual.len_with_nul() - 1],
        *expected.as_os_str().as_bytes()
    );
}

#[test]
fn getcwd_rejects_an_empty_buffer() {
    assert!(matches!(getcwd(&mut []), Err(Error::RANGE)));
}

#[cfg(target_os = "macos")]
#[test]
fn fcntl_getpath_returns_descriptor_path() {
    let root = super::openat(
        crate::CWD,
        // SAFETY: the literal is NUL-terminated and static.
        unsafe { crate::CStr::<crate::Thin>::from_ptr(c"/".as_ptr().cast()) },
        super::OFlags::RDONLY,
        super::Mode::empty(),
    )
    .unwrap();
    let mut buf = [MaybeUninit::uninit(); super::PATH_MAX];
    let buf_ptr = buf.as_ptr().cast::<u8>();

    let path = super::fcntl_getpath(root.as_fd(), &mut buf).unwrap();

    assert_eq!(path.as_ptr().cast::<u8>(), buf_ptr);
    let path = path.count();
    assert_eq!(path.as_units_with_nul(), b"/\0");
}

#[cfg(target_os = "linux")]
#[test]
fn readlinkat_returns_the_initialized_target() {
    let mut buf = [MaybeUninit::uninit(); super::PATH_MAX];
    // SAFETY: the literal is NUL-terminated and lives for the call.
    let path = unsafe { crate::CStr::<crate::Thin>::from_ptr(c"/proc/self/exe".as_ptr().cast()) };

    let target = super::readlinkat(crate::CWD, path, &mut buf).unwrap();

    assert_eq!(target, std::fs::read_link("/proc/self/exe").unwrap().as_os_str().as_bytes());
}

#[test]
fn faccessat_checks_existence_and_execute_permission() {
    use std::{ffi::CString, fs::Permissions, os::unix::fs::PermissionsExt as _};

    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("target");
    std::fs::write(&file_path, b"x").unwrap();
    std::fs::set_permissions(&file_path, Permissions::from_mode(0o600)).unwrap();

    let file_c = CString::new(file_path.as_os_str().as_bytes()).unwrap();
    // SAFETY: `file_c` is NUL-terminated for the duration of the test.
    let file_cstr = unsafe { crate::CStr::<crate::Thin>::from_ptr(file_c.as_ptr().cast()) };

    assert_eq!(super::faccessat(crate::CWD, file_cstr, super::AccessFlags::empty()), Ok(()));
    assert_eq!(
        super::faccessat(crate::CWD, file_cstr, super::AccessFlags::X_OK),
        Err(Error::ACCES)
    );

    std::fs::set_permissions(&file_path, Permissions::from_mode(0o700)).unwrap();
    assert_eq!(super::faccessat(crate::CWD, file_cstr, super::AccessFlags::X_OK), Ok(()));

    let missing_path = dir.path().join("missing");
    let missing_c = CString::new(missing_path.as_os_str().as_bytes()).unwrap();
    // SAFETY: `missing_c` is NUL-terminated for the duration of the test.
    let missing_cstr = unsafe { crate::CStr::<crate::Thin>::from_ptr(missing_c.as_ptr().cast()) };
    assert_eq!(
        super::faccessat(crate::CWD, missing_cstr, super::AccessFlags::empty()),
        Err(Error::NOENT)
    );
}

#[test]
fn stat_distinguishes_regular_files_from_directories() {
    use std::os::fd::AsRawFd as _;

    let file = tempfile::tempfile().unwrap();
    // SAFETY: `file` remains open for the call.
    let file_fd = unsafe { crate::BorrowedFd::borrow_raw(file.as_raw_fd()) };
    assert!(super::fstat(file_fd).unwrap().is_regular_file());

    let dir = tempfile::tempdir().unwrap();
    let dir_file = std::fs::File::open(dir.path()).unwrap();
    // SAFETY: `dir_file` remains open for the call.
    let dir_fd = unsafe { crate::BorrowedFd::borrow_raw(dir_file.as_raw_fd()) };
    assert!(!super::fstat(dir_fd).unwrap().is_regular_file());
}

#[cfg(target_os = "linux")]
#[test]
fn fcntl_dupfd_duplicates_above_min_fd_without_cloexec() {
    use std::{io::Write as _, os::fd::AsRawFd as _};

    let mut file = tempfile::tempfile().unwrap();
    file.write_all(b"dup-test").unwrap();

    // SAFETY: `file` remains open for the call.
    let fd = unsafe { crate::BorrowedFd::borrow_raw(file.as_raw_fd()) };
    let dup = super::fcntl_dupfd(fd, 20).unwrap();

    assert!(dup.as_raw_fd() >= 20);
    assert!(!super::fcntl_is_cloexec(dup.as_fd()).unwrap());

    let dup_cloexec = super::fcntl_dupfd_cloexec(fd, 25).unwrap();
    assert!(dup_cloexec.as_raw_fd() >= 25);
    assert!(super::fcntl_is_cloexec(dup_cloexec.as_fd()).unwrap());

    let mut buf = [0u8; 8];
    assert_eq!(crate::io::pread(dup.as_fd(), &mut buf, 0).unwrap(), 8);
    assert_eq!(&buf, b"dup-test");
}
