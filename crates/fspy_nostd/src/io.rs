//! I/O calls with caller-owned buffers.

use crate::{BorrowedFd, Error, Result};

/// Reads into `buf` with a single `read(2)` and returns the initialized byte
/// count.
///
/// # Errors
///
/// Returns the error reported by `read`.
#[cfg(any(target_os = "linux", target_os = "none"))]
pub fn read(fd: BorrowedFd<'_>, buf: &mut [u8]) -> Result<usize> {
    // SAFETY: `fd` stays borrowed and `buf` is writable for its whole length.
    unsafe {
        syscalls::syscall!(syscalls::Sysno::read, fd.as_raw_fd(), buf.as_mut_ptr(), buf.len())
    }
    .map_err(Error::from)
}

/// Reads from `fd` starting at byte `offset` into `buf` without modifying the
/// descriptor's file offset, returning the initialized byte count.
///
/// # Errors
///
/// Returns [`Error::INVAL`] if `offset` exceeds `i64::MAX`, or the error
/// reported by `pread`.
pub fn pread(fd: BorrowedFd<'_>, buf: &mut [u8], offset: u64) -> Result<usize> {
    let offset = i64::try_from(offset).map_err(|_| Error::INVAL)?;
    #[cfg(any(target_os = "linux", target_os = "none"))]
    {
        // SAFETY: `fd` stays borrowed and `buf` is writable for its whole length.
        unsafe {
            syscalls::syscall!(
                syscalls::Sysno::pread64,
                fd.as_raw_fd(),
                buf.as_mut_ptr(),
                buf.len(),
                offset
            )
        }
        .map_err(Error::from)
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `fd` stays borrowed and `buf` is writable for its whole length.
        let read =
            unsafe { libc::pread(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), offset) };
        if read == -1 { Err(Error::last_os_error()) } else { Ok(read.cast_unsigned()) }
    }
}

/// Writes `buf` to `fd` with a single `write(2)` and returns the number of
/// bytes the kernel accepted.
///
/// A short write is reported as-is, exactly like the syscall; the caller
/// decides whether to write the remainder.
///
/// # Errors
///
/// Returns the error reported by `write`.
pub fn write(fd: BorrowedFd<'_>, buf: &[u8]) -> Result<usize> {
    #[cfg(any(target_os = "linux", target_os = "none"))]
    {
        // SAFETY: `fd` stays borrowed and `buf` is readable for its full length.
        unsafe {
            syscalls::syscall!(syscalls::Sysno::write, fd.as_raw_fd(), buf.as_ptr(), buf.len())
        }
        .map_err(Error::from)
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `fd` stays borrowed and `buf` is readable for its full length.
        let written = unsafe { libc::write(fd.as_raw_fd(), buf.as_ptr().cast(), buf.len()) };
        if written == -1 { Err(Error::last_os_error()) } else { Ok(written.cast_unsigned()) }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Seek as _, SeekFrom, Write as _},
        os::fd::AsRawFd as _,
    };

    use super::pread;
    use crate::{BorrowedFd, Error};

    #[test]
    fn pread_reads_at_offset_without_mutating_seek_position() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"abcdef").unwrap();
        file.seek(SeekFrom::Start(3)).unwrap();

        // SAFETY: `file` remains open for the duration of the calls.
        let fd = unsafe { BorrowedFd::borrow_raw(file.as_raw_fd()) };

        let mut head = [0u8; 3];
        assert_eq!(pread(fd, &mut head, 0).unwrap(), 3);
        assert_eq!(&head, b"abc");

        // Offset 0 read must not disturb the current seek position.
        assert_eq!(file.stream_position().unwrap(), 3);

        let mut mid = [0u8; 2];
        assert_eq!(pread(fd, &mut mid, 2).unwrap(), 2);
        assert_eq!(&mid, b"cd");
        assert_eq!(file.stream_position().unwrap(), 3);

        let mut tail = [0u8; 3];
        file.read_exact(&mut tail).unwrap();
        assert_eq!(&tail, b"def");

        assert_eq!(pread(fd, &mut head, u64::MAX), Err(Error::INVAL));
    }
}
