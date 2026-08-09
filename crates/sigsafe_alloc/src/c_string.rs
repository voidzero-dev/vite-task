use core::mem::MaybeUninit;

use allocator_api2::{alloc::Allocator, boxed::Box, vec::Vec};
use sigsafe::Fat;

/// An allocator-backed owned C string.
pub struct CString<R, A: Allocator> {
    bytes: Box<[MaybeUninit<u8>], A>,
    repr: R,
}

impl<R, A: Allocator> CString<R, A> {
    /// Creates a C string from caller-initialized storage and its metadata.
    ///
    /// # Safety
    ///
    /// `bytes` must begin with the C string described by `repr`.
    pub(crate) unsafe fn from_buffer_unchecked<const N: usize>(
        bytes: Box<[MaybeUninit<u8>; N], A>,
        repr: R,
    ) -> Self {
        let (bytes, allocator) = Box::into_raw_with_allocator(bytes);
        let bytes = core::ptr::slice_from_raw_parts_mut(bytes.cast(), N);
        // SAFETY: the array and slice have the same element layout and length.
        let bytes = unsafe { Box::from_raw_in(bytes, allocator) };
        Self { bytes, repr }
    }
}

impl<A: Allocator> CString<Fat, A> {
    /// Consumes this C string and returns its bytes without the terminating NUL.
    ///
    /// This reuses the existing allocation without scanning the string.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8, A> {
        let Self { bytes, repr } = self;
        let len = repr.len_with_nul() - 1;
        let capacity = bytes.len();
        let (bytes, allocator) = Box::into_raw_with_allocator(bytes);

        // SAFETY: the box was allocated for `capacity` bytes with `allocator`.
        // The first `len` bytes are initialized and the next is the NUL.
        unsafe { Vec::from_raw_parts_in(bytes.cast::<u8>(), len, capacity, allocator) }
    }
}
