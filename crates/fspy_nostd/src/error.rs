#[cfg(target_os = "macos")]
use libc::__error;
#[cfg(windows)]
use windows_sys::Win32::Foundation::GetLastError;

/// An operating-system error code.
#[cfg(any(target_os = "linux", target_os = "none", target_os = "macos"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct Error(i32);

/// An operating-system error code.
#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct Error(u32);

#[cfg(any(target_os = "linux", target_os = "none", target_os = "macos"))]
impl Error {
    pub const BADF: Self = Self(errno::BADF);
    pub const INVAL: Self = Self(errno::INVAL);
    pub const NOENT: Self = Self(errno::NOENT);
    pub const NOMEM: Self = Self(errno::NOMEM);
    pub const OVERFLOW: Self = Self(errno::OVERFLOW);
    pub const RANGE: Self = Self(errno::RANGE);

    /// Creates an error from a raw errno value.
    #[must_use]
    pub const fn from_raw_os_error(code: i32) -> Self {
        Self(code)
    }

    /// Returns the raw errno value.
    #[must_use]
    pub const fn raw_os_error(self) -> i32 {
        self.0
    }

    /// Returns the calling thread's last operating-system error.
    ///
    /// Call this immediately after the failing libc call: anything in between,
    /// including drops, can overwrite the thread-local error code.
    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn last_os_error() -> Self {
        // SAFETY: libSystem exposes the calling thread's errno through this
        // non-null pointer.
        Self::from_raw_os_error(unsafe { __error().read() })
    }
}

#[cfg(windows)]
impl Error {
    /// Creates an error from a raw Windows error code.
    #[must_use]
    pub const fn from_raw_os_error(code: u32) -> Self {
        Self(code)
    }

    /// Returns the raw Windows error code.
    #[must_use]
    pub const fn raw_os_error(self) -> u32 {
        self.0
    }

    /// Returns the calling thread's last operating-system error.
    ///
    /// Call this immediately after the failing Win32 call: anything in
    /// between, including drops, can overwrite the thread-local error code.
    #[must_use]
    pub fn last_os_error() -> Self {
        // SAFETY: `GetLastError` reads thread-local error state.
        Self::from_raw_os_error(unsafe { GetLastError() })
    }
}

#[cfg(any(target_os = "linux", target_os = "none"))]
impl From<syscalls::Errno> for Error {
    fn from(error: syscalls::Errno) -> Self {
        Self::from_raw_os_error(error.into_raw())
    }
}

#[cfg(any(target_os = "linux", target_os = "none"))]
mod errno {
    pub const BADF: i32 = linux_raw_sys::errno::EBADF.cast_signed();
    pub const INVAL: i32 = linux_raw_sys::errno::EINVAL.cast_signed();
    pub const NOMEM: i32 = linux_raw_sys::errno::ENOMEM.cast_signed();
    pub const NOENT: i32 = linux_raw_sys::errno::ENOENT.cast_signed();
    pub const OVERFLOW: i32 = linux_raw_sys::errno::EOVERFLOW.cast_signed();
    pub const RANGE: i32 = linux_raw_sys::errno::ERANGE.cast_signed();
}

#[cfg(target_os = "macos")]
mod errno {
    pub const BADF: i32 = libc::EBADF;
    pub const INVAL: i32 = libc::EINVAL;
    pub const NOMEM: i32 = libc::ENOMEM;
    pub const NOENT: i32 = libc::ENOENT;
    pub const OVERFLOW: i32 = libc::EOVERFLOW;
    pub const RANGE: i32 = libc::ERANGE;
}

pub type Result<T> = core::result::Result<T, Error>;
