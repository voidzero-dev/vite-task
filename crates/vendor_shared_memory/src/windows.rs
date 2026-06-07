use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
use std::path::PathBuf;

use crate::{log::*, ShmemConf};
use win_sys::*;

use crate::ShmemError;

#[derive(Clone, Default)]
pub struct ShmemConfExt {
    allow_raw: bool,
}

impl ShmemConf {
    /// If set to true, enables openning raw shared memory that is not managed by this crate
    pub fn allow_raw(mut self, allow: bool) -> Self {
        self.ext.allow_raw = allow;
        self
    }
}

pub struct MapData {
    owner: bool,

    /// Pointer to the first byte of our mapping
    /// Keep this above `file_map` so it gets dropped first
    pub view: ViewOfFile,

    /// The handle to our open mapping
    #[allow(dead_code)]
    file_map: FileMapping,

    /// This file is used for shmem persistence. When an owner wants to drop the mapping,
    /// it opens the file with FILE_FLAG_DELETE_ON_CLOSE, renames the file and closes it.
    /// This makes it so future calls to open the old mapping will fail (as it was renamed) and
    /// deletes the renamed file once all handles have been closed.
    #[allow(dead_code)]
    persistent_file: Option<File>,

    //Shared mapping uid
    pub unique_id: String,
    //Total size of the mapping
    pub map_size: usize,
}
///Teardown UnmapViewOfFile and close CreateMapping handle
impl Drop for MapData {
    ///Takes care of properly closing the SharedMem
    fn drop(&mut self) {
        // Inspired by the boost implementation at
        // https://github.com/boostorg/interprocess/blob/140b50efb3281fa3898f3a4cf939cfbda174718f/include/boost/interprocess/detail/win32_api.hpp
        // Emulate POSIX behavior by
        // 1. Opening the mmapped file with `FILE_FLAG_DELETE_ON_CLOSE`, causing it to be
        // deleted when all its handles have been closed.
        // 2. Renaming the mmapped file to prevent future access/opening.
        // Once this has run, existing file/mapping handles remain usable but the file is
        // deleted once all handles have been closed and no new handles can be opened
        // because the file has been renamed. This matches the behavior of shm_unlink()
        // on unix.
        if self.owner {
            let mut base_path = get_tmp_dir().unwrap();

            // 1. Set file attributes so that it deletes itself once everyone has closed it
            let file_path = base_path.join(self.unique_id.trim_start_matches('/'));
            debug!("Setting mapping to delete after everyone has closed it");
            match OpenOptions::new()
                .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
                .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
                .create(false)
                .attributes((FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_DELETE_ON_CLOSE).0)
                .open(&file_path)
            {
                Ok(_) => {
                    // 2. Rename file to prevent further use
                    base_path.push(&format!(
                        "{}_deleted",
                        self.unique_id.trim_start_matches('/')
                    ));
                    debug!(
                        "Renaming {} to {}",
                        file_path.to_string_lossy(),
                        base_path.to_string_lossy()
                    );
                    if let Err(_e) = std::fs::rename(&file_path, &base_path) {
                        debug!(
                            "Failed to rename persistent_file {} : {}",
                            file_path.to_string_lossy(),
                            _e
                        );
                    }
                }
                Err(_e) => {
                    debug!(
                        "Failed to set DELETE_ON_CLOSE on persistent_file {} : {}",
                        file_path.to_string_lossy(),
                        _e
                    );
                }
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
    pub fn as_mut_ptr(&self) -> *mut u8 {
        self.view.as_mut_ptr() as _
    }
}

/// Returns the path to a temporary directory in which to store files backing the shared memory. If it
/// doesn't exist, the directory is created.
fn get_tmp_dir() -> Result<PathBuf, ShmemError> {
    debug!("Getting & creating shared_memory-rs temp dir");
    let mut path = std::env::temp_dir();
    path.push("shared_memory-rs");

    if path.is_dir() {
        return Ok(path);
    }

    match std::fs::create_dir_all(path.as_path()) {
        Ok(_) => Ok(path),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(path),
        Err(e) => Err(ShmemError::UnknownOsError(e.raw_os_error().unwrap() as _)),
    }
}

fn new_map(
    unique_id: &str,
    mut map_size: usize,
    create: bool,
    allow_raw: bool,
) -> Result<MapData, ShmemError> {
    // Create file to back the shared memory
    let mut file_path = get_tmp_dir()?;
    file_path.push(unique_id.trim_start_matches('/'));
    debug!(
        "{} persistent_file at {}",
        if create { "Creating" } else { "Openning" },
        file_path.to_string_lossy()
    );

    let mut opt = OpenOptions::new();
    opt.read(true)
        .write(true)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .attributes((FILE_ATTRIBUTE_TEMPORARY).0);
    if create {
        opt.create_new(true);
    } else {
        opt.create(false);
    };

    let mut persistent_file = None;
    let map_h = match opt.open(&file_path) {
        Ok(f) => {
            //Create/open Mapping using persistent file
            debug!(
                "{} memory mapping",
                if create { "Creating" } else { "Openning" },
            );
            let high_size: u32 = ((map_size as u64 & 0xFFFF_FFFF_0000_0000_u64) >> 32) as u32;
            let low_size: u32 = (map_size as u64 & 0xFFFF_FFFF_u64) as u32;
            trace!(
                "CreateFileMapping({:?}, NULL, {:X}, {}, {}, '{}')",
                HANDLE(f.as_raw_handle() as _),
                PAGE_READWRITE.0,
                high_size,
                low_size,
                unique_id,
            );

            match CreateFileMapping(
                HANDLE(f.as_raw_handle() as _),
                None,
                PAGE_READWRITE,
                high_size,
                low_size,
                unique_id,
            ) {
                Ok(v) => {
                    persistent_file = Some(f);
                    v
                }
                Err(e) => {
                    let err_code = e.win32_error().unwrap();
                    return if err_code == ERROR_ALREADY_EXISTS {
                        Err(ShmemError::MappingIdExists)
                    } else {
                        Err(if create {
                            ShmemError::MapCreateFailed(err_code.0)
                        } else {
                            ShmemError::MapOpenFailed(err_code.0)
                        })
                    };
                }
            }
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => return Err(ShmemError::MappingIdExists),
        Err(e) => {
            if create {
                return Err(ShmemError::MapCreateFailed(e.raw_os_error().unwrap() as _));
            } else if !allow_raw {
                return Err(ShmemError::MapOpenFailed(ERROR_FILE_NOT_FOUND.0));
            }

            // This may be a mapping that isnt managed by this crate
            // Try to open the mapping without any backing file
            trace!(
                "OpenFileMappingW({:?}, {}, '{}')",
                FILE_MAP_ALL_ACCESS,
                false,
                unique_id,
            );
            match OpenFileMapping(FILE_MAP_ALL_ACCESS, false, unique_id) {
                Ok(h) => h,
                Err(e) => {
                    return Err(ShmemError::MapOpenFailed(e.win32_error().unwrap().0));
                }
            }
        }
    };
    trace!("0x{:X}", map_h);

    //Map mapping into address space
    debug!("Loading mapping into address space");
    trace!(
        "MapViewOfFile(0x{:X}, {:X}, 0, 0, 0)",
        map_h,
        (FILE_MAP_READ | FILE_MAP_WRITE).0,
    );
    let map_ptr = match MapViewOfFile(map_h.as_handle(), FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, 0) {
        Ok(v) => v,
        Err(e) => {
            return Err(if create {
                ShmemError::MapCreateFailed(e.win32_error().unwrap().0)
            } else {
                ShmemError::MapOpenFailed(e.win32_error().unwrap().0)
            })
        }
    };
    trace!("\t{:p}", map_ptr);

    if !create {
        //Get the real size of the openned mapping
        let mut info = MEMORY_BASIC_INFORMATION::default();
        if let Err(e) = VirtualQuery(map_ptr.as_mut_ptr(), &mut info) {
            return Err(ShmemError::UnknownOsError(e.win32_error().unwrap().0));
        }
        map_size = info.RegionSize;
    }

    Ok(MapData {
        owner: create,
        file_map: map_h,
        persistent_file,
        unique_id: unique_id.to_string(),
        map_size,
        view: map_ptr,
    })
}

//Creates a mapping specified by the uid and size
pub fn create_mapping(unique_id: &str, map_size: usize) -> Result<MapData, ShmemError> {
    new_map(unique_id, map_size, true, false)
}

//Opens an existing mapping specified by its uid
pub fn open_mapping(
    unique_id: &str,
    map_size: usize,
    ext: &ShmemConfExt,
) -> Result<MapData, ShmemError> {
    new_map(unique_id, map_size, false, ext.allow_raw)
}
