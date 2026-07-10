use std::{
    io,
    iter::once,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::NonNull,
};

#[cfg(test)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_STANDARD_INFO, FileStandardInfo, GetFileInformationByHandleEx,
};
use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, GetLastError},
    Storage::FileSystem::{
        FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_DELETE_ON_CLOSE, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    },
    System::{
        IO::DeviceIoControl,
        Ioctl::FSCTL_SET_SPARSE,
        Memory::{
            CreateFileMappingW, FILE_MAP_WRITE, MEMORY_BASIC_INFORMATION,
            MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW, PAGE_READWRITE,
            UnmapViewOfFile, VirtualQuery,
        },
    },
};

pub(super) const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
pub(super) const TEMPORARY: u32 = FILE_ATTRIBUTE_TEMPORARY;
pub(super) const DELETE_ON_CLOSE: u32 = FILE_FLAG_DELETE_ON_CLOSE;

pub(super) fn set_sparse(file: &std::fs::File) -> io::Result<()> {
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

#[cfg(test)]
pub(super) fn file_sizes(file: &std::fs::File) -> io::Result<(u64, u64)> {
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

pub(super) fn create_file_mapping(file: &std::fs::File, name: &str) -> io::Result<OwnedHandle> {
    let name = wide_name(name)?;
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
    let name = wide_name(name)?;
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
    _mapping: OwnedHandle,
}

impl MappedView {
    pub(super) fn new(mapping: OwnedHandle) -> io::Result<Self> {
        // SAFETY: `mapping` is a valid file-mapping handle. Offset and length
        // zero map the complete section.
        let view =
            unsafe { MapViewOfFile(mapping.as_raw_handle().cast(), FILE_MAP_WRITE, 0, 0, 0) };
        let pointer = NonNull::new(view.Value.cast::<u8>()).ok_or_else(last_error)?;

        let mut info = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: `pointer` is inside the mapped view and `info` is writable for
        // its exact size.
        let result = unsafe {
            VirtualQuery(
                pointer.as_ptr().cast(),
                &raw mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if result == 0 {
            let error = last_error();
            // SAFETY: `pointer` is the base address returned by MapViewOfFile.
            let _ = unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: pointer.as_ptr().cast() })
            };
            return Err(error);
        }

        let len = info.RegionSize;
        Ok(Self { pointer, len, _mapping: mapping })
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

fn wide_name(name: &str) -> io::Result<Vec<u16>> {
    if name.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "shared-memory name contains a NUL",
        ));
    }
    Ok(name.encode_utf16().chain(once(0)).collect())
}

fn last_error() -> io::Error {
    // SAFETY: GetLastError has no preconditions and is called immediately after
    // the failing Win32 operation on the same thread.
    io::Error::from_raw_os_error(unsafe { GetLastError() }.cast_signed())
}
