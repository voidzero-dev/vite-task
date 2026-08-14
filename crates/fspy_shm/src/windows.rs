//! Windows shared memory backed by a sparse temporary file and identified by
//! its path.

use core::{ffi::c_void, mem::size_of, ptr};
use std::{ffi::OsStr, io, num::NonZeroUsize, os::windows::ffi::OsStrExt as _};
#[cfg(test)]
use std::{fs::File, os::windows::io::AsRawHandle as _};

use fspy_nostd::{
    BorrowedHandle, bool_result,
    fs::{CreationDisposition, FileAccess, FileOptions, FileShare},
    mm::{MappingAccess, PageProtection},
};
#[cfg(test)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_STANDARD_INFO, FileStandardInfo, GetFileInformationByHandleEx,
};
use windows_sys::Win32::{
    Storage::FileSystem::{
        FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX, FILE_END_OF_FILE_INFO,
        FileDispositionInfoEx, FileEndOfFileInfo, SetFileInformationByHandle,
    },
    System::{IO::DeviceIoControl, Ioctl::FSCTL_SET_SPARSE},
};

const SHARE_ALL: FileShare = FileShare::READ.union(FileShare::WRITE).union(FileShare::DELETE);

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

/// Creates `size` bytes of zero-initialized shared memory backed by the file
/// at `path`.
///
/// The file must not exist yet; the per-user `%TEMP%` ACL provides same-user
/// gating when `path` sits in the temporary directory. The path is how other
/// processes reach the shared memory, so the caller should supply an absolute
/// path that keeps working across working-directory changes. Paths at or
/// beyond the legacy `MAX_PATH` limit must already be in verbatim (`\\?\`)
/// form: this crate passes paths to the OS unchanged.
///
/// Returns an already opened [`ShmHandle`], so the creating process never has
/// to look its own file up by path.
///
/// Only pages that are actually written occupy memory or disk, so a large
/// capacity is cheap.
///
/// # Errors
///
/// Returns an error if the shared memory cannot be created or sized. Creation
/// fails on volumes without sparse-file support. A file created by the
/// failing call is removed before it returns.
pub fn create(path: &OsStr, size: usize) -> io::Result<ShmHandle> {
    let size = NonZeroUsize::new(size).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "shared-memory size must be nonzero")
    })?;
    let size_i64 = i64::try_from(size.get()).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "shared-memory size exceeds i64")
    })?;

    let file = open_file(
        path,
        FileAccess::GENERIC_READ | FileAccess::GENERIC_WRITE,
        CreationDisposition::CreateNew,
        // Ask Windows to keep the data in memory when it can. Opening the
        // reparse point itself makes a link planted at the fresh path fail
        // instead of redirecting the file, as std does for `create_new`.
        FileOptions::TEMPORARY | FileOptions::OPEN_REPARSE_POINT,
    )?;

    if let Err(error) = size_backing_file(file.as_handle(), size_i64) {
        // Do not hand the caller an unusable partial file to clean up.
        let _ = remove(path);
        return Err(error_to_io(error));
    }

    Ok(ShmHandle { file, size })
}

fn size_backing_file(file: BorrowedHandle<'_>, len: i64) -> fspy_nostd::Result<()> {
    // NTFS allocates clusters for the whole logical size unless the file is
    // marked sparse first, which would turn the capacity into real disk usage.
    // Volumes without sparse-file support fail here.
    set_sparse(file)?;
    // Every byte reads as zero because the file is all holes.
    set_end_of_file(file, len)
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
        FileAccess::GENERIC_READ | FileAccess::GENERIC_WRITE,
        CreationDisposition::OpenExisting,
        FileOptions::empty(),
    )?;
    // If another process shrinks the file before `map`, mapping fails. If it
    // resizes afterwards, nothing here touches the mapped pages. A concurrent
    // resize cannot make a mapping access invalid memory.
    let size =
        usize::try_from(fspy_nostd::fs::get_file_size(file.as_handle()).map_err(error_to_io)?)
            .map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid shared-memory size")
            })?;
    let size = NonZeroUsize::new(size)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "shared-memory size is zero"))?;
    Ok(ShmHandle { file, size })
}

fn open_file(
    path: &OsStr,
    access: FileAccess,
    disposition: CreationDisposition,
    options: FileOptions,
) -> io::Result<fspy_nostd::OwnedHandle> {
    let path = copy_path(path)?;
    open_file_wide(as_nostd_path(&path), access, disposition, options).map_err(error_to_io)
}

