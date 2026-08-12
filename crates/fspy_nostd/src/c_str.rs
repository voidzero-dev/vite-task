use core::{
    ffi::c_char, iter::FusedIterator, marker::PhantomData, num::NonZeroUsize, ptr::NonNull, slice,
};

/// Marks a [`CStr`] whose length is not known.
#[derive(Clone, Copy)]
pub struct Thin {
    _private: (),
}

/// Marks a [`CStr`] that retains its length, including the terminating NUL.
#[derive(Clone, Copy)]
pub struct Fat {
    len_with_nul: NonZeroUsize,
}

/// A borrowed NUL-terminated string.
///
/// [`CStr<'_, Thin>`] stores only the string pointer, while
/// [`CStr<'_, Fat>`] also stores the length including the terminating NUL.
#[derive(Clone, Copy)]
pub struct CStr<'a, R> {
    ptr: NonNull<c_char>,
    repr: R,
    lifetime: PhantomData<&'a c_char>,
}

/// An iterator over the non-NUL bytes of a thin C string.
#[derive(Clone)]
pub struct Bytes<'a> {
    ptr: NonNull<u8>,
    lifetime: PhantomData<&'a u8>,
}

impl Iterator for Bytes<'_> {
    type Item = u8;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        // SAFETY: `ptr` starts within a valid C string and is advanced only
        // after reading a non-NUL byte, so it remains readable and never moves
        // beyond the terminating NUL.
        unsafe {
            let byte = self.ptr.read();
            if byte == 0 {
                None
            } else {
                self.ptr = self.ptr.add(1);
                Some(byte)
            }
        }
    }
}

impl FusedIterator for Bytes<'_> {}

impl<R> CStr<'_, R> {
    /// Returns a pointer to the first byte of this C string.
    #[must_use]
    pub const fn as_ptr(&self) -> *const c_char {
        self.ptr.as_ptr()
    }

    /// Consumes this C string view and returns its representation metadata.
    #[must_use]
    pub fn into_repr(self) -> R {
        self.repr
    }
}

impl Fat {
    /// Returns the represented length, including the terminating NUL.
    #[must_use]
    pub const fn len_with_nul(self) -> usize {
        self.len_with_nul.get()
    }
}

impl<'a> CStr<'a, Thin> {
    /// Creates a thin C string view from a non-null pointer without finding
    /// its length.
    ///
    /// # Safety
    ///
    /// `ptr` must point to an immutable NUL-terminated string that remains
    /// valid for the lifetime of the returned view.
    #[must_use]
    pub const unsafe fn from_non_null(ptr: NonNull<c_char>) -> Self {
        Self { ptr, repr: Thin { _private: () }, lifetime: PhantomData }
    }

    /// Creates a thin C string view without finding its length.
    ///
    /// # Safety
    ///
    /// `ptr` must be non-null and point to an immutable NUL-terminated string
    /// that remains valid for the lifetime of the returned view.
    #[must_use]
    pub const unsafe fn from_ptr(ptr: *const c_char) -> Self {
        // SAFETY: the caller guarantees that `ptr` is non-null.
        let ptr = unsafe { NonNull::new_unchecked(ptr.cast_mut()) };
        // SAFETY: the caller guarantees the remaining C string invariants.
        unsafe { Self::from_non_null(ptr) }
    }

    /// Returns an iterator over the bytes before the terminating NUL.
    #[inline]
    #[must_use]
    pub const fn bytes(self) -> Bytes<'a> {
        Bytes { ptr: self.ptr.cast(), lifetime: PhantomData }
    }

    /// Counts through the terminating NUL and returns a length-retaining view.
    #[inline]
    #[must_use]
    pub fn count(self) -> CStr<'a, Fat> {
        let count = self.bytes().count();

        CStr {
            ptr: self.ptr,
            repr: Fat {
                // SAFETY: adding the terminator makes the represented length
                // nonzero, and a valid allocation cannot contain `usize::MAX`
                // non-NUL bytes.
                len_with_nul: unsafe { NonZeroUsize::new_unchecked(count + 1) },
            },
            lifetime: PhantomData,
        }
    }
}

impl<'a> CStr<'a, Fat> {
    /// Creates a length-retaining C string from bytes without validation.
    ///
    /// # Safety
    ///
    /// `bytes` must end with exactly one NUL byte and contain no other NUL
    /// bytes.
    #[must_use]
    pub const unsafe fn from_bytes_with_nul_unchecked(bytes: &'a [u8]) -> Self {
        Self {
            // SAFETY: a valid C string is nonempty, so its pointer is non-null.
            ptr: unsafe { NonNull::new_unchecked(bytes.as_ptr().cast::<c_char>().cast_mut()) },
            repr: Fat {
                // SAFETY: a valid C string contains at least its terminating NUL.
                len_with_nul: unsafe { NonZeroUsize::new_unchecked(bytes.len()) },
            },
            lifetime: PhantomData,
        }
    }

    /// Returns the string's bytes without the terminating NUL.
    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        let bytes = self.as_bytes_with_nul();
        bytes.split_at(bytes.len() - 1).0
    }

    /// Returns the string's bytes, including the terminating NUL.
    #[must_use]
    pub const fn as_bytes_with_nul(&self) -> &'a [u8] {
        // SAFETY: this view carries the exact initialized C string length.
        unsafe { slice::from_raw_parts(self.ptr.as_ptr().cast(), self.len_with_nul()) }
    }

    /// Returns the number of bytes including the terminating NUL.
    #[must_use]
    pub const fn len_with_nul(&self) -> usize {
        self.repr.len_with_nul()
    }
}

#[cfg(test)]
mod tests {
    use core::{mem::size_of, ptr::NonNull};

    use super::{CStr, Fat, Thin};

    #[test]
    fn representations_retain_the_expected_metadata() {
        // SAFETY: the input contains one trailing NUL.
        let fat = unsafe { CStr::<Fat>::from_bytes_with_nul_unchecked(b"abc\0") };
        // SAFETY: the input contains one trailing NUL.
        let counted = unsafe { CStr::<Thin>::from_ptr(c"abc".as_ptr()) }.count();

        assert_eq!(size_of::<CStr<'_, Thin>>(), size_of::<*const u8>());
        assert_eq!(size_of::<CStr<'_, Fat>>(), size_of::<(*const u8, usize)>());
        assert_eq!(fat.len_with_nul(), 4);
        assert_eq!(fat.as_bytes(), b"abc");
        assert_eq!(fat.as_bytes_with_nul(), b"abc\0");
        assert_eq!(counted.as_bytes_with_nul(), fat.as_bytes_with_nul());
    }

    #[test]
    fn thin_view_accepts_a_checked_non_null_pointer() {
        let ptr = NonNull::new(c"abc".as_ptr().cast_mut()).unwrap();
        // SAFETY: the literal is an immutable NUL-terminated string.
        let thin = unsafe { CStr::<Thin>::from_non_null(ptr) };

        assert!(thin.bytes().eq(b"abc".iter().copied()));
    }

    #[test]
    fn thin_bytes_exclude_the_nul_and_remain_fused() {
        let mut bytes = {
            // SAFETY: the literal is NUL-terminated and outlives the iterator.
            let thin = unsafe { CStr::<Thin>::from_ptr(c"abc".as_ptr()) };
            thin.bytes()
        };

        assert_eq!(bytes.by_ref().collect::<Vec<_>>(), b"abc");
        assert_eq!(bytes.next(), None);
        assert_eq!(bytes.next(), None);
    }
}
