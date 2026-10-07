#![doc = include_str!("../README.md")]
#![cfg_attr(not(test), no_std)]

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

pub use platform::{Mapping, ShmHandle, create, open, remove};
#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
use windows as platform;

// Sizes travel as `u64` through the file APIs and as `usize` through the
// mapping APIs. Supported targets give both the same width, so conversions
// between them are lossless.
const _: () = assert!(usize::BITS == u64::BITS);

/// Converts a backing file's size to a mapping length.
#[expect(clippy::cast_possible_truncation, reason = "lossless; see the width assert above")]
const fn file_size_to_len(size: u64) -> usize {
    size as usize
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use std::fs::File;
    use std::{ffi::OsStr, mem::align_of, path::PathBuf, process::Command};

    use fspy_nostd::{OsCStr, Thin};
    use subprocess_test::command_for_fn;

    use super::{Mapping, create, open, remove};

    // Page-aligned on all supported targets.
    const SIZE: usize = 64 * 1024;
    // Use one byte more than 64 KiB to test multiple pages and a partial last page.
    const ZERO_INITIALIZED_SIZE: usize = SIZE + 1;

    #[cfg(unix)]
    fn encode(path: &OsStr) -> Vec<u8> {
        use std::os::unix::ffi::OsStrExt as _;

        let mut units = path.as_bytes().to_vec();
        units.push(0);
        units
    }

    #[cfg(windows)]
    fn encode(path: &OsStr) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt as _;

        let mut units: Vec<u16> = path.encode_wide().collect();
        units.push(0);
        units
    }

    /// A fresh backing path whose directory removes the file when the test
    /// ends, even on panic — the job the fspy channel's keeper does in
    /// production.
    struct BackingPath {
        _dir: tempfile::TempDir,
        path: PathBuf,
        #[cfg(unix)]
        units: Vec<u8>,
        #[cfg(windows)]
        units: Vec<u16>,
    }

    impl BackingPath {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("backing.shm");
            let units = encode(path.as_os_str());
            Self { _dir: dir, path, units }
        }

        fn as_c_str(&self) -> OsCStr<'_, Thin> {
            fspy_nostd::OsCStr::from_units_with_nul(&self.units).unwrap().as_thin()
        }

        fn exists(&self) -> bool {
            self.path.exists()
        }

        fn to_str(&self) -> String {
            self.path.to_str().expect("test temp dir is UTF-8").to_owned()
        }
    }

    #[test]
    fn new_mapping_is_zero_initialized_in_all_views() {
        let path = BackingPath::new();
        let handle = create(path.as_c_str(), ZERO_INITIALIZED_SIZE).unwrap();
        let first = handle.map().unwrap();
        let second = open(path.as_c_str()).unwrap().map().unwrap();

        assert_zero_initialized(&first);
        assert_zero_initialized(&second);
    }

    #[test]
    fn mappings_of_one_backing_file_are_shared() {
        let path = BackingPath::new();
        let handle = create(path.as_c_str(), SIZE).unwrap();
        let first = handle.map().unwrap();
        assert_eq!(first.len(), SIZE);
        assert_eq!(first.as_ptr() as usize % align_of::<usize>(), 0);

        let second = open(path.as_c_str()).unwrap().map().unwrap();
        assert_eq!(second.len(), SIZE);

        write_byte(&first, 0, 17);
        assert_eq!(read_byte(&second, 0), 17);
        write_byte(&second, SIZE - 1, 29);
        assert_eq!(read_byte(&first, SIZE - 1), 29);
    }

    #[test]
    fn one_handle_maps_repeatedly() {
        let path = BackingPath::new();
        let handle = create(path.as_c_str(), SIZE).unwrap();
        let first = handle.map().unwrap();
        let second = handle.map().unwrap();

        write_byte(&first, 0, 17);
        assert_eq!(read_byte(&second, 0), 17);
    }

    #[test]
    fn create_rejects_an_existing_path() {
        let path = BackingPath::new();
        let _handle = create(path.as_c_str(), SIZE).unwrap();

        assert!(create(path.as_c_str(), SIZE).is_err());
    }

    #[test]
    fn mapping_is_visible_across_processes() {
        let path = BackingPath::new();
        let handle = create(path.as_c_str(), SIZE).unwrap();
        let mapping = handle.map().unwrap();
        write_byte(&mapping, 0, 17);

        let command = command_for_fn!(path.to_str(), |path: String| {
            let units = encode(OsStr::new(&path));
            let path = fspy_nostd::OsCStr::from_units_with_nul(&units).unwrap().as_thin();
            let opened = open(path).unwrap().map().unwrap();
            assert_eq!(read_byte(&opened, 0), 17);
            write_byte(&opened, SIZE - 1, 29);
        });
        assert!(Command::from(command).status().unwrap().success());
        assert_eq!(read_byte(&mapping, SIZE - 1), 29);
    }

    #[test]
    fn remove_prevents_new_opens() {
        let path = BackingPath::new();
        let handle = create(path.as_c_str(), SIZE).unwrap();
        drop(handle);

        remove(path.as_c_str()).unwrap();

        assert!(open(path.as_c_str()).is_err());
    }

    #[test]
    fn opened_mapping_survives_remove() {
        let path = BackingPath::new();
        let handle = create(path.as_c_str(), SIZE).unwrap();
        let opened = open(path.as_c_str()).unwrap().map().unwrap();
        write_byte(&opened, 0, 17);
        drop(handle);

        remove(path.as_c_str()).unwrap();

        assert!(open(path.as_c_str()).is_err());
        assert_eq!(read_byte(&opened, 0), 17);
        write_byte(&opened, SIZE - 1, 29);
        assert_eq!(read_byte(&opened, SIZE - 1), 29);
    }

    /// Removal semantics, part one: a mapping alone (no handle) keeps the
    /// bytes alive across the removal of the name.
    #[test]
    fn remove_deletes_backing_file_and_preserves_existing_mappings() {
        let path = BackingPath::new();
        let handle = create(path.as_c_str(), SIZE).unwrap();
        let opened = open(path.as_c_str()).unwrap().map().unwrap();
        drop(handle);
        assert!(path.exists());

        remove(path.as_c_str()).unwrap();

        assert!(!path.exists());
        assert!(open(path.as_c_str()).is_err());
        write_byte(&opened, 0, 17);
        assert_eq!(read_byte(&opened, 0), 17);
    }

    /// Removal semantics, part two: the name goes away even while a handle is
    /// still open, and that handle keeps mapping the same bytes afterwards.
    #[test]
    fn remove_with_open_handle_removes_name_and_handle_still_maps() {
        let path = BackingPath::new();
        let handle = create(path.as_c_str(), SIZE).unwrap();
        let before = handle.map().unwrap();
        write_byte(&before, 0, 17);

        remove(path.as_c_str()).unwrap();

        assert!(!path.exists());
        assert!(open(path.as_c_str()).is_err());

        let after = handle.map().unwrap();
        assert_eq!(read_byte(&after, 0), 17);
        write_byte(&after, SIZE - 1, 29);
        assert_eq!(read_byte(&before, SIZE - 1), 29);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn four_gib_mapping_is_sparse_and_supports_endpoint_access() {
        const PRODUCTION_SIZE: usize = 4 * 1024 * 1024 * 1024;
        #[cfg(windows)]
        const MAX_ENDPOINT_ALLOCATION: u64 = 16 * 1024 * 1024;

        let path = BackingPath::new();
        let handle = create(path.as_c_str(), PRODUCTION_SIZE).unwrap();
        #[cfg(windows)]
        {
            let (logical_size, initial_allocation) = backing_file_sizes(&path);
            assert_eq!(logical_size, PRODUCTION_SIZE as u64);
            assert!(initial_allocation < MAX_ENDPOINT_ALLOCATION);
        }

        let first = handle.map().unwrap();
        let opened = open(path.as_c_str()).unwrap().map().unwrap();
        write_byte(&first, 0, 17);
        write_byte(&first, PRODUCTION_SIZE - 1, 29);
        assert_eq!(read_byte(&opened, 0), 17);
        assert_eq!(read_byte(&opened, PRODUCTION_SIZE - 1), 29);

        // Touching both endpoints must not have allocated the range between them.
        #[cfg(windows)]
        {
            let (logical_size, endpoint_allocation) = backing_file_sizes(&path);
            assert_eq!(logical_size, PRODUCTION_SIZE as u64);
            assert!(endpoint_allocation < MAX_ENDPOINT_ALLOCATION);
        }
    }

    #[cfg(windows)]
    fn backing_file_sizes(path: &BackingPath) -> (u64, u64) {
        let file = File::open(&path.path).unwrap();
        super::windows::file_sizes(&file).unwrap()
    }

    fn read_byte(mapping: &Mapping, index: usize) -> u8 {
        assert!(index < mapping.len());
        // SAFETY: The index is in bounds and tests synchronize all accesses.
        unsafe { mapping.as_ptr().add(index).read() }
    }

    fn assert_zero_initialized(mapping: &Mapping) {
        assert!(mapping.len() >= ZERO_INITIALIZED_SIZE);
        assert!((0..mapping.len()).all(|index| read_byte(mapping, index) == 0));
    }

    fn write_byte(mapping: &Mapping, index: usize, value: u8) {
        assert!(index < mapping.len());
        // SAFETY: The index is in bounds and tests synchronize all accesses.
        unsafe { mapping.as_ptr().add(index).write(value) };
    }
}
