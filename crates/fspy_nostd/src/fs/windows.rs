use core::{ffi::c_void, marker::PhantomData, mem::size_of, ptr};

use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    Storage::FileSystem::{
        CreateFileW, DeleteFileW, FileDispositionInfoEx as RAW_FILE_DISPOSITION_INFO_EX,
        FileEndOfFileInfo as RAW_FILE_END_OF_FILE_INFO, GetFileSizeEx, SetFileInformationByHandle,
    },
    System::{IO::DeviceIoControl, Ioctl::FSCTL_SET_SPARSE as RAW_FSCTL_SET_SPARSE},
};
pub use windows_sys::Win32::{
    Foundation::{ERROR_ACCESS_DENIED, GENERIC_READ, GENERIC_WRITE},
    Storage::FileSystem::{
        CREATE_NEW, DELETE, FILE_ATTRIBUTE_TEMPORARY, FILE_CREATION_DISPOSITION,
        FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX, FILE_END_OF_FILE_INFO,
        FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAGS_AND_ATTRIBUTES,
        FILE_INFO_BY_HANDLE_CLASS, FILE_SHARE_DELETE, FILE_SHARE_MODE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    },
    System::Ioctl::FILE_SET_SPARSE_BUFFER,
};

use crate::{Overlapped, OwnedHandle, Result, SecurityAttributes, WideCStr};

/// A file-information class tied to its buffer type and size.
///
/// Values are provided by this module for the Win32 classes it supports. The
/// private fields prevent safe code from pairing a class with the wrong
/// buffer representation.
pub struct FileInformationClass<I> {
    raw: FILE_INFO_BY_HANDLE_CLASS,
    size: u32,
    marker: PhantomData<fn(&I)>,
}

impl<I> Clone for FileInformationClass<I> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<I> Copy for FileInformationClass<I> {}

const _: [(); 4] = [(); size_of::<FILE_DISPOSITION_INFO_EX>()];
const _: [(); 8] = [(); size_of::<FILE_END_OF_FILE_INFO>()];

/// The `FileDispositionInfoEx` class and its buffer representation.
#[expect(non_upper_case_globals, reason = "matches the Win32 class name")]
pub const FileDispositionInfoEx: FileInformationClass<FILE_DISPOSITION_INFO_EX> =
    FileInformationClass { raw: RAW_FILE_DISPOSITION_INFO_EX, size: 4, marker: PhantomData };

/// The `FileEndOfFileInfo` class and its buffer representation.
#[expect(non_upper_case_globals, reason = "matches the Win32 class name")]
pub const FileEndOfFileInfo: FileInformationClass<FILE_END_OF_FILE_INFO> =
    FileInformationClass { raw: RAW_FILE_END_OF_FILE_INFO, size: 8, marker: PhantomData };

/// A device-control code tied to its input and output buffer types and sizes.
///
/// Values are provided by this module for the controls it supports. The
/// private fields prevent safe code from changing the code or its buffer
/// contract.
pub struct DeviceIoControlCode<I, O> {
    raw: u32,
    input_size: u32,
    output_size: u32,
    marker: PhantomData<fn(&I, &mut O)>,
}

impl<I, O> Clone for DeviceIoControlCode<I, O> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<I, O> Copy for DeviceIoControlCode<I, O> {}

const _: [(); 1] = [(); size_of::<FILE_SET_SPARSE_BUFFER>()];

/// The `FSCTL_SET_SPARSE` control and its buffer representations.
pub const FSCTL_SET_SPARSE: DeviceIoControlCode<FILE_SET_SPARSE_BUFFER, ()> = DeviceIoControlCode {
    raw: RAW_FSCTL_SET_SPARSE,
    input_size: 1,
    output_size: 0,
    marker: PhantomData,
};

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

/// Calls `SetFileInformationByHandle`.
///
/// # Errors
///
/// Returns the error reported by `SetFileInformationByHandle`.
pub fn set_file_information_by_handle<I>(
    file: &OwnedHandle,
    class: FileInformationClass<I>,
    information: &I,
) -> Result<()> {
    // SAFETY: `file` is valid. An unforgeable `class` value ties the class and
    // byte count to `I`, and `information` remains readable for the call.
    crate::windows::bool_result(unsafe {
        SetFileInformationByHandle(
            file.as_raw(),
            class.raw,
            ptr::from_ref(information).cast::<c_void>(),
            class.size,
        )
    })
}

/// Calls `GetFileSizeEx`.
///
/// # Errors
///
/// Returns the error reported by `GetFileSizeEx`.
pub fn get_file_size(file: &OwnedHandle, size: &mut i64) -> Result<()> {
    // SAFETY: `file` is valid and `size` is writable for the call.
    crate::windows::bool_result(unsafe { GetFileSizeEx(file.as_raw(), size) })
}

/// Calls `DeviceIoControl`.
///
/// # Errors
///
/// Returns the error reported by `DeviceIoControl`.
pub fn device_io_control<I, O>(
    device: &OwnedHandle,
    control_code: DeviceIoControlCode<I, O>,
    input: Option<&I>,
    output: Option<&mut O>,
    bytes_returned: &mut u32,
    overlapped: Option<&mut Overlapped>,
) -> Result<()> {
    let (input, input_size) = match input {
        Some(input) if control_code.input_size != 0 => {
            (ptr::from_ref(input).cast::<c_void>(), control_code.input_size)
        }
        _ => (ptr::null(), 0),
    };
    let (output, output_size) = match output {
        Some(output) if control_code.output_size != 0 => {
            (ptr::from_mut(output).cast::<c_void>(), control_code.output_size)
        }
        _ => (ptr::null_mut(), 0),
    };
    let overlapped = overlapped.map_or(ptr::null_mut(), Overlapped::as_raw_mut);

    // SAFETY: `device` is valid. The unforgeable control-code value ties the
    // control to the input and output buffer types and sizes. Every non-null
    // pointer is backed by its mutable or shared borrow for the call. Safe
    // callers cannot construct overlapped state, so their calls are
    // synchronous.
    crate::windows::bool_result(unsafe {
        DeviceIoControl(
            device.as_raw(),
            control_code.raw,
            input,
            input_size,
            output,
            output_size,
            bytes_returned,
            overlapped,
        )
    })
}
