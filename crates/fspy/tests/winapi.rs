#![cfg(windows)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods, clippy::disallowed_macros)]

mod test_utils;

use std::{ffi::OsStr, os::windows::ffi::OsStrExt, path::Path, ptr::null_mut};

use fspy::AccessMode;
use test_log::test;
use test_utils::assert_contains;
use winapi::um::processthreadsapi::{
    CreateProcessA, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOA, STARTUPINFOW,
};

#[test(tokio::test)]
async fn create_process_a() -> anyhow::Result<()> {
    let accesses = track_child!((), |(): ()| {
        let mut si: STARTUPINFOA = unsafe { std::mem::zeroed() };
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
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
                &mut si,
                &mut pi,
            )
        };
    })
    .await?;
    assert_contains(&accesses, Path::new("C:\\fspy_test_not_exist_program.exe"), AccessMode::READ);

    Ok(())
}

#[test(tokio::test)]
async fn create_process_w() -> anyhow::Result<()> {
    let accesses = track_child!((), |(): ()| {
        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let program =
            OsStr::new("C:\\fspy_test_not_exist_program.exe\0").encode_wide().collect::<Vec<u16>>();
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
                &mut si,
                &mut pi,
            )
        };
    })
    .await?;
    assert_contains(&accesses, Path::new("C:\\fspy_test_not_exist_program.exe"), AccessMode::READ);

    Ok(())
}
