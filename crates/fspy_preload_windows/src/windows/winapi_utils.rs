use std::{
    mem::{offset_of, size_of},
    slice,
};

use fspy_shared::ipc::AccessMode;
use smallvec::SmallVec;
use widestring::{U16CStr, U16CString, U16Str};
use winapi::{
    ctypes::c_long,
    shared::{
        minwindef::{BOOL, FALSE, MAX_PATH},
        ntdef::{HANDLE, PWSTR, UNICODE_STRING},
        winerror::{NO_ERROR, S_OK},
    },
    um::{
        fileapi::GetFinalPathNameByHandleW,
        winnt::{
            ACCESS_MASK, FILE_APPEND_DATA, FILE_READ_DATA, FILE_WRITE_DATA, GENERIC_READ,
            GENERIC_WRITE,
        },
    },
};
use windows_sys::Win32::{
    Foundation::LocalFree,
    UI::Shell::{PATHCCH_ALLOW_LONG_PATHS, PathAllocCombine},
};
use winsafe::{GetLastError, co};

pub fn ck(b: BOOL) -> winsafe::SysResult<()> {
    if b == FALSE { Err(GetLastError()) } else { Ok(()) }
}

pub const fn ck_long(val: c_long) -> winsafe::SysResult<()> {
    if 0 == NO_ERROR {
        Ok(())
    } else {
        // SAFETY: creating an ERROR from the raw c_long value for the Windows error code
        Err(unsafe { winsafe::co::ERROR::from_raw(val.cast_unsigned()) })
    }
}

pub unsafe fn get_u16_str(ustring: &UNICODE_STRING) -> &U16Str {
    // https://learn.microsoft.com/en-us/windows/win32/api/subauth/ns-subauth-unicode_string
    // UNICODE_STRING.Length is in bytes
    let u16_count = ustring.Length / 2;
    let chars: &[u16] = if u16_count == 0 {
        // If length is zero, we can't use slice::from_raw_parts as it requires a non-null pointer but
        // Buffer may be null in that case.
        &[]
    } else {
        // SAFETY: UNICODE_STRING.Buffer points to a valid u16 array of Length/2 elements
        unsafe { slice::from_raw_parts(ustring.Buffer, usize::from(u16_count)) }
    };
    U16CStr::from_slice_truncate(chars).map_or_else(|_| chars.into(), U16CStr::as_ustr)
}

pub unsafe fn get_path_name(handle: HANDLE) -> winsafe::SysResult<SmallVec<u16, MAX_PATH>> {
    let mut path = SmallVec::<u16, MAX_PATH>::new();
    // SAFETY: FFI call to GetFinalPathNameByHandleW to query the file path from a handle
    let len = unsafe {
        GetFinalPathNameByHandleW(
            handle,
            path.as_mut_ptr(),
            path.capacity().try_into().unwrap(),
            0, /*FILE_NAME_NORMALIZED*/
        )
    };
    if len == 0 {
        return Err(winsafe::GetLastError());
    }
    let len = usize::try_from(len).unwrap();
    if len <= path.capacity() {
        // SAFETY: GetFinalPathNameByHandleW wrote `len` u16 characters into the buffer
        unsafe { path.set_len(len) };
    } else {
        path.reserve_exact(len);
        // SAFETY: FFI call to GetFinalPathNameByHandleW with larger buffer after first call indicated needed size
        let len = unsafe {
            GetFinalPathNameByHandleW(
                handle,
                path.as_mut_ptr(),
                path.capacity().try_into().unwrap(),
                0, /*FILE_NAME_NORMALIZED*/
            )
        };
        let len = usize::try_from(len).unwrap();
        if len == 0 {
            return Err(winsafe::GetLastError());
        } else if len > path.capacity() {
            unreachable!()
        }
        // SAFETY: GetFinalPathNameByHandleW wrote `len` u16 characters into the buffer
        unsafe { path.set_len(len) };
    }
    Ok(path)
}

