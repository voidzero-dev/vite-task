use std::{io, slice};

use memmap2::{MmapOptions, MmapRaw};
use rustix::{
    fs::{Mode, fstat, ftruncate},
    io::Errno,
    param::page_size,
    shm::{self, OFlags},
};
use uuid::Uuid;

const NAME_PREFIX: &str = "/fspy_";
// Darwin rounds `st_size` to a VM page, so retain the exact-size residue in
// the name while leaving 80 random bits for collision resistance.
const NAME_RANDOM_BYTES: usize = 10;
const NAME_SIZE_BYTES: usize = 2;
const NAME_SUFFIX_LEN: usize = (NAME_RANDOM_BYTES + NAME_SIZE_BYTES) * 2;
const NAME_LEN: usize = NAME_PREFIX.len() + NAME_SUFFIX_LEN;
const SIZE_TAG_MODULUS: u64 = 1 << (NAME_SIZE_BYTES * 8);
const HEX: &[u8; 16] = b"0123456789abcdef";

/// An owned macOS shared-memory mapping.
pub struct Shm {
    name: ShmName,
    mapping: MmapRaw,
}

/// Creates a POSIX shared-memory mapping of `size` bytes and returns its
/// owner.
///
/// # Errors
///
/// Returns an error if the object cannot be created, sized, or mapped.
pub fn create(size: usize) -> io::Result<Shm> {
    create_with(size, || new_id(size))
}

fn create_with(size: usize, mut next_id: impl FnMut() -> String) -> io::Result<Shm> {
    let size_u64 = valid_size(size)?;

    loop {
        let id = next_id();
        validate_id(&id)?;
        if !size_tag_matches(&id, size) {
            return Err(invalid_id());
        }
        let fd = match shm::open(
            id.as_str(),
            OFlags::CREATE | OFlags::EXCL | OFlags::RDWR,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(fd) => fd,
            Err(Errno::EXIST) => continue,
            Err(error) => return Err(error.into()),
        };
        // From this point on, every failure path unlinks the newly created name.
        let owner = OwnerName { id };

        ftruncate(&fd, size_u64).map_err(io::Error::from)?;
        let mapping = MmapOptions::new().len(size).map_raw(&fd)?;
        drop(fd);

        return Ok(Shm { name: ShmName::Owner(owner), mapping });
    }
}

/// Opens the POSIX shared-memory mapping identified by `id`.
///
/// # Errors
///
/// Returns an error if the identifier is invalid, the object is unavailable,
/// or its size does not exactly match `size`.
pub fn open(id: &str, size: usize) -> io::Result<Shm> {
    let size_u64 = valid_size(size)?;
    validate_id(id)?;

    let fd = shm::open(id, OFlags::RDWR, Mode::empty()).map_err(io::Error::from)?;
    let stat = fstat(&fd).map_err(io::Error::from)?;
    let actual_size = u64::try_from(stat.st_size).ok();
    if !size_tag_matches(id, size)
        || (actual_size != Some(size_u64) && actual_size != rounded_size(size_u64))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "shared-memory object has an unexpected size",
        ));
    }
    let mapping = MmapOptions::new().len(size).map_raw(&fd)?;
    drop(fd);

    Ok(Shm { name: ShmName::Opened(id.to_owned()), mapping })
}

fn new_id(size: usize) -> String {
    let uuid = Uuid::new_v4();
    let mut id = String::with_capacity(NAME_LEN);
    id.push_str(NAME_PREFIX);
    for byte in &uuid.as_bytes()[..NAME_RANDOM_BYTES] {
        push_hex_byte(&mut id, *byte);
    }
    let size_bytes = size.to_be_bytes();
    for byte in &size_bytes[size_bytes.len() - NAME_SIZE_BYTES..] {
        push_hex_byte(&mut id, *byte);
    }
    id
}

fn push_hex_byte(output: &mut String, byte: u8) {
    output.push(char::from(HEX[usize::from(byte >> 4)]));
    output.push(char::from(HEX[usize::from(byte & 0x0f)]));
}

