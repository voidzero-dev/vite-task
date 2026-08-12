use core::{mem::MaybeUninit, ptr, slice};

use allocator_api2::{alloc::Allocator, boxed::Box, vec::Vec};
use fspy_nostd::{CStr, Fat, Thin};

/// An allocator-backed owned C string.
pub struct CString<R, A: Allocator> {
    bytes: Box<[MaybeUninit<u8>], A>,
    repr: R,
}

impl<A: Allocator> CString<Fat, A> {
    /// Converts a byte vector to a C string without checking its contents.
    ///
    /// # Safety
    ///
    /// `bytes` must end with exactly one NUL byte and contain no other NUL
    /// bytes.
    #[must_use]
    pub unsafe fn from_vec_with_nul_unchecked(bytes: Vec<u8, A>) -> Self {
        // SAFETY: upheld by the caller.
        let repr = unsafe { CStr::<Fat>::from_bytes_with_nul_unchecked(&bytes) }.into_repr();
        let (bytes, _len, capacity, allocator) = bytes.into_raw_parts_with_alloc();
        let bytes = ptr::slice_from_raw_parts_mut(bytes.cast::<MaybeUninit<u8>>(), capacity);

        Self {
            // SAFETY: this uses the vector's original pointer, capacity, and
            // allocator. Its initialized prefix is described by `repr`; any
            // spare capacity is valid as `MaybeUninit<u8>`.
            bytes: unsafe { Box::from_raw_in(bytes, allocator) },
            repr,
        }
    }
}

impl<A: Allocator> CString<Thin, A> {
    /// Converts boxed storage to a thin C string without checking its
    /// contents.
    ///
    /// # Safety
    ///
    /// `bytes` must begin with a valid NUL-terminated C string. Bytes after
    /// that string may be uninitialized.
    #[must_use]
    pub unsafe fn from_boxed_with_nul_unchecked(bytes: Box<[MaybeUninit<u8>], A>) -> Self {
        // SAFETY: upheld by the caller. Creating a thin view does not scan the
        // allocation or retain the string length.
        let repr = unsafe { CStr::<Thin>::from_ptr(bytes.as_ptr().cast()) }.into_repr();
        Self { bytes, repr }
    }

    /// Returns a thin borrowed view of this C string.
    #[must_use]
    pub fn as_c_str(&self) -> CStr<'_, Thin> {
        // SAFETY: upheld by the constructor; `self` owns the storage.
        unsafe { CStr::from_ptr(self.bytes.as_ptr().cast()) }
    }

    /// Counts through the terminating NUL and returns a length-retaining C
    /// string using the same allocation.
    #[must_use]
    pub fn count(self) -> CString<Fat, A> {
        let repr = self.as_c_str().count().into_repr();
        CString { bytes: self.bytes, repr }
    }
}

impl<A: Allocator> CString<Fat, A> {
    /// Extracts a borrowed C string view containing the entire string.
    #[must_use]
    pub fn as_c_str(&self) -> CStr<'_, Fat> {
        // SAFETY: the initialized prefix described by `repr` is a C string.
        unsafe { CStr::from_bytes_with_nul_unchecked(self.as_bytes_with_nul()) }
    }

    /// Returns the contents of this C string without the terminating NUL.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        let bytes = self.as_bytes_with_nul();
        bytes.split_at(bytes.len() - 1).0
    }

    /// Returns the contents of this C string, including the terminating NUL.
    #[must_use]
    pub fn as_bytes_with_nul(&self) -> &[u8] {
        // SAFETY: `repr` describes the initialized C string prefix.
        unsafe { slice::from_raw_parts(self.bytes.as_ptr().cast(), self.repr.len_with_nul()) }
    }

    /// Consumes this C string and returns its bytes without the terminating NUL.
    #[must_use = "`self` will be dropped if the result is not used"]
    pub fn into_bytes(self) -> Vec<u8, A> {
        let len = self.repr.len_with_nul() - 1;
        self.into_vec(len)
    }

    /// Consumes this C string and returns its bytes, including the terminating
    /// NUL.
    #[must_use = "`self` will be dropped if the result is not used"]
    pub fn into_bytes_with_nul(self) -> Vec<u8, A> {
        let len = self.repr.len_with_nul();
        self.into_vec(len)
    }

    fn into_vec(self, len: usize) -> Vec<u8, A> {
        let Self { bytes, .. } = self;
        let capacity = bytes.len();
        let (bytes, allocator) = Box::into_raw_with_allocator(bytes);

        // SAFETY: the box was allocated for `capacity` bytes with `allocator`.
        // The first `len` bytes are initialized.
        unsafe { Vec::from_raw_parts_in(bytes.cast::<u8>(), len, capacity, allocator) }
    }
}
