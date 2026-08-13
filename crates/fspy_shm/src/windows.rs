//! Windows shared memory backed by a sparse temporary file and identified by
//! its path.

use core::{ffi::c_void, mem::size_of, ptr};
use std::{
    env::temp_dir, ffi::OsStr, io, num::NonZeroUsize, os::windows::ffi::OsStrExt as _,
    path::PathBuf,
};
#[cfg(test)]
use std::{fs::File, os::windows::io::AsRawHandle as _};

use uuid::Uuid;
#[cfg(test)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_STANDARD_INFO, FileStandardInfo, GetFileInformationByHandleEx,
};

use crate::BACKING_PREFIX;

const SHARE_ALL: fspy_nostd::fs::ShareMode = fspy_nostd::fs::ShareMode::READ
    .union(fspy_nostd::fs::ShareMode::WRITE)
    .union(fspy_nostd::fs::ShareMode::DELETE);

/// Keeps the shared memory's identifier alive and removes it on drop.
///
/// Removal is cleanup, not a stop signal: later opens fail, but existing
/// [`ShmHandle`]s and [`Mapping`]s keep reading and writing. To stop them,
/// store a flag in the shared bytes, as the fspy channel's close gate does.
pub struct ShmKeeper {
    path: PathBuf,
}

/// Opened shared memory that is not mapped yet.
///
/// [`map`](Self::map) can be called more than once; every call returns another
/// view of the same bytes. Drop the handle once the mappings exist.
pub struct ShmHandle {
    file: fspy_nostd::OwnedHandle,
    size: NonZeroUsize,
}

/// The mapped shared bytes.
///
/// A `Mapping` keeps the bytes alive until it is dropped and cannot affect the
/// shared memory's identifier.
pub struct Mapping {
    view: fspy_nostd::mm::MappingView,
    len: NonZeroUsize,
}

/// Creates `size` bytes of zero-initialized shared memory.
///
/// Returns its [`ShmKeeper`] and an already opened [`ShmHandle`], so the
/// creating process never has to go through [`open`].
///
/// Only pages that are actually written occupy memory or disk, so a large
/// capacity is cheap.
///
/// # Errors
///
/// Returns an error if the shared memory cannot be created or sized. Creation
/// fails on volumes without sparse-file support.
pub fn create(size: usize) -> io::Result<(ShmKeeper, ShmHandle)> {
    let size = NonZeroUsize::new(size).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "shared-memory size must be nonzero")
    })?;
    let size_i64 = i64::try_from(size.get()).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "shared-memory size exceeds i64")
    })?;

    // The per-user `%TEMP%` ACL provides same-user gating. The identifier is
    // absolute so it keeps working after a working-directory change.
    let path = std::path::absolute(temp_dir())?
        .join(format!("{BACKING_PREFIX}{}.shm", Uuid::new_v4().simple()));
    let file = open_file(
        path.as_os_str(),
        fspy_nostd::fs::Access::READ | fspy_nostd::fs::Access::WRITE,
        fspy_nostd::fs::CreationDisposition::CREATE_NEW,
        // Ask Windows to keep the data in memory when it can.
        fspy_nostd::fs::OpenFlags::TEMPORARY,
    )?;
    // The keeper exists from here on, so every error path below cleans up.
    let keeper = ShmKeeper { path };

    // NTFS allocates clusters for the whole logical size unless the file is
    // marked sparse first, which would turn the capacity into real disk usage.
    // Volumes without sparse-file support fail here.
    fspy_nostd::fs::set_sparse(&file).map_err(error_to_io)?;
    // Every byte reads as zero because the file is all holes.
    fspy_nostd::fs::set_len(&file, size_i64).map_err(error_to_io)?;

    Ok((keeper, ShmHandle { file, size }))
}

/// Opens the shared memory identified by `id`.
///
/// The identifier works from any process, regardless of the process's working
/// directory or environment.
///
/// # Errors
///
/// Returns an error if the shared memory is unavailable, which is the common
/// case once its keeper has been dropped.
pub fn open(id: &OsStr) -> io::Result<ShmHandle> {
    let file = open_file(
        id,
        fspy_nostd::fs::Access::READ | fspy_nostd::fs::Access::WRITE,
        fspy_nostd::fs::CreationDisposition::OPEN_EXISTING,
        fspy_nostd::fs::OpenFlags::empty(),
    )?;
    // If another process shrinks the file before `map`, mapping fails. If it
    // resizes afterwards, nothing here touches the mapped pages. A concurrent
    // resize cannot make a mapping access invalid memory.
    let size = usize::try_from(fspy_nostd::fs::file_size(&file).map_err(error_to_io)?)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid shared-memory size"))?;
    let size = NonZeroUsize::new(size)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "shared-memory size is zero"))?;
    Ok(ShmHandle { file, size })
}

fn open_file(
    path: &OsStr,
    access: fspy_nostd::fs::Access,
    creation: fspy_nostd::fs::CreationDisposition,
    flags: fspy_nostd::fs::OpenFlags,
) -> io::Result<fspy_nostd::OwnedHandle> {
    let path = copy_path(path)?;
    fspy_nostd::fs::open(as_nostd_path(&path), access, SHARE_ALL, creation, flags)
        .map_err(error_to_io)
}

