mod sys;

use std::{
    env::temp_dir,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::OpenOptionsExt,
        io::OwnedHandle,
    },
    path::{Path, PathBuf},
    slice,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sys::{CreatedMapping, MappedView};
use uuid::Uuid;

const ID_PREFIX: &str = "fspy-win32-v1";
const MAPPING_NAME_PREFIX: &str = r"Local\vite-task-fspy-";
const BACKING_DIR: &str = "vite-task-fspy";

/// An owned Windows shared-memory mapping.
pub struct Shm {
    id: String,
    view: MappedView,
    _mapping: OwnedHandle,
    backing_file: BackingFile,
}

/// Creates a file-backed named mapping of `size` bytes and returns its owner.
///
/// # Errors
///
/// Returns an error if the backing file or mapping cannot be created.
pub fn create(size: usize) -> io::Result<Shm> {
    create_with(size, || format!("{MAPPING_NAME_PREFIX}{}", Uuid::new_v4()))
}

fn create_with(size: usize, mut next_name: impl FnMut() -> String) -> io::Result<Shm> {
    let size = valid_size(size)?;
    let backing_dir = create_backing_dir()?;

    loop {
        let mapping_name = next_name();
        if let Some(shm) = create_named(&backing_dir, &mapping_name, size)? {
            return Ok(shm);
        }
    }
}

fn create_named(backing_dir: &Path, mapping_name: &str, size: u64) -> io::Result<Option<Shm>> {
    let path = backing_path(backing_dir, mapping_name)?;
    let id = encode_id(&path, mapping_name)?;
    let backing_file = match BackingFile::create(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(None),
        Err(error) => return Err(error),
    };
    backing_file.file.set_len(size)?;

    let mapping = match sys::create_file_mapping(&backing_file.file, mapping_name)? {
        CreatedMapping::Created(mapping) => mapping,
        CreatedMapping::AlreadyExists => return Ok(None),
    };
    let len = usize::try_from(size).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "shared-memory size exceeds usize")
    })?;
    let view = MappedView::new(&mapping, len)?;

    Ok(Some(Shm { id, view, _mapping: mapping, backing_file }))
}

/// Opens the named mapping identified by `id`.
///
/// # Errors
///
/// Returns an error if the identifier is invalid, the owner has torn down the
/// mapping, or its backing file has a different size.
pub fn open(id: &str, size: usize) -> io::Result<Shm> {
    let expected_size = valid_size(size)?;
    let DecodedId { backing_path, mapping_name } = decode_id(id)?;
    let backing_file = BackingFile::open(backing_path)?;
    if backing_file.file.metadata()?.len() != expected_size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "shared-memory backing file has an unexpected size",
        ));
    }

    let mapping = sys::open_file_mapping(&mapping_name)?;
    let view = MappedView::new(&mapping, size)?;
    Ok(Shm { id: id.to_owned(), view, _mapping: mapping, backing_file })
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

fn create_backing_dir() -> io::Result<PathBuf> {
    let backing_dir = temp_dir().join(BACKING_DIR);
    fs::create_dir_all(&backing_dir)?;
    backing_dir.canonicalize()
}

fn backing_path(backing_dir: &Path, mapping_name: &str) -> io::Result<PathBuf> {
    let suffix = mapping_name_suffix(mapping_name)?;
    Ok(backing_dir.join(format!("{suffix}.shm")))
}

fn mapping_name_suffix(mapping_name: &str) -> io::Result<&str> {
    let suffix = mapping_name.strip_prefix(MAPPING_NAME_PREFIX).ok_or_else(invalid_id)?;
    let uuid = Uuid::parse_str(suffix).map_err(|_| invalid_id())?;
    if uuid.hyphenated().to_string() != suffix {
        return Err(invalid_id());
    }
    Ok(suffix)
}

fn encode_id(backing_path: &Path, mapping_name: &str) -> io::Result<String> {
    validate_id_parts(backing_path, mapping_name)?;
    let path_bytes: Vec<u8> =
        backing_path.as_os_str().encode_wide().flat_map(u16::to_le_bytes).collect();
    Ok(format!(
        "{ID_PREFIX}.{}.{}",
        URL_SAFE_NO_PAD.encode(path_bytes),
        URL_SAFE_NO_PAD.encode(mapping_name)
    ))
}

struct DecodedId {
    backing_path: PathBuf,
    mapping_name: String,
}

