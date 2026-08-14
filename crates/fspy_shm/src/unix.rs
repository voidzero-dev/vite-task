//! Unix shared memory backed by a sparse temporary file and identified by its
//! path.

use std::{
    ffi::{CString, OsStr},
    io,
    num::NonZeroUsize,
    os::unix::ffi::OsStrExt as _,
    ptr::{self, NonNull},
};

/// Opened shared memory that is not mapped yet.
///
/// [`map`](Self::map) can be called more than once; every call returns another
/// view of the same bytes. Drop the handle once the mappings exist.
pub struct ShmHandle {
    file: fspy_nostd::OwnedFd,
    size: NonZeroUsize,
}

/// The mapped shared bytes.
///
/// A `Mapping` keeps the bytes alive until it is dropped and cannot affect the
/// shared memory's identifier.
pub struct Mapping {
    ptr: NonNull<u8>,
    len: NonZeroUsize,
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
/// # Errors
///
/// Returns an error if the shared memory cannot be created or sized. A file
/// created by the failing call is removed before it returns.
pub fn create(path: &OsStr, size: usize) -> io::Result<ShmHandle> {
    let size = NonZeroUsize::new(size).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "shared-memory size must be nonzero")
    })?;
    let size_u64 = u64::try_from(size.get()).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "shared-memory size exceeds u64")
    })?;

    let file = open_file(
        path,
        fspy_nostd::fs::OFlags::RDWR
            | fspy_nostd::fs::OFlags::CREATE
            | fspy_nostd::fs::OFlags::EXCL
            | fspy_nostd::fs::OFlags::CLOEXEC,
        // Only the creating user may open the mapping.
        fspy_nostd::fs::Mode::RUSR | fspy_nostd::fs::Mode::WUSR,
    )?;

    // Every byte reads as zero because the file is all holes.
    if let Err(error) = fspy_nostd::fs::ftruncate(&file, size_u64) {
        // Do not hand the caller an unusable partial file to clean up.
        let _ = remove(path);
        return Err(error_to_io(error));
    }

    Ok(ShmHandle { file, size })
}

/// Opens the shared memory backed by the file at `path`.
///
/// # Errors
///
/// Returns an error if the shared memory is unavailable, which is the common
/// case once the backing file has been removed.
pub fn open(path: &OsStr) -> io::Result<ShmHandle> {
    let file = open_file(
        path,
        fspy_nostd::fs::OFlags::RDWR | fspy_nostd::fs::OFlags::CLOEXEC,
        fspy_nostd::fs::Mode::empty(),
    )?;
    // If another process shrinks the file before `map`, mapping fails. If it
    // resizes afterwards, nothing here touches the mapped pages. A concurrent
    // resize cannot make a mapping access invalid memory.
    let size = usize::try_from(fspy_nostd::fs::fstat(&file).map_err(error_to_io)?.st_size)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid shared-memory size"))?;
    let size = NonZeroUsize::new(size)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "shared-memory size is zero"))?;
    Ok(ShmHandle { file, size })
}

fn open_file(
    path: &OsStr,
    flags: fspy_nostd::fs::OFlags,
    mode: fspy_nostd::fs::Mode,
) -> io::Result<fspy_nostd::OwnedFd> {
    let path = CString::new(path.as_bytes())?;
    fspy_nostd::fs::openat(fspy_nostd::CWD, as_nostd_path(&path), flags, mode).map_err(error_to_io)
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
pub fn remove(path: &OsStr) -> io::Result<()> {
    let path = CString::new(path.as_bytes())?;
    fspy_nostd::fs::unlinkat(
        fspy_nostd::CWD,
        as_nostd_path(&path),
        fspy_nostd::fs::AtFlags::empty(),
    )
    .map_err(error_to_io)
}

fn as_nostd_path(path: &CString) -> fspy_nostd::CStr<'_, fspy_nostd::Fat> {
    // SAFETY: `CString` contains no interior NUL and includes one terminating
    // NUL; the returned view borrows it.
    unsafe { fspy_nostd::CStr::from_units_with_nul_unchecked(path.as_bytes_with_nul()) }
}

fn error_to_io(error: fspy_nostd::Error) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

impl ShmHandle {
    /// Maps the shared bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the mapping cannot be established.
    pub fn map(&self) -> io::Result<Mapping> {
        let _slice_len = isize::try_from(self.size.get()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "shared-memory size exceeds isize")
        })?;
        // SAFETY: the address is only a hint, the validated nonzero length is
        // representable as a Rust slice, the descriptor remains borrowed, and
        // the resulting shared mapping is owned by `Mapping`.
        let mapped = unsafe {
            fspy_nostd::mm::mmap(
                ptr::null_mut(),
                self.size.get(),
                fspy_nostd::mm::ProtFlags::READ | fspy_nostd::mm::ProtFlags::WRITE,
                fspy_nostd::mm::MapFlags::SHARED,
                &self.file,
                0,
            )
        }
        .map_err(error_to_io)?;
        let Some(ptr) = NonNull::new(mapped.cast()) else {
            // `mmap` reports failure with `MAP_FAILED`, not null, so this is a
            // successful mapping at address zero. Rust references cannot
            // represent it.
            // SAFETY: release that complete mapping before returning an error.
            let _ = unsafe { fspy_nostd::mm::munmap(mapped, self.size.get()) };
            return Err(io::Error::other("mmap returned address zero"));
        };
        Ok(Mapping { ptr, len: self.size })
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: this is the complete mapping owned by `self`, and dropping
        // it proves that no safe borrow through `self` remains.
        let _ = unsafe { fspy_nostd::mm::munmap(self.ptr.as_ptr().cast(), self.len.get()) };
    }
}

#[expect(clippy::len_without_is_empty, reason = "shared-memory mappings are always non-empty")]
impl Mapping {
    /// Returns the mapped length in bytes.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len.get()
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
        // SAFETY: The mapping is valid for its full length, and the caller
        // guarantees that it is not mutated while the slice is borrowed.
        unsafe { std::slice::from_raw_parts(self.as_ptr().cast_const(), self.len()) }
    }
}