pub const fn access_mask_to_mode(desired_access: ACCESS_MASK) -> AccessMode {
    let has_write = (desired_access & (FILE_WRITE_DATA | FILE_APPEND_DATA | GENERIC_WRITE)) != 0;
    let has_read = (desired_access & (FILE_READ_DATA | GENERIC_READ)) != 0;
    if has_write {
        if has_read { AccessMode::READ.union(AccessMode::WRITE) } else { AccessMode::WRITE }
    } else {
        AccessMode::READ
    }
}

/// Translate `NtCreateFile`'s `CreateDisposition` into creation and truncation
/// intent.
///
/// This is Windows' equivalent of `O_CREAT`, `O_TRUNC` and `O_EXCL`, and the
/// consumer needs it for the same reason: a handle opened for writing is only
/// capability, while replacing or newly creating a file is a mutation.
pub const fn create_disposition_to_mode(create_disposition: u32) -> AccessMode {
    // Values from the NtCreateFile contract. Named locally because ntapi
    // exposes them as plain constants of varying integer types.
    const FILE_SUPERSEDE: u32 = 0;
    const FILE_CREATE: u32 = 2;
    const FILE_OPEN_IF: u32 = 3;
    const FILE_OVERWRITE: u32 = 4;
    const FILE_OVERWRITE_IF: u32 = 5;

    match create_disposition {
        // Replaces the file if it exists, creates it otherwise.
        FILE_SUPERSEDE | FILE_OVERWRITE_IF => AccessMode::CREATE.union(AccessMode::TRUNCATE),
        // Fails if the file already exists, so the handle refers to new content.
        FILE_CREATE => AccessMode::CREATE.union(AccessMode::EXCLUSIVE),
        // Creates only when absent, leaving existing content in place.
        FILE_OPEN_IF => AccessMode::CREATE,
        // Requires the file to exist, then discards its contents.
        FILE_OVERWRITE => AccessMode::TRUNCATE,
        // FILE_OPEN and anything unrecognized: no creation, no truncation.
        _ => AccessMode::empty(),
    }
}

pub struct HeapPath(PWSTR);
impl HeapPath {
    #[must_use]
    pub fn to_u16_str(&self) -> &U16Str {
        // SAFETY: the PWSTR was allocated by PathAllocCombine and is a valid null-terminated wide string
        unsafe { U16CStr::from_ptr_str(self.0).as_ustr() }
    }
}
impl Drop for HeapPath {
    fn drop(&mut self) {
        // SAFETY: freeing the PWSTR allocated by PathAllocCombine via LocalFree
        unsafe { LocalFree(self.0.cast()) };
    }
}

pub fn combine_paths(path1: &U16CStr, path2: &U16CStr) -> winsafe::SysResult<HeapPath> {
    let mut out = std::ptr::null_mut();
    // SAFETY: FFI call to PathAllocCombine with valid null-terminated wide string pointers
    let hr = unsafe {
        PathAllocCombine(
            path1.as_ptr(),
            path2.as_ptr(),
            PATHCCH_ALLOW_LONG_PATHS, /*PATHCOMBINE_DEFAULT*/
            &raw mut out,
        )
    };
    if hr != S_OK {
        // SAFETY: creating an ERROR from the HRESULT value
        return Err(unsafe { co::ERROR::from_raw(hr.try_into().unwrap()) });
    }
    Ok(HeapPath(out))
}

/// Whether a handle refers to a directory.
///
/// Needed so a consumer can re-attribute writes recorded beneath a renamed
/// directory: a build that stages into `dist.tmp` and renames it into place
/// records every write against the staging path, and only the directory carries
/// the rename.
pub unsafe fn handle_is_directory(handle: HANDLE) -> bool {
    use winapi::um::{
        fileapi::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle},
        winnt::FILE_ATTRIBUTE_DIRECTORY,
    };

    // SAFETY: an all-zero BY_HANDLE_FILE_INFORMATION is a valid initial value
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { core::mem::zeroed() };
    // SAFETY: handle comes from the interposed caller and info is a valid owned struct
    if unsafe { GetFileInformationByHandle(handle, &raw mut info) } == 0 {
        return false;
    }
    info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
}

