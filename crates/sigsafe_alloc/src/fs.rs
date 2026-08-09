//! Allocating filesystem wrappers.

use allocator_api2::{alloc::Allocator, vec::Vec};
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
}
