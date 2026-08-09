use core::{ffi::c_char, marker::PhantomData, num::NonZeroUsize, ptr::NonNull, slice};

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
    /// Creates a thin C string view without finding its length.
    ///
    /// # Safety
    ///
    /// `ptr` must be non-null and point to an immutable NUL-terminated string
    /// that remains valid for the lifetime of the returned view.
    #[must_use]
    pub const unsafe fn from_ptr(ptr: *const c_char) -> Self {
        Self {
            // SAFETY: upheld by the caller.
            ptr: unsafe { NonNull::new_unchecked(ptr.cast_mut()) },
            repr: Thin { _private: () },
            lifetime: PhantomData,
        }
    }

    /// Counts through the terminating NUL and returns a length-retaining view.
    #[must_use]
    pub const fn count(self) -> CStr<'a, Fat> {
        let mut count = 0;
        // SAFETY: `CStr<Thin>` points to a NUL-terminated string.
        while unsafe { self.as_ptr().add(count).read() } != 0 {
            count += 1;
        }

        CStr {
            ptr: self.ptr,
            repr: Fat {
                // SAFETY: the count includes at least the terminating NUL.
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
    use core::mem::size_of;

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
}
