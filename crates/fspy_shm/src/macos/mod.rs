#![doc = include_str!("README.md")]

use std::{io, slice};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use memmap2::{MmapOptions, MmapRaw};
use rustix::{
    fs::{Mode, ftruncate},
    io::Errno,
    shm::{self, OFlags},
};
use uuid::Uuid;

const NAME_PREFIX: &str = "/fspy_";
const ID_BYTES: usize = 9;

/// An owned macOS shared-memory mapping.
pub struct Shm {
    id: String,
    mapping: MmapRaw,
    owner_name: Option<String>,
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
        let name = mapping_name(&id, size);
        let fd = match shm::open(
            name.as_str(),
            OFlags::CREATE | OFlags::EXCL | OFlags::RDWR,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(fd) => fd,
            Err(Errno::EXIST) => continue,
            Err(error) => return Err(error.into()),
        };

        if let Err(error) = ftruncate(&fd, size_u64) {
            let _ = shm::unlink(name.as_str());
            return Err(error.into());
        }
        let mapping = match MmapOptions::new().len(size).map_raw(&fd) {
            Ok(mapping) => mapping,
            Err(error) => {
                let _ = shm::unlink(name.as_str());
                return Err(error);
            }
        };

        return Ok(Shm { id, mapping, owner_name: Some(name) });
    }
}

/// Opens the POSIX shared-memory mapping identified by `id`.
///
/// # Errors
///
/// Returns an error if the mapping is unavailable.
pub fn open(id: &str, size: usize) -> io::Result<Shm> {
    let name = mapping_name(id, size);
    let fd = shm::open(name.as_str(), OFlags::RDWR, Mode::empty()).map_err(io::Error::from)?;
    let mapping = MmapOptions::new().len(size).map_raw(&fd)?;

    Ok(Shm { id: id.to_owned(), mapping, owner_name: None })
}

fn new_id() -> String {
    let uuid = Uuid::new_v4();
    URL_SAFE_NO_PAD.encode(&uuid.as_bytes()[..ID_BYTES])
}

fn mapping_name(id: &str, size: usize) -> String {
    format!("{NAME_PREFIX}{id}_{}", URL_SAFE_NO_PAD.encode(size.to_be_bytes()))
}

impl Drop for Shm {
    fn drop(&mut self) {
        if let Some(name) = &self.owner_name {
            let _ = shm::unlink(name.as_str());
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

    #[test]
    fn size_mismatches_are_rejected() {
        let owner = create(100).unwrap();

        assert!(open(owner.id(), 99).is_err());
        assert!(open(owner.id(), 101).is_err());
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn four_gib_mapping_supports_endpoint_access() {
        const PRODUCTION_SIZE: usize = 4 * 1024 * 1024 * 1024;

        let owner = create(PRODUCTION_SIZE).unwrap();
        let opened = open(owner.id(), PRODUCTION_SIZE).unwrap();

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
