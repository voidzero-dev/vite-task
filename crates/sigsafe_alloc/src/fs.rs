//! Allocating filesystem wrappers.

#[cfg(target_os = "linux")]
use allocator_api2::vec::Vec;
use allocator_api2::{alloc::Allocator, boxed::Box};
#[cfg(target_os = "linux")]
use sigsafe::CStr;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use sigsafe::Thin;
use sigsafe::{Errno, Fat, Result};

use crate::CString;

/// Returns the absolute pathname of the current working directory, allocated
/// with `allocator`.
///
/// # Errors
///
/// Returns [`Errno::NOMEM`] if storage cannot be allocated, or the error from
/// [`sigsafe::fs::getcwd`].
pub fn getcwd<A: Allocator>(allocator: A) -> Result<CString<Fat, A>> {
    let mut bytes = path_buffer(allocator)?;
    let repr = sigsafe::fs::getcwd(&mut bytes[..])?.into_repr();

    // SAFETY: `getcwd` initialized the C string described by `repr`.
    Ok(unsafe { CString::from_buffer_unchecked(bytes, repr) })
}

/// Returns the target of `path`, allocated with `allocator`.
///
/// The buffer grows and retries when `readlinkat` fills it, because the
/// underlying operation does not otherwise distinguish an exact fit from a
/// truncated result.
///
/// # Errors
///
/// Returns [`Errno::NOMEM`] if storage cannot be allocated, or the error from
/// [`sigsafe::fs::readlink`].
#[cfg(target_os = "linux")]
pub fn readlink<A: Allocator>(allocator: A, path: CStr<'_, Thin>) -> Result<Vec<u8, A>> {
    let mut bytes = Vec::new_in(allocator);
    bytes.try_reserve_exact(sigsafe::fs::PATH_MAX).map_err(|_| Errno::NOMEM)?;

    loop {
        let capacity = bytes.capacity();
        let initialized = sigsafe::fs::readlink(path, bytes.spare_capacity_mut())?.len();
        if initialized < capacity {
            // SAFETY: `readlink` initialized this prefix.
            unsafe { bytes.set_len(initialized) };
            return Ok(bytes);
        }

        // A full buffer may be truncated. It is entirely initialized, so use
        // it as the current length while requesting twice the storage.
        // SAFETY: `readlink` returned `capacity`, initializing every byte.
        unsafe { bytes.set_len(capacity) };
        bytes.try_reserve_exact(capacity).map_err(|_| Errno::NOMEM)?;
        bytes.clear();
    }
}

/// Returns the path associated with `fd`, allocated with `allocator`.
///
/// The returned C string remains thin because `F_GETPATH` reports no length.
///
/// # Errors
///
/// Returns [`Errno::NOMEM`] if storage cannot be allocated, or the error from
/// [`sigsafe::fs::fcntl_getpath`].
#[cfg(target_os = "macos")]
pub fn fcntl_getpath<A: Allocator>(
    allocator: A,
    fd: sigsafe::BorrowedFd<'_>,
) -> Result<CString<Thin, A>> {
    let mut bytes = path_buffer(allocator)?;
    let repr = sigsafe::fs::fcntl_getpath(fd, &mut bytes)?.into_repr();

    // SAFETY: `fcntl_getpath` initialized the C string described by `repr`.
    Ok(unsafe { CString::from_buffer_unchecked(bytes, repr) })
}

fn path_buffer<A: Allocator>(
    allocator: A,
) -> Result<Box<[core::mem::MaybeUninit<u8>; sigsafe::fs::PATH_MAX], A>> {
    let bytes =
        Box::<[core::mem::MaybeUninit<u8>; sigsafe::fs::PATH_MAX], A>::try_new_uninit_in(allocator)
            .map_err(|_| Errno::NOMEM)?;

    // SAFETY: an array of `MaybeUninit<u8>` requires no initialization.
    Ok(unsafe { bytes.assume_init() })
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    #[test]
    fn getcwd_allocates_a_c_string() {
        let path = super::getcwd(Global).unwrap().into_bytes();
        let mut buffer = [core::mem::MaybeUninit::uninit(); sigsafe::fs::PATH_MAX];
        let expected = sigsafe::fs::getcwd(&mut buffer).unwrap();

        assert_eq!(path.as_slice(), &expected.as_bytes_with_nul()[..expected.len_with_nul() - 1]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn fcntl_getpath_allocates_a_thin_c_string() {
        use std::{fs::File, os::fd::AsRawFd as _};

        let root = File::open("/").unwrap();
        // SAFETY: `root` remains open for the call.
        let root = unsafe { sigsafe::BorrowedFd::borrow_raw(root.as_raw_fd()) };
        let path = super::fcntl_getpath(Global, root).unwrap();

        assert_eq!(path.as_c_str().count().as_bytes_with_nul(), b"/\0");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn readlink_allocates_the_complete_target() {
        // SAFETY: the literal is NUL-terminated and lives for the call.
        let path = unsafe { sigsafe::CStr::<sigsafe::Thin>::from_ptr(c"/proc/self/exe".as_ptr()) };
        let target = super::readlink(Global, path).unwrap();

        assert_eq!(
            target.as_slice(),
            std::fs::read_link("/proc/self/exe").unwrap().as_os_str().as_encoded_bytes()
        );
    }
}