fn decode_id(id: &str) -> io::Result<DecodedId> {
    let mut parts = id.split('.');
    if parts.next() != Some(ID_PREFIX) {
        return Err(invalid_id());
    }
    let encoded_path = parts.next().ok_or_else(invalid_id)?;
    let encoded_mapping_name = parts.next().ok_or_else(invalid_id)?;
    if parts.next().is_some() {
        return Err(invalid_id());
    }

    let path_bytes = decode_id_part(encoded_path)?;
    if path_bytes.is_empty() || path_bytes.len() % 2 != 0 {
        return Err(invalid_id());
    }
    let path_units = path_bytes
        .chunks_exact(2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        .collect::<Vec<_>>();
    let backing_path = PathBuf::from(OsString::from_wide(&path_units));

    let mapping_name =
        String::from_utf8(decode_id_part(encoded_mapping_name)?).map_err(|_| invalid_id())?;
    validate_id_parts(&backing_path, &mapping_name)?;
    Ok(DecodedId { backing_path, mapping_name })
}

fn decode_id_part(encoded: &str) -> io::Result<Vec<u8>> {
    let decoded = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid_id())?;
    if URL_SAFE_NO_PAD.encode(&decoded) != encoded {
        return Err(invalid_id());
    }
    Ok(decoded)
}

fn validate_id_parts(backing_path: &Path, mapping_name: &str) -> io::Result<()> {
    let suffix = mapping_name_suffix(mapping_name)?;
    if !backing_path.is_absolute()
        || backing_path.file_name() != Some(OsStr::new(&format!("{suffix}.shm")))
    {
        return Err(invalid_id());
    }
    Ok(())
}

fn invalid_id() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid Windows shared-memory identifier")
}

struct BackingFile {
    file: File,
    owner_path: Option<PathBuf>,
}

impl BackingFile {
    fn create(path: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .share_mode(sys::SHARE_ALL)
            .attributes(sys::TEMPORARY)
            .open(&path)?;
        Ok(Self { file, owner_path: Some(path) })
    }

    fn open(path: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(sys::SHARE_ALL)
            .attributes(sys::TEMPORARY)
            .open(path)?;
        Ok(Self { file, owner_path: None })
    }

    fn unlink(&mut self) {
        let Some(path) = self.owner_path.take() else {
            return;
        };

        let deletion_file = OpenOptions::new()
            .access_mode(sys::DELETE_ACCESS)
            .share_mode(sys::SHARE_ALL)
            .attributes(sys::DELETE_ON_CLOSE)
            .open(&path);
        if let Ok(deletion_file) = deletion_file {
            let deleted_path = deleted_path(&path);
            if fs::rename(&path, deleted_path).is_err() {
                let _ = fs::remove_file(&path);
            }
            drop(deletion_file);
        } else {
            let _ = fs::remove_file(path);
        }
    }
}

impl Drop for BackingFile {
    fn drop(&mut self) {
        self.unlink();
    }
}

fn deleted_path(path: &Path) -> PathBuf {
    path.with_extension(format!("deleted-{}", Uuid::new_v4()))
}

impl Drop for Shm {
    fn drop(&mut self) {
        // Remove the public backing-file name before the mapping handle drops,
        // preventing opens that begin after owner teardown.
        self.backing_file.unlink();
    }
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

    use super::*;

    const SIZE: usize = 64 * 1024;

    #[test]
    fn identifiers_encode_absolute_paths_and_local_canonical_uuids() {
        let owner = create(SIZE).unwrap();
        let decoded = decode_id(owner.id()).unwrap();
        let suffix = mapping_name_suffix(&decoded.mapping_name).unwrap();
        let uuid = Uuid::parse_str(suffix).unwrap();

        assert!(decoded.backing_path.is_absolute());
        assert!(decoded.backing_path.exists());
        assert_eq!(uuid.hyphenated().to_string(), suffix);
        assert_eq!(decoded.backing_path.file_name(), Some(OsStr::new(&format!("{suffix}.shm"))));
    }

    #[test]
    fn identifiers_round_trip_nontrivial_utf16_paths() {
        let mapping_name = format!("{MAPPING_NAME_PREFIX}{}", Uuid::new_v4());
        let suffix = mapping_name_suffix(&mapping_name).unwrap();
        let mut path_units = r"C:\vite-task-fspy-".encode_utf16().collect::<Vec<_>>();
        path_units.extend([0x00e9, 0x6c34, 0xd83d, 0xde80, 0xd800, u16::from(b'\\')]);
        path_units.extend(format!("{suffix}.shm").encode_utf16());
        let path = PathBuf::from(OsString::from_wide(&path_units));

        let id = encode_id(&path, &mapping_name).unwrap();
        let decoded = decode_id(&id).unwrap();

        assert_eq!(decoded.mapping_name, mapping_name);
        assert_eq!(decoded.backing_path.as_os_str().encode_wide().collect::<Vec<_>>(), path_units);
    }

