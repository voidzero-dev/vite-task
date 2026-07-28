#![doc = include_str!("../README.md")]

mod file_backed;

pub use file_backed::{Mapping, ShmHandle, ShmKeeper, create, open};

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, mem::align_of, process::Command};

    use subprocess_test::command_for_fn;

    use super::{Mapping, create, open};

    // Page-aligned on all supported targets.
    const SIZE: usize = 64 * 1024;
    // Use one byte more than 64 KiB to test multiple pages and a partial last page.
    const ZERO_INITIALIZED_SIZE: usize = SIZE + 1;

    #[test]
    fn new_mapping_is_zero_initialized_in_all_views() {
        let (keeper, handle) = create(ZERO_INITIALIZED_SIZE).unwrap();
        let first = handle.map().unwrap();
        let second = open(keeper.id()).unwrap().map().unwrap();

        assert_zero_initialized(&first);
        assert_zero_initialized(&second);
    }

    #[test]
    fn mappings_of_one_keeper_are_shared() {
        let (keeper, handle) = create(SIZE).unwrap();
        let first = handle.map().unwrap();
        assert_eq!(first.len(), SIZE);
        assert_eq!(first.as_ptr() as usize % align_of::<usize>(), 0);

        let second = open(keeper.id()).unwrap().map().unwrap();
        assert_eq!(second.len(), SIZE);

        write_byte(&first, 0, 17);
        assert_eq!(read_byte(&second, 0), 17);
        write_byte(&second, SIZE - 1, 29);
        assert_eq!(read_byte(&first, SIZE - 1), 29);
    }

    #[test]
    fn mapping_is_visible_across_processes() {
        let (keeper, handle) = create(SIZE).unwrap();
        let mapping = handle.map().unwrap();
        write_byte(&mapping, 0, 17);

        let id = keeper.id().to_str().expect("test temp dir is UTF-8").to_owned();
        let command = command_for_fn!(id, |id: String| {
            let opened = open(OsStr::new(&id)).unwrap().map().unwrap();
            assert_eq!(read_byte(&opened, 0), 17);
            write_byte(&opened, SIZE - 1, 29);
        });
        assert!(Command::from(command).status().unwrap().success());
        assert_eq!(read_byte(&mapping, SIZE - 1), 29);
    }

    #[test]
    fn keeper_drop_prevents_new_opens() {
        let (keeper, handle) = create(SIZE).unwrap();
        let id = keeper.id().to_owned();
        drop(handle);
        drop(keeper);

        assert!(open(&id).is_err());
    }

    #[test]
    fn opened_mapping_survives_keeper_drop() {
        let (keeper, handle) = create(SIZE).unwrap();
        let id = keeper.id().to_owned();
        let opened = open(&id).unwrap().map().unwrap();
        write_byte(&opened, 0, 17);
        drop(handle);
        drop(keeper);

        assert!(open(&id).is_err());
        assert_eq!(read_byte(&opened, 0), 17);
        write_byte(&opened, SIZE - 1, 29);
        assert_eq!(read_byte(&opened, SIZE - 1), 29);
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
