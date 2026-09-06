use core::fmt::Write as _;
use std::{
    io::Read as _,
    os::{fd::AsRawFd as _, unix::net::UnixStream},
    time::Duration,
};

use super::*;

#[test]
fn formats_into_a_borrowed_descriptor() {
    let (writer, mut reader) = UnixStream::pair().unwrap();
    reader.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    // SAFETY: writer owns the descriptor throughout this borrow.
    let mut fd = unsafe { BorrowedFd::borrow_raw(writer.as_raw_fd()) };
    write!(fd, "value={} ", 42).unwrap();
    fd.write_char('é').unwrap();
    fd.write_str("\n").unwrap();

    let expected = "value=42 é\n".as_bytes();
    let mut actual = vec![0; expected.len()];
    reader.read_exact(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn writing_a_read_only_descriptor_returns_an_error() {
    let file = std::fs::File::open("/dev/null").unwrap();
    // SAFETY: file owns the descriptor throughout this borrow. Write access
    // is not required to borrow it; the kernel rejects the write below.
    let mut fd = unsafe { BorrowedFd::borrow_raw(file.as_raw_fd()) };
    assert_eq!(fd.write_str("test"), Err(fmt::Error));
}

#[test]
fn retries_interrupted_and_partial_writes() {
    let expected = "éxyz".as_bytes();
    let mut results = [Err(Error::INTR), Ok(1), Err(Error::INTR), Ok(1), Ok(3)].into_iter();
    let mut output = Vec::new();
    write_all(expected, |bytes| {
        let result = results.next().unwrap();
        if let Ok(count) = result {
            output.extend_from_slice(&bytes[..count]);
        }
        result
    })
    .unwrap();
    assert_eq!(output, expected);
    assert!(results.next().is_none());
}

#[test]
fn empty_text_needs_no_write() {
    let mut calls = 0;
    assert_eq!(
        write_all(b"", |_| {
            calls += 1;
            Err(Error::BADF)
        }),
        Ok(())
    );
    assert_eq!(calls, 0);
}

#[test]
fn zero_progress_and_errors_do_not_retry() {
    #[cfg(any(target_os = "linux", target_os = "none"))]
    let again = Error::from_raw_os_error(linux_raw_sys::errno::EAGAIN.cast_signed());
    #[cfg(target_os = "macos")]
    let again = Error::from_raw_os_error(libc::EAGAIN);

    for result in [Ok(0), Ok(4), Err(Error::BADF), Err(again)] {
        let mut calls = 0;
        assert_eq!(
            write_all(b"abc", |_| {
                calls += 1;
                result
            }),
            Err(fmt::Error)
        );
        assert_eq!(calls, 1);
    }
}

#[test]
fn reports_an_error_after_a_partial_write() {
    let mut results = [Ok(1), Err(Error::BADF)].into_iter();
    assert_eq!(write_all(b"abc", |_| results.next().unwrap()), Err(fmt::Error));
    assert!(results.next().is_none());
}

#[test]
fn standard_descriptors_have_the_expected_numbers() {
    // SAFETY: the std test process keeps its standard descriptors open.
    let (stdout, stderr) = unsafe { (crate::stdio::stdout(), crate::stdio::stderr()) };
    assert_eq!(stdout.as_raw_fd(), 1);
    assert_eq!(stderr.as_raw_fd(), 2);
}
