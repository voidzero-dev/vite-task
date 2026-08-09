//! Allocating filesystem wrappers.

use allocator_api2::{alloc::Allocator, boxed::Box};
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
}
