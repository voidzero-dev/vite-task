use core::ffi::c_void;

use super::{MapFlags, MprotectFlags, ProtFlags};
use crate::{BorrowedFd, Error, Result};

pub(super) const PROT_READ: u32 = libc::PROT_READ.cast_unsigned();
pub(super) const PROT_WRITE: u32 = libc::PROT_WRITE.cast_unsigned();
pub(super) const PROT_EXEC: u32 = libc::PROT_EXEC.cast_unsigned();
pub(super) const MAP_SHARED: u32 = libc::MAP_SHARED.cast_unsigned();
pub(super) const MAP_PRIVATE: u32 = libc::MAP_PRIVATE.cast_unsigned();

pub(super) unsafe fn mmap(
    address: *mut c_void,
    length: usize,
    protection: ProtFlags,
    flags: MapFlags,
    fd: BorrowedFd<'_>,
    offset: u64,
) -> Result<*mut c_void> {
    let offset = i64::try_from(offset).map_err(|_| Error::OVERFLOW)?;
    // SAFETY: the caller upholds the mapping contract; libSystem validates
    // every scalar argument.
    let mapped = unsafe {
        libc::mmap(
            address,
            length,
            protection.bits().cast_signed(),
            flags.bits().cast_signed(),
            fd.as_raw_fd(),
            offset,
        )
    };
    if mapped == libc::MAP_FAILED { Err(Error::last_os_error()) } else { Ok(mapped) }
}

pub(super) unsafe fn mmap_anonymous(
    address: *mut c_void,
    length: usize,
    protection: ProtFlags,
    flags: MapFlags,
) -> Result<*mut c_void> {
    let flags = flags.bits().cast_signed() | libc::MAP_ANON;
    // SAFETY: the caller upholds the mapping contract; anonymous mappings do
    // not consume a file descriptor or offset.
    let mapped =
        unsafe { libc::mmap(address, length, protection.bits().cast_signed(), flags, -1, 0) };
    if mapped == libc::MAP_FAILED { Err(Error::last_os_error()) } else { Ok(mapped) }
}

pub(super) unsafe fn mprotect(
    address: *mut c_void,
    length: usize,
    protection: MprotectFlags,
) -> Result<()> {
    // SAFETY: the caller upholds the mapped-region contract.
    if unsafe { libc::mprotect(address, length, protection.bits().cast_signed()) } == -1 {
        Err(Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) unsafe fn munmap(address: *mut c_void, length: usize) -> Result<()> {
    // SAFETY: the caller upholds the mapping-ownership contract.
    if unsafe { libc::munmap(address, length) } == -1 {
        Err(Error::last_os_error())
    } else {
        Ok(())
    }
}
