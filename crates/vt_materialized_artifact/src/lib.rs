//! Materialize a compile-time–embedded file to disk on demand.
//!
//! Some APIs need a file on disk — `LoadLibrary` and `LD_PRELOAD` take a
//! path, and helper binaries have to exist as actual files to be spawned —
//! but we want to ship a single executable. `vt_materialized_artifact` embeds
//! the file content as a `&'static [u8]` at compile time via the
//! [`artifact!`] macro (same as `include_bytes!`), and [`Materialize::at`]
//! writes it out to disk when first needed — that materialization step is
//! the value-add over a bare `include_bytes!`.
//!
//! Materialized files are named `{name}_{hash}{suffix}` in the caller-chosen
//! directory, normally the current user's own folder in the system temp dir,
//! which no other user can write to (see [`Materialize::at_user_temp_dir`]).
//! The hash (computed at macro-expansion time by [`artifact!`]) gives three
//! properties without any coordination between processes:
//!
//! - **No repeated writes.** [`Materialize::at`] returns the existing path if
//!   the file is already there; repeated calls and re-runs skip I/O.
//! - **Correctness.** Two binaries with different embedded content produce
//!   different filenames, so a stale file from an older build is never
//!   mistaken for the current one.
//! - **Coexistence.** Multiple versions of a materialized artifact (e.g. from
//!   different builds of the host program on the same machine) share `dir`
//!   without overwriting each other.

use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// The folder named after `prefix` in the system temp dir that only the
/// current user can write to, created if missing: see
/// [`Materialize::at_user_temp_dir`].
fn user_temp_dir(prefix: &str) -> io::Result<PathBuf> {
    user_dir_in(&env::temp_dir(), prefix)
}

#[cfg(unix)]
fn user_dir_in(temp_dir: &Path, name: &str) -> io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    let uid = current_uid();
    let dir = temp_dir.join(format!("{name}-{uid}"));
    match fs::DirBuilder::new().mode(0o700).create(&dir) {
        Err(err) if err.kind() != io::ErrorKind::AlreadyExists => return Err(err),
        _ => {}
    }
    // `symlink_metadata` doesn't follow a symlink, so one planted by another
    // user fails the `is_dir` check instead of redirecting the files.
    let metadata = fs::symlink_metadata(&dir)?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.permissions().mode() & 0o022 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} isn't a directory that only the current user can write to", dir.display()),
        ));
    }
    Ok(dir)
}

#[cfg(unix)]
fn current_uid() -> libc::uid_t {
    // SAFETY: `geteuid` has no preconditions and always succeeds.
    unsafe { libc::geteuid() }
}

#[cfg(windows)]
fn user_dir_in(temp_dir: &Path, name: &str) -> io::Result<PathBuf> {
    let dir = temp_dir.join(name);
    match fs::create_dir(&dir) {
        Err(err) if err.kind() != io::ErrorKind::AlreadyExists => Err(err),
        _ => Ok(dir),
    }
}

/// A file embedded into the executable at compile time.
///
/// Construct with [`artifact!`]; materialize to disk via
/// [`Artifact::materialize`] + [`Materialize::at_user_temp_dir`] or
/// [`Materialize::at`]. See the [crate docs] for the design rationale.
///
/// [crate docs]: crate
#[derive(Clone, Copy)]
pub struct Artifact {
    name: &'static str,
    content: &'static [u8],
    hash: &'static str,
}

/// Construct an [`Artifact`] from an env var holding a file path at compile
/// time — see the macro's own docs for usage and design notes.
pub use vt_materialized_artifact_macros::artifact;

impl Artifact {
    #[doc(hidden)]
    #[must_use]
    pub const fn __new(name: &'static str, content: &'static [u8], hash: &'static str) -> Self {
        Self { name, content, hash }
    }

    /// Start a fluent materialize chain. Supply optional [`Materialize::suffix`]
    /// / [`Materialize::executable`] knobs, then terminate with
    /// [`Materialize::at_user_temp_dir`] or [`Materialize::at`].
    pub const fn materialize(&self) -> Materialize<'static> {
        Materialize {
            artifact: *self,
            suffix: "",
            #[cfg(unix)]
            executable: false,
        }
    }
}

/// Builder returned by [`Artifact::materialize`]. Terminate with
/// [`Materialize::at_user_temp_dir`] or [`Materialize::at`] to write the file.
#[derive(Clone, Copy)]
#[must_use = "materialize() only configures — call .at_user_temp_dir(prefix) or .at(dir) to write the file"]
pub struct Materialize<'a> {
    artifact: Artifact,
    suffix: &'a str,
    #[cfg(unix)]
    executable: bool,
}

