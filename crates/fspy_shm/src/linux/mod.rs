#![doc = include_str!("README.md")]

mod broker;

use std::{io, slice};

use memmap2::{MmapOptions, MmapRaw};
use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, fstat, ftruncate, memfd_create};
use tokio_util::sync::DropGuard;

/// An owned Linux shared-memory mapping.
pub struct Shm {
    id: String,
    mapping: MmapRaw,
    /// Stops the owner's broker on drop. `None` for opened views.
    _service: Option<DropGuard>,
}

/// Creates a sealed memfd mapping of `size` bytes and returns its owner.
///
/// The memfd is handed out to other processes by a broker task spawned onto
/// the ambient tokio runtime. The broker stops on its own when the owner is
/// dropped, after which new [`open`] calls fail while already-open views stay
/// usable (see the [ownership semantics](crate)).
///
/// # Errors
///
/// Returns an error if no tokio runtime is active or the memfd, mapping, or
/// broker listener cannot be created.
pub fn create(size: usize) -> io::Result<Shm> {
    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|_| io::Error::other("creating Linux shared memory requires a tokio runtime"))?;
    let size_u64 = valid_size(size)?;
    // Prevent the descriptor from leaking across exec while permitting the
    // size and seal set to be locked after initialization.
    let memfd =
        memfd_create("vite-task-shared-memory", MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING)?;
    ftruncate(&memfd, size_u64)?;
    // Keep the initialized size fixed and prevent removal of these seals;
    // writes through the shared mapping remain allowed.
    fcntl_add_seals(&memfd, SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL)?;
    let mapping = MmapOptions::new().len(size).map_raw(&memfd)?;
    let (id, service, guard) = broker::new(memfd)?;
    runtime.spawn(service);

    Ok(Shm { id, mapping, _service: Some(guard) })
}

/// Opens a view of the memfd mapping identified by `id` through its broker.
///
/// Guaranteed to succeed only while the mapping's owner is alive; the
/// returned view stays usable independently of the owner afterwards (see the
/// [ownership semantics](crate)).
///
/// # Errors
///
/// Returns an error if the identifier is invalid, the broker is gone, or the
/// received memfd has an invalid size.
pub fn open(id: &str) -> io::Result<Shm> {
    let memfd = broker::request_memfd(id)?;
    let stat = fstat(&memfd)?;
    let size = usize::try_from(stat.st_size)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid shared-memory size"))?;
    if size == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "shared-memory size is zero"));
    }
    let mapping = MmapOptions::new().len(size).map_raw(&memfd)?;
    Ok(Shm { id: id.to_owned(), mapping, _service: None })
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

#[expect(clippy::len_without_is_empty, reason = "shared-memory mappings are always non-empty")]
impl Shm {
    /// Returns this mapping's opaque broker identifier.
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
