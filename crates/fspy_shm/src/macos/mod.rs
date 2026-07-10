#![doc = include_str!("README.md")]

use std::{io, slice};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use memmap2::{MmapOptions, MmapRaw};
use rustix::{
    fs::{Mode, fstat, ftruncate},
    io::Errno,
    shm::{self, OFlags},
};
use uuid::Uuid;

const NAME_PREFIX: &str = "/fspy_";

/// An owned macOS shared-memory mapping.
pub struct Shm {
    id: String,
    mapping: MmapRaw,
    owner: bool,
}

/// Creates a POSIX shared-memory mapping of `size` bytes and returns its
/// owner.
///
/// # Errors
///
/// Returns an error if the object cannot be created, sized, or mapped.
pub fn create(size: usize) -> io::Result<Shm> {
    if size == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "shared-memory size must be nonzero",
        ));
    }
    let size_u64 = u64::try_from(size).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "shared-memory size exceeds u64")
    })?;

    loop {
        let id = new_id();
        // `rustix::shm::open` sets `FD_CLOEXEC` on the returned descriptor.
        let fd = match shm::open(
            id.as_str(),
            OFlags::CREATE | OFlags::EXCL | OFlags::RDWR,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(fd) => fd,
            Err(Errno::EXIST) => continue,
            Err(error) => return Err(error.into()),
        };

        if let Err(error) = ftruncate(&fd, size_u64) {
            let _ = shm::unlink(id.as_str());
            return Err(error.into());
        }
        let mapping = match MmapOptions::new().len(size).map_raw(&fd) {
            Ok(mapping) => mapping,
            Err(error) => {
                let _ = shm::unlink(id.as_str());
                return Err(error);
            }
        };

        return Ok(Shm { id, mapping, owner: true });
    }
}

/// Opens the POSIX shared-memory mapping identified by `id`.
///
/// # Errors
///
/// Returns an error if the mapping is unavailable.
pub fn open(id: &str) -> io::Result<Shm> {
    // `rustix::shm::open` sets `FD_CLOEXEC` on the returned descriptor.
    let fd = shm::open(id, OFlags::RDWR, Mode::empty()).map_err(io::Error::from)?;
    // If another process shrinks the object before `mmap`, `mmap` returns an
    // error. If it resizes the object after `mmap`, `open` does not access the
    // mapped pages. A concurrent resize cannot make `open` access invalid memory.
    let stat = fstat(&fd)?;
    let size = usize::try_from(stat.st_size)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid shared-memory size"))?;
    if size == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "shared-memory size is zero"));
    }
    let mapping = MmapOptions::new().len(size).map_raw(&fd)?;

    Ok(Shm { id: id.to_owned(), mapping, owner: false })
}

fn new_id() -> String {
    format!("{NAME_PREFIX}{}", URL_SAFE_NO_PAD.encode(Uuid::new_v4().as_bytes()))
}

impl Drop for Shm {
    fn drop(&mut self) {
        if self.owner {
            let _ = shm::unlink(self.id.as_str());
        }
    }
}

#[expect(clippy::len_without_is_empty, reason = "shared-memory mappings are always non-empty")]
impl Shm {
    /// Returns this mapping's opaque macOS identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the mapped length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.mapping.len()
    }

    /// Returns a raw pointer to the first mapped byte.
    #[must_use]
    pub fn as_ptr(&self) -> *mut u8 {
        self.mapping.as_mut_ptr()
    }

    /// Returns the mapped bytes as a shared slice.
    ///
    /// # Safety
    ///
    /// The caller must ensure that no process or thread mutates the mapping for
    /// the lifetime of the returned slice.
    #[must_use]
    pub unsafe fn as_slice(&self) -> &[u8] {
        // SAFETY: The mapping is valid for its full length, and the caller
        // guarantees that it is not mutated while the slice is borrowed.
        unsafe { slice::from_raw_parts(self.mapping.as_ptr(), self.mapping.len()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn four_gib_mapping_supports_endpoint_access() {
        const PRODUCTION_SIZE: usize = 4 * 1024 * 1024 * 1024;

        let owner = create(PRODUCTION_SIZE).unwrap();
        let opened = open(owner.id()).unwrap();

        // SAFETY: Both endpoint indexes are within the exact mapped length and
        // accesses are synchronized within this test.
        unsafe {
            owner.as_ptr().write(17);
            owner.as_ptr().add(PRODUCTION_SIZE - 1).write(29);
            assert_eq!(opened.as_ptr().read(), 17);
            assert_eq!(opened.as_ptr().add(PRODUCTION_SIZE - 1).read(), 29);
        }
    }
}
