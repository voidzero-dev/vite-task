use core::{mem::MaybeUninit, ptr, slice};

use allocator_api2::{alloc::Allocator, boxed::Box, vec::Vec};
use fspy_nostd::{CStr, CStrUnit, Fat, Thin};

/// An allocator-backed owned C string.
pub struct CString<R, A: Allocator, U: CStrUnit = u8> {
    units: Box<[MaybeUninit<U>], A>,
    repr: R,
}

/// An allocator-backed owned C string of the platform's native path code
/// units: bytes on Unix and wide (`u16`) code units on Windows.
#[cfg(unix)]
pub type OsCString<R, A> = CString<R, A>;
/// An allocator-backed owned C string of the platform's native path code
/// units: bytes on Unix and wide (`u16`) code units on Windows.
#[cfg(windows)]
pub type OsCString<R, A> = CString<R, A, u16>;

impl<A: Allocator, U: CStrUnit> CString<Fat, A, U> {
    /// Converts a code-unit vector that ends with the single NUL terminator
    /// to a C string.
    ///
    /// Returns [`None`] when `units` is empty, does not end with a NUL code
    /// unit, or contains an interior NUL code unit.
    #[must_use]
    pub fn from_vec_with_nul(units: Vec<U, A>) -> Option<Self> {
        CStr::<Fat, U>::from_units_with_nul(&units)?;
        // SAFETY: validated just above.
        Some(unsafe { Self::from_vec_with_nul_unchecked(units) })
    }

    /// Converts a code-unit vector to a C string without checking its
    /// contents.
    ///
    /// # Safety
    ///
    /// `units` must end with exactly one NUL code unit and contain no other
    /// NUL code units.
    #[must_use]
    pub unsafe fn from_vec_with_nul_unchecked(units: Vec<U, A>) -> Self {
        // SAFETY: upheld by the caller.
        let repr = unsafe { CStr::<Fat, U>::from_units_with_nul_unchecked(&units) }.into_repr();
        let (units, _len, capacity, allocator) = units.into_raw_parts_with_alloc();
        let units = ptr::slice_from_raw_parts_mut(units.cast::<MaybeUninit<U>>(), capacity);

        Self {
            // SAFETY: this uses the vector's original pointer, capacity, and
            // allocator. Its initialized prefix is described by `repr`; any
            // spare capacity is valid as `MaybeUninit<U>`.
            units: unsafe { Box::from_raw_in(units, allocator) },
            repr,
        }
    }
}

impl<A: Allocator, U: CStrUnit> CString<Thin, A, U> {
    /// Converts boxed storage to a thin C string without checking its
    /// contents.
    ///
    /// # Safety
    ///
    /// `units` must begin with a valid NUL-terminated C string. Code units
    /// after that string may be uninitialized.
    #[must_use]
    pub unsafe fn from_boxed_with_nul_unchecked(units: Box<[MaybeUninit<U>], A>) -> Self {
        // SAFETY: upheld by the caller. Creating a thin view does not scan the
        // allocation or retain the string length.
        let repr = unsafe { CStr::<Thin, U>::from_ptr(units.as_ptr().cast()) }.into_repr();
        Self { units, repr }
    }

    /// Returns a thin borrowed view of this C string.
    #[must_use]
    pub fn as_c_str(&self) -> CStr<'_, Thin, U> {
        // SAFETY: upheld by the constructor; `self` owns the storage.
        unsafe { CStr::from_ptr(self.units.as_ptr().cast()) }
    }

    /// Counts through the terminating NUL and returns a length-retaining C
    /// string using the same allocation.
    #[must_use]
    pub fn count(self) -> CString<Fat, A, U> {
        let repr = self.as_c_str().count().into_repr();
        CString { units: self.units, repr }
    }
}

impl<A: Allocator, U: CStrUnit> CString<Fat, A, U> {
    /// Extracts a borrowed C string view containing the entire string.
    #[must_use]
    pub fn as_c_str(&self) -> CStr<'_, Fat, U> {
        // SAFETY: the initialized prefix described by `repr` is a C string.
        unsafe { CStr::from_units_with_nul_unchecked(self.as_units_with_nul()) }
    }

    /// Returns the contents of this C string without the terminating NUL.
    #[must_use]
    pub fn as_units(&self) -> &[U] {
        let units = self.as_units_with_nul();
        units.split_at(units.len() - 1).0
    }

    /// Returns the contents of this C string, including the terminating NUL.
    #[must_use]
    pub fn as_units_with_nul(&self) -> &[U] {
        // SAFETY: `repr` describes the initialized C string prefix.
        unsafe { slice::from_raw_parts(self.units.as_ptr().cast(), self.repr.len_with_nul()) }
    }

    /// Consumes this C string and returns its code units without the
    /// terminating NUL.
    #[must_use = "`self` will be dropped if the result is not used"]
    pub fn into_units(self) -> Vec<U, A> {
        let len = self.repr.len_with_nul() - 1;
        self.into_vec(len)
    }

    /// Consumes this C string and returns its code units, including the
    /// terminating NUL.
    #[must_use = "`self` will be dropped if the result is not used"]
    pub fn into_units_with_nul(self) -> Vec<U, A> {
        let len = self.repr.len_with_nul();
        self.into_vec(len)
    }

    fn into_vec(self, len: usize) -> Vec<U, A> {
        let Self { units, .. } = self;
        let capacity = units.len();
        let (units, allocator) = Box::into_raw_with_allocator(units);

        // SAFETY: the box was allocated for `capacity` units with `allocator`.
        // The first `len` units are initialized.
        unsafe { Vec::from_raw_parts_in(units.cast::<U>(), len, capacity, allocator) }
    }
}
