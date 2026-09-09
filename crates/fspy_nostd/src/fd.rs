use core::marker::PhantomData;

/// A raw Unix file descriptor.
pub type RawFd = i32;

/// A borrowed file descriptor or a reserved descriptor value accepted by the
/// receiving system call, such as [`CWD`].
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct BorrowedFd<'fd> {
    fd: RawFd,
    lifetime: PhantomData<&'fd OwnedFd>,
}

/// An owned file descriptor, closed on drop.
#[repr(transparent)]
pub struct OwnedFd {
    fd: RawFd,
}

impl BorrowedFd<'_> {
    /// Borrows a raw descriptor or reserved descriptor value.
    ///
    /// # Safety
    ///
    /// A real descriptor must remain open for the returned lifetime. A
    /// reserved value must be valid for every operation receiving the borrow.
    /// The value `-1` is never permitted.
    ///
    /// # Panics
    ///
    /// Panics when `fd` is `-1`.
    #[must_use]
    pub const unsafe fn borrow_raw(fd: RawFd) -> Self {
        assert!(fd != -1, "-1 is not a borrowed file descriptor");
        Self { fd, lifetime: PhantomData }
    }

    /// Returns the raw descriptor without transferring ownership.
    #[must_use]
    pub const fn as_raw_fd(self) -> RawFd {
        self.fd
    }
}

impl OwnedFd {
    /// Takes ownership of a raw descriptor returned by the operating system.
    ///
    /// # Safety
    ///
    /// `fd` must be a valid, uniquely owned descriptor that may be closed.
    pub(crate) const unsafe fn from_raw_fd(fd: RawFd) -> Self {
        debug_assert!(fd >= 0);
        Self { fd }
    }

    /// Borrows this descriptor.
    #[must_use]
    pub const fn as_fd(&self) -> BorrowedFd<'_> {
        // SAFETY: `self` keeps the descriptor open for the returned lifetime.
        unsafe { BorrowedFd::borrow_raw(self.fd) }
    }

    /// Returns the raw descriptor without transferring ownership.
    #[must_use]
    pub const fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl Drop for OwnedFd {
    fn drop(&mut self) {
        #[cfg(any(target_os = "linux", target_os = "none"))]
        {
            // SAFETY: this type owns the descriptor and closes it exactly
            // once. Close errors cannot be acted on during drop.
            let _ = unsafe { syscalls::syscall!(syscalls::Sysno::close, self.fd) };
        }
        #[cfg(any(target_os = "macos", target_os = "freebsd"))]
        {
            // SAFETY: this type owns the descriptor and closes it exactly
            // once. Close errors cannot be acted on during drop.
            let _ = unsafe { libc::close(self.fd) };
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "none"))]
const CWD_RAW: RawFd = linux_raw_sys::general::AT_FDCWD;
#[cfg(any(target_os = "macos", target_os = "freebsd"))]
const CWD_RAW: RawFd = libc::AT_FDCWD;

/// The reserved directory descriptor representing the current directory.
// SAFETY: `AT_FDCWD` is a permanent reserved value accepted by `*at` calls.
pub const CWD: BorrowedFd<'static> = unsafe { BorrowedFd::borrow_raw(CWD_RAW) };
