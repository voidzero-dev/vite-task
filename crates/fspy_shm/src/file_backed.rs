//! Shared memory backed by a sparse temporary file and identified by its path.
//!
//! One implementation serves every platform. The platform-specific parts are
//! the creation flags, marking the file sparse on NTFS, and a fallback for
//! removing the name on old Windows.

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt as _;
use std::{
    env::temp_dir,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io,
    path::PathBuf,
};

use memmap2::{MmapOptions, MmapRaw};
use uuid::Uuid;

#[cfg(windows)]
mod sys;

/// Prefix of backing file names inside the system temporary directory.
///
/// The files sit directly in the temporary directory. A shared subdirectory
/// would belong to whichever user created it first and block everyone else;
/// uniquely named `0o600` files in the sticky-bit temp directory avoid that.
const BACKING_PREFIX: &str = "vite-task-fspy-";

/// Keeps the shared memory's identifier alive and removes it on drop.
///
/// Removal is cleanup, not a stop signal: later opens fail, but existing
/// [`ShmHandle`]s and [`Mapping`]s keep reading and writing. To stop them,
/// store a flag in the shared bytes, as the fspy channel's close gate does.
pub struct ShmKeeper {
    path: PathBuf,
}

/// Opened shared memory that is not mapped yet.
///
/// [`map`](Self::map) can be called more than once; every call returns another
/// view of the same bytes. Drop the handle once the mappings exist.
pub struct ShmHandle {
    file: File,
    size: usize,
}

/// The mapped shared bytes.
///
/// A `Mapping` keeps the bytes alive until it is dropped and cannot affect the
/// shared memory's identifier.
pub struct Mapping {
    raw: MmapRaw,
}

/// Creates `size` bytes of zero-initialized shared memory.
///
/// Returns its [`ShmKeeper`] and an already opened [`ShmHandle`], so the
/// creating process never has to go through [`open`].
///
/// Only pages that are actually written occupy memory or disk, so a large
/// capacity is cheap.
///
/// # Errors
///
/// Returns an error if the shared memory cannot be created or sized. Creation
/// fails on volumes without sparse-file support.
pub fn create(size: usize) -> io::Result<(ShmKeeper, ShmHandle)> {
    if size == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "shared-memory size must be nonzero",
        ));
    }
    let size_u64 = u64::try_from(size).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "shared-memory size exceeds u64")
    })?;

    // `temp_dir` reflects `TMPDIR` verbatim, which may be relative. The
    // identifier travels to processes with other working directories, so
    // resolve it against the creator's current directory first.
    let path = std::path::absolute(temp_dir())?
        .join(format!("{BACKING_PREFIX}{}.shm", Uuid::new_v4().simple()));

    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    // Only the creating user may open the mapping.
    #[cfg(unix)]
    options.mode(0o600);
    // The per-user `%TEMP%` ACL provides the same-user gating that `0o600`
    // provides on Unix. `FILE_ATTRIBUTE_TEMPORARY` asks Windows to keep the
    // data in memory when it can.
    #[cfg(windows)]
    options.share_mode(sys::SHARE_ALL).attributes(sys::TEMPORARY);

    let file = options.open(&path)?;
    // The keeper exists from here on, so every error path below cleans up.
    let keeper = ShmKeeper { path };

    // NTFS allocates clusters for the whole logical size unless the file is
    // marked sparse first, which would turn the capacity into real disk usage.
    // Volumes without sparse-file support fail here.
    #[cfg(windows)]
    sys::set_sparse(&file)?;
    // Every byte reads as zero because the file is all holes.
    file.set_len(size_u64)?;

    Ok((keeper, ShmHandle { file, size }))
}

