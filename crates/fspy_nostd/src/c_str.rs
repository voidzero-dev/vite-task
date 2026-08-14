use core::{iter::FusedIterator, marker::PhantomData, num::NonZeroUsize, ptr::NonNull, slice};

mod private {
    pub trait Sealed {
        const NUL: Self;
    }

    impl Sealed for u8 {
        const NUL: Self = 0;
    }

    impl Sealed for u16 {
        const NUL: Self = 0;
    }
}

/// A code unit supported by [`CStr`].
pub trait CStrUnit: private::Sealed + Copy + Eq {}

impl CStrUnit for u8 {}
impl CStrUnit for u16 {}

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

/// A borrowed NUL-terminated string of code units.
///
/// [`CStr<'_, Thin>`] stores only the string pointer, while
/// [`CStr<'_, Fat>`] also stores the length including the terminating NUL.
#[derive(Clone, Copy)]
pub struct CStr<'a, R, U: CStrUnit = u8> {
    ptr: NonNull<U>,
    repr: R,
    lifetime: PhantomData<&'a U>,
}

/// A borrowed NUL-terminated string of `u16` code units.
pub type WideCStr<'a, R> = CStr<'a, R, u16>;

/// A borrowed NUL-terminated string of the platform's native path code
/// units: bytes on Unix and wide (`u16`) code units on Windows.
#[cfg(unix)]
pub type OsCStr<'a, R> = CStr<'a, R>;
/// A borrowed NUL-terminated string of the platform's native path code
/// units: bytes on Unix and wide (`u16`) code units on Windows.
#[cfg(windows)]
pub type OsCStr<'a, R> = WideCStr<'a, R>;

/// An iterator over the non-NUL code units of a thin C string.
#[derive(Clone)]
pub struct Units<'a, U: CStrUnit> {
    ptr: NonNull<U>,
    lifetime: PhantomData<&'a U>,
}

impl<U: CStrUnit> Iterator for Units<'_, U> {
    type Item = U;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        // SAFETY: `ptr` starts within a valid C string and is advanced only
        // after reading a non-NUL code unit, so it remains readable and never
        // moves beyond the terminating NUL.
        unsafe {
            let unit = self.ptr.read();
            if unit == U::NUL {
                None
            } else {
                self.ptr = self.ptr.add(1);
                Some(unit)
            }
        }
    }
}

impl<U: CStrUnit> FusedIterator for Units<'_, U> {}

impl<R, U: CStrUnit> CStr<'_, R, U> {
    /// Returns a pointer to the first code unit of this C string.
    #[must_use]
    pub const fn as_ptr(&self) -> *const U {
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

impl<'a, U: CStrUnit> CStr<'a, Thin, U> {
    /// Creates a thin C string view from a non-null code-unit pointer without
    /// finding its length.
    ///
    /// # Safety
    ///
    /// `ptr` must point to an immutable NUL-terminated string that remains
    /// valid for the lifetime of the returned view.
    #[must_use]
    pub const unsafe fn from_non_null(ptr: NonNull<U>) -> Self {
        Self { ptr, repr: Thin { _private: () }, lifetime: PhantomData }
    }

    /// Creates a thin C string view from a code-unit pointer without finding
    /// its length.
    ///
    /// # Safety
    ///
    /// `ptr` must be non-null and point to an immutable NUL-terminated string
    /// that remains valid for the lifetime of the returned view.
    #[must_use]
    pub const unsafe fn from_ptr(ptr: *const U) -> Self {
        // SAFETY: the caller guarantees that `ptr` is non-null.
        let ptr = unsafe { NonNull::new_unchecked(ptr.cast_mut()) };
        // SAFETY: the caller guarantees the remaining C string invariants.
        unsafe { Self::from_non_null(ptr) }
    }

    /// Returns an iterator over the code units before the terminating NUL.
    #[inline]
    #[must_use]
    pub const fn units(self) -> Units<'a, U> {
        Units { ptr: self.ptr, lifetime: PhantomData }
    }

    /// Counts through the terminating NUL and returns a length-retaining view.
    #[inline]
    #[must_use]
    pub fn count(self) -> CStr<'a, Fat, U> {
        let count = self.units().count();

        CStr {
            ptr: self.ptr,
            repr: Fat {
                // SAFETY: adding the terminator makes the represented length
                // nonzero, and a valid allocation cannot contain `usize::MAX`
                // non-NUL code units.
                len_with_nul: unsafe { NonZeroUsize::new_unchecked(count + 1) },
            },
            lifetime: PhantomData,
        }
    }
}

impl<'a, U: CStrUnit> CStr<'a, Fat, U> {
    /// Creates a length-retaining C string from code units that end with the
    /// single NUL terminator.
    ///
    /// Returns [`None`] when `units` is empty, does not end with a NUL code
    /// unit, or contains an interior NUL code unit.
    #[must_use]
    pub fn from_units_with_nul(units: &'a [U]) -> Option<Self> {
        let (last, rest) = units.split_last()?;
        if *last != U::NUL || rest.contains(&U::NUL) {
            return None;
        }
        // SAFETY: `units` ends with exactly one NUL code unit and contains no
        // other NUL code units.
        Some(unsafe { Self::from_units_with_nul_unchecked(units) })
    }

