use core::ptr;

use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    Storage::FileSystem::{
        CreateFileW, DeleteFileW, FILE_CREATION_DISPOSITION, FILE_FLAGS_AND_ATTRIBUTES,
        FILE_SHARE_MODE, GetFileSizeEx,
    },
};

use crate::{OwnedHandle, Result, SecurityAttributes, WideCStr};

/// Calls `CreateFileW` and returns the new owned handle.
///
/// # Errors
///
/// Returns the error reported by `CreateFileW`.
#[expect(clippy::needless_pass_by_value, reason = "CStr is a borrowed value type")]
pub fn create_file<R>(
    path: WideCStr<'_, R>,
    desired_access: u32,
    share_mode: FILE_SHARE_MODE,
    security_attributes: Option<&SecurityAttributes>,
    creation_disposition: FILE_CREATION_DISPOSITION,
    flags_and_attributes: FILE_FLAGS_AND_ATTRIBUTES,
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
            desired_access,
            share_mode,
            security_attributes,
            creation_disposition,
            flags_and_attributes,
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
