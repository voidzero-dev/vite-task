//! Materialize a compile-time–embedded file to disk on demand.
//!
//! Some APIs need a file on disk — `LoadLibrary` and `LD_PRELOAD` take a
//! path, and helper binaries have to exist as actual files to be spawned —
//! but we want to ship a single executable. `materialized_artifact` embeds
//! the file content as a `&'static [u8]` at compile time via the
//! [`artifact!`] macro (same as `include_bytes!`), and [`Materialize::at`]
//! writes it out to disk when first needed — that materialization step is
//! the value-add over a bare `include_bytes!`.
//!
//! Materialized files are named `{name}_{hash}{suffix}` in the caller-chosen
//! directory. The hash (computed at build time by
//! `materialized_artifact_build::register`) gives three properties without
//! any coordination between processes:
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
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// A file embedded into the executable at compile time.
///
/// Construct with [`artifact!`]; materialize to disk via
/// [`Artifact::materialize`] + [`Materialize::at`]. See the [crate docs] for
/// the design rationale.
///
/// [crate docs]: crate
#[derive(Clone, Copy)]
pub struct Artifact {
    name: &'static str,
    content: &'static [u8],
    hash: &'static str,
}

/// Construct an [`Artifact`] from the env vars published by a build script
/// via `materialized_artifact_build::register`.
#[macro_export]
macro_rules! artifact {
    ($name:literal) => {
        $crate::Artifact::__new(
            $name,
            ::core::include_bytes!(::core::env!(::core::concat!(
                "MATERIALIZED_ARTIFACT_",
                $name,
                "_PATH"
            ))),
            ::core::env!(::core::concat!("MATERIALIZED_ARTIFACT_", $name, "_HASH")),
        )
    };
}

impl Artifact {
    #[doc(hidden)]
    #[must_use]
    pub const fn __new(name: &'static str, content: &'static [u8], hash: &'static str) -> Self {
        Self { name, content, hash }
    }

    /// Start a fluent materialize chain. Supply optional [`Materialize::suffix`]
    /// / [`Materialize::executable`] knobs, then terminate with
    /// [`Materialize::at`].
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
/// [`Materialize::at`] to write the file.
#[derive(Clone, Copy)]
#[must_use = "materialize() only configures — call .at(dir) to write the file"]
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
    /// Windows where the filesystem has no executable bit).
    #[cfg_attr(not(unix), expect(unused_mut, reason = "executable is Unix-only"))]
    pub const fn executable(mut self) -> Self {
        #[cfg(unix)]
        {
            self.executable = true;
        }
        self
    }

    /// Materialize the artifact in `dir` under a content-addressed filename,
    /// writing it if missing. On Unix, newly created files get `0o755` when
    /// [`Materialize::executable`] was called and `0o644` otherwise, and an
    /// existing file's mode is reconciled if it drifted.
    ///
    /// Returns the final path. If the target already exists and its mode
    /// already matches, no I/O beyond the stat is performed.
    ///
    /// # Preconditions
    ///
    /// `dir` must already exist — this method does not create it.
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

        #[cfg(unix)]
        let want_mode: u32 = if self.executable { 0o755 } else { 0o644 };

        // Fast path: one stat tells us both whether the file exists and,
        // on Unix, what its permission bits are. The content is assumed
        // correct because the hash is in the filename, so there is nothing
        // else to verify.
        match fs::metadata(&path) {
            #[cfg(unix)]
            Ok(meta) => {
                use std::os::unix::fs::PermissionsExt;
                // Reconcile a drifted mode (e.g. someone chmod'd it away)
                // but skip the syscall when it already matches.
                if meta.permissions().mode() & 0o777 != want_mode {
                    fs::set_permissions(&path, fs::Permissions::from_mode(want_mode))?;
                }
                return Ok(path);
            }
            // On non-Unix there is no mode to reconcile; existence alone is
            // enough to declare success.
            #[cfg(not(unix))]
            Ok(_) => return Ok(path),
            // Not found: fall through to the create-and-rename path.
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            // Any other stat failure (permission denied, I/O error, etc.)
            // propagates — we can't reason about what's on disk.
            Err(err) => return Err(err),
        }

        // Slow path: write to a unique temp file in the same directory, then
        // rename into place atomically. The temp must live in `dir` (not the
        // system temp) so the final rename stays within one filesystem — cross-
        // filesystem rename isn't atomic. `NamedTempFile`'s `Drop` removes the
        // temp on any early return, so we never leak partial files on error.
        #[cfg(unix)]
        let mut tmp = {
            use std::os::unix::fs::PermissionsExt;
            // `Builder::permissions` sets the mode at open(2) time, so there's
            // no window where the temp exists with the wrong bits.
            tempfile::Builder::new()
                .permissions(fs::Permissions::from_mode(want_mode))
                .tempfile_in(dir)?
        };
        #[cfg(not(unix))]
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
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
