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
    use core::{ffi::c_void, num::NonZeroUsize, ptr};

    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS,
        MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
    };

    use crate::{OwnedHandle, Result};

    /// An owned writable view of a shared file mapping.
    pub struct MappedView {
        ptr: core::ptr::NonNull<u8>,
    }

    // SAFETY: a view owns no thread-affine state. Synchronization of accesses
    // to its shared bytes is the caller's responsibility.
    unsafe impl Send for MappedView {}
    // SAFETY: sharing the view does not itself access the mapped bytes.
    unsafe impl Sync for MappedView {}

    impl MappedView {
        /// Returns a raw pointer to the first mapped byte.
        #[must_use]
        pub const fn as_ptr(&self) -> *mut u8 {
            self.ptr.as_ptr()
        }
    }

    impl Drop for MappedView {
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

    /// Maps the first `len` bytes of `file` as shared readable and writable
    /// memory.
    ///
    /// # Errors
    ///
    /// Returns the error reported while creating or mapping the view.
    pub fn map_file(file: &OwnedHandle, len: NonZeroUsize) -> Result<MappedView> {
        // SAFETY: `file` is valid. Null security attributes and name request
        // an unnamed, non-inheritable mapping object backed by the file's
        // current size.
        let mapping = unsafe {
            CreateFileMappingW(file.as_raw(), ptr::null(), PAGE_READWRITE, 0, 0, ptr::null())
        };
        let Some(mapping) = core::ptr::NonNull::new(mapping) else {
            return Err(crate::windows::last_error());
        };
        // SAFETY: `CreateFileMappingW` returned a valid, newly owned handle.
        let mapping = unsafe { OwnedHandle::from_raw(mapping.as_ptr()) };

        // SAFETY: `mapping` is a valid file-mapping object, the requested
        // access matches its protection, and `len` is nonzero.
        let view = unsafe {
            MapViewOfFile(mapping.as_raw(), FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, len.get())
        };
        let Some(ptr) = core::ptr::NonNull::new(view.Value.cast::<u8>()) else {
            return Err(crate::windows::last_error());
        };

        // A mapped view remains valid after its mapping-object handle closes.
        Ok(MappedView { ptr })
    }
}

#[cfg(windows)]
pub use windows::{MappedView, map_file};
