use core::{
    ffi::c_void,
    ptr::{self, NonNull},
};

use bitflags::bitflags;
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE,
    MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile, VirtualAlloc,
    VirtualFree,
};

use crate::{BorrowedHandle, OwnedHandle, Result, SecurityAttributes};

/// Page protection for a file mapping.
///
/// Protection values are mutually exclusive, so they are modeled as an enum
/// rather than as flags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum PageProtection {
    /// Permit pages to be read and written.
    ReadWrite = PAGE_READWRITE,
}

bitflags! {
    /// Access requested for a mapped view.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct MappingAccess: u32 {
        /// Permit reads from the view.
        const READ = FILE_MAP_READ;
        /// Permit writes to the view.
        const WRITE = FILE_MAP_WRITE;
    }

    /// How `VirtualAlloc` claims address space.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct AllocationType: u32 {
        /// Reserve address space.
        const RESERVE = MEM_RESERVE;
        /// Commit pages within reserved address space.
        const COMMIT = MEM_COMMIT;
    }
}

/// Calls `VirtualAlloc` at a system-chosen address and returns the new
/// region's base.
///
/// Committed pages read as zero until written.
///
/// # Errors
///
/// Returns the error reported by `VirtualAlloc`.
pub fn virtual_alloc(
    size: usize,
    allocation_type: AllocationType,
    protection: PageProtection,
) -> Result<NonNull<c_void>> {
    // SAFETY: a system-chosen address cannot alias an existing allocation,
    // and Windows validates the size, allocation type, and protection.
    let address =
        unsafe { VirtualAlloc(ptr::null(), size, allocation_type.bits(), protection as u32) };
    NonNull::new(address).ok_or_else(crate::Error::last_os_error)
}

/// Calls `VirtualFree` with `MEM_RELEASE`, releasing the whole region.
///
/// # Errors
///
/// Returns the error reported by `VirtualFree`.
///
/// # Safety
///
/// `address` must be the base of a region returned by [`virtual_alloc`] that
/// has not been released, with no live references into it.
pub unsafe fn virtual_free(address: NonNull<c_void>) -> Result<()> {
    // SAFETY: the caller guarantees that `address` is an unreleased region
    // base; releasing passes zero as the size.
    crate::windows::bool_result(unsafe { VirtualFree(address.as_ptr(), 0, MEM_RELEASE) })
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

/// Calls `CreateFileMappingW` and returns the new unnamed mapping object's
/// handle.
///
/// Named mappings are unsupported: for a name that already exists,
/// `CreateFileMappingW` reports the pre-existing object only through
/// `GetLastError`, which this wrapper does not surface.
///
/// # Errors
///
/// Returns the error reported by `CreateFileMappingW`.
pub fn create_file_mapping(
    file: BorrowedHandle<'_>,
    mapping_attributes: Option<&SecurityAttributes>,
    protection: PageProtection,
    maximum_size_high: u32,
    maximum_size_low: u32,
) -> Result<OwnedHandle> {
    let mapping_attributes = mapping_attributes.map_or(ptr::null(), SecurityAttributes::as_raw);

    // SAFETY: `file` keeps the opaque handle open. The optional security
    // attributes are either null or readable for the call. Windows validates
    // the handle's object type, protection, and sizes.
    let mapping = unsafe {
        CreateFileMappingW(
            file.as_raw_handle(),
            mapping_attributes,
            protection as u32,
            maximum_size_high,
            maximum_size_low,
            ptr::null(),
        )
    };
    if mapping.is_null() {
        return Err(crate::Error::last_os_error());
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
        return Err(crate::Error::last_os_error());
    };
    Ok(MappingView { ptr })
}
