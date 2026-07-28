//! The Win32 calls the shared implementation needs on Windows: marking the
//! backing file sparse and, in tests, reading back how much of it the
//! filesystem actually allocated.

use std::{fs::File, io, os::windows::io::AsRawHandle};

#[cfg(test)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_STANDARD_INFO, FileStandardInfo, GetFileInformationByHandleEx,
};
use windows_sys::Win32::{
    Storage::FileSystem::{
        FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_DELETE_ON_CLOSE, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    },
    System::{IO::DeviceIoControl, Ioctl::FSCTL_SET_SPARSE},
};

pub(super) const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
pub(super) const TEMPORARY: u32 = FILE_ATTRIBUTE_TEMPORARY;
pub(super) const DELETE_ON_CLOSE: u32 = FILE_FLAG_DELETE_ON_CLOSE;
pub(super) const DELETE: u32 = windows_sys::Win32::Storage::FileSystem::DELETE;

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
    if result == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
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
        return Err(io::Error::last_os_error());
    }

    let logical_size = u64::try_from(info.EndOfFile)
        .map_err(|_| io::Error::other("file has a negative logical size"))?;
    let allocated_size = u64::try_from(info.AllocationSize)
        .map_err(|_| io::Error::other("file has a negative allocated size"))?;
    Ok((logical_size, allocated_size))
}
