//! Memory mappings with no process-runtime dependency.

#[cfg(any(target_os = "linux", target_os = "none"))]
mod linux;
#[cfg(any(target_os = "macos", target_os = "freebsd"))]
mod mac;
#[cfg(windows)]
mod windows;

#[cfg(any(target_os = "linux", target_os = "none"))]
use linux as imp;
#[cfg(any(target_os = "macos", target_os = "freebsd"))]
use mac as imp;
#[cfg(windows)]
pub use windows::*;

#[cfg(any(target_os = "linux", target_os = "none", target_os = "macos", target_os = "freebsd"))]
mod unix {
    use core::ffi::c_void;

    use bitflags::bitflags;

    use super::imp;
    use crate::{BorrowedFd, Result};

    bitflags! {
        /// Access permitted for mapped pages.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub struct ProtFlags: u32 {
            /// Pages may be read.
            const READ = imp::PROT_READ;
            /// Pages may be written.
            const WRITE = imp::PROT_WRITE;
            /// Pages may be executed.
            const EXEC = imp::PROT_EXEC;
        }

        /// Access permitted after changing a mapping's protection.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub struct MprotectFlags: u32 {
            /// Pages may be read.
            const READ = imp::PROT_READ;
            /// Pages may be written.
            const WRITE = imp::PROT_WRITE;
            /// Pages may be executed.
            const EXEC = imp::PROT_EXEC;
        }

        /// Sharing behavior for a mapping.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub struct MapFlags: u32 {
            /// Changes are shared with mappings of the same object.
            const SHARED = imp::MAP_SHARED;
            /// Changes are private to this mapping.
            const PRIVATE = imp::MAP_PRIVATE;
        }
    }

    /// Maps bytes from `fd`.
    ///
    /// # Errors
    ///
    /// Returns the error reported by the operating system.
    ///
    /// # Safety
    ///
    /// The caller must uphold the kernel's address, length, offset, and
    /// aliasing requirements for the requested mapping.
    pub unsafe fn mmap(
        address: *mut c_void,
        length: usize,
        protection: ProtFlags,
        flags: MapFlags,
        fd: BorrowedFd<'_>,
        offset: u64,
    ) -> Result<*mut c_void> {
        // SAFETY: forwarded from this function's contract.
        unsafe { imp::mmap(address, length, protection, flags, fd, offset) }
    }

    /// Creates an anonymous mapping.
    ///
    /// # Errors
    ///
    /// Returns the error reported by the operating system.
    ///
    /// # Safety
    ///
    /// The caller must uphold the kernel's address, length, and aliasing
    /// requirements for the requested mapping.
    pub unsafe fn mmap_anonymous(
        address: *mut c_void,
        length: usize,
        protection: ProtFlags,
        flags: MapFlags,
    ) -> Result<*mut c_void> {
        // SAFETY: forwarded from this function's contract.
        unsafe { imp::mmap_anonymous(address, length, protection, flags) }
    }

    /// Changes protection on mapped pages.
    ///
    /// # Errors
    ///
    /// Returns the error reported by the operating system.
    ///
    /// # Safety
    ///
    /// `address..address + length` must be a mapped region on which changing
    /// protection does not violate live-reference requirements.
    pub unsafe fn mprotect(
        address: *mut c_void,
        length: usize,
        protection: MprotectFlags,
    ) -> Result<()> {
        // SAFETY: forwarded from this function's contract.
        unsafe { imp::mprotect(address, length, protection) }
    }

    /// Releases a mapping.
    ///
    /// # Errors
    ///
    /// Returns the error reported by the operating system.
    ///
    /// # Safety
    ///
    /// `address..address + length` must be a mapping owned by the caller with
    /// no remaining live references.
    pub unsafe fn munmap(address: *mut c_void, length: usize) -> Result<()> {
        // SAFETY: forwarded from this function's contract.
        unsafe { imp::munmap(address, length) }
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "none",
    target_os = "macos",
    target_os = "freebsd"
))]
pub use unix::*;