/// Opens the shared memory identified by `id`.
///
/// The identifier works from any process, regardless of the process's working
/// directory or environment.
///
/// # Errors
///
/// Returns an error if the shared memory is unavailable, which is the common
/// case once its keeper has been dropped.
pub fn open(id: &OsStr) -> io::Result<ShmHandle> {
    // Rust opens are `O_CLOEXEC` / non-inheritable on every platform, so a
    // traced process never leaks this descriptor, and Rust's default Windows
    // share mode permits concurrent read, write and delete access.
    let file = OpenOptions::new().read(true).write(true).open(id)?;
    // If another process shrinks the file before `map`, mapping fails. If it
    // resizes afterwards, nothing here touches the mapped pages. A concurrent
    // resize cannot make a mapping access invalid memory.
    let size = usize::try_from(file.metadata()?.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid shared-memory size"))?;
    if size == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "shared-memory size is zero"));
    }
    Ok(ShmHandle { file, size })
}

impl Drop for ShmKeeper {
    fn drop(&mut self) {
        let _removed = fs::remove_file(&self.path);
        // Windows versions without POSIX delete refuse to remove the name of a
        // mapped file. Arm the deferred delete instead: a handle opened with
        // `FILE_FLAG_DELETE_ON_CLOSE` deletes the file once every handle to it
        // is closed.
        #[cfg(windows)]
        if _removed.is_err() {
            let _ = OpenOptions::new()
                .access_mode(sys::DELETE)
                .share_mode(sys::SHARE_ALL)
                .custom_flags(sys::DELETE_ON_CLOSE)
                .open(&self.path);
        }
    }
}

impl ShmKeeper {
    /// Returns the shared memory's opaque identifier, which any process passes
    /// to [`open`].
    #[must_use]
    pub fn id(&self) -> &OsStr {
        self.path.as_os_str()
    }
}

impl ShmHandle {
    /// Maps the shared bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the mapping cannot be established.
    pub fn map(&self) -> io::Result<Mapping> {
        Ok(Mapping { raw: MmapOptions::new().len(self.size).map_raw(&self.file)? })
    }
}

