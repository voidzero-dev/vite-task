//! Shared memory backed by a sparse temporary file and identified by its path.
//!
//! Creating and opening are the same code on every platform. What differs is
//! contained: the creation flags that make the file same-user and
//! self-deleting, marking the file sparse on NTFS, how the owner's cleanup
//! removes the name, and which primitive maps the bytes.

#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::path::PathBuf;
use std::{
    env::temp_dir,
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io,
    path::Path,
};

#[cfg(unix)]
use memmap2::{MmapOptions, MmapRaw};
use uuid::Uuid;

#[cfg(windows)]
mod sys;

/// The platform's raw mapped view of a backing file.
///
/// Unix uses `memmap2`. Windows cannot: `memmap2` duplicates the file handle
/// and keeps it for the mapping's lifetime, which would hold the owner's
/// delete-on-close hostage until every view unmapped, so Windows maps the file
/// itself and keeps only the view.
#[cfg(unix)]
type RawMapping = MmapRaw;
#[cfg(windows)]
type RawMapping = sys::MappedView;

/// Prefix of backing file names inside the system temporary directory.
///
/// The files sit directly in the temporary directory. A shared subdirectory
/// would belong to whichever user created it first and block everyone else;
/// uniquely named `0o600` files in the sticky-bit temp directory avoid that.
const BACKING_PREFIX: &str = "vite-task-fspy-";

/// The mapped bytes of a shared-memory backing file.
///
/// A `Mapping` carries no identifier, no handle, and no cleanup duty. It keeps
/// the bytes alive until it is dropped and cannot affect the backing file's
/// name.
pub struct Mapping {
    raw: RawMapping,
}

/// Keeps the shared memory's name alive and removes it on drop.
///
/// A keeper holds no view of the bytes. The creating process calls [`open`]
/// with the keeper's identifier, like every other process.
///
/// Removal on drop is cleanup: later opens fail, existing [`Mapping`]s keep
/// working. A protocol that needs to invalidate the shared memory's contents
/// must write that into the bytes.
pub struct ShmKeeper {
    id: OsString,
    _cleanup: OwnerCleanup,
}

/// Creates a zero-initialized, sparse, temporary backing file of `size` bytes
/// and returns its [`ShmKeeper`].
///
/// Only pages that are actually written occupy memory or disk, so a large
/// capacity is cheap.
///
/// # Errors
///
/// Returns an error if the backing file cannot be created or sized. Creation
/// also fails when the volume holding the temporary directory does not support
/// sparse files.
pub fn create(size: usize) -> io::Result<ShmKeeper> {
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
    let id = path.as_os_str().to_owned();

    let file = create_options().open(&path)?;
    match size_backing_file(&file, size_u64) {
        Ok(()) => Ok(ShmKeeper {
            id,
            _cleanup: OwnerCleanup {
                #[cfg(unix)]
                path,
                #[cfg(windows)]
                _file: file,
            },
        }),
        Err(error) => {
            discard_backing_file(&path);
            Err(error)
        }
    }
}

/// Opens a [`Mapping`] of the shared memory identified by `id`.
///
/// `id` is the backing file's absolute path, so the opener's temporary
/// directory and working directory do not participate.
///
/// # Errors
///
/// Returns an error if the shared memory is unavailable, which is the common
/// case once its keeper has been dropped.
pub fn open(id: &OsStr) -> io::Result<Mapping> {
    // Rust's default share mode on Windows permits the owner's concurrent
    // read/write/delete access, and Rust opens are `O_CLOEXEC` / non-inheritable
    // on every platform, so a traced process never leaks this descriptor.
    let file = OpenOptions::new().read(true).write(true).open(id)?;
    // If another process shrinks the file before `map_raw`, mapping fails. If it
    // resizes afterwards, `open` does not touch the mapped pages. A concurrent
    // resize cannot make `open` access invalid memory.
    let size = usize::try_from(file.metadata()?.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid shared-memory size"))?;
    if size == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "shared-memory size is zero"));
    }
    // The mapping keeps the data alive; the handle is not needed any more.
    map(&file, size)
}

/// Open options for the creating process: exclusive, same-user, and on Windows
/// self-deleting once the owner's handle closes.
fn create_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        // Only the creating user may open the mapping.
        options.mode(0o600);
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;

        // The per-user `%TEMP%` ACL provides the same-user gating that `0o600`
        // provides on Unix.
        options
            .share_mode(sys::SHARE_ALL)
            .attributes(sys::TEMPORARY)
            .custom_flags(sys::DELETE_ON_CLOSE);
    }

    options
}

/// Gives the freshly created backing file its logical size. Every byte reads
/// as zero because the file is all holes.
fn size_backing_file(file: &File, size_u64: u64) -> io::Result<()> {
    // NTFS allocates clusters for the whole logical size unless the file is
    // marked sparse first, which would turn the capacity into real disk usage.
    // Volumes without sparse-file support fail here.
    #[cfg(windows)]
    sys::set_sparse(file)?;

    file.set_len(size_u64)
}

/// Maps the first `size` bytes of an already sized backing file.
fn map(file: &File, size: usize) -> io::Result<Mapping> {
    #[cfg(unix)]
    let raw = MmapOptions::new().len(size).map_raw(file)?;
    #[cfg(windows)]
    let raw = sys::MappedView::new(file, size)?;

    Ok(Mapping { raw })
}