/// The destination path carried by a `FILE_RENAME_INFORMATION` buffer.
///
/// The struct is variable-length: a fixed header followed by the target name.
/// When `RootDirectory` is set the name is relative to that directory, matching
/// `renameat` on Unix.
///
/// # Safety
///
/// `info` must point to at least `length` readable bytes holding a
/// `FILE_RENAME_INFORMATION`, as supplied by the `NtSetInformationFile` caller.
#[expect(
    clippy::cast_ptr_alignment,
    reason = "the kernel requires callers to pass a correctly aligned FILE_RENAME_INFORMATION"
)]
pub unsafe fn rename_target_path(info: *const u8, length: u32) -> Option<U16CString> {
    use ntapi::ntioapi::FILE_RENAME_INFORMATION;

    if info.is_null() || (length as usize) < size_of::<FILE_RENAME_INFORMATION>() {
        return None;
    }
    // SAFETY: the caller guarantees `info` holds a FILE_RENAME_INFORMATION
    let rename = unsafe { &*info.cast::<FILE_RENAME_INFORMATION>() };
    let name_bytes = rename.FileNameLength as usize;
    if name_bytes == 0 || !name_bytes.is_multiple_of(2) {
        return None;
    }
    let name_offset = offset_of!(FILE_RENAME_INFORMATION, FileName);
    if name_offset.saturating_add(name_bytes) > length as usize {
        return None;
    }
    // SAFETY: bounds checked against `length` above; FileName is a wide string
    let name =
        unsafe { std::slice::from_raw_parts(info.add(name_offset).cast::<u16>(), name_bytes / 2) };
    let target = U16Str::from_slice(name);

    let is_absolute =
        name.first() == Some(&u16::from(b'\\')) || name.get(1) == Some(&u16::from(b':'));
    if is_absolute || rename.RootDirectory.is_null() {
        return Some(U16CString::from_ustr_truncate(target));
    }
    // Relative to RootDirectory, so resolve it the same way an open would.
    // SAFETY: RootDirectory is a directory handle supplied by the caller
    let mut root = unsafe { get_path_name(rename.RootDirectory) }.ok()?;
    root.push(0);
    // SAFETY: a null terminator was just pushed
    let root = unsafe { U16CStr::from_ptr_str(root.as_ptr()) };
    let combined = combine_paths(root, U16CString::from_ustr_truncate(target).as_ucstr()).ok()?;
    Some(U16CString::from_ustr_truncate(combined.to_u16_str()))
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        fs::File,
        os::windows::{ffi::OsStringExt, io::AsRawHandle},
        path::PathBuf,
    };

    use super::get_path_name;

    fn test_get_path_name(filename: &str) {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().canonicalize().unwrap().join(filename);
        let file = File::create(&path).unwrap();
        // SAFETY: passing a valid raw file handle to get_path_name
        let actual_path = unsafe { get_path_name(file.as_raw_handle().cast()) }.unwrap();
        let actual_path = PathBuf::from(OsString::from_wide(&actual_path));
        assert_eq!(path, actual_path);
    }

    #[test]
    fn test_get_path_name_short() {
        test_get_path_name("foo");
    }
    #[test]
    fn test_get_path_name_long() {
        test_get_path_name(str::repeat("a", 255).as_str());
    }

    #[test]
    fn test_combine_path() {
        use widestring::u16cstr;

        use super::combine_paths;

        let path1 = u16cstr!("C:\\foo");
        let path2 = u16cstr!("bar\\baz");
        let combined = combine_paths(path1, path2).unwrap();
        assert_eq!(combined.to_u16_str(), u16cstr!("C:\\foo\\bar\\baz"));
    }
}
