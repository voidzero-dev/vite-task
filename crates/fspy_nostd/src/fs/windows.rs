use core::ptr;

use bitflags::bitflags;
use windows_sys::Win32::{
    Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        CREATE_NEW, CreateFileW, DELETE, FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileSizeEx, OPEN_EXISTING,
        TRUNCATE_EXISTING,
    },
};

use crate::{BorrowedHandle, OwnedHandle, Result, SecurityAttributes, WideCStr};

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
        /// Open a reparse point rather than its target.
        const OPEN_REPARSE_POINT = FILE_FLAG_OPEN_REPARSE_POINT;
    }
}

/// How `CreateFileW` handles an existing or missing file.
///
/// `CREATE_ALWAYS` and `OPEN_ALWAYS` are omitted: their success reports
/// whether the file already existed only through `GetLastError`, which
/// [`create_file`] does not surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum CreationDisposition {
    /// Create a new file and fail if it already exists.
    CreateNew = CREATE_NEW,
    /// Open an existing file and fail if it does not exist.
    OpenExisting = OPEN_EXISTING,
    /// Open and truncate an existing file.
    TruncateExisting = TRUNCATE_EXISTING,
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
    template_file: Option<BorrowedHandle<'_>>,
) -> Result<OwnedHandle> {
    let security_attributes = security_attributes.map_or(ptr::null(), SecurityAttributes::as_raw);
    let template_file = template_file.map_or(ptr::null_mut(), |file| file.as_raw_handle());

    // SAFETY: `path` and the optional security-attributes pointer remain
    // readable for the call, and the optional template handle remains open.
    // Windows validates the template's object type and all scalar options.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access.bits(),
            share.bits(),
            security_attributes,
            disposition as u32,
            options.bits(),
            template_file,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        Err(crate::windows::last_error())
    } else {
        // SAFETY: `CreateFileW` returned a valid, newly owned handle.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

/// Calls `GetFileSizeEx`.
///
/// # Errors
///
/// Returns the error reported by `GetFileSizeEx`.
pub fn get_file_size(file: BorrowedHandle<'_>) -> Result<i64> {
    let mut size = 0;
    // SAFETY: `file` keeps the opaque handle open and `size` is writable for
    // the call. Windows rejects handles that do not support a size query.
    crate::windows::bool_result(unsafe { GetFileSizeEx(file.as_raw_handle(), &raw mut size) })?;
    Ok(size)
}
