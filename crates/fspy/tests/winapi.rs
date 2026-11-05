#![cfg(windows)]
mod test_utils;

use std::{ffi::OsStr, os::windows::ffi::OsStrExt, path::Path, ptr::null_mut};

use fspy::PathAccess;
use test_log::test;
use test_utils::assert_contains;
use winapi::um::processthreadsapi::{
    CreateProcessA, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOA, STARTUPINFOW,
};

#[test(tokio::test)]
async fn create_process_a() -> anyhow::Result<()> {
    let accesses = track_fn!((), |(): ()| {
        // SAFETY: zeroing STARTUPINFOA is valid for the Windows API
        let mut si: STARTUPINFOA = unsafe { std::mem::zeroed() };
        // CreateProcess requires cb to identify the STARTUPINFO layout.
        si.cb = std::mem::size_of::<STARTUPINFOA>().try_into().unwrap();
        // SAFETY: zeroing PROCESS_INFORMATION is valid for the Windows API
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: all pointers are valid or null_mut as required by CreateProcessA
        unsafe {
            CreateProcessA(
                c"C:\\fspy_test_not_exist_program.exe".as_ptr().cast(),
                null_mut(),
                null_mut(),
                null_mut(),
                0,
                0,
                null_mut(),
                null_mut(),
                &raw mut si,
                &raw mut pi,
            )
        };
    })
    .await?;
    assert_contains(&accesses, Path::new("C:\\fspy_test_not_exist_program.exe"), PathAccess::Read);

    Ok(())
}

#[test(tokio::test)]
async fn create_process_w() -> anyhow::Result<()> {
    let accesses = track_fn!((), |(): ()| {
        // SAFETY: zeroing STARTUPINFOW is valid for the Windows API
        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        // CreateProcess requires cb to identify the STARTUPINFO layout.
        si.cb = std::mem::size_of::<STARTUPINFOW>().try_into().unwrap();
        // SAFETY: zeroing PROCESS_INFORMATION is valid for the Windows API
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let program =
            OsStr::new("C:\\fspy_test_not_exist_program.exe\0").encode_wide().collect::<Vec<u16>>();
        // SAFETY: all pointers are valid or null_mut as required by CreateProcessW
        unsafe {
            CreateProcessW(
                program.as_ptr(),
                null_mut(),
                null_mut(),
                null_mut(),
                0,
                0,
                null_mut(),
                null_mut(),
                &raw mut si,
                &raw mut pi,
            )
        };
    })
    .await?;
    assert_contains(&accesses, Path::new("C:\\fspy_test_not_exist_program.exe"), PathAccess::Read);

    Ok(())
}

#[test(tokio::test)]
async fn rename_destination_relative_to_root_directory() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    // The destination is recorded by the long name of the root directory's
    // handle, while `TEMP` may hold an 8.3 short name.
    let tmp_path = tmp_dir.path().canonicalize()?;
    std::fs::File::create(tmp_path.join("source"))?;
    let tmp_path_str = tmp_path.to_str().unwrap().to_owned();
    let accesses = track_fn!(tmp_path_str, |tmp_path_str: String| {
        use std::{
            fs::OpenOptions,
            mem::offset_of,
            os::windows::{fs::OpenOptionsExt as _, io::AsRawHandle as _},
        };

        use ntapi::ntioapi::{
            FILE_RENAME_INFORMATION, FileRenameInformation, IO_STATUS_BLOCK, NtSetInformationFile,
        };
        use winapi::um::{winbase::FILE_FLAG_BACKUP_SEMANTICS, winnt::DELETE};

        // Zig's and Bun's renames pass the destination's folder as
        // `RootDirectory` and the name relative to it.
        let tmp_dir = Path::new(&tmp_path_str);
        let root = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(tmp_dir)
            .unwrap();
        let source = OpenOptions::new().access_mode(DELETE).open(tmp_dir.join("source")).unwrap();
        let name = OsStr::new("destination").encode_wide().collect::<Vec<u16>>();
        let name_offset = offset_of!(FILE_RENAME_INFORMATION, FileName);
        let name_bytes = name.len() * size_of::<u16>();
        let length = name_offset + name_bytes;
        let mut buffer = vec![0_u64; length.div_ceil(size_of::<u64>())];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        // SAFETY: the zeroed buffer is aligned for the structure and holds its
        // fixed part and the name in its trailing array.
        unsafe {
            (*info).RootDirectory = root.as_raw_handle().cast();
            (*info).FileNameLength = name_bytes.try_into().unwrap();
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                name.len(),
            );
        }
        // SAFETY: zeroing IO_STATUS_BLOCK is valid for the NT API
        let mut io_status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
        // SAFETY: the source handle is open with `DELETE` access, and the
        // buffer is valid for `length` bytes
        let status = unsafe {
            NtSetInformationFile(
                source.as_raw_handle().cast(),
                &raw mut io_status,
                info.cast(),
                length.try_into().unwrap(),
                FileRenameInformation,
            )
        };
        assert_eq!(status, 0, "STATUS_SUCCESS");
    })
    .await?;
    assert_contains(&accesses, &tmp_path.join("destination"), PathAccess::RenameTarget);

    Ok(())
}

#[test(tokio::test)]
async fn rename_destination_bare_name_in_source_folder() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    // The destination is recorded by the long name of the source's handle,
    // while `TEMP` may hold an 8.3 short name.
    let tmp_path = tmp_dir.path().canonicalize()?;
    std::fs::File::create(tmp_path.join("source"))?;
    let tmp_path_str = tmp_path.to_str().unwrap().to_owned();
    let accesses = track_fn!(tmp_path_str, |tmp_path_str: String| {
        use std::{
            fs::OpenOptions,
            mem::offset_of,
            os::windows::{fs::OpenOptionsExt as _, io::AsRawHandle as _},
        };

        use ntapi::ntioapi::{
            FILE_RENAME_INFORMATION, FileRenameInformation, IO_STATUS_BLOCK, NtSetInformationFile,
        };
        use winapi::um::winnt::DELETE;

        // With no `RootDirectory`, a name without a `\` renames the file
        // within its own folder.
        let tmp_dir = Path::new(&tmp_path_str);
        let source = OpenOptions::new().access_mode(DELETE).open(tmp_dir.join("source")).unwrap();
        let name = OsStr::new("destination").encode_wide().collect::<Vec<u16>>();
        let name_offset = offset_of!(FILE_RENAME_INFORMATION, FileName);
        let name_bytes = name.len() * size_of::<u16>();
        let length = name_offset + name_bytes;
        let mut buffer = vec![0_u64; length.div_ceil(size_of::<u64>())];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        // SAFETY: the zeroed buffer is aligned for the structure and holds its
        // fixed part and the name in its trailing array; `RootDirectory` stays
        // null.
        unsafe {
            (*info).FileNameLength = name_bytes.try_into().unwrap();
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                name.len(),
            );
        }
        // SAFETY: zeroing IO_STATUS_BLOCK is valid for the NT API
        let mut io_status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
        // SAFETY: the source handle is open with `DELETE` access, and the
        // buffer is valid for `length` bytes
        let status = unsafe {
            NtSetInformationFile(
                source.as_raw_handle().cast(),
                &raw mut io_status,
                info.cast(),
                length.try_into().unwrap(),
                FileRenameInformation,
            )
        };
        assert_eq!(status, 0, "STATUS_SUCCESS");
    })
    .await?;
    assert_contains(&accesses, &tmp_path.join("destination"), PathAccess::RenameTarget);

    Ok(())
}
