//! Owned memory regions obtained from a [`Sys`] provider.

use core::{marker::PhantomData, mem, ptr::NonNull};

use crate::sys::Sys;

/// An owned region obtained from `S`, released on drop.
///
/// This is the only place that calls [`Sys::unmap`]: pool code either lets a
/// `Mapping` drop (probe scratch, install races, freed large allocations) or
/// deliberately leaks it with [`Mapping::into_raw`] (published slabs, live
/// large allocations). Reconstructing ownership from a raw pointer via
/// [`Mapping::from_raw`] is the single unsafe step.
pub struct Mapping<S: Sys> {
    ptr: NonNull<u8>,
    size: usize,
    align: usize,
    sys: PhantomData<fn() -> S>,
}

impl<S: Sys> Mapping<S> {
    /// Maps `size` bytes of zero-initialized memory aligned to `align`
    /// (a power of two). Returns `None` when memory is exhausted.
    pub fn new(size: usize, align: usize) -> Option<Self> {
        let ptr = S::map(size, align)?;
        Some(Self { ptr, size, align, sys: PhantomData })
    }

    /// Reclaims ownership of a mapping previously released with
    /// [`Mapping::into_raw`].
    ///
    /// # Safety
    ///
    /// `ptr` must have come from `Mapping::<S>::into_raw` (or `Sys::map`)
    /// with exactly this `size` and `align`, the region must not be in use,
    /// and ownership must not be reclaimed twice.
    pub unsafe fn from_raw(ptr: NonNull<u8>, size: usize, align: usize) -> Self {
        Self { ptr, size, align, sys: PhantomData }
    }

    /// The mapped region's base address.
    pub const fn ptr(&self) -> NonNull<u8> {
        self.ptr
    }

    /// Releases ownership without unmapping; the region lives until (unless)
    /// [`Mapping::from_raw`] reclaims it.
    pub const fn into_raw(self) -> NonNull<u8> {
        let ptr = self.ptr;
        mem::forget(self);
        ptr
    }
}

impl<S: Sys> Drop for Mapping<S> {
    fn drop(&mut self) {
        // SAFETY: this type owns the mapping (constructed from `Sys::map`
        // directly or via the `from_raw` contract), and after drop nothing
        // can use it.
        unsafe { S::unmap(self.ptr, self.size, self.align) }
    }
}