fn size_tag_matches(id: &str, size: usize) -> bool {
    let size_bytes = size.to_be_bytes();
    let expected = &size_bytes[size_bytes.len() - NAME_SIZE_BYTES..];
    let encoded = &id.as_bytes()[NAME_LEN - NAME_SIZE_BYTES * 2..];

    encoded.chunks_exact(2).zip(expected).all(|(digits, byte)| {
        digits[0] == HEX[usize::from(byte >> 4)] && digits[1] == HEX[usize::from(byte & 0x0f)]
    })
}

fn rounded_size(size: u64) -> Option<u64> {
    let page_size = u64::try_from(page_size()).ok()?;
    if page_size == 0 || page_size > SIZE_TAG_MODULUS {
        return None;
    }
    let remainder = size.checked_rem(page_size)?;
    if remainder == 0 { Some(size) } else { size.checked_add(page_size - remainder) }
}

fn validate_id(id: &str) -> io::Result<()> {
    let Some(suffix) = id.strip_prefix(NAME_PREFIX) else {
        return Err(invalid_id());
    };
    if id.len() != NAME_LEN
        || !suffix.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid_id());
    }
    Ok(())
}

fn invalid_id() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid macOS shared-memory identifier")
}

fn valid_size(size: usize) -> io::Result<u64> {
    if size == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "shared-memory size must be nonzero",
        ));
    }
    u64::try_from(size)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "shared-memory size exceeds u64"))
}

struct OwnerName {
    id: String,
}

impl Drop for OwnerName {
    fn drop(&mut self) {
        let _ = shm::unlink(self.id.as_str());
    }
}

enum ShmName {
    Owner(OwnerName),
    Opened(String),
}

impl ShmName {
    fn as_str(&self) -> &str {
        match self {
            Self::Owner(owner) => &owner.id,
            Self::Opened(id) => id,
        }
    }
}

#[expect(clippy::len_without_is_empty, reason = "shared-memory mappings are always non-empty")]
impl Shm {
    /// Returns this mapping's opaque POSIX shared-memory name.
    #[must_use]
    pub fn id(&self) -> &str {
        self.name.as_str()
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

    const SIZE: usize = 64 * 1024;

    #[test]
    fn identifiers_are_canonical_darwin_names() {
        let owner = create(SIZE).unwrap();
        let suffix = owner.id().strip_prefix(NAME_PREFIX).unwrap();

        assert_eq!(owner.id().len(), NAME_LEN);
        assert!(owner.id().len() <= 31);
        assert_eq!(suffix.len(), NAME_SUFFIX_LEN);
        assert!(size_tag_matches(owner.id(), SIZE));
        assert!(suffix.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }

    #[test]
    fn malformed_ids_and_inexact_sizes_are_rejected() {
        for id in [
            "fspy_000000000000000000000000",
            "/fspy_00000000000000000000000",
            "/fspy_0000000000000000000000000",
            "/fspy_00000000000000000000000g",
            "/fspy_00000000000000000000000A",
            "/fspy_00000000000/000000000000",
        ] {
            assert_eq!(open(id, SIZE).err().unwrap().kind(), io::ErrorKind::InvalidInput);
        }

        let owner = create(SIZE).unwrap();
        assert_eq!(open(owner.id(), 0).err().unwrap().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(open(owner.id(), SIZE - 1).err().unwrap().kind(), io::ErrorKind::InvalidData);
        assert_eq!(open(owner.id(), SIZE + 1).err().unwrap().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn non_page_aligned_size_reopens_exactly() {
        let owner = create(100).unwrap();
        let opened = open(owner.id(), 100).unwrap();

        assert_eq!(owner.len(), 100);
        assert_eq!(opened.len(), 100);
    }

    #[test]
    fn name_collisions_are_retried() {
        let existing = create(SIZE).unwrap();
        let fresh_id = loop {
            let id = new_id(SIZE);
            if id != existing.id() {
                break id;
            }
        };
        let mut ids = [existing.id().to_owned(), fresh_id.clone()].into_iter();

        let created =
            create_with(SIZE, || ids.next().expect("creation did not stop after retry")).unwrap();

        assert_eq!(created.id(), fresh_id);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn failed_initialization_unlinks_the_created_name() {
        let id = new_id(usize::MAX);

        assert!(create_with(usize::MAX, || id.clone()).is_err());
        assert!(matches!(shm::open(id.as_str(), OFlags::RDWR, Mode::empty()), Err(Errno::NOENT)));
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