    /// Creates a length-retaining C string from code units without validation.
    ///
    /// # Safety
    ///
    /// `units` must end with exactly one NUL code unit and contain no other
    /// NUL code units.
    #[must_use]
    pub const unsafe fn from_units_with_nul_unchecked(units: &'a [U]) -> Self {
        Self {
            // SAFETY: a valid C string is nonempty, so its pointer is non-null.
            ptr: unsafe { NonNull::new_unchecked(units.as_ptr().cast_mut()) },
            repr: Fat {
                // SAFETY: a valid C string contains at least its terminating NUL.
                len_with_nul: unsafe { NonZeroUsize::new_unchecked(units.len()) },
            },
            lifetime: PhantomData,
        }
    }

    /// Discards the retained length and returns a thin view of the same
    /// string.
    #[must_use]
    pub const fn as_thin(self) -> CStr<'a, Thin, U> {
        CStr { ptr: self.ptr, repr: Thin { _private: () }, lifetime: PhantomData }
    }

    /// Returns the string's code units without the terminating NUL.
    #[must_use]
    pub const fn as_units(&self) -> &'a [U] {
        let units = self.as_units_with_nul();
        units.split_at(units.len() - 1).0
    }

    /// Returns the string's code units, including the terminating NUL.
    #[must_use]
    pub const fn as_units_with_nul(&self) -> &'a [U] {
        // SAFETY: this view carries the exact initialized C string length.
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len_with_nul()) }
    }

    /// Returns the number of code units including the terminating NUL.
    #[must_use]
    pub const fn len_with_nul(&self) -> usize {
        self.repr.len_with_nul()
    }
}

#[cfg(test)]
mod tests {
    use core::{mem::size_of, ptr::NonNull};

    use super::{CStr, Fat, Thin, WideCStr};

    #[test]
    fn representations_retain_the_expected_metadata() {
        // SAFETY: the input contains one trailing NUL.
        let fat = unsafe { CStr::<Fat>::from_units_with_nul_unchecked(b"abc\0") };
        // SAFETY: the input contains one trailing NUL.
        let counted = unsafe { CStr::<Thin>::from_ptr(c"abc".as_ptr().cast()) }.count();

        assert_eq!(size_of::<CStr<'_, Thin>>(), size_of::<*const u8>());
        assert_eq!(size_of::<CStr<'_, Fat>>(), size_of::<(*const u8, usize)>());
        assert_eq!(size_of::<WideCStr<'_, Thin>>(), size_of::<*const u16>());
        assert_eq!(size_of::<WideCStr<'_, Fat>>(), size_of::<(*const u16, usize)>());
        assert_eq!(fat.len_with_nul(), 4);
        assert_eq!(fat.as_units(), b"abc");
        assert_eq!(fat.as_units_with_nul(), b"abc\0");
        assert_eq!(counted.as_units_with_nul(), fat.as_units_with_nul());
    }

    #[test]
    fn checked_construction_accepts_only_a_single_trailing_nul() {
        let checked = CStr::<Fat>::from_units_with_nul(b"abc\0").unwrap();

        assert_eq!(checked.as_units(), b"abc");
        assert_eq!(checked.len_with_nul(), 4);
        assert!(CStr::<Fat>::from_units_with_nul(b"").is_none());
        assert!(CStr::<Fat>::from_units_with_nul(b"abc").is_none());
        assert!(CStr::<Fat>::from_units_with_nul(b"a\0c\0").is_none());
        assert!(WideCStr::<Fat>::from_units_with_nul(&[0u16]).is_some());
    }

    #[test]
    fn thin_view_accepts_a_checked_non_null_pointer() {
        let ptr = NonNull::new(c"abc".as_ptr().cast_mut().cast()).unwrap();
        // SAFETY: the literal is an immutable NUL-terminated string.
        let thin = unsafe { CStr::<Thin>::from_non_null(ptr) };

        assert!(thin.units().eq(b"abc".iter().copied()));
    }

    #[test]
    fn thin_units_exclude_the_nul_and_remain_fused() {
        let mut units = {
            // SAFETY: the literal is NUL-terminated and outlives the iterator.
            let thin = unsafe { CStr::<Thin>::from_ptr(c"abc".as_ptr().cast()) };
            thin.units()
        };

        assert_eq!(units.by_ref().collect::<Vec<_>>(), b"abc");
        assert_eq!(units.next(), None);
        assert_eq!(units.next(), None);
    }

    #[test]
    fn wide_strings_iterate_and_retain_code_unit_lengths() {
        let units = [u16::from(b'a'), u16::from(b'b'), 0];
        // SAFETY: the input contains one trailing NUL.
        let fat = unsafe { WideCStr::<Fat>::from_units_with_nul_unchecked(&units) };
        // SAFETY: the same input is immutable and NUL-terminated.
        let thin = unsafe { WideCStr::<Thin>::from_ptr(units.as_ptr()) };

        assert!(thin.units().eq([u16::from(b'a'), u16::from(b'b')]));
        assert_eq!(thin.count().as_units_with_nul(), units);
        assert_eq!(fat.as_units(), [u16::from(b'a'), u16::from(b'b')]);
        assert_eq!(fat.len_with_nul(), 3);
    }
}
