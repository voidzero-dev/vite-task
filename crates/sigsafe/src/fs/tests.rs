use core::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt as _;

use super::getcwd;
use crate::Errno;

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
        actual.as_bytes_with_nul()[..actual.len_with_nul() - 1],
        *expected.as_os_str().as_bytes()
    );
}

#[test]
fn getcwd_rejects_an_empty_buffer() {
    assert!(matches!(getcwd(&mut []), Err(Errno::RANGE)));
}