#[expect(clippy::len_without_is_empty, reason = "shared-memory mappings are always non-empty")]
impl Mapping {
    /// Returns the mapped length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.raw.len()
    }

    /// Returns a raw pointer to the first mapped byte.
    #[must_use]
    pub fn as_ptr(&self) -> *mut u8 {
        self.raw.as_mut_ptr()
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
        unsafe { std::slice::from_raw_parts(self.as_ptr().cast_const(), self.len()) }
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, path::Path, process::Command};

    use subprocess_test::command_for_fn;

    use super::*;

    const SIZE: usize = 64 * 1024;

    #[test]
    fn one_handle_maps_repeatedly() {
        let (_keeper, handle) = create(SIZE).unwrap();
        let first = handle.map().unwrap();
        let second = handle.map().unwrap();

        // SAFETY: In bounds, and this test synchronizes all accesses.
        unsafe {
            first.as_ptr().write(17);
            assert_eq!(second.as_ptr().read(), 17);
        }
    }

    #[test]
    fn subprocess_open_ignores_changed_temp_and_working_directory() {
        let (keeper, handle) = create(SIZE).unwrap();
        let mapping = handle.map().unwrap();
        let changed_cwd =
            temp_dir().join(format!("{BACKING_PREFIX}changed-cwd-{}", Uuid::new_v4()));
        fs::create_dir(&changed_cwd).unwrap();
        // SAFETY: The child does not access the mapping until this write completes.
        unsafe { mapping.as_ptr().write(17) };

        let id = keeper.id().to_str().expect("test temp dir is UTF-8").to_owned();
        let mut command = command_for_fn!(id, |id: String| {
            let opened = open(OsStr::new(&id)).unwrap().map().unwrap();
            // SAFETY: The parent waits for this child and does not access the
            // mapping concurrently.
            unsafe {
                assert_eq!(opened.as_ptr().read(), 17);
                opened.as_ptr().add(SIZE - 1).write(29);
            }
        });
        command.cwd = changed_cwd.clone();
        // The identifier is an absolute path, so a relative temporary directory
        // in the child must make no difference on any platform.
        for name in ["TMPDIR", "TMP", "TEMP"] {
            command.envs.insert(OsString::from(name), OsString::from("changed-relative-tmp"));
        }
        let succeeded = Command::from(command).status().unwrap().success();
        fs::remove_dir(changed_cwd).unwrap();

        assert!(succeeded);
        // SAFETY: The child exited before this read.
        assert_eq!(unsafe { mapping.as_ptr().add(SIZE - 1).read() }, 29);
    }

    /// Removal semantics, part one: a mapping alone (no handle) keeps the
    /// bytes alive across the keeper's removal of the name.
    #[test]
    fn keeper_drop_removes_backing_file_and_preserves_existing_mappings() {
        let (keeper, handle) = create(SIZE).unwrap();
        let id = keeper.id().to_owned();
        let path = Path::new(&id).to_owned();
        let opened = open(&id).unwrap().map().unwrap();
        drop(handle);
        assert!(path.exists());

        drop(keeper);

        assert!(!path.exists());
        assert!(open(&id).is_err());
        // SAFETY: The mapping remains live and no other access is concurrent.
        unsafe { opened.as_ptr().write(17) };
        // SAFETY: The preceding write is complete and the mapping remains live.
        assert_eq!(unsafe { opened.as_ptr().read() }, 17);
    }

    /// Removal semantics, part two: the name goes away even while a handle is
    /// still open, and that handle keeps mapping the same bytes afterwards.
    #[test]
    fn keeper_drop_with_open_handle_removes_name_and_handle_still_maps() {
        let (keeper, handle) = create(SIZE).unwrap();
        let id = keeper.id().to_owned();
        let before = handle.map().unwrap();
        // SAFETY: In bounds, and this test synchronizes all accesses.
        unsafe { before.as_ptr().write(17) };

        drop(keeper);

        assert!(!Path::new(&id).exists());
        assert!(open(&id).is_err());

        let after = handle.map().unwrap();
        // SAFETY: In bounds, and this test synchronizes all accesses.
        unsafe {
            assert_eq!(after.as_ptr().read(), 17);
            after.as_ptr().add(SIZE - 1).write(29);
            assert_eq!(before.as_ptr().add(SIZE - 1).read(), 29);
        }
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn four_gib_mapping_is_sparse_and_supports_endpoint_access() {
        const PRODUCTION_SIZE: usize = 4 * 1024 * 1024 * 1024;
        #[cfg(windows)]
        const MAX_ENDPOINT_ALLOCATION: u64 = 16 * 1024 * 1024;

        let (keeper, handle) = create(PRODUCTION_SIZE).unwrap();
        #[cfg(windows)]
        {
            let (logical_size, initial_allocation) = backing_file_sizes(keeper.id());
            assert_eq!(logical_size, PRODUCTION_SIZE as u64);
            assert!(initial_allocation < MAX_ENDPOINT_ALLOCATION);
        }

        let first = handle.map().unwrap();
        let opened = open(keeper.id()).unwrap().map().unwrap();
        // SAFETY: Both endpoint indexes are within the exact mapped length and
        // accesses are synchronized within this test.
        unsafe {
            first.as_ptr().write(17);
            first.as_ptr().add(PRODUCTION_SIZE - 1).write(29);
            assert_eq!(opened.as_ptr().read(), 17);
            assert_eq!(opened.as_ptr().add(PRODUCTION_SIZE - 1).read(), 29);
        }

        // Touching both endpoints must not have allocated the range between them.
        #[cfg(windows)]
        {
            let (logical_size, endpoint_allocation) = backing_file_sizes(keeper.id());
            assert_eq!(logical_size, PRODUCTION_SIZE as u64);
            assert!(endpoint_allocation < MAX_ENDPOINT_ALLOCATION);
        }
    }

    /// The backing file's logical size and the bytes NTFS actually allocated
    /// for it, read through a separate handle on the keeper's path.
    #[cfg(windows)]
    fn backing_file_sizes(id: &OsStr) -> (u64, u64) {
        let file = File::open(id).unwrap();
        sys::file_sizes(&file).unwrap()
    }
}
