use core::{
    ffi::c_void,
    ptr::{self, NonNull},
};

use bitflags::bitflags;
use windows_sys::Win32::{
    Foundation::HANDLE,
    System::{
        LibraryLoader::GetProcAddress,
        Memory::{
            CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEM_ADDRESS_REQUIREMENTS,
            MEM_COMMIT, MEM_EXTENDED_PARAMETER, MEM_EXTENDED_PARAMETER_0,
            MEM_EXTENDED_PARAMETER_1, MEM_RELEASE, MEM_RESERVE, MEMORY_MAPPED_VIEW_ADDRESS,
            MapViewOfFile, MemExtendedParameterAddressRequirements, PAGE_READWRITE,
            UnmapViewOfFile, VIRTUAL_ALLOCATION_TYPE, VirtualAlloc, VirtualFree,
        },
    },
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

/// `MapViewOfFile3`, which `kernelbase.dll` exports from Windows 10 1803 on.
type MapViewOfFile3Fn = unsafe extern "system" fn(
    file_mapping: HANDLE,
    process: HANDLE,
    base_address: *const c_void,
    offset: u64,
    view_size: usize,
    allocation_type: VIRTUAL_ALLOCATION_TYPE,
    page_protection: u32,
    extended_parameters: *mut MEM_EXTENDED_PARAMETER,
    parameter_count: u32,
) -> MEMORY_MAPPED_VIEW_ADDRESS;

/// Looks `MapViewOfFile3` up at run time rather than importing it, so this
/// crate still loads on Windows releases that predate it.
fn map_view_of_file3() -> Option<MapViewOfFile3Fn> {
    let kernelbase = crate::windows::get_module_handle(crate::wide_cstr!("kernelbase.dll")).ok()?;
    // SAFETY: `kernelbase` is a loaded module handle and the symbol name is a
    // NUL-terminated ANSI string.
    let symbol = unsafe { GetProcAddress(kernelbase.as_ptr(), c"MapViewOfFile3".as_ptr().cast()) }?;
    // SAFETY: `MapViewOfFile3` has exactly the signature of `MapViewOfFile3Fn`.
    Some(unsafe { core::mem::transmute::<unsafe extern "system" fn() -> isize, MapViewOfFile3Fn>(symbol) })
}

/// Maps the whole of `mapping` read-write, at an address no lower than
/// `lowest_address`.
///
/// Returns `None` when this Windows has no `MapViewOfFile3` (before Windows 10
/// 1803), so the caller can fall back to [`map_view_of_file`].
///
/// # Errors
///
/// Returns the error reported by `MapViewOfFile3`, for example when no free
/// range of `bytes_to_map` bytes exists above `lowest_address`.
pub fn map_view_of_file_above(
    mapping: BorrowedHandle<'_>,
    bytes_to_map: usize,
    lowest_address: usize,
) -> Option<Result<MappingView>> {
    let map_view_of_file3 = map_view_of_file3()?;
    let mut requirements = MEM_ADDRESS_REQUIREMENTS {
        LowestStartingAddress: ptr::without_provenance_mut(lowest_address),
        // Null means the highest address the process can use.
        HighestEndingAddress: ptr::null_mut(),
        // Zero means the allocation granularity.
        Alignment: 0,
    };
    let mut parameter = MEM_EXTENDED_PARAMETER {
        // The type occupies the low 8 bits.
        Anonymous1: MEM_EXTENDED_PARAMETER_0 {
            _bitfield: MemExtendedParameterAddressRequirements.cast_unsigned().into(),
        },
        Anonymous2: MEM_EXTENDED_PARAMETER_1 { Pointer: (&raw mut requirements).cast() },
    };
    // `GetCurrentProcess()`'s pseudo-handle, which never needs closing.
    let current_process: HANDLE = ptr::without_provenance_mut(usize::MAX);
    // SAFETY: `mapping` keeps the opaque handle open for the call, and
    // `parameter` points at `requirements`, both of which outlive the call.
    // Windows validates the handle, size, protection, and address range; any
    // mismatch is reported as a null result.
    let view = unsafe {
        map_view_of_file3(
            mapping.as_raw_handle(),
            current_process,
            ptr::null(),
            0,
            bytes_to_map,
            0,
            PAGE_READWRITE,
            &raw mut parameter,
            1,
        )
    };
    Some(
        core::ptr::NonNull::new(view.Value.cast::<u8>())
            .map(|ptr| MappingView { ptr })
            .ok_or_else(crate::Error::last_os_error),
    )
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
