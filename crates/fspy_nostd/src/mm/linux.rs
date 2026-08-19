use core::{ffi::c_void, ptr};

use super::{MapFlags, MprotectFlags, ProtFlags};
use crate::{BorrowedFd, Error, Result};

pub(super) const PROT_READ: u32 = linux_raw_sys::general::PROT_READ;
pub(super) const PROT_WRITE: u32 = linux_raw_sys::general::PROT_WRITE;
pub(super) const PROT_EXEC: u32 = linux_raw_sys::general::PROT_EXEC;
pub(super) const MAP_SHARED: u32 = linux_raw_sys::general::MAP_SHARED;
pub(super) const MAP_PRIVATE: u32 = linux_raw_sys::general::MAP_PRIVATE;

pub(super) unsafe fn mmap(
    address: *mut c_void,
    length: usize,
    protection: ProtFlags,
    flags: MapFlags,
    fd: BorrowedFd<'_>,
    offset: u64,
) -> Result<*mut c_void> {
    // SAFETY: the caller upholds the mapping contract and every argument is
    // passed directly to the kernel.
    let mapped = unsafe {
        syscalls::syscall!(
            syscalls::Sysno::mmap,
            address,
            length,
            protection.bits(),
            flags.bits(),
            fd.as_raw_fd(),
            offset
        )
    }
    .map_err(Error::from)?;
    Ok(ptr::with_exposed_provenance_mut(mapped))
}

pub(super) unsafe fn mmap_anonymous(
    address: *mut c_void,
    length: usize,
    protection: ProtFlags,
    flags: MapFlags,
) -> Result<*mut c_void> {
    let flags = flags.bits() | linux_raw_sys::general::MAP_ANONYMOUS;
    // SAFETY: the caller upholds the mapping contract and every argument is
    // passed directly to the kernel. Anonymous mappings require fd -1.
    let mapped = unsafe {
        syscalls::syscall!(
            syscalls::Sysno::mmap,
            address,
            length,
            protection.bits(),
            flags,
            -1_i32,
            0_usize
        )
    }
    .map_err(Error::from)?;
    Ok(ptr::with_exposed_provenance_mut(mapped))
}

pub(super) unsafe fn mprotect(
    address: *mut c_void,
    length: usize,
    protection: MprotectFlags,
) -> Result<()> {
    // SAFETY: the caller upholds the mapped-region contract.
    unsafe { syscalls::syscall!(syscalls::Sysno::mprotect, address, length, protection.bits()) }
        .map_err(Error::from)?;
    Ok(())
}

pub(super) unsafe fn munmap(address: *mut c_void, length: usize) -> Result<()> {
    // SAFETY: the caller upholds the mapping-ownership contract.
    unsafe { syscalls::syscall!(syscalls::Sysno::munmap, address, length) }.map_err(Error::from)?;
    Ok(())
}
