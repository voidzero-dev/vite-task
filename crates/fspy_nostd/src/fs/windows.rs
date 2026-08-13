use core::{ffi::c_void, mem::size_of, ptr};

use bitflags::bitflags;
use windows_sys::Win32::{
    Foundation::{ERROR_ACCESS_DENIED, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        CREATE_NEW, CreateFileW, DELETE, DeleteFileW, FILE_ATTRIBUTE_TEMPORARY,
        FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX, FILE_END_OF_FILE_INFO,
        FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FileDispositionInfoEx, FileEndOfFileInfo, GetFileSizeEx,
        OPEN_EXISTING, SetFileInformationByHandle,
    },
    System::{IO::DeviceIoControl, Ioctl::FSCTL_SET_SPARSE},
};

use crate::{OwnedHandle, Result, WideCStr};

bitflags! {
    /// Access rights requested when opening a file.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct Access: u32 {
        const READ = GENERIC_READ;
        const WRITE = GENERIC_WRITE;
        const DELETE = DELETE;
    }

    /// Operations that other handles may perform while a file is open.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct ShareMode: u32 {
        const READ = FILE_SHARE_READ;
        const WRITE = FILE_SHARE_WRITE;
        const DELETE = FILE_SHARE_DELETE;
    }

    /// File attributes and flags applied while opening a file.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct OpenFlags: u32 {
        const TEMPORARY = FILE_ATTRIBUTE_TEMPORARY;
        const DELETE_ON_CLOSE = FILE_FLAG_DELETE_ON_CLOSE;
        const OPEN_REPARSE_POINT = FILE_FLAG_OPEN_REPARSE_POINT;
    }
}

/// Controls whether opening a file creates it or requires it to exist.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreationDisposition(u32);

impl CreationDisposition {
    pub const CREATE_NEW: Self = Self(CREATE_NEW);
    pub const OPEN_EXISTING: Self = Self(OPEN_EXISTING);
}

/// Opens a file and returns its owned handle.
///
/// The returned handle is non-inheritable because this function supplies no
/// security attributes.
///
/// # Errors
///
/// Returns the error reported by `CreateFileW`.
#[expect(clippy::needless_pass_by_value, reason = "CStr is a borrowed value type")]
pub fn open<R>(
    path: WideCStr<'_, R>,
    access: Access,
    share_mode: ShareMode,
    creation: CreationDisposition,
    flags: OpenFlags,
) -> Result<OwnedHandle> {
    // SAFETY: `path` is NUL-terminated. Null security attributes make the
    // handle non-inheritable, and the null template is permitted.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access.bits(),
            share_mode.bits(),
            ptr::null(),
            creation.0,
            flags.bits(),
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        Err(crate::windows::last_error())
    } else {
        // SAFETY: `CreateFileW` returned a valid, newly owned handle.
        Ok(unsafe { OwnedHandle::from_raw(handle) })
    }
}

/// Removes a file name.
///
/// # Errors
///
/// Returns the error reported while removing the file.
pub fn remove<R>(path: WideCStr<'_, R>) -> Result<()> {
    // SAFETY: `path` is a valid NUL-terminated wide string.
    if unsafe { DeleteFileW(path.as_ptr()) } != 0 {
        return Ok(());
    }

    let error = crate::windows::last_error();
    if error.raw_os_error() == ERROR_ACCESS_DENIED {
        const SHARE_ALL: ShareMode =
            ShareMode::READ.union(ShareMode::WRITE).union(ShareMode::DELETE);
        if let Ok(file) = open(
            path,
            Access::DELETE,
            SHARE_ALL,
            CreationDisposition::OPEN_EXISTING,
            OpenFlags::OPEN_REPARSE_POINT,
        ) && set_posix_delete(&file).is_ok()
        {
            return Ok(());
        }
    }

    // Match `std::fs::remove_file`: if the POSIX fallback is unavailable,
    // preserve the original `DeleteFileW` error.
    Err(error)
}

fn set_posix_delete(file: &OwnedHandle) -> Result<()> {
    const INFO_SIZE: u32 = 4;
    const _: [(); 4] = [(); size_of::<FILE_DISPOSITION_INFO_EX>()];

    let info = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    // SAFETY: `file` is valid and `info` has the type and exact size required
    // by `FileDispositionInfoEx`.
    crate::windows::bool_result(unsafe {
        SetFileInformationByHandle(
            file.as_raw(),
            FileDispositionInfoEx,
            (&raw const info).cast::<c_void>(),
            INFO_SIZE,
        )
    })
}

/// Returns a file's logical length.
///
/// # Errors
///
/// Returns the error reported by `GetFileSizeEx`.
pub fn file_size(file: &OwnedHandle) -> Result<i64> {
    let mut size = 0;
    // SAFETY: `file` is valid and `size` is writable for the call.
    crate::windows::bool_result(unsafe { GetFileSizeEx(file.as_raw(), &raw mut size) })?;
    Ok(size)
}

/// Sets a file's logical length.
///
/// # Errors
///
/// Returns the error reported by `SetFileInformationByHandle`.
pub fn set_len(file: &OwnedHandle, len: i64) -> Result<()> {
    const INFO_SIZE: u32 = 8;
    const _: [(); 8] = [(); size_of::<FILE_END_OF_FILE_INFO>()];

    let info = FILE_END_OF_FILE_INFO { EndOfFile: len };
    // SAFETY: `file` is valid and `info` has the type and exact size required
    // by `FileEndOfFileInfo`.
    crate::windows::bool_result(unsafe {
        SetFileInformationByHandle(
            file.as_raw(),
            FileEndOfFileInfo,
            (&raw const info).cast::<c_void>(),
            INFO_SIZE,
        )
    })
}

/// Marks a file sparse.
///
/// # Errors
///
/// Returns the error reported by `FSCTL_SET_SPARSE`.
pub fn set_sparse(file: &OwnedHandle) -> Result<()> {
    let mut bytes_returned = 0;
    // SAFETY: `file` is valid and synchronous. `FSCTL_SET_SPARSE` needs no
    // input or output buffer, and `bytes_returned` is writable for the call.
    crate::windows::bool_result(unsafe {
        DeviceIoControl(
            file.as_raw(),
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