/// Removes a backing file whose owner never came into existence.
#[cfg(unix)]
fn discard_backing_file(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Windows deletes the file as soon as the creating handle closes
/// (`FILE_FLAG_DELETE_ON_CLOSE`), so there is nothing to do here.
#[cfg(windows)]
const fn discard_backing_file(_path: &Path) {}

/// Keeper-side cleanup. Dropping it makes the backing file's name disappear
/// while existing mappings keep the bytes alive.
struct OwnerCleanup {
    /// Unlinked on drop.
    #[cfg(unix)]
    path: PathBuf,
    /// The creating handle, opened with `FILE_FLAG_DELETE_ON_CLOSE`. Closing it
    /// applies the delete disposition even while other processes hold views,
    /// because those are section references rather than handles. A mapped file
    /// cannot be unlinked by path on Windows, so this is how the name goes away.
    #[cfg(windows)]
    _file: File,
}

#[cfg(unix)]
impl Drop for OwnerCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[expect(clippy::len_without_is_empty, reason = "shared-memory mappings are always non-empty")]
impl Mapping {
    /// Returns the mapped length in bytes.
    #[must_use]
    #[cfg_attr(
        windows,
        expect(
            clippy::missing_const_for_fn,
            reason = "the Unix mapping's accessors are not const, so this cannot be either"
        )
    )]
    pub fn len(&self) -> usize {
        self.raw.len()
    }

    /// Returns a raw pointer to the first mapped byte.
    #[must_use]
    #[cfg_attr(
        windows,
        expect(
            clippy::missing_const_for_fn,
            reason = "the Unix mapping's accessors are not const, so this cannot be either"
        )
    )]
    pub fn as_ptr(&self) -> *mut u8 {
        #[cfg(unix)]
        {
            self.raw.as_mut_ptr()
        }
        #[cfg(windows)]
        {
            self.raw.as_ptr()
        }
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

impl ShmKeeper {
    /// Returns the shared memory's opaque identifier, which any process —
    /// including the creating one — passes to [`open`].
    #[must_use]
    pub fn id(&self) -> &OsStr {
        &self.id
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, fs, process::Command};

    use subprocess_test::command_for_fn;

    use super::*;

    const SIZE: usize = 64 * 1024;

    /// Throwaway probe, never to merge: can Windows remove a mapped file's
    /// name manually when no handle is open and only a written view remains?
    #[cfg(windows)]
    #[test]
    fn probe_manual_delete_of_mapped_file() {
        let path = std::path::absolute(temp_dir())
            .unwrap()
            .join(format!("{BACKING_PREFIX}probe-{}.shm", Uuid::new_v4().simple()));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            options.share_mode(sys::SHARE_ALL).attributes(sys::TEMPORARY);
        }
        let file = options.open(&path).unwrap();
        sys::set_sparse(&file).unwrap();
        file.set_len(65536).unwrap();
        let mapping = map(&file, 65536).unwrap();
        drop(file);
        // SAFETY: in bounds, no concurrent access.
        unsafe { mapping.as_ptr().write(17) };

        let removed = fs::remove_file(&path);
        let exists_after = path.exists();
        let reopen_fails = OpenOptions::new().read(true).open(&path).is_err();
        // SAFETY: in bounds, no concurrent access.
        let view_alive = unsafe { mapping.as_ptr().read() } == 17;
        let rename_target = path.with_extension("renamed");
        let renamed = fs::rename(&path, &rename_target);
        panic!(
            "PROBE remove_file={removed:?} exists_after={exists_after} \
             reopen_fails={reopen_fails} view_alive={view_alive} rename_after={renamed:?}"
        );
    }

    #[test]
    fn subprocess_open_ignores_changed_temp_and_working_directory() {
        let keeper = create(SIZE).unwrap();
        let mapping = open(keeper.id()).unwrap();
        let changed_cwd =
            temp_dir().join(format!("{BACKING_PREFIX}changed-cwd-{}", Uuid::new_v4()));
        fs::create_dir(&changed_cwd).unwrap();
        // SAFETY: The child does not access the mapping until this write completes.
        unsafe { mapping.as_ptr().write(17) };

        let id = keeper.id().to_str().expect("test temp dir is UTF-8").to_owned();
        let mut command = command_for_fn!(id, |id: String| {
            let opened = open(OsStr::new(&id)).unwrap();
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

    #[test]
    fn keeper_drop_removes_backing_file_and_preserves_existing_mappings() {
        let keeper = create(SIZE).unwrap();
        let id = keeper.id().to_owned();
        let path = Path::new(&id).to_owned();
        let opened = open(&id).unwrap();
        assert!(path.exists());

        drop(keeper);

        assert!(!path.exists());
        assert!(open(&id).is_err());
        // SAFETY: The mapping remains live and no other access is concurrent.
        unsafe { opened.as_ptr().write(17) };
        // SAFETY: The preceding write is complete and the mapping remains live.
        assert_eq!(unsafe { opened.as_ptr().read() }, 17);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn four_gib_mapping_is_sparse_and_supports_endpoint_access() {
        const PRODUCTION_SIZE: usize = 4 * 1024 * 1024 * 1024;
        #[cfg(windows)]
        const MAX_ENDPOINT_ALLOCATION: u64 = 16 * 1024 * 1024;

        let keeper = create(PRODUCTION_SIZE).unwrap();
        #[cfg(windows)]
        {
            let (logical_size, initial_allocation) = backing_file_sizes(keeper.id());
            assert_eq!(logical_size, PRODUCTION_SIZE as u64);
            assert!(initial_allocation < MAX_ENDPOINT_ALLOCATION);
        }

        let first = open(keeper.id()).unwrap();
        let opened = open(keeper.id()).unwrap();
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
