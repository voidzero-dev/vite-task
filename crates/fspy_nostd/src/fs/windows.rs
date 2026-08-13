use core::ptr;

use bitflags::bitflags;
use windows_sys::Win32::{
    Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        CREATE_ALWAYS, CREATE_NEW, CreateFileW, DELETE, DeleteFileW, FILE_ATTRIBUTE_TEMPORARY,
        FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileSizeEx, OPEN_ALWAYS, OPEN_EXISTING,
        TRUNCATE_EXISTING,
    },
};

use crate::{OwnedHandle, Result, SecurityAttributes, WideCStr};

bitflags! {
    /// Access rights requested for a file handle.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct FileAccess: u32 {
        /// Generic read access.
        const GENERIC_READ = GENERIC_READ;
        /// Generic write access.
        const GENERIC_WRITE = GENERIC_WRITE;
        /// Permission to delete the file.
        const DELETE = DELETE;
    }

    /// Operations that other handles may perform while a file is open.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct FileShare: u32 {
        /// Permit subsequent opens for reading.
        const READ = FILE_SHARE_READ;
        /// Permit subsequent opens for writing.
        const WRITE = FILE_SHARE_WRITE;
        /// Permit subsequent opens for deletion.
        const DELETE = FILE_SHARE_DELETE;
    }

    /// File attributes and creation options accepted by `CreateFileW`.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct FileOptions: u32 {
        /// Hint that the file should be kept in memory when possible.
        const TEMPORARY = FILE_ATTRIBUTE_TEMPORARY;
        /// Delete the file after its last handle closes.
        const DELETE_ON_CLOSE = FILE_FLAG_DELETE_ON_CLOSE;
        /// Open a reparse point rather than its target.
        const OPEN_REPARSE_POINT = FILE_FLAG_OPEN_REPARSE_POINT;
    }
}

/// How `CreateFileW` handles an existing or missing file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreationDisposition {
    /// Create a new file and fail if it already exists.
    CreateNew,
    /// Create a new file or replace an existing file.
    CreateAlways,
    /// Open an existing file and fail if it does not exist.
    OpenExisting,
    /// Open an existing file or create a new file.
    OpenAlways,
    /// Open and truncate an existing file.
    TruncateExisting,
}

impl CreationDisposition {
    const fn into_raw(self) -> u32 {
        match self {
            Self::CreateNew => CREATE_NEW,
            Self::CreateAlways => CREATE_ALWAYS,
            Self::OpenExisting => OPEN_EXISTING,
            Self::OpenAlways => OPEN_ALWAYS,
            Self::TruncateExisting => TRUNCATE_EXISTING,
        }
    }
}

/// Calls `CreateFileW` and returns the new owned handle.
///
/// # Errors
///
/// Returns the error reported by `CreateFileW`.
#[expect(clippy::needless_pass_by_value, reason = "CStr is a borrowed value type")]
pub fn create_file<R>(
    path: WideCStr<'_, R>,
    access: FileAccess,
    share: FileShare,
    security_attributes: Option<&SecurityAttributes>,
    disposition: CreationDisposition,
    options: FileOptions,
    template_file: Option<&OwnedHandle>,
) -> Result<OwnedHandle> {
    let security_attributes = security_attributes.map_or(ptr::null(), SecurityAttributes::as_raw);
    let template_file = template_file.map_or(ptr::null_mut(), OwnedHandle::as_raw);

    // SAFETY: `path` is NUL-terminated. Both optional pointers are either null
    // or backed by their valid borrowed wrapper types. Other arguments are
    // passed through unchanged for Windows to validate.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access.bits(),
            share.bits(),
            security_attributes,
            disposition.into_raw(),
            options.bits(),
            template_file,
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        Err(crate::windows::last_error())
    } else {
        // SAFETY: `CreateFileW` returned a valid, newly owned handle.
        Ok(unsafe { OwnedHandle::from_raw(handle) })
    }
}

/// Calls `DeleteFileW`.
///
/// # Errors
///
/// Returns the error reported by `DeleteFileW`.
#[expect(clippy::needless_pass_by_value, reason = "CStr is a borrowed value type")]
pub fn delete_file<R>(path: WideCStr<'_, R>) -> Result<()> {
    // SAFETY: `path` is a valid NUL-terminated wide string.
    crate::windows::bool_result(unsafe { DeleteFileW(path.as_ptr()) })
}

/// Calls `GetFileSizeEx`.
///
/// # Errors
///
/// Returns the error reported by `GetFileSizeEx`.
pub fn get_file_size(file: &OwnedHandle) -> Result<i64> {
    let mut size = 0;
    // SAFETY: `file` is valid and `size` is writable for the call.
    crate::windows::bool_result(unsafe { GetFileSizeEx(file.as_raw(), &raw mut size) })?;
    Ok(size)
}
