//! Detours for FindFirstFile/FindNextFile APIs to track directory reads.
//!
//! These Win32 APIs are commonly used for directory enumeration and need to be
//! intercepted to track READ_DIR accesses.

#![allow(non_snake_case)] // Windows API parameter naming convention

use fspy_shared::ipc::{AccessMode, NativeStr, PathAccess};
use smallvec::SmallVec;
use widestring::U16CStr;
use winapi::{
    shared::{
        minwindef::{DWORD, LPVOID, MAX_PATH},
        ntdef::HANDLE,
    },
    um::{
        fileapi::FindFirstFileExW,
        minwinbase::{FINDEX_INFO_LEVELS, FINDEX_SEARCH_OPS, LPWIN32_FIND_DATAW},
        processenv::GetCurrentDirectoryW,
        winnt::LPCWSTR,
    },
};

use crate::windows::{
    client::global_client,
    detour::{Detour, DetourAny},
};

/// Get the current directory as a wide string
fn get_current_directory() -> SmallVec<u16, MAX_PATH> {
    let mut buffer = SmallVec::<u16, MAX_PATH>::new();
    buffer.resize(MAX_PATH, 0);

    let len = unsafe { GetCurrentDirectoryW(buffer.len() as DWORD, buffer.as_mut_ptr()) };
    if len == 0 {
        // Failed to get current directory, return empty
        return SmallVec::new();
    }

    let len = len as usize;
    if len > buffer.len() {
        // Buffer too small, allocate more
        buffer.resize(len, 0);
        let len = unsafe { GetCurrentDirectoryW(buffer.len() as DWORD, buffer.as_mut_ptr()) };
        if len == 0 {
            return SmallVec::new();
        }
        buffer.truncate(len as usize);
    } else {
        buffer.truncate(len);
    }

    buffer
}

/// Extract the directory path from a search pattern like "C:\foo\*" -> "C:\foo"
/// For patterns without a directory separator (like "*"), returns the current directory
fn extract_directory_from_pattern(pattern: &U16CStr) -> SmallVec<u16, MAX_PATH> {
    let slice = pattern.as_slice();

    // Find the last backslash or forward slash
    if let Some(last_sep_pos) = slice.iter().rposition(|&c| c == b'\\' as u16 || c == b'/' as u16) {
        // Return the directory part (without trailing separator)
        slice[..last_sep_pos].iter().cloned().collect()
    } else {
        // No separator found - pattern is in current directory (e.g., "*" or "*.js")
        // Return the current directory
        get_current_directory()
    }
}

static DETOUR_FIND_FIRST_FILE_EX_W: Detour<
    unsafe extern "system" fn(
        lpFileName: LPCWSTR,
        fInfoLevelId: FINDEX_INFO_LEVELS,
        lpFindFileData: LPVOID,
        fSearchOp: FINDEX_SEARCH_OPS,
        lpSearchFilter: LPVOID,
        dwAdditionalFlags: DWORD,
    ) -> HANDLE,
> = unsafe {
    Detour::new(c"FindFirstFileExW", FindFirstFileExW, {
        unsafe extern "system" fn new_find_first_file_ex_w(
            lpFileName: LPCWSTR,
            fInfoLevelId: FINDEX_INFO_LEVELS,
            lpFindFileData: LPVOID,
            fSearchOp: FINDEX_SEARCH_OPS,
            lpSearchFilter: LPVOID,
            dwAdditionalFlags: DWORD,
        ) -> HANDLE {
            // Track the directory access before calling the real function
            if !lpFileName.is_null() {
                let pattern = unsafe { U16CStr::from_ptr_str(lpFileName) };
                let dir_path = extract_directory_from_pattern(pattern);
                let client = unsafe { global_client() };
                let path_access = PathAccess {
                    mode: AccessMode::READ_DIR,
                    path: NativeStr::from_wide(&dir_path),
                };
                client.send(path_access);
            }

            // Call the original function
            unsafe {
                (DETOUR_FIND_FIRST_FILE_EX_W.real())(
                    lpFileName,
                    fInfoLevelId,
                    lpFindFileData,
                    fSearchOp,
                    lpSearchFilter,
                    dwAdditionalFlags,
                )
            }
        }
        new_find_first_file_ex_w
    })
};

// FindFirstFileW is typically a wrapper around FindFirstFileExW, but let's intercept it too
// in case some applications call it directly
static DETOUR_FIND_FIRST_FILE_W: Detour<
    unsafe extern "system" fn(lpFileName: LPCWSTR, lpFindFileData: LPWIN32_FIND_DATAW) -> HANDLE,
> = unsafe {
    Detour::new(c"FindFirstFileW", winapi::um::fileapi::FindFirstFileW, {
        unsafe extern "system" fn new_find_first_file_w(
            lpFileName: LPCWSTR,
            lpFindFileData: LPWIN32_FIND_DATAW,
        ) -> HANDLE {
            // Track the directory access before calling the real function
            if !lpFileName.is_null() {
                let pattern = unsafe { U16CStr::from_ptr_str(lpFileName) };
                let dir_path = extract_directory_from_pattern(pattern);
                let client = unsafe { global_client() };
                let path_access = PathAccess {
                    mode: AccessMode::READ_DIR,
                    path: NativeStr::from_wide(&dir_path),
                };
                client.send(path_access);
            }

            // Call the original function
            unsafe { (DETOUR_FIND_FIRST_FILE_W.real())(lpFileName, lpFindFileData) }
        }
        new_find_first_file_w
    })
};

pub const DETOURS: &[DetourAny] =
    &[DETOUR_FIND_FIRST_FILE_EX_W.as_any(), DETOUR_FIND_FIRST_FILE_W.as_any()];
