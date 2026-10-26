#[cfg(target_os = "linux")]
#[path = "./linux/mod.rs"]
mod os_specific;

#[cfg(target_os = "macos")]
#[path = "./macos.rs"]
mod os_specific;

use std::{
    ffi::OsStr,
    os::unix::ffi::OsStrExt,
    path::{Component, Path},
};

use allocator_api2::{alloc::Allocator, vec::Vec};
#[doc(hidden)]
#[cfg(target_os = "macos")]
pub use os_specific::COREUTILS_FUNCTIONS as COREUTILS_FUNCTIONS_FOR_TEST;
pub use os_specific::PreExec;

use crate::{
    exec::{Exec, ExecResolveConfig},
    payload::EncodedPayload,
};

/// Makes the relative `path` absolute against the current directory, as
/// [`std::path::absolute`] does, but in `allocator` rather than libc's heap:
/// exec may run in the child of `fork()`, where malloc's lock may be held by
/// a thread that no longer exists.
///
/// # Errors
///
/// Like [`std::path::absolute`], returns `EINVAL` for an empty path, and
/// the error from getting the current directory.
pub(crate) fn absolute<A: Allocator>(path: &Path, allocator: A) -> nix::Result<Vec<u8, A>> {
    debug_assert!(path.is_relative());
    if path.as_os_str().is_empty() {
        return Err(nix::Error::EINVAL);
    }
    let mut absolute = fspy_nostd_alloc::fs::getcwd(allocator)
        .map_err(|errno| nix::Error::from_raw(errno.raw_os_error()))?
        .into_units();
    // A relative path's only `.` component is a leading one, which
    // `std::path::absolute` drops.
    for component in path.components().filter(|component| *component != Component::CurDir) {
        if absolute.last() != Some(&b'/') {
            absolute.push(b'/');
        }
        absolute.extend_from_slice(component.as_os_str().as_bytes());
    }
    // Like `std::path::absolute`, keep a trailing slash.
    if path.as_os_str().as_bytes().ends_with(b"/") && absolute.last() != Some(&b'/') {
        absolute.push(b'/');
    }
    Ok(absolute)
}

/// Handles exec command resolution and injection
///
/// Resolves the program path and prepares the command for execution with
/// appropriate environment variables and hooks.
///
/// # Errors
///
/// Returns an error if:
/// - Program resolution fails (see [`Exec::resolve`] error variants, such as `ENOENT` (file not found) or `EACCES` (permission denied))
/// - Environment variable operations fail (e.g., `ensure_env` may return `EINVAL` if an existing value conflicts)
/// - Platform-specific errors from `os_specific::handle_exec`
///
/// # Panics
///
/// Panics if the current working directory cannot be determined when converting a relative path to absolute.
pub fn handle_exec<A: Allocator + Clone>(
    command: &mut Exec<A>,
    config: ExecResolveConfig,
    encoded_payload: &EncodedPayload,
    mut on_path_read: impl FnMut(&Path),
) -> nix::Result<Option<PreExec>> {
    let allocator = command.allocator().clone();
    let mut on_path_read = |path: &Path| {
        if path.is_absolute() {
            on_path_read(path);
        } else {
            let path = absolute(path, &allocator).expect("Failed to get cwd");
            on_path_read(Path::new(OsStr::from_bytes(&path)));
        }
    };

    command.resolve(&mut on_path_read, config)?;
    on_path_read(Path::new(OsStr::from_bytes(&command.program)));

    os_specific::handle_exec(command, encoded_payload)
}

#[cfg(test)]
mod tests {
    use std::{os::unix::ffi::OsStrExt, path::Path};

    use allocator_api2::alloc::Global;

    use super::absolute;

    #[test]
    fn absolute_matches_std() {
        for path in ["foo", "./foo", "foo/./bar", "foo//bar", "../foo", "foo/", ".", "./", ""] {
            let path = Path::new(path);
            assert_eq!(
                absolute(path, Global).ok().as_deref(),
                std::path::absolute(path).ok().as_ref().map(|path| path.as_os_str().as_bytes()),
                "{}",
                path.display(),
            );
        }
    }
}
