use core::{ffi::c_void, ptr};

use bitflags::bitflags;
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};

use crate::{OwnedHandle, Result, SecurityAttributes, WideCStr};

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
    file: &OwnedHandle,
    mapping_attributes: Option<&SecurityAttributes>,
    protection: PageProtection,
    maximum_size_high: u32,
    maximum_size_low: u32,
    name: Option<WideCStr<'_, R>>,
) -> Result<OwnedHandle> {
    let mapping_attributes = mapping_attributes.map_or(ptr::null(), SecurityAttributes::as_raw);
    let name = name.map_or(ptr::null(), |name| name.as_ptr());

    // SAFETY: `file` is valid. Security attributes are either null or a valid
    // opaque value owned by this module, and `name` is either null or backed
    // by a valid borrowed wide C string. Windows validates the protection and
    // size arguments.
    let mapping = unsafe {
        CreateFileMappingW(
            file.as_raw(),
            mapping_attributes,
            protection.bits(),
            maximum_size_high,
            maximum_size_low,
            name,
        )
    };
    let Some(mapping) = core::ptr::NonNull::new(mapping) else {
        return Err(crate::windows::last_error());
    };
    // SAFETY: `CreateFileMappingW` returned a valid, newly owned handle.
    Ok(unsafe { OwnedHandle::from_raw(mapping.as_ptr()) })
}

/// Calls `MapViewOfFile` and returns the new owned view.
///
/// # Errors
///
/// Returns the error reported by `MapViewOfFile`.
pub fn map_view_of_file(
    mapping: &OwnedHandle,
    access: MappingAccess,
    file_offset_high: u32,
    file_offset_low: u32,
    bytes_to_map: usize,
) -> Result<MappingView> {
    // SAFETY: `mapping` is a valid file-mapping object. Windows validates the
    // requested access, offset, and size against that object.
    let view = unsafe {
        MapViewOfFile(
            mapping.as_raw(),
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
