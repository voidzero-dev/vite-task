//! Allocating filesystem wrappers.

use allocator_api2::{alloc::Allocator, vec::Vec};
#[cfg(target_os = "linux")]
use sigsafe::{BorrowedFd, CStr, Thin};
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
    let mut bytes = Vec::new_in(allocator);
    bytes.try_reserve_exact(sigsafe::fs::PATH_MAX).map_err(|_| Errno::NOMEM)?;
    let initialized =
        sigsafe::fs::getcwd(&mut bytes.spare_capacity_mut()[..sigsafe::fs::PATH_MAX])?
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
/// Returns [`Errno::NOMEM`] if storage cannot be allocated, or the error from
/// [`sigsafe::fs::readlinkat`].
#[cfg(target_os = "linux")]
pub fn readlinkat<A: Allocator>(
    allocator: A,
    dirfd: BorrowedFd<'_>,
    path: CStr<'_, Thin>,
) -> Result<Vec<u8, A>> {
    let mut bytes = Vec::new_in(allocator);
    bytes.try_reserve_exact(sigsafe::fs::PATH_MAX).map_err(|_| Errno::NOMEM)?;

    loop {
        let capacity = bytes.capacity();
        let initialized = sigsafe::fs::readlinkat(dirfd, path, bytes.spare_capacity_mut())?.len();
        if initialized < capacity {
            // SAFETY: `readlinkat` initialized this prefix.
            unsafe { bytes.set_len(initialized) };
            return Ok(bytes);
        }

        // The length remains zero, so reserve the desired total capacity.
        let next_capacity = capacity.checked_mul(2).ok_or(Errno::NOMEM)?;
        bytes.try_reserve_exact(next_capacity).map_err(|_| Errno::NOMEM)?;
    }
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    #[test]
    fn getcwd_allocates_a_c_string() {
        let path = super::getcwd(Global).unwrap();
        let mut buffer = [core::mem::MaybeUninit::uninit(); sigsafe::fs::PATH_MAX];
        let expected = sigsafe::fs::getcwd(&mut buffer).unwrap();
        let expected = expected.as_bytes_with_nul();

        assert_eq!(path.as_c_str().as_bytes_with_nul(), expected);
        assert_eq!(path.as_bytes(), &expected[..expected.len() - 1]);
        assert_eq!(path.as_bytes_with_nul(), expected);
        assert_eq!(path.into_bytes().as_slice(), &expected[..expected.len() - 1]);

        let path = super::getcwd(Global).unwrap();
        assert_eq!(path.into_bytes_with_nul().as_slice(), expected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn readlinkat_allocates_the_complete_target() {
        // SAFETY: the literal is NUL-terminated and lives for the call.
        let path = unsafe { sigsafe::CStr::<sigsafe::Thin>::from_ptr(c"/proc/self/exe".as_ptr()) };
        let target = super::readlinkat(Global, sigsafe::CWD, path).unwrap();

        assert_eq!(
            target.as_slice(),
            std::fs::read_link("/proc/self/exe").unwrap().as_os_str().as_encoded_bytes()
        );
    }
}
