mod sys;

use std::{
    env::temp_dir,
    fs::{self, File, OpenOptions},
    io,
    os::windows::fs::OpenOptionsExt,
    path::PathBuf,
    slice,
};

use sys::MappedView;
use uuid::Uuid;

const MAPPING_NAME_PREFIX: &str = r"Local\vite-task-fspy-";
const BACKING_DIR: &str = "vite-task-fspy";

/// An owned Windows shared-memory mapping.
pub struct Shm {
    id: String,
    view: MappedView,
    _backing_file: Option<File>,
}

/// Creates a sparse, temporary file-backed named mapping of `size` bytes and
/// returns its owner.
///
/// # Errors
///
/// Returns an error if the backing file or mapping cannot be created.
pub fn create(size: usize) -> io::Result<Shm> {
    let size_u64 = valid_size(size)?;
    let id = Uuid::new_v4().simple().to_string();
    let backing_file = create_backing_file(&id)?;
    initialize_backing_file(&backing_file, size_u64)?;
    let mapping = sys::create_file_mapping(&backing_file, &mapping_name(&id, size))?;
    let view = MappedView::new(&mapping, size)?;

    Ok(Shm { id, view, _backing_file: Some(backing_file) })
}

/// Opens the named mapping identified by `id`.
///
/// # Errors
///
/// Returns an error if the identifier is invalid or the mapping is unavailable.
pub fn open(id: &str, size: usize) -> io::Result<Shm> {
    valid_size(size)?;
    validate_id(id)?;
    let mapping = sys::open_file_mapping(&mapping_name(id, size))?;
    let view = MappedView::new(&mapping, size)?;

    Ok(Shm { id: id.to_owned(), view, _backing_file: None })
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

fn validate_id(id: &str) -> io::Result<()> {
    let uuid = Uuid::parse_str(id).map_err(|_| invalid_id())?;
    if uuid.simple().to_string() != id {
        return Err(invalid_id());
    }
    Ok(())
}

fn invalid_id() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid Windows shared-memory identifier")
}

fn mapping_name(id: &str, size: usize) -> String {
    format!("{MAPPING_NAME_PREFIX}{id}-{size}")
}

fn backing_path(id: &str) -> PathBuf {
    temp_dir().join(BACKING_DIR).join(format!("{id}.shm"))
}

fn create_backing_file(id: &str) -> io::Result<File> {
    let path = backing_path(id);
    fs::create_dir_all(path.parent().unwrap())?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(sys::SHARE_ALL)
        .attributes(sys::TEMPORARY)
        .custom_flags(sys::DELETE_ON_CLOSE)
        .open(path)
}

fn initialize_backing_file(file: &File, size: u64) -> io::Result<()> {
    initialize_backing_file_with(file, size, sys::set_sparse)
}

fn initialize_backing_file_with(
    file: &File,
    size: u64,
    set_sparse: impl FnOnce(&File) -> io::Result<()>,
) -> io::Result<()> {
    if let Err(error) = set_sparse(file)
        && !sys::is_sparse_unsupported(&error)
    {
        return Err(error);
    }
    file.set_len(size)
}

