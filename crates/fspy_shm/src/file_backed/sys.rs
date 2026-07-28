//! The Win32 calls the shared implementation needs on Windows: marking the
//! backing file sparse, mapping it without retaining a file handle, and (in
//! tests) reading back how much of it the filesystem actually allocated.

use std::{
    fs::File,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::NonNull,
};

#[cfg(test)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_STANDARD_INFO, FileStandardInfo, GetFileInformationByHandleEx,
};
use windows_sys::Win32::{
    Foundation::GetLastError,
    Storage::FileSystem::{
        FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_DELETE_ON_CLOSE, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    },
    System::{
        IO::DeviceIoControl,
        Ioctl::FSCTL_SET_SPARSE,
        Memory::{
            CreateFileMappingW, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
            PAGE_READWRITE, UnmapViewOfFile,
        },
    },
};

pub(super) const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
pub(super) const TEMPORARY: u32 = FILE_ATTRIBUTE_TEMPORARY;
pub(super) const DELETE_ON_CLOSE: u32 = FILE_FLAG_DELETE_ON_CLOSE;

/// Marks `file` sparse so that setting its length reserves no clusters.
pub(super) fn set_sparse(file: &File) -> io::Result<()> {
    let mut bytes_returned = 0;
    // SAFETY: `file` supplies a valid synchronous file handle. FSCTL_SET_SPARSE
    // requires no input or output buffers, and `bytes_returned` is writable for
    // the duration of the call.
    let result = unsafe {
        DeviceIoControl(
            file.as_raw_handle().cast(),
            FSCTL_SET_SPARSE,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &raw mut bytes_returned,
            std::ptr::null_mut(),
        )
    };
    if result == 0 { Err(last_error()) } else { Ok(()) }
}

/// Returns `file`'s logical size and the number of bytes the filesystem
/// allocated for it.
#[cfg(test)]
pub(super) fn file_sizes(file: &File) -> io::Result<(u64, u64)> {
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
        return Err(last_error());
    }

    let logical_size = u64::try_from(info.EndOfFile)
        .map_err(|_| io::Error::other("file has a negative logical size"))?;
    let allocated_size = u64::try_from(info.AllocationSize)
        .map_err(|_| io::Error::other("file has a negative allocated size"))?;
    Ok((logical_size, allocated_size))
}

fn last_error() -> io::Error {
    // SAFETY: GetLastError has no preconditions and is called immediately after
    // the failing Win32 operation on the same thread.
    io::Error::from_raw_os_error(unsafe { GetLastError() }.cast_signed())
}

/// A mapped view of a backing file that holds no file handle of its own.
///
/// This is why Windows does not use `memmap2` here: `memmap2` duplicates the
/// file handle and keeps it for the mapping's lifetime, and `FILE_FLAG_
/// DELETE_ON_CLOSE` only fires once *every* handle to the file is closed. A
/// view holds a section reference rather than a handle, so an opener cannot
/// hold the owner's cleanup hostage.
pub(super) struct MappedView {
    pointer: NonNull<u8>,
    len: usize,
}

// SAFETY: a view is a plain memory region; there is no thread affinity in
// `MapViewOfFile`. Callers govern access to the bytes, exactly as they do for
// the `memmap2` mapping used on Unix.
unsafe impl Send for MappedView {}
// SAFETY: as above.
unsafe impl Sync for MappedView {}

impl MappedView {
    /// Maps the first `len` bytes of `file` for reading and writing.
    pub(super) fn new(file: &File, len: usize) -> io::Result<Self> {
        // SAFETY: `file` supplies a valid handle, the security pointer is null,
        // and a null name creates an unnamed section covering the whole file.
        let mapping = unsafe {
            CreateFileMappingW(
                file.as_raw_handle().cast(),
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                0,
                std::ptr::null(),
            )
        };
        if mapping.is_null() {
            return Err(last_error());
        }
        // SAFETY: a non-null CreateFileMappingW result is an owned mapping handle.
        let mapping = unsafe { OwnedHandle::from_raw_handle(mapping.cast()) };

        // SAFETY: `mapping` is a valid section handle, and offset zero with
        // `len` maps exactly the range the caller asked for.
        let view =
            unsafe { MapViewOfFile(mapping.as_raw_handle().cast(), FILE_MAP_WRITE, 0, 0, len) };
        let pointer = NonNull::new(view.Value.cast::<u8>()).ok_or_else(last_error)?;

        // The view keeps an internal reference to the section, so dropping the
        // section handle here — and the caller's file handle later — is safe.
        Ok(Self { pointer, len })
    }

    pub(super) const fn as_ptr(&self) -> *mut u8 {
        self.pointer.as_ptr()
    }

    pub(super) const fn len(&self) -> usize {
        self.len
    }
}

impl Drop for MappedView {
    fn drop(&mut self) {
        // SAFETY: `pointer` is the base address returned by MapViewOfFile and
        // this value owns that view until this single unmap operation.
        let _ = unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.pointer.as_ptr().cast() })
        };
    }
}
