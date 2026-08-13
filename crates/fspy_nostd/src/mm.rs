//! Memory mappings.
//!
//! On Unix, these are re-exposed from rustix as-is: they are single syscalls against the
//! kernel's own address-space bookkeeping — no libc state, no locks, no
//! allocation — so they already meet this crate's rules everywhere it
//! promises to work. What this module adds is the curation (being listed
//! here is what marks them safe for signal handlers, fork children, and
//! pre-libc startup) and the crate-level backend check, which guarantees
//! they cannot silently turn into libc calls on Linux.

#[cfg(unix)]
pub use rustix::mm::{MapFlags, MprotectFlags, ProtFlags, mmap, mmap_anonymous, mprotect, munmap};

#[cfg(windows)]
mod windows {
    use core::{ffi::c_void, ptr};

    pub use windows_sys::Win32::System::Memory::{
        FILE_MAP, FILE_MAP_READ, FILE_MAP_WRITE, PAGE_PROTECTION_FLAGS, PAGE_READWRITE,
    };
    use windows_sys::Win32::{
        Security::SECURITY_ATTRIBUTES,
        System::Memory::{
            CreateFileMappingW, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, UnmapViewOfFile,
        },
    };

    use crate::{OwnedHandle, Result, WideCStr};

    /// Opaque security attributes accepted by `CreateFileMappingW`.
    ///
    /// This type cannot be constructed outside `fspy_nostd`. No constructor
    /// is exposed until a caller needs non-default security attributes and
    /// their embedded security descriptor can be represented safely.
    #[repr(transparent)]
    pub struct SecurityAttributes(SECURITY_ATTRIBUTES);

    /// An owned view of a file-mapping object.
    pub struct MappingView {
        ptr: core::ptr::NonNull<u8>,
    }

    // SAFETY: a view owns no thread-affine state. Synchronization of accesses
    // to its shared bytes is the caller's responsibility.
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
            // SAFETY: this is the complete view owned by `self` and is
            // unmapped exactly once.
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
    ///
    pub fn create_file_mapping<R>(
        file: &OwnedHandle,
        mapping_attributes: Option<&SecurityAttributes>,
        protection: PAGE_PROTECTION_FLAGS,
        maximum_size_high: u32,
        maximum_size_low: u32,
        name: Option<WideCStr<'_, R>>,
    ) -> Result<OwnedHandle> {
        let mapping_attributes =
            mapping_attributes.map_or(ptr::null(), |attributes| ptr::from_ref(&attributes.0));
        let name = name.map_or(ptr::null(), |name| name.as_ptr());

        // SAFETY: `file` is valid. Security attributes are either null or a
        // valid opaque value owned by this module, and `name` is either null
        // or backed by a valid borrowed wide C string. The remaining values
        // are passed through unchanged.
        let mapping = unsafe {
            CreateFileMappingW(
                file.as_raw(),
                mapping_attributes,
                protection,
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
        access: FILE_MAP,
        file_offset_high: u32,
        file_offset_low: u32,
        bytes_to_map: usize,
    ) -> Result<MappingView> {
        // SAFETY: `mapping` is a valid file-mapping object. Windows validates
        // the requested access, offset, and size against that object.
        let view = unsafe {
            MapViewOfFile(mapping.as_raw(), access, file_offset_high, file_offset_low, bytes_to_map)
        };
        let Some(ptr) = core::ptr::NonNull::new(view.Value.cast::<u8>()) else {
            return Err(crate::windows::last_error());
        };
        Ok(MappingView { ptr })
    }
}

#[cfg(windows)]
pub use windows::{
    FILE_MAP, FILE_MAP_READ, FILE_MAP_WRITE, MappingView, PAGE_PROTECTION_FLAGS, PAGE_READWRITE,
    SecurityAttributes, create_file_mapping, map_view_of_file,
};
