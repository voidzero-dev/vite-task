use std::path::Path;

use fspy::{AccessMode, PathAccessIterable};
#[cfg(windows)]
use fspy_shared::ipc::NativeStr;
#[doc(hidden)]
#[allow(unused)]
pub use fspy_test_utils::command_executing;

/// Canonicalize a path using the same method as fspy (GetFinalPathNameByHandleW on Windows).
/// This ensures test expectations match what fspy records.
///
/// For existing files/directories, this uses GetFinalPathNameByHandleW which returns
/// UNC paths for network drives (matching what fspy records for READ_DIR operations).
///
/// For non-existent files, this uses NativeStr::canonicalize_path which preserves
/// the original drive letter format (matching what fspy records for READ operations
/// on files opened via NtCreateFile/NtOpenFile where the path comes from OBJECT_ATTRIBUTES).
#[cfg(windows)]
fn canonicalize_path_like_fspy(path: &Path) -> std::path::PathBuf {
    use std::{
        ffi::OsString,
        os::windows::{ffi::OsStringExt, fs::OpenOptionsExt, io::AsRawHandle},
    };

    // Use GetFinalPathNameByHandleW with FILE_NAME_OPENED (0x8) to match fspy's behavior
    const FILE_NAME_OPENED: u32 = 0x8;

    // Try to open the path directly and get the final path name
    if let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(0x02000000) // FILE_FLAG_BACKUP_SEMANTICS for directories
        .open(path)
    {
        let mut buf = vec![0u16; 1024];
        let len = unsafe {
            winapi::um::fileapi::GetFinalPathNameByHandleW(
                file.as_raw_handle() as *mut _,
                buf.as_mut_ptr(),
                buf.len() as u32,
                FILE_NAME_OPENED,
            )
        };
        if len > 0 && (len as usize) < buf.len() {
            buf.truncate(len as usize);
            return std::path::PathBuf::from(OsString::from_wide(&buf));
        }
    }

    // For non-existent files, use NativeStr::canonicalize_path which preserves
    // the original path format (e.g., Z:\... stays as \\?\Z:\...)
    // This matches what fspy records for file opens via NtCreateFile/NtOpenFile.
    let native: Box<NativeStr> = path.into();
    native.canonicalize_path()
}

#[cfg(unix)]
fn canonicalize_path_like_fspy(path: &Path) -> std::path::PathBuf {
    // On Unix, just use the path as-is (fspy doesn't transform paths)
    path.to_path_buf()
}

#[track_caller]
pub fn assert_contains(
    accesses: &PathAccessIterable,
    expected_path: &Path,
    expected_mode: AccessMode,
) {
    // Canonicalize the expected path. On Windows, try both:
    // 1. GetFinalPathNameByHandleW (for existing files, gives UNC path on network drives)
    // 2. NativeStr::canonicalize_path (gives drive letter path format)
    // 3. Parent directory canonicalized + filename (for non-existent files on network drives)
    // We accept any of these formats since fspy may record paths in different formats
    // depending on how the file was accessed (via NtCreateFile vs handle operations).
    let canonical_expected_primary = canonicalize_path_like_fspy(expected_path);

    #[cfg(windows)]
    let canonical_expected_secondary = {
        let native: Box<NativeStr> = expected_path.into();
        native.canonicalize_path()
    };
    #[cfg(unix)]
    let canonical_expected_secondary = canonical_expected_primary.clone();

    // For non-existent files, also try canonicalizing the parent and joining the filename
    #[cfg(windows)]
    let canonical_expected_tertiary = if let (Some(parent), Some(file_name)) =
        (expected_path.parent(), expected_path.file_name())
    {
        let parent_canonical = canonicalize_path_like_fspy(parent);
        Some(parent_canonical.join(file_name))
    } else {
        None
    };
    #[cfg(unix)]
    let canonical_expected_tertiary: Option<std::path::PathBuf> = None;

    let mut actual_mode: AccessMode = AccessMode::empty();
    for access in accesses.iter() {
        let canonical_access = access.path.canonicalize_path();

        if canonical_access == canonical_expected_primary
            || canonical_access == canonical_expected_secondary
            || canonical_expected_tertiary.as_ref().is_some_and(|t| canonical_access == *t)
        {
            actual_mode.insert(access.mode);
        }
    }

    if actual_mode.contains(AccessMode::READ_DIR) {
        // READ_DIR already implies READ.
        actual_mode.remove(AccessMode::READ);
    }

    assert_eq!(
        expected_mode,
        actual_mode,
        "Expected to find access to path {:?} with mode {:?}, but it was not found in: {:?}",
        expected_path,
        expected_mode,
        accesses.iter().collect::<Vec<_>>()
    );
}

#[macro_export]
macro_rules! track_child {
    ($arg: expr, $body: expr) => {{
        let std_cmd = $crate::test_utils::command_executing!($arg, $body);
        $crate::test_utils::spawn_std(std_cmd)
    }};
}

#[doc(hidden)]
#[allow(unused)]
pub async fn spawn_std(std_cmd: std::process::Command) -> anyhow::Result<PathAccessIterable> {
    let mut command = fspy::Command::new(std_cmd.get_program());
    command
        .args(std_cmd.get_args())
        .envs(std_cmd.get_envs().filter_map(|(name, value)| Some((name, value?))));

    let termination = command.spawn().await?.wait_handle.await?;
    assert!(termination.status.success());
    Ok(termination.path_accesses)
}
