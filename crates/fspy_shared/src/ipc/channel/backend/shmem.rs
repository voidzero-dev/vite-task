use std::io;

use shared_memory::{Shmem, ShmemConf};

pub struct Shm {
    pub shm: Shmem,
    pub os_id: String,
}

pub fn create(size: usize) -> io::Result<Shm> {
    let shm = ShmemConf::new().size(size).create().map_err(io::Error::other)?;

    let os_id = shm.get_os_id().to_string();

    Ok(Shm { shm, os_id })
}

pub fn open(os_id: &str, size: usize) -> io::Result<Shmem> {
    ShmemConf::new().size(size).os_id(os_id).open().map_err(io::Error::other)
}