impl Materialize<'_> {
    /// Filename suffix appended after `{name}_{hash}` (e.g. `.dll`, `.dylib`).
    /// Defaults to empty.
    pub const fn suffix(self, suffix: &str) -> Materialize<'_> {
        Materialize {
            artifact: self.artifact,
            suffix,
            #[cfg(unix)]
            executable: self.executable,
        }
    }

    /// Mark the materialized file as executable (`0o755` on Unix; no-op on
    /// Windows where the filesystem has no executable bit). Other files get
    /// the default mode for new temp files, `0o600`.
    #[cfg_attr(not(unix), expect(unused_mut, reason = "executable is Unix-only"))]
    pub const fn executable(mut self) -> Self {
        #[cfg(unix)]
        {
            self.executable = true;
        }
        self
    }

    /// Materialize the artifact, as [`Self::at`] does, in the folder named
    /// after `prefix` in the system temp dir that only the current user can
    /// write to. The folder is created if missing.
    ///
    /// - On Unix, it's `{temp}/{prefix}-{uid}`, created with mode `0o700`,
    ///   since the temp dir can be shared by every user (`/tmp` on Linux). An
    ///   existing path must be a directory, not a symlink, owned by the
    ///   current user, and not writable by anyone else; otherwise this returns
    ///   an error, so another user can't plant files there.
    /// - On Windows, it's `{temp}\{prefix}`: the temp dir is already the
    ///   user's own.
    ///
    /// # Errors
    ///
    /// Returns an error if the folder can't be created, or, on Unix, if the
    /// existing path isn't a directory only the current user can write to,
    /// and otherwise as [`Self::at`] does.
    pub fn at_user_temp_dir(self, prefix: &str) -> io::Result<PathBuf> {
        self.at(user_temp_dir(prefix)?)
    }

    /// Materialize the artifact in `dir` under a content-addressed filename,
    /// writing it if missing.
    ///
    /// Returns the final path. If the target already exists, no I/O beyond the
    /// stat is performed: only the current user can write to `dir`, so the file
    /// is the one an earlier call wrote.
    ///
    /// # Preconditions
    ///
    /// `dir` must already exist and be writable only by the current user, as
    /// [`Self::at_user_temp_dir`]'s folder is — this method does not create
    /// it.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory can't be read/written, the stat
    /// fails for any reason other than not-found, or the temp-file rename
    /// fails and the destination still doesn't exist.
    pub fn at(self, dir: impl AsRef<Path>) -> io::Result<PathBuf> {
        let dir = dir.as_ref();
        let path =
            dir.join(format!("{}_{}{}", self.artifact.name, self.artifact.hash, self.suffix));

        // Fast path: the content is assumed correct because the hash is in the
        // filename, so existence is all there is to check. Any stat failure
        // other than not-found (permission denied, I/O error, etc.) propagates.
        if fs::exists(&path)? {
            return Ok(path);
        }

        // Slow path: write to a unique temp file in the same directory, then
        // rename into place atomically. The temp must live in `dir` (not the
        // system temp) so the final rename stays within one filesystem — cross-
        // filesystem rename isn't atomic. `NamedTempFile`'s `Drop` removes the
        // temp on any early return, so we never leak partial files on error.
        #[cfg_attr(not(unix), expect(unused_mut, reason = "only Unix sets the mode"))]
        let mut builder = tempfile::Builder::new();
        #[cfg(unix)]
        if self.executable {
            use std::os::unix::fs::PermissionsExt;
            // `Builder::permissions` sets the mode at open(2) time, so there's
            // no window where the temp exists without the executable bit.
            builder.permissions(fs::Permissions::from_mode(0o755));
        }
        let mut tmp = builder.tempfile_in(dir)?;
        tmp.as_file_mut().write_all(self.artifact.content)?;

        // `persist_noclobber` (link+unlink on Unix, MoveFileExW without
        // REPLACE_EXISTING on Windows) fails atomically if the destination
        // already exists — so two racing processes can't clobber each other
        // mid-write, and the loser sees the error below.
        if let Err(err) = tmp.persist_noclobber(&path) {
            // If another process won the race and the destination now exists,
            // treat that as success; `err.file` drops here, cleaning up our
            // temp. Otherwise propagate the original error.
            if !fs::exists(&path)? {
                return Err(err.error);
            }
        }
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_dir_is_created_once() {
        let temp = tempfile::tempdir().unwrap();
        let dir = user_dir_in(temp.path(), "artifacts").unwrap();
        assert_eq!(dir.parent(), Some(temp.path()));
        assert!(dir.is_dir());
        assert_eq!(user_dir_in(temp.path(), "artifacts").unwrap(), dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }

    #[cfg(unix)]
    #[test]
    fn user_dir_rejects_a_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let link = temp.path().join(format!("artifacts-{}", current_uid()));
        std::os::unix::fs::symlink(elsewhere.path(), link).unwrap();
        let err = user_dir_in(temp.path(), "artifacts").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[cfg(unix)]
    #[test]
    fn user_dir_rejects_a_dir_others_can_write_to() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(format!("artifacts-{}", current_uid()));
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        let err = user_dir_in(temp.path(), "artifacts").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }
}
