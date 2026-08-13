use core::{ffi::c_void, ptr};

use bitflags::bitflags;
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};

use crate::{AsRawHandle as _, BorrowedHandle, OwnedHandle, Result, SecurityAttributes, WideCStr};

bitflags! {
    /// Page protection for a file mapping.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct PageProtection: u32 {
        /// Permit pages to be read and written.
        const READ_WRITE = PAGE_READWRITE;
    }

    /// Access requested for a mapped view.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct MappingAccess: u32 {
        /// Permit reads from the view.
        const READ = FILE_MAP_READ;
        /// Permit writes to the view.
        const WRITE = FILE_MAP_WRITE;
    }
}

/// An owned view of a file-mapping object.
pub struct MappingView {
    ptr: core::ptr::NonNull<u8>,
}

// SAFETY: a view owns no thread-affine state. Synchronization of accesses to
// its shared bytes is the caller's responsibility.
unsafe impl Send for MappingView {}
// SAFETY: sharing the view does not itself access the mapped bytes.
unsafe impl Sync for MappingView {}

impl MappingView {
    /// Returns a raw pointer to the first mapped byte.
    #[must_use]
    pub const fn as_ptr(&self) -> *mut u8 {
        self.ptr.as_ptr()
    }
}

impl Drop for MappingView {
    fn drop(&mut self) {
        // SAFETY: this is the complete view owned by `self` and is unmapped
        // exactly once.
        let _ = unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.ptr.as_ptr().cast::<c_void>(),
            })
        };
    }
}

/// Calls `CreateFileMappingW` and returns the new mapping-object handle.
///
/// # Errors
///
/// Returns the error reported by `CreateFileMappingW`.
pub fn create_file_mapping<R>(
    file: BorrowedHandle<'_>,
    mapping_attributes: Option<&SecurityAttributes>,
    protection: PageProtection,
    maximum_size_high: u32,
    maximum_size_low: u32,
    name: Option<WideCStr<'_, R>>,
) -> Result<OwnedHandle> {
    let mapping_attributes = mapping_attributes.map_or(ptr::null(), SecurityAttributes::as_raw);
    let name = name.map_or(ptr::null(), |name| name.as_ptr());

    // SAFETY: `file` keeps the opaque handle open. The optional security
    // attributes and name are either null or readable for the call. Windows
    // validates the handle's object type, protection, and sizes.
    let mapping = unsafe {
        CreateFileMappingW(
            file.as_raw_handle(),
            mapping_attributes,
            protection.bits(),
            maximum_size_high,
            maximum_size_low,
            name,
        )
    };
    if mapping.is_null() {
        return Err(crate::windows::last_error());
    }
    // SAFETY: `CreateFileMappingW` returned a valid, newly owned handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(mapping) })
}

/// Calls `MapViewOfFile` and returns the new owned view.
///
/// # Errors
///
/// Returns the error reported by `MapViewOfFile`.
pub fn map_view_of_file(
    mapping: BorrowedHandle<'_>,
    access: MappingAccess,
    file_offset_high: u32,
    file_offset_low: u32,
    bytes_to_map: usize,
) -> Result<MappingView> {
    // SAFETY: `mapping` keeps the opaque handle open for the call. Windows
    // verifies that it names a file-mapping object and validates the access,
    // offset, and size; any mismatch is reported as a null result.
    let view = unsafe {
        MapViewOfFile(
            mapping.as_raw_handle(),
            access.bits(),
            file_offset_high,
            file_offset_low,
            bytes_to_map,
        )
    };
    let Some(ptr) = core::ptr::NonNull::new(view.Value.cast::<u8>()) else {
        return Err(crate::windows::last_error());
    };
    Ok(MappingView { ptr })
}
