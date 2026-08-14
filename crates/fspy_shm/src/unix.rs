//! Unix shared memory backed by a sparse temporary file and identified by its
//! path.

use core::ptr::{self, NonNull};

use fspy_nostd::{OsCStr, Result, Thin};

/// Opened shared memory that is not mapped yet.
///
/// [`map`](Self::map) can be called more than once; every call returns another
/// view of the same bytes. Drop the handle once the mappings exist.
pub struct ShmHandle {
    file: fspy_nostd::OwnedFd,
    size: usize,
}

/// The mapped shared bytes.
///
/// A `Mapping` keeps the bytes alive until it is dropped and cannot affect the
/// shared memory's identifier.
pub struct Mapping {
    ptr: NonNull<u8>,
    len: usize,
}

// SAFETY: a mapping owns no thread-affine state; access synchronization is
// supplied by the fspy channel built on top of it.
unsafe impl Send for Mapping {}
// SAFETY: sharing a `Mapping` does not itself access its bytes, and all actual
// concurrent access is synchronized by the fspy channel.
unsafe impl Sync for Mapping {}

/// Creates `size` bytes of zero-initialized shared memory backed by the file
/// at `path`.
///
/// The file must not exist yet and is created with mode `0o600`. The path is
/// how other processes reach the shared memory, so the caller should supply
/// an absolute path that keeps working across working-directory changes.
///
/// Returns an already opened [`ShmHandle`], so the creating process never has
/// to look its own file up by path.
///
/// Only pages that are actually written occupy memory or disk, so a large
/// capacity is cheap.
///
/// A `size` of zero is not rejected here: mapping the empty file fails with
/// the OS's own error.
///
/// # Errors
///
/// Returns the error reported while creating or sizing the shared memory. A
/// file created by the failing call is removed before it returns.
pub fn create(path: OsCStr<'_, Thin>, size: usize) -> Result<ShmHandle> {
    let file = fspy_nostd::fs::openat(
        fspy_nostd::CWD,
        path,
        fspy_nostd::fs::OFlags::RDWR
            | fspy_nostd::fs::OFlags::CREATE
            | fspy_nostd::fs::OFlags::EXCL
            | fspy_nostd::fs::OFlags::CLOEXEC,
        // Only the creating user may open the mapping.
        fspy_nostd::fs::Mode::RUSR | fspy_nostd::fs::Mode::WUSR,
    )?;

    // Every byte reads as zero because the file is all holes.
    if let Err(error) = fspy_nostd::fs::ftruncate(&file, size as u64) {
        // Do not hand the caller an unusable partial file to clean up.
        let _ = remove(path);
        return Err(error);
    }

    Ok(ShmHandle { file, size })
}

/// Opens the shared memory backed by the file at `path`.
///
/// The file's size is read here but validated by [`map`](ShmHandle::map):
/// mapping an empty or oversized file fails there with the OS's own error.
///
/// # Errors
///
/// Returns the error reported while opening the file, which is the common
/// case once the backing file has been removed.
pub fn open(path: OsCStr<'_, Thin>) -> Result<ShmHandle> {
    let file = fspy_nostd::fs::openat(
        fspy_nostd::CWD,
        path,
        fspy_nostd::fs::OFlags::RDWR | fspy_nostd::fs::OFlags::CLOEXEC,
        fspy_nostd::fs::Mode::empty(),
    )?;
    // If another process shrinks the file before `map`, mapping fails. If it
    // resizes afterwards, nothing here touches the mapped pages. A concurrent
    // resize cannot make a mapping access invalid memory.
    //
    // A regular file's size is never negative.
    let size = crate::file_size_to_len(fspy_nostd::fs::fstat(&file)?.st_size.cast_unsigned());
    Ok(ShmHandle { file, size })
}

/// Removes the shared memory at `path`.
///
/// Removal is cleanup, not a stop signal: later opens fail, but existing
/// [`ShmHandle`]s and [`Mapping`]s keep reading and writing. To stop them,
/// store a flag in the shared bytes, as the fspy channel's close gate does.
///
/// # Errors
///
/// Returns the error reported while unlinking the path.
pub fn remove(path: OsCStr<'_, Thin>) -> Result<()> {
    fspy_nostd::fs::unlinkat(fspy_nostd::CWD, path, fspy_nostd::fs::AtFlags::empty())
}

impl ShmHandle {
    /// Maps the shared bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the mapping cannot be established.
    pub fn map(&self) -> Result<Mapping> {
        let len = self.size;
        // SAFETY: the address is only a hint, the descriptor remains
        // borrowed, and the resulting shared mapping is owned by `Mapping`.
        // The kernel rejects zero and address-space-exceeding lengths.
        let mapped = unsafe {
            fspy_nostd::mm::mmap(
                ptr::null_mut(),
                len,
                fspy_nostd::mm::ProtFlags::READ | fspy_nostd::mm::ProtFlags::WRITE,
                fspy_nostd::mm::MapFlags::SHARED,
                &self.file,
                0,
            )
        }?;
        let Some(ptr) = NonNull::new(mapped.cast()) else {
            // `mmap` reports failure with `MAP_FAILED`, not null, so this is a
            // successful mapping at address zero. Rust references cannot
            // represent it.
            // SAFETY: release that complete mapping before returning an error.
            let _ = unsafe { fspy_nostd::mm::munmap(mapped, len) };
            return Err(fspy_nostd::Error::INVAL);
        };
        Ok(Mapping { ptr, len })
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: this is the complete mapping owned by `self`, and dropping
        // it proves that no safe borrow through `self` remains.
        let _ = unsafe { fspy_nostd::mm::munmap(self.ptr.as_ptr().cast(), self.len) };
    }
}

#[expect(clippy::len_without_is_empty, reason = "shared-memory mappings are always non-empty")]
impl Mapping {
    /// Returns the mapped length in bytes.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns a raw pointer to the first mapped byte.
    #[must_use]
    pub const fn as_ptr(&self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    /// Returns the mapped bytes as a shared slice.
    ///
    /// # Safety
    ///
    /// The caller must ensure that no process or thread mutates the mapping for
    /// the lifetime of the returned slice.
    #[must_use]
    pub const unsafe fn as_slice(&self) -> &[u8] {
        // SAFETY: The mapping is valid for its full length, which fits the
        // virtual address space and is therefore far below the `isize::MAX`
        // slice bound on every supported 64-bit target, and the caller
        // guarantees that it is not mutated while the slice is borrowed.
        unsafe { core::slice::from_raw_parts(self.as_ptr().cast_const(), self.len()) }
    }
}
