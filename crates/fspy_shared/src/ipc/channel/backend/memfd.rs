use std::{fs::File, io, os::unix::io::AsRawFd, sync::Arc};

use memfd::MemfdOptions;
use memmap2::{MmapMut, MmapOptions};

pub struct Shm {
    pub mmap: MmapMut,
    pub fd_path: Arc<str>,
}

pub fn create(size: usize) -> io::Result<Shm> {
    let memfd = MemfdOptions::default().create("ipc").map_err(io::Error::other)?;
    memfd.as_file().set_len(size as u64)?;

    let fd = memfd.as_file().as_raw_fd();
    let fd_path = format!("/proc/self/fd/{fd}");

    let mmap = unsafe { MmapOptions::new().len(size).map_mut(memfd.as_file())? };

    Ok(Shm { mmap, fd_path: fd_path.into() })
}

pub fn open(shm_id: &str, size: usize) -> io::Result<memmap2::MmapMut> {
    let file = File::open(shm_id)?;

    let mmap = unsafe { memmap2::MmapOptions::new().len(size).map_mut(&file)? };

    Ok(mmap)
}
