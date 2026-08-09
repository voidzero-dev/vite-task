use core::{ffi::c_char, marker::PhantomData, ptr::NonNull};

/// Marks a [`CStr`] whose length is not known.
#[derive(Clone, Copy)]
pub struct Thin {
    _private: (),
}

/// A borrowed NUL-terminated string.
///
/// [`CStr<'_, Thin>`] stores only the string pointer.
#[derive(Clone, Copy)]
pub struct CStr<'a, R> {
    ptr: NonNull<c_char>,
    #[expect(dead_code, reason = "the representation value carries the type state")]
    repr: R,
    lifetime: PhantomData<&'a c_char>,
}

impl<R> CStr<'_, R> {
    /// Returns a pointer to the first byte of this C string.
    #[must_use]
    pub const fn as_ptr(&self) -> *const c_char {
        self.ptr.as_ptr()
    }
}

impl CStr<'_, Thin> {
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
}

#[cfg(test)]
mod tests {
    use core::mem::size_of;

    use super::{CStr, Thin};

    #[test]
    fn thin_representation_is_one_pointer() {
        // SAFETY: the input contains one trailing NUL.
        let value = unsafe { CStr::<Thin>::from_ptr(c"abc".as_ptr()) };

        assert_eq!(size_of::<CStr<'_, Thin>>(), size_of::<*const u8>());
        assert_eq!(value.as_ptr(), c"abc".as_ptr());
    }
}
