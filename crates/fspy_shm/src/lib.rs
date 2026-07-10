#![doc = include_str!("../README.md")]

#[cfg(target_os = "linux")]
#[path = "linux/mod.rs"]
mod os_impl;
#[cfg(target_os = "macos")]
#[path = "macos/mod.rs"]
mod os_impl;
#[cfg(target_os = "windows")]
#[path = "windows/mod.rs"]
mod os_impl;

pub use os_impl::{Shm, create, open};

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
