//! Allocating filesystem wrappers.

#[cfg(target_os = "macos")]
use allocator_api2::boxed::Box;
use allocator_api2::{alloc::Allocator, vec::Vec};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use fspy_nostd::Thin;
#[cfg(target_os = "linux")]
use fspy_nostd::{BorrowedFd, CStr};
use fspy_nostd::{Error, Fat, Result};

use crate::CString;

/// Returns the absolute pathname of the current working directory, allocated
/// with `allocator`.
///
/// # Errors
///
/// Returns [`Error::NOMEM`] if storage cannot be allocated, or the error from
/// [`fspy_nostd::fs::getcwd`].
pub fn getcwd<A: Allocator>(allocator: A) -> Result<CString<Fat, A>> {
    let mut bytes = Vec::new_in(allocator);
    bytes.try_reserve_exact(fspy_nostd::fs::PATH_MAX).map_err(|_| Error::NOMEM)?;
    let initialized =
        fspy_nostd::fs::getcwd(&mut bytes.spare_capacity_mut()[..fspy_nostd::fs::PATH_MAX])?
            .len_with_nul();

    // SAFETY: `getcwd` initialized this prefix.
    unsafe { bytes.set_len(initialized) };

    // SAFETY: `getcwd` returned one C string including its terminating NUL.
    Ok(unsafe { CString::from_vec_with_nul_unchecked(bytes) })
}

/// Returns the target of `path` relative to `dirfd`, allocated with
/// `allocator`.
///
/// The buffer grows and retries when `readlinkat` fills it, because the
/// underlying operation does not otherwise distinguish an exact fit from a
/// truncated result.
///
/// # Errors
///
/// Returns [`Error::NOMEM`] if storage cannot be allocated, or the error from
/// [`fspy_nostd::fs::readlinkat`].
#[cfg(target_os = "linux")]
pub fn readlinkat<A: Allocator>(
    allocator: A,
    dirfd: BorrowedFd<'_>,
    path: CStr<'_, Thin>,
) -> Result<Vec<u8, A>> {
    let mut bytes = Vec::new_in(allocator);
    bytes.try_reserve_exact(fspy_nostd::fs::PATH_MAX).map_err(|_| Error::NOMEM)?;

    loop {
        let capacity = bytes.capacity();
        let initialized =
            fspy_nostd::fs::readlinkat(dirfd, path, bytes.spare_capacity_mut())?.len();
        if initialized < capacity {
            // SAFETY: `readlinkat` initialized this prefix.
            unsafe { bytes.set_len(initialized) };
            return Ok(bytes);
        }

        // The length remains zero, so reserve the desired total capacity.
        let next_capacity = capacity.checked_mul(2).ok_or(Error::NOMEM)?;
        bytes.try_reserve_exact(next_capacity).map_err(|_| Error::NOMEM)?;
    }
}

/// Returns the path associated with `fd`, allocated with `allocator`.
///
/// The returned C string remains thin because `F_GETPATH` reports no length.
///
/// # Errors
///
/// Returns [`Error::NOMEM`] if storage cannot be allocated, or the error from
/// [`fspy_nostd::fs::fcntl_getpath`].
#[cfg(target_os = "macos")]
pub fn fcntl_getpath<A: Allocator>(
    allocator: A,
    fd: fspy_nostd::BorrowedFd<'_>,
) -> Result<CString<Thin, A>> {
    let mut bytes = path_buffer(allocator)?;
    fspy_nostd::fs::fcntl_getpath(fd, &mut bytes)?;

    // SAFETY: `fcntl_getpath` initialized a NUL-terminated string at the start
    // of `bytes`.
    Ok(unsafe { CString::from_boxed_with_nul_unchecked(Box::slice(bytes)) })
}

#[cfg(target_os = "macos")]
fn path_buffer<A: Allocator>(
    allocator: A,
) -> Result<Box<[core::mem::MaybeUninit<u8>; fspy_nostd::fs::PATH_MAX], A>> {
    let bytes =
        Box::<[core::mem::MaybeUninit<u8>; fspy_nostd::fs::PATH_MAX], A>::try_new_uninit_in(
            allocator,
        )
        .map_err(|_| Error::NOMEM)?;

    // SAFETY: an array of `MaybeUninit<u8>` requires no initialization.
    Ok(unsafe { bytes.assume_init() })
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    #[test]
    fn getcwd_allocates_a_c_string() {
        let path = super::getcwd(Global).unwrap();
        let mut buffer = [core::mem::MaybeUninit::uninit(); fspy_nostd::fs::PATH_MAX];
        let expected = fspy_nostd::fs::getcwd(&mut buffer).unwrap();
        let expected = expected.as_units_with_nul();

        assert_eq!(path.as_c_str().as_units_with_nul(), expected);
        assert_eq!(path.as_bytes(), &expected[..expected.len() - 1]);
        assert_eq!(path.as_bytes_with_nul(), expected);
        assert_eq!(path.into_bytes().as_slice(), &expected[..expected.len() - 1]);

        let path = super::getcwd(Global).unwrap();
        assert_eq!(path.into_bytes_with_nul().as_slice(), expected);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn fcntl_getpath_allocates_a_thin_c_string() {
        use std::{fs::File, os::fd::AsRawFd as _};

        let root = File::open("/").unwrap();
        // SAFETY: `root` remains open for the call.
        let root = unsafe { fspy_nostd::BorrowedFd::borrow_raw(root.as_raw_fd()) };
        let path = super::fcntl_getpath(Global, root).unwrap();

        assert_eq!(path.as_c_str().count().as_units_with_nul(), b"/\0");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn readlinkat_allocates_the_complete_target() {
        // SAFETY: the literal is NUL-terminated and lives for the call.
        let path = unsafe {
            fspy_nostd::CStr::<fspy_nostd::Thin>::from_ptr(c"/proc/self/exe".as_ptr().cast())
        };
        let target = super::readlinkat(Global, fspy_nostd::CWD, path).unwrap();

        assert_eq!(
            target.as_slice(),
            std::fs::read_link("/proc/self/exe").unwrap().as_os_str().as_encoded_bytes()
        );
    }
}