fn open_file_wide(
    path: fspy_nostd::WideCStr<'_, fspy_nostd::Fat>,
    access: FileAccess,
    disposition: CreationDisposition,
    options: FileOptions,
) -> fspy_nostd::Result<fspy_nostd::OwnedHandle> {
    fspy_nostd::fs::create_file(path, access, SHARE_ALL, None, disposition, options, None)
}

fn copy_path(path: &OsStr) -> io::Result<Vec<u16>> {
    let mut units: Vec<_> = path.encode_wide().collect();
    if units.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"));
    }
    units.push(0);
    Ok(units)
}

fn as_nostd_path(path: &[u16]) -> fspy_nostd::WideCStr<'_, fspy_nostd::Fat> {
    fspy_nostd::WideCStr::from_units_with_nul(path)
        .expect("copy_path rejects interior NUL and appends one terminator")
}

fn error_to_io(error: fspy_nostd::Error) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error().cast_signed())
}

fn set_sparse(file: BorrowedHandle<'_>) -> fspy_nostd::Result<()> {
    let mut bytes_returned = 0;
    // SAFETY: `file` keeps the handle open, and every caller supplies a file
    // opened without `FILE_FLAG_OVERLAPPED`. `FSCTL_SET_SPARSE` accepts null
    // input and output buffers, and `bytes_returned` is writable. Windows
    // validates that the handle supports this control code.
    bool_result(unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            ptr::null(),
            0,
            ptr::null_mut(),
            0,
            &raw mut bytes_returned,
            ptr::null_mut(),
        )
    })
}

fn set_end_of_file(file: BorrowedHandle<'_>, len: i64) -> fspy_nostd::Result<()> {
    const INFO_SIZE: u32 = 8;
    const _: [(); 8] = [(); size_of::<FILE_END_OF_FILE_INFO>()];

    let info = FILE_END_OF_FILE_INFO { EndOfFile: len };
    // SAFETY: `file` keeps the opaque handle open. `info` is initialized and
    // has the type and exact size required by `FileEndOfFileInfo`; Windows
    // validates the handle type and access rights.
    bool_result(unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileEndOfFileInfo,
            (&raw const info).cast::<c_void>(),
            INFO_SIZE,
        )
    })
}

/// Removes the shared memory at `path`.
///
/// Removal relies on POSIX delete semantics, which requires NTFS on Windows
/// 10 1607 or newer: the name is unlinked immediately even while views of the
/// backing file remain mapped.
///
/// Removal is cleanup, not a stop signal: later opens fail, but existing
/// [`ShmHandle`]s and [`Mapping`]s keep reading and writing. To stop them,
/// store a flag in the shared bytes, as the fspy channel's close gate does.
///
/// # Errors
///
/// Returns the error reported while unlinking the path.
pub fn remove(path: &OsStr) -> io::Result<()> {
    let path = copy_path(path)?;
    // Opening the reparse point itself removes a link rather than its target.
    let file = open_file_wide(
        as_nostd_path(&path),
        FileAccess::DELETE,
        CreationDisposition::OpenExisting,
        FileOptions::OPEN_REPARSE_POINT,
    )
    .map_err(error_to_io)?;
    // POSIX delete removes the name as soon as `file` closes below, while
    // existing handles and mapped views keep working until they are dropped.
    set_posix_delete(file.as_handle()).map_err(error_to_io)
}

fn set_posix_delete(file: BorrowedHandle<'_>) -> fspy_nostd::Result<()> {
    const INFO_SIZE: u32 = 4;
    const _: [(); 4] = [(); size_of::<FILE_DISPOSITION_INFO_EX>()];

    let info = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    // SAFETY: `file` keeps the opaque handle open. `info` is initialized and
    // has the type and exact size required by `FileDispositionInfoEx`; Windows
    // validates the handle type, access rights, and supported flags.
    bool_result(unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfoEx,
            (&raw const info).cast::<c_void>(),
            INFO_SIZE,
        )
    })
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
        // Default security attributes create a non-inheritable mapping object.
        // Both maximum-size halves are zero, so Windows uses the current file
        // size.
        let mapping = fspy_nostd::mm::create_file_mapping(
            self.file.as_handle(),
            None,
            PageProtection::ReadWrite,
            0,
            0,
        )
        .map_err(error_to_io)?;
        let view = fspy_nostd::mm::map_view_of_file(
            mapping.as_handle(),
            MappingAccess::READ | MappingAccess::WRITE,
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
    // SAFETY: `file` keeps its handle open and `info` is a writable
    // `FILE_STANDARD_INFO` buffer of exactly `info_size` bytes. Windows
    // validates that the handle supports this information class.
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