#[expect(clippy::len_without_is_empty, reason = "shared-memory mappings are always non-empty")]
impl Shm {
    /// Returns this mapping's opaque Windows identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the mapped length in bytes.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.view.len()
    }

    /// Returns a raw pointer to the first mapped byte.
    #[must_use]
    pub const fn as_ptr(&self) -> *mut u8 {
        self.view.as_ptr()
    }

    /// Returns the mapped bytes as a shared slice.
    ///
    /// # Safety
    ///
    /// The caller must ensure that no process or thread mutates the mapping for
    /// the lifetime of the returned slice.
    #[must_use]
    pub const unsafe fn as_slice(&self) -> &[u8] {
        // SAFETY: The view is valid for its exact length, and the caller
        // guarantees that it is not mutated while the slice is borrowed.
        unsafe { slice::from_raw_parts(self.view.as_ptr(), self.view.len()) }
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, fs, process::Command};

    use subprocess_test::command_for_fn;
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED,
    };

    use super::*;

    const SIZE: usize = 64 * 1024;

    #[test]
    fn identifiers_are_canonical_simple_uuids() {
        let owner = create(SIZE).unwrap();
        let uuid = Uuid::parse_str(owner.id()).unwrap();

        assert_eq!(uuid.simple().to_string(), owner.id());
    }

    #[test]
    fn malformed_ids_and_size_mismatches_are_rejected() {
        let owner = create(SIZE).unwrap();
        let uuid = Uuid::parse_str(owner.id()).unwrap();
        for id in [String::new(), uuid.hyphenated().to_string(), format!("{}0", owner.id())] {
            assert_eq!(open(&id, SIZE).err().unwrap().kind(), io::ErrorKind::InvalidInput);
        }

        assert_eq!(open(owner.id(), 0).err().unwrap().kind(), io::ErrorKind::InvalidInput);
        assert!(open(owner.id(), SIZE / 2).is_err());
        assert!(open(owner.id(), SIZE + 1).is_err());
    }

    #[test]
    fn subprocess_open_ignores_changed_temp_and_working_directory() {
        let owner = create(SIZE).unwrap();
        let changed_cwd =
            temp_dir().join(BACKING_DIR).join(format!("changed-cwd-{}", Uuid::new_v4()));
        fs::create_dir(&changed_cwd).unwrap();
        // SAFETY: The child does not access the mapping until this write completes.
        unsafe { owner.as_ptr().write(17) };

        let mut command = command_for_fn!(owner.id().to_owned(), |id: String| {
            let opened = open(&id, SIZE).unwrap();
            // SAFETY: The parent waits for this child and does not access the
            // mapping concurrently.
            unsafe {
                assert_eq!(opened.as_ptr().read(), 17);
                opened.as_ptr().add(SIZE - 1).write(29);
            }
        });
        command.cwd = changed_cwd.clone();
        command.envs.insert(OsString::from("TMP"), OsString::from("changed-relative-tmp"));
        command.envs.insert(OsString::from("TEMP"), OsString::from("changed-relative-temp"));
        let succeeded = Command::from(command).status().unwrap().success();
        fs::remove_dir(changed_cwd).unwrap();

        assert!(succeeded);
        // SAFETY: The child exited before this read.
        assert_eq!(unsafe { owner.as_ptr().add(SIZE - 1).read() }, 29);
    }

    #[test]
    fn owner_cleanup_deletes_backing_file_and_preserves_existing_views() {
        let owner = create(SIZE).unwrap();
        let id = owner.id().to_owned();
        let path = backing_path(&id);
        let opened = open(&id, SIZE).unwrap();
        assert!(path.exists());

        drop(owner);

        assert!(!path.exists());
        // SAFETY: The mapping remains live and no other test access is concurrent.
        unsafe { opened.as_ptr().write(17) };
        // SAFETY: The preceding write is complete and the mapping remains live.
        assert_eq!(unsafe { opened.as_ptr().read() }, 17);
        let reopened = open(&id, SIZE).unwrap();
        drop(opened);
        drop(reopened);
        assert!(open(&id, SIZE).is_err());
    }

    #[test]
    fn unsupported_sparse_errors_fall_back_to_extending_the_file() {
        for code in [ERROR_INVALID_FUNCTION, ERROR_NOT_SUPPORTED] {
            let file = test_backing_file();
            initialize_backing_file_with(&file, SIZE as u64, |_| {
                Err(io::Error::from_raw_os_error(code.cast_signed()))
            })
            .unwrap();

            assert_eq!(file.metadata().unwrap().len(), SIZE as u64);
        }
    }

    #[test]
    fn other_sparse_errors_do_not_extend_the_file() {
        for code in [ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER] {
            let file = test_backing_file();
            let error = initialize_backing_file_with(&file, SIZE as u64, |_| {
                Err(io::Error::from_raw_os_error(code.cast_signed()))
            })
            .unwrap_err();

            assert_eq!(error.raw_os_error(), Some(code.cast_signed()));
            assert_eq!(file.metadata().unwrap().len(), 0);
        }
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn four_gib_mapping_is_sparse_and_supports_endpoint_access() {
        const PRODUCTION_SIZE: usize = 4 * 1024 * 1024 * 1024;
        const MAX_ENDPOINT_ALLOCATION: u64 = 16 * 1024 * 1024;

        let owner = create(PRODUCTION_SIZE).unwrap();
        let backing_file = owner._backing_file.as_ref().unwrap();
        let (logical_size, initial_allocation) = sys::file_sizes(backing_file).unwrap();
        assert_eq!(logical_size, PRODUCTION_SIZE as u64);
        assert!(initial_allocation < MAX_ENDPOINT_ALLOCATION);

        let opened = open(owner.id(), PRODUCTION_SIZE).unwrap();
        // SAFETY: Both endpoint indexes are within the exact mapped length and
        // accesses are synchronized within this test.
        unsafe {
            owner.as_ptr().write(17);
            owner.as_ptr().add(PRODUCTION_SIZE - 1).write(29);
            assert_eq!(opened.as_ptr().read(), 17);
            assert_eq!(opened.as_ptr().add(PRODUCTION_SIZE - 1).read(), 29);
        }

        let (logical_size, endpoint_allocation) = sys::file_sizes(backing_file).unwrap();
        assert_eq!(logical_size, PRODUCTION_SIZE as u64);
        assert!(endpoint_allocation < MAX_ENDPOINT_ALLOCATION);
    }

    fn test_backing_file() -> File {
        create_backing_file(&Uuid::new_v4().simple().to_string()).unwrap()
    }
}