fn copy_path(path: &OsStr) -> io::Result<Vec<u16>> {
    let mut units: Vec<_> = path.encode_wide().collect();
    if units.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"));
    }
    units.push(0);
    Ok(units)
}

const fn as_nostd_path(path: &[u16]) -> fspy_nostd::WideCStr<'_, fspy_nostd::Fat> {
    // SAFETY: `copy_path` rejects interior NUL and appends one terminator.
    unsafe { fspy_nostd::WideCStr::from_units_with_nul_unchecked(path) }
}

fn error_to_io(error: fspy_nostd::Error) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error().cast_signed())
}

fn remove_file(path: fspy_nostd::WideCStr<'_, fspy_nostd::Fat>) -> fspy_nostd::Result<()> {
    let error = match fspy_nostd::fs::delete_file(path) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    if error.raw_os_error() == fspy_nostd::fs::ERROR_ACCESS_DENIED
        && let Ok(file) = fspy_nostd::fs::open(
            path,
            fspy_nostd::fs::Access::DELETE,
            SHARE_ALL,
            fspy_nostd::fs::CreationDisposition::OPEN_EXISTING,
            fspy_nostd::fs::OpenFlags::OPEN_REPARSE_POINT,
        )
        && set_posix_delete(&file).is_ok()
    {
        return Ok(());
    }

    // Preserve the original `DeleteFileW` error when POSIX deletion is not
    // available.
    Err(error)
}

fn set_posix_delete(file: &fspy_nostd::OwnedHandle) -> fspy_nostd::Result<()> {
    const INFO_SIZE: u32 = 4;
    const _: [(); 4] = [(); size_of::<fspy_nostd::fs::FILE_DISPOSITION_INFO_EX>()];

    let info = fspy_nostd::fs::FILE_DISPOSITION_INFO_EX {
        Flags: fspy_nostd::fs::FILE_DISPOSITION_FLAG_DELETE
            | fspy_nostd::fs::FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | fspy_nostd::fs::FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    // SAFETY: `info` has the type and exact size required by
    // `FileDispositionInfoEx`.
    unsafe {
        fspy_nostd::fs::set_file_information_by_handle(
            file,
            fspy_nostd::fs::FileDispositionInfoEx,
            (&raw const info).cast::<c_void>(),
            INFO_SIZE,
        )
    }
}

impl Drop for ShmKeeper {
    fn drop(&mut self) {
        // Windows versions without POSIX delete refuse to remove the name of a
        // mapped file. Arm the deferred delete instead: a handle opened with
        // `FILE_FLAG_DELETE_ON_CLOSE` deletes the file once every handle to it
        // is closed.
        let Ok(path) = copy_path(self.path.as_os_str()) else {
            return;
        };
        if remove_file(as_nostd_path(&path)).is_err() {
            let _ = fspy_nostd::fs::open(
                as_nostd_path(&path),
                fspy_nostd::fs::Access::DELETE,
                SHARE_ALL,
                fspy_nostd::fs::CreationDisposition::OPEN_EXISTING,
                fspy_nostd::fs::OpenFlags::DELETE_ON_CLOSE,
            );
        }
    }
}

impl ShmKeeper {
    /// Returns the shared memory's opaque identifier, which any process passes
    /// to [`open`].
    #[must_use]
    pub fn id(&self) -> &OsStr {
        self.path.as_os_str()
    }
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
        // SAFETY: null attributes create a non-inheritable mapping object and
        // a null name creates an unnamed one. Both maximum-size halves are
        // zero, so Windows uses the current file size.
        let mapping = unsafe {
            fspy_nostd::mm::create_file_mapping(
                &self.file,
                ptr::null(),
                fspy_nostd::mm::PAGE_READWRITE,
                0,
                0,
                ptr::null(),
            )
        }
        .map_err(error_to_io)?;
        let view = fspy_nostd::mm::map_view_of_file(
            &mapping,
            fspy_nostd::mm::FILE_MAP_READ | fspy_nostd::mm::FILE_MAP_WRITE,
            0,
            0,
            self.size.get(),
        )
        .map_err(error_to_io)?;
        // The view remains valid after its mapping-object handle closes.
        Ok(Mapping { view, len: self.size })
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
        self.view.as_ptr()
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
        unsafe { core::slice::from_raw_parts(self.as_ptr().cast_const(), self.len()) }
    }
}

/// Returns the backing file's logical size and allocated byte count.
#[cfg(test)]
pub fn file_sizes(file: &File) -> io::Result<(u64, u64)> {
    let mut info = FILE_STANDARD_INFO::default();
    let info_size = u32::try_from(std::mem::size_of::<FILE_STANDARD_INFO>())
        .map_err(|_| io::Error::other("file size information is too large"))?;
    // SAFETY: `file` supplies a valid handle and `info` is a writable
    // FILE_STANDARD_INFO buffer of exactly `info_size` bytes.
    let result = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle().cast(),
            FileStandardInfo,
            (&raw mut info).cast(),
            info_size,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }

    let logical_size = u64::try_from(info.EndOfFile)
        .map_err(|_| io::Error::other("file has a negative logical size"))?;
    let allocated_size = u64::try_from(info.AllocationSize)
        .map_err(|_| io::Error::other("file has a negative allocated size"))?;
    Ok((logical_size, allocated_size))
}