    #[test]
    fn malformed_ids_and_size_mismatches_are_rejected() {
        let owner = create(SIZE).unwrap();
        let decoded = decode_id(owner.id()).unwrap();
        let encoded_path = URL_SAFE_NO_PAD.encode(
            decoded
                .backing_path
                .as_os_str()
                .encode_wide()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        let encoded_mapping_name = URL_SAFE_NO_PAD.encode(&decoded.mapping_name);
        let relative_path = PathBuf::from(format!(
            r"relative\{}.shm",
            mapping_name_suffix(&decoded.mapping_name).unwrap()
        ));
        let encoded_relative_path = URL_SAFE_NO_PAD.encode(
            relative_path.as_os_str().encode_wide().flat_map(u16::to_le_bytes).collect::<Vec<_>>(),
        );
        let wrong_path = decoded.backing_path.with_file_name("wrong.shm");
        let encoded_wrong_path = URL_SAFE_NO_PAD.encode(
            wrong_path.as_os_str().encode_wide().flat_map(u16::to_le_bytes).collect::<Vec<_>>(),
        );

        let malformed = [
            String::new(),
            decoded.mapping_name,
            ID_PREFIX.to_owned(),
            format!("{ID_PREFIX}..{encoded_mapping_name}"),
            format!("{ID_PREFIX}.AA.{encoded_mapping_name}"),
            format!("{ID_PREFIX}.{encoded_relative_path}.{encoded_mapping_name}"),
            format!("{ID_PREFIX}.{encoded_wrong_path}.{encoded_mapping_name}"),
            format!("{ID_PREFIX}.{encoded_path}.%"),
            format!("{ID_PREFIX}.{encoded_path}.{}", URL_SAFE_NO_PAD.encode([0xff])),
            format!(
                "{ID_PREFIX}.{encoded_path}.{}",
                URL_SAFE_NO_PAD.encode(r"Local\vite-task-fspy-not-a-uuid")
            ),
            format!("{ID_PREFIX}.{encoded_path}=.{encoded_mapping_name}"),
            format!("{}.{encoded_mapping_name}.extra", owner.id()),
        ];
        for id in malformed {
            assert_eq!(open(&id, SIZE).err().unwrap().kind(), io::ErrorKind::InvalidInput);
        }

        assert_eq!(open(owner.id(), 0).err().unwrap().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(open(owner.id(), SIZE / 2).err().unwrap().kind(), io::ErrorKind::InvalidData);
        assert_eq!(open(owner.id(), SIZE + 1).err().unwrap().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn mapping_name_collisions_retry_and_clean_up() {
        let backing_dir = create_backing_dir().unwrap();
        let collision_name = format!("{MAPPING_NAME_PREFIX}{}", Uuid::new_v4());
        let raw_backing_name = format!("{MAPPING_NAME_PREFIX}{}", Uuid::new_v4());
        let raw_backing_path = backing_path(&backing_dir, &raw_backing_name).unwrap();
        let raw_backing = BackingFile::create(raw_backing_path).unwrap();
        raw_backing.file.set_len(SIZE as u64).unwrap();
        let collision_mapping =
            match sys::create_file_mapping(&raw_backing.file, &collision_name).unwrap() {
                CreatedMapping::Created(mapping) => mapping,
                CreatedMapping::AlreadyExists => panic!("random test mapping name collided"),
            };

        let fresh_name = format!("{MAPPING_NAME_PREFIX}{}", Uuid::new_v4());
        let mut names = [collision_name.clone(), fresh_name.clone()].into_iter();
        let created =
            create_with(SIZE, || names.next().expect("creation did not stop after retry")).unwrap();
        let decoded = decode_id(created.id()).unwrap();

        assert_eq!(decoded.mapping_name, fresh_name);
        assert_eq!(
            decoded.backing_path,
            backing_path(&backing_dir, &decoded.mapping_name).unwrap()
        );
        assert!(!backing_path(&backing_dir, &collision_name).unwrap().exists());
        drop(collision_mapping);
        drop(raw_backing);
    }

    #[test]
    fn subprocess_open_ignores_changed_temp_and_working_directory() {
        let owner = create(SIZE).unwrap();
        let decoded = decode_id(owner.id()).unwrap();
        let changed_cwd =
            decoded.backing_path.parent().unwrap().join(format!("changed-cwd-{}", Uuid::new_v4()));
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
    fn owner_cleanup_removes_backing_file_after_existing_views_close() {
        let owner = create(SIZE).unwrap();
        let id = owner.id().to_owned();
        let DecodedId { backing_path: path, mapping_name } = decode_id(&id).unwrap();
        let backing_dir = path.parent().unwrap().to_owned();
        let suffix = mapping_name_suffix(&mapping_name).unwrap().to_owned();
        let opened = open(&id, SIZE).unwrap();
        assert!(path.exists());

        drop(owner);

        assert!(!path.exists());
        assert!(open(&id, SIZE).is_err());
        // SAFETY: The mapping remains live and no other test access is concurrent.
        unsafe { opened.as_ptr().write(17) };
        // SAFETY: The preceding write is complete and the mapping remains live.
        assert_eq!(unsafe { opened.as_ptr().read() }, 17);
        drop(opened);

        assert!(
            fs::read_dir(backing_dir).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(&suffix))
        );
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
