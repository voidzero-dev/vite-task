#![doc = include_str!("../README.md")]

#[cfg(not(target_os = "linux"))]
use std::io;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::{Shm, create, open};
#[cfg(not(target_os = "linux"))]
use shared_memory::{Shmem, ShmemConf};

/// An owned shared-memory mapping.
#[cfg(not(target_os = "linux"))]
pub struct Shm {
    inner: Shmem,
}

/// Creates a shared-memory mapping of `size` bytes and returns its owner.
///
/// Dropping the returned owner stops new [`open`] calls from being guaranteed
/// to succeed, while views that are already open stay usable (see the
/// [ownership semantics](crate)).
///
/// # Errors
///
/// Returns an error if the platform cannot create or map the region.
#[cfg(not(target_os = "linux"))]
pub fn create(size: usize) -> io::Result<Shm> {
    let conf = ShmemConf::new().size(size);
    #[cfg(target_os = "windows")]
    let conf = conf.allow_raw(true);

    let inner = conf.create().map_err(io::Error::other)?;
    Ok(Shm { inner })
}

/// Opens a view of the shared-memory mapping identified by `id`.
///
/// Guaranteed to succeed only while the mapping's owner is alive; the
/// returned view stays usable independently of the owner afterwards (see the
/// [ownership semantics](crate)).
///
/// # Errors
///
/// Returns an error if the mapping does not exist or cannot be mapped.
#[cfg(not(target_os = "linux"))]
pub fn open(id: &str) -> io::Result<Shm> {
    let conf = ShmemConf::new().os_id(id);
    #[cfg(target_os = "windows")]
    let conf = conf.allow_raw(true);

    let inner = conf.open().map_err(io::Error::other)?;
    Ok(Shm { inner })
}

#[cfg(not(target_os = "linux"))]
#[expect(clippy::len_without_is_empty, reason = "shared-memory mappings are always non-empty")]
impl Shm {
    /// Returns this mapping's opaque platform identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        self.inner.get_os_id()
    }

    /// Returns the mapped length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns a raw pointer to the first mapped byte.
    #[must_use]
    pub fn as_ptr(&self) -> *mut u8 {
        self.inner.as_ptr()
    }

    /// Returns the mapped bytes as a shared slice.
    ///
    /// # Safety
    ///
    /// The caller must ensure that no process or thread mutates the mapping for
    /// the lifetime of the returned slice.
    #[must_use]
    pub unsafe fn as_slice(&self) -> &[u8] {
        // SAFETY: The caller upholds the same synchronization contract required by `Shmem`.
        unsafe { self.inner.as_slice() }
    }
}

#[cfg(test)]
mod tests {
    use std::{mem::align_of, process::Command};

    use subprocess_test::command_for_fn;

    use super::{Shm, create, open};

    // Page-aligned on all supported targets.
    const SIZE: usize = 64 * 1024;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn create_and_open_are_shared() {
        let owner = create(SIZE).unwrap();
        assert_eq!(owner.len(), SIZE);
        assert_eq!(owner.as_ptr() as usize % align_of::<usize>(), 0);
        // SAFETY: No writes occur while this slice is borrowed.
        assert!(unsafe { owner.as_slice() }.iter().all(|byte| *byte == 0));

        let opened = open(owner.id()).unwrap();
        assert_eq!(opened.id(), owner.id());
        assert_eq!(opened.len(), SIZE);

        write_byte(&owner, 0, 17);
        assert_eq!(read_byte(&opened, 0), 17);
        write_byte(&opened, SIZE - 1, 29);
        assert_eq!(read_byte(&owner, SIZE - 1), 29);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mapping_is_visible_across_processes() {
        let owner = create(SIZE).unwrap();
        write_byte(&owner, 0, 17);

        let command = command_for_fn!(owner.id().to_owned(), |id: String| {
            let opened = open(&id).unwrap();
            assert_eq!(read_byte(&opened, 0), 17);
            write_byte(&opened, SIZE - 1, 29);
        });
        let success =
            tokio::task::spawn_blocking(move || Command::from(command).status().unwrap().success())
                .await
                .unwrap();
        assert!(success);
        assert_eq!(read_byte(&owner, SIZE - 1), 29);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn owner_drop_prevents_new_opens() {
        let owner = create(SIZE).unwrap();
        let id = owner.id().to_owned();
        drop(owner);

        assert!(open(&id).is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn opened_mapping_survives_owner_drop() {
        let owner = create(SIZE).unwrap();
        let id = owner.id().to_owned();
        let opened = open(&id).unwrap();
        write_byte(&owner, 0, 17);
        drop(owner);

        // Windows keeps the named object alive while an opened view exists.
        #[cfg(not(target_os = "windows"))]
        assert!(open(&id).is_err());
        assert_eq!(read_byte(&opened, 0), 17);
        write_byte(&opened, SIZE - 1, 29);
        assert_eq!(read_byte(&opened, SIZE - 1), 29);
    }

    fn read_byte(shm: &Shm, index: usize) -> u8 {
        assert!(index < shm.len());
        // SAFETY: The index is in bounds and tests synchronize all accesses.
        unsafe { shm.as_ptr().add(index).read() }
    }

    fn write_byte(shm: &Shm, index: usize, value: u8) {
        assert!(index < shm.len());
        // SAFETY: The index is in bounds and tests synchronize all accesses.
        unsafe { shm.as_ptr().add(index).write(value) };
    }
}
