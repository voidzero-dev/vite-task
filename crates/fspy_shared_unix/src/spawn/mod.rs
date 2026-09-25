#[cfg(target_os = "linux")]
#[path = "./linux/mod.rs"]
mod os_specific;

#[cfg(target_os = "macos")]
#[path = "./macos.rs"]
mod os_specific;

use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::Path};

use fspy_shared::ipc::AccessMode;
#[doc(hidden)]
#[cfg(target_os = "macos")]
pub use os_specific::COREUTILS_FUNCTIONS as COREUTILS_FUNCTIONS_FOR_TEST;
pub use os_specific::PreExec;

use crate::{
    exec::{Exec, ExecResolveConfig},
    payload::EncodedPayload,
};

/// Resolves the exec's program path and reports the accesses that takes.
///
/// This is the half of [`handle_exec`] whose failures mean what the real
/// exec's failure would have meant (see [`Exec::resolve`]), so a caller can
/// forward the errno authentically.
///
/// # Errors
///
/// Returns an error if program resolution fails (see [`Exec::resolve`] error
/// variants, such as `ENOENT` (file not found) or `EACCES` (permission denied)).
///
/// # Panics
///
/// Panics if the current working directory cannot be determined when converting a relative path to absolute.
pub fn resolve_exec(
    command: &mut Exec,
    config: ExecResolveConfig,
    mut on_path_access: impl FnMut(AccessMode, &Path),
) -> nix::Result<()> {
    let mut on_path_access = |mode: AccessMode, path: &Path| {
        if path.is_absolute() {
            on_path_access(mode, path);
        } else {
            let path = std::path::absolute(path).expect("Failed to get cwd");
            on_path_access(mode, &path);
        }
    };

    command.resolve(&mut on_path_access, config)?;
    on_path_access(AccessMode::READ, Path::new(OsStr::from_bytes(&command.program)));
    Ok(())
}

/// Prepares a resolved exec for tracked execution: injects the preload
/// environment, or arms the seccomp filter to install before exec.
///
/// This is the half of [`handle_exec`] whose failures are the tracing
/// machinery's own, never the exec's.
///
/// # Errors
///
/// Returns an error if environment variable operations fail (e.g.,
/// `ensure_env` may return `EINVAL` if an existing value conflicts) or from
/// platform-specific errors in `os_specific::handle_exec`.
pub fn prepare_exec(
    command: &mut Exec,
    encoded_payload: &EncodedPayload,
) -> nix::Result<Option<PreExec>> {
    os_specific::handle_exec(command, encoded_payload)
}

/// Handles exec command resolution and injection
///
/// Resolves the program path and prepares the command for execution with
/// appropriate environment variables and hooks. Composed of [`resolve_exec`]
/// followed by [`prepare_exec`]; call them separately to tell an authentic
/// resolution failure apart from an injection-machinery failure.
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
pub fn handle_exec(
    command: &mut Exec,
    config: ExecResolveConfig,
    encoded_payload: &EncodedPayload,
    on_path_access: impl FnMut(AccessMode, &Path),
) -> nix::Result<Option<PreExec>> {
    resolve_exec(command, config, on_path_access)?;
    prepare_exec(command, encoded_payload)
}
