use std::{
    io,
    iter::once,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::NonNull,
};

use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, GetLastError},
    Storage::FileSystem::{
        FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_DELETE_ON_CLOSE, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    },
    System::Memory::{
        CreateFileMappingW, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
        OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
    },
};

pub(super) const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
pub(super) const TEMPORARY: u32 = FILE_ATTRIBUTE_TEMPORARY;
pub(super) const DELETE_ON_CLOSE: u32 = FILE_FLAG_DELETE_ON_CLOSE;

pub(super) fn create_file_mapping(file: &std::fs::File, name: &str) -> io::Result<OwnedHandle> {
    let name = wide_name(name);
    // SAFETY: `file` supplies a valid handle, the security pointer is null, and
    // `name` is a live, NUL-terminated UTF-16 buffer for the duration of the call.
    let raw_handle = unsafe {
        CreateFileMappingW(
            file.as_raw_handle().cast(),
            std::ptr::null(),
            PAGE_READWRITE,
            0,
            0,
            name.as_ptr(),
        )
    };
    if raw_handle.is_null() {
        return Err(last_error());
    }

    // CreateFileMappingW reports name collisions through the thread's last-error
    // value even though it returns a valid handle.
    // SAFETY: GetLastError has no preconditions and immediately follows that call.
    let error = unsafe { GetLastError() };
    // SAFETY: A non-null CreateFileMappingW result is an owned mapping handle.
    let handle = unsafe { OwnedHandle::from_raw_handle(raw_handle.cast()) };
    if error == ERROR_ALREADY_EXISTS {
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "shared-memory mapping already exists"))
    } else {
        Ok(handle)
    }
}

pub(super) fn open_file_mapping(name: &str) -> io::Result<OwnedHandle> {
    let name = wide_name(name);
    // SAFETY: `name` is a live, NUL-terminated UTF-16 buffer and inheritance is disabled.
    let raw_handle = unsafe { OpenFileMappingW(FILE_MAP_WRITE, 0, name.as_ptr()) };
    if raw_handle.is_null() {
        return Err(last_error());
    }

    // SAFETY: A non-null OpenFileMappingW result is an owned mapping handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw_handle.cast()) })
}

pub(super) struct MappedView {
    pointer: NonNull<u8>,
    len: usize,
}

impl MappedView {
    pub(super) fn new(mapping: &OwnedHandle, len: usize) -> io::Result<Self> {
        // SAFETY: `mapping` is a valid file-mapping handle. Offset zero and
        // `len` describe the exact view retained by the returned guard.
        let view =
            unsafe { MapViewOfFile(mapping.as_raw_handle().cast(), FILE_MAP_WRITE, 0, 0, len) };
        let pointer = NonNull::new(view.Value.cast::<u8>()).ok_or_else(last_error)?;
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
        // this guard owns that view until this single unmap operation.
        let _ = unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.pointer.as_ptr().cast() })
        };
    }
}

fn wide_name(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(once(0)).collect()
}

fn last_error() -> io::Error {
    // SAFETY: GetLastError has no preconditions and is called immediately after
    // the failing Win32 operation on the same thread.
    io::Error::from_raw_os_error(unsafe { GetLastError() }.cast_signed())
}
