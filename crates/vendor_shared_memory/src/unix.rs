use std::num::NonZeroUsize;
use std::os::unix::io::{BorrowedFd, IntoRawFd, RawFd};
use std::ptr::{null_mut, NonNull};

use crate::log::*;
use nix::fcntl::OFlag;
use nix::sys::mman::{mmap, munmap, shm_open, shm_unlink, MapFlags, ProtFlags};
use nix::sys::stat::{fstat, Mode};
use nix::unistd::{close, ftruncate};

use crate::ShmemError;

#[derive(Clone, Default)]
pub struct ShmemConfExt;

pub struct MapData {
    //On linux, you must shm_unlink() the object created for the mapping. It wont disappear automatically.
    owner: bool,

    //File descriptor to our open mapping
    map_fd: RawFd,

    //Shared mapping uid
    pub unique_id: String,
    //Total size of the mapping
    pub map_size: usize,
    //Pointer to the first address of our mapping
    pub map_ptr: *mut u8,
}

impl MapData {
    pub fn as_mut_ptr(&self) -> *mut u8 {
        self.map_ptr
    }
}

/// Shared memory teardown for linux
impl Drop for MapData {
    ///Takes care of properly closing the SharedMem (munmap(), shmem_unlink(), close())
    fn drop(&mut self) {
        //Unmap memory
        if !self.map_ptr.is_null() {
            trace!(
                "munmap(map_ptr:{:p},map_size:{})",
                self.map_ptr,
                self.map_size
            );
            if let Err(_e) = unsafe {
                munmap(NonNull::new_unchecked(self.map_ptr as *mut _), self.map_size)
            } {
                debug!("Failed to munmap() shared memory mapping : {}", _e);
            };
        }

        //Unlink shmem
        if self.map_fd != 0 {
            //unlink shmem if we created it
            if self.owner {
                debug!("Deleting persistent mapping");
                trace!("shm_unlink({})", self.unique_id.as_str());
                if let Err(_e) = shm_unlink(self.unique_id.as_str()) {
                    debug!("Failed to shm_unlink() shared memory : {}", _e);
                };
            }

            trace!("close({})", self.map_fd);
            if let Err(_e) = close(self.map_fd) {
                debug!(
                    "os_impl::Linux : Failed to close() shared memory file descriptor : {}",
                    _e
                );
            };
        }
    }
}

impl MapData {
    pub fn set_owner(&mut self, is_owner: bool) -> bool {
        let prev_val = self.owner;
        self.owner = is_owner;
        prev_val
    }
}

/// Creates a mapping specified by the uid and size
pub fn create_mapping(unique_id: &str, map_size: usize) -> Result<MapData, ShmemError> {
    //Create shared memory file descriptor
    debug!("Creating persistent mapping at {}", unique_id);
    let shmem_fd = match shm_open(
        unique_id, //Unique name that usualy pops up in /dev/shm/
        OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_RDWR, //create exclusively (error if collision) and read/write to allow resize
        Mode::S_IRUSR | Mode::S_IWUSR,                  //Permission allow user+rw
    ) {
        Ok(v) => {
            let v = v.into_raw_fd();
            trace!(
                "shm_open({}, {:X}, {:X}) == {}",
                unique_id,
                OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_RDWR,
                Mode::S_IRUSR | Mode::S_IWUSR,
                v
            );
            v
        }
        Err(nix::Error::EEXIST) => return Err(ShmemError::MappingIdExists),
        Err(e) => return Err(ShmemError::MapCreateFailed(e as u32)),
    };

    let mut new_map: MapData = MapData {
        owner: true,
        unique_id: String::from(unique_id),
        map_fd: shmem_fd,
        map_size,
        map_ptr: null_mut(),
    };

    //Enlarge the memory descriptor file size to the requested map size
    debug!("Creating memory mapping");
    trace!("ftruncate({}, {})", new_map.map_fd, new_map.map_size);
    match ftruncate(
        unsafe { BorrowedFd::borrow_raw(new_map.map_fd) },
        new_map.map_size as _,
    ) {
        Ok(_) => {}
        Err(e) => return Err(ShmemError::UnknownOsError(e as u32)),
    };

    //Put the mapping in our address space
    debug!("Loading mapping into address space");
    new_map.map_ptr = match unsafe {
        mmap(
            None,                                         //Desired addr
            NonZeroUsize::new(new_map.map_size).unwrap(), //size of mapping
            ProtFlags::PROT_READ | ProtFlags::PROT_WRITE, //Permissions on pages
            MapFlags::MAP_SHARED,                         //What kind of mapping
            BorrowedFd::borrow_raw(new_map.map_fd),       //fd
            0,                                            //Offset into fd
        )
    } {
        Ok(v) => {
            trace!(
                "mmap(NULL, {}, {:X}, {:X}, {}, 0) == {:p}",
                new_map.map_size,
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_SHARED,
                new_map.map_fd,
                v
            );
            v.as_ptr() as *mut _
        }
        Err(e) => return Err(ShmemError::MapCreateFailed(e as u32)),
    };

    Ok(new_map)
}

/// Opens an existing mapping specified by its uid
pub fn open_mapping(
    unique_id: &str,
    _map_size: usize,
    _ext: &ShmemConfExt,
) -> Result<MapData, ShmemError> {
    //Open shared memory
    debug!("Openning persistent mapping at {}", unique_id);
    let shmem_fd = match shm_open(
        unique_id,
        OFlag::O_RDWR, //Open read write
        Mode::S_IRUSR,
    ) {
        Ok(v) => {
            let v = v.into_raw_fd();
            trace!(
                "shm_open({}, {:X}, {:X}) == {}",
                unique_id,
                OFlag::O_RDWR,
                Mode::S_IRUSR,
                v
            );
            v
        }
        Err(e) => return Err(ShmemError::MapOpenFailed(e as u32)),
    };

    let mut new_map: MapData = MapData {
        owner: false,
        unique_id: String::from(unique_id),
        map_fd: shmem_fd,
        map_size: 0,
        map_ptr: null_mut(),
    };

    //Get mmap size
    new_map.map_size = match fstat(unsafe { BorrowedFd::borrow_raw(new_map.map_fd) }) {
        Ok(v) => v.st_size as usize,
        Err(e) => return Err(ShmemError::MapOpenFailed(e as u32)),
    };

    //Map memory into our address space
    debug!("Loading mapping into address space");
    new_map.map_ptr = match unsafe {
        mmap(
            None,                                         //Desired addr
            NonZeroUsize::new(new_map.map_size).unwrap(), //size of mapping
            ProtFlags::PROT_READ | ProtFlags::PROT_WRITE, //Permissions on pages
            MapFlags::MAP_SHARED,                         //What kind of mapping
            BorrowedFd::borrow_raw(new_map.map_fd),       //fd
            0,                                            //Offset into fd
        )
    } {
        Ok(v) => {
            trace!(
                "mmap(NULL, {}, {:X}, {:X}, {}, 0) == {:p}",
                new_map.map_size,
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_SHARED,
                new_map.map_fd,
                v
            );
            v.as_ptr() as *mut _
        }
        Err(e) => return Err(ShmemError::MapOpenFailed(e as u32)),
    };

    Ok(new_map)
}
