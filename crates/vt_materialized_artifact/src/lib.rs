//! Materialize a compile-time–embedded file to disk on demand.
//!
//! Some APIs need a file on disk — `LoadLibrary`, `dlopen`, and helper
//! binaries have to exist as actual files — but we want to ship a single
//! executable. `vt_materialized_artifact` embeds the file content as a
//! `&'static [u8]` at compile time via the [`artifact!`] macro (same as
//! `include_bytes!`), and [`Materialize::at`] writes it out to disk when first
//! needed — that materialization step is the value-add over a bare
//! `include_bytes!`.
//!
//! Materialized files are named `{name}_{hash}{suffix}` in the caller-chosen
//! directory, normally the current user's own folder in the system temp dir
//! (on Unix, falling back to the user cache directory if the temp dir is
//! unwritable or mounted `noexec`; see [`Materialize::at_user_temp_dir`]).
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

#[cfg(unix)]
use std::ffi::{CString, OsString};
use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct CandidateBase {
    path: PathBuf,
    create_if_missing: bool,
}

#[cfg(unix)]
fn candidate_bases() -> Vec<CandidateBase> {
    #[cfg(target_vendor = "apple")]
    {
        candidate_bases_from_apple(env::temp_dir(), env::var_os("HOME"))
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        candidate_bases_from_xdg(
            env::temp_dir(),
            env::var_os("XDG_CACHE_HOME"),
            env::var_os("HOME"),
        )
    }
}

#[cfg(unix)]
fn build_candidate_bases(temp_dir: PathBuf, cache_dir: Option<PathBuf>) -> Vec<CandidateBase> {
    let mut candidates = Vec::with_capacity(2);
    let mut push_unique = |path: PathBuf, create_if_missing: bool| {
        if !path.as_os_str().is_empty()
            && !candidates.iter().any(|c: &CandidateBase| c.path == path)
        {
            candidates.push(CandidateBase { path, create_if_missing });
        }
    };

    // 1. System temp dir (respects `TMPDIR`).
    push_unique(temp_dir, false);

    // 2. Per-user cache directory (created if missing).
    if let Some(cache_dir) = cache_dir {
        push_unique(cache_dir, true);
    }

    candidates
}

#[cfg(all(unix, any(target_vendor = "apple", test)))]
fn candidate_bases_from_apple(temp_dir: PathBuf, home: Option<OsString>) -> Vec<CandidateBase> {
    let cache_dir = home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|h| h.join("Library").join("Caches"));
    build_candidate_bases(temp_dir, cache_dir)
}

#[cfg(all(unix, any(not(target_vendor = "apple"), test)))]
fn candidate_bases_from_xdg(
    temp_dir: PathBuf,
    xdg_cache_home: Option<OsString>,
    home: Option<OsString>,
) -> Vec<CandidateBase> {
    let cache_dir = xdg_cache_home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(PathBuf::from).filter(|p| p.is_absolute()).map(|h| h.join(".cache")));
    build_candidate_bases(temp_dir, cache_dir)
}

#[cfg(unix)]
fn path_to_cstring(path: &Path) -> io::Result<CString> {
    use std::os::unix::ffi::OsStrExt;

    CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path contains interior NUL byte: {}", path.display()),
        )
    })
}

#[cfg(unix)]
fn nearest_existing_ancestor(path: &Path) -> Option<&Path> {
    let mut current = path.parent();
    while let Some(ancestor) = current {
        if fs::symlink_metadata(ancestor).is_ok() {
            return Some(ancestor);
        }
        current = ancestor.parent();
    }
    None
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "openbsd"
))]
trait MountFlags {
    fn has_mnt_flag(self, flag: libc::c_int) -> bool;
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "openbsd"
))]
impl MountFlags for u32 {
    fn has_mnt_flag(self, flag: libc::c_int) -> bool {
        Self::try_from(flag).is_ok_and(|f| self & f != 0)
    }
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "openbsd"
))]
impl MountFlags for u64 {
    fn has_mnt_flag(self, flag: libc::c_int) -> bool {
        Self::try_from(flag).is_ok_and(|f| self & f != 0)
    }
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "openbsd"
))]
impl MountFlags for i32 {
    fn has_mnt_flag(self, flag: libc::c_int) -> bool {
        self & flag != 0
    }
}

/// Returns `Ok(false)` if `path` resides on a filesystem mounted `noexec` (or
/// if the OS/filesystem rejects the mount-flag query with `ENOSYS` or
/// `EOPNOTSUPP`), `Ok(true)` if execution is allowed, or `Err` if querying the
/// path fails for another reason.
#[cfg(unix)]
fn is_dir_executable(path: &Path) -> io::Result<bool> {
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "emscripten",
        target_os = "hurd"
    ))]
    {
        let cpath = path_to_cstring(path)?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: `cpath` is a valid NUL-terminated path and `stat` is a valid
        // writable pointer to a `libc::statvfs` struct.
        if unsafe { libc::statvfs(cpath.as_ptr(), stat.as_mut_ptr()) } != 0 {
            let err = io::Error::last_os_error();
            if matches!(err.raw_os_error(), Some(libc::ENOSYS | libc::EOPNOTSUPP)) {
                return Ok(false);
            }
            return Err(err);
        }
        // SAFETY: `libc::statvfs` returned 0, so `stat` is initialized.
        let stat = unsafe { stat.assume_init() };
        Ok(stat.f_flag & libc::ST_NOEXEC == 0)
    }

    #[cfg(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd"
    ))]
    {
        let cpath = path_to_cstring(path)?;
        let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: `cpath` is a valid NUL-terminated path and `stat` is a valid
        // writable pointer to a `libc::statfs` struct.
        if unsafe { libc::statfs(cpath.as_ptr(), stat.as_mut_ptr()) } != 0 {
            let err = io::Error::last_os_error();
            if matches!(err.raw_os_error(), Some(libc::ENOSYS | libc::EOPNOTSUPP)) {
                return Ok(false);
            }
            return Err(err);
        }
        // SAFETY: `libc::statfs` returned 0, so `stat` is initialized.
        let stat = unsafe { stat.assume_init() };
        Ok(!stat.f_flags.has_mnt_flag(libc::MNT_NOEXEC))
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "emscripten",
        target_os = "hurd",
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd"
    )))]
    {
        fs::metadata(path).map(|_| true)
    }
}

#[cfg(unix)]
enum CandidateFailure {
    Noexec(String),
    Io { err: io::Error, detail: String },
}

#[cfg(unix)]
impl CandidateFailure {
    fn io(path: &Path, err: io::Error) -> Self {
        let detail = format!("{}: {err}", path.display());
        Self::Io { err, detail }
    }
}

#[cfg(unix)]
fn check_dir_executable<FD>(
    checked_path: &Path,
    reported_path: &Path,
    is_dir_executable: &mut FD,
) -> Result<(), CandidateFailure>
where
    FD: FnMut(&Path) -> io::Result<bool>,
{
    match is_dir_executable(checked_path) {
        Ok(true) => Ok(()),
        Ok(false) => Err(CandidateFailure::Noexec(format!(
            "{}: filesystem is mounted noexec",
            reported_path.display()
        ))),
        Err(err) => Err(CandidateFailure::io(reported_path, err)),
    }
}

/// Returns `Ok(true)` if the current process has execute permission bits on
/// `path` via `access(2)` with `X_OK`, `Ok(false)` on permission denied, or
/// `Err` on other OS errors. Mount-level `noexec` enforcement is checked
/// separately via [`is_dir_executable`].
#[cfg(unix)]
fn is_file_executable(path: &Path) -> io::Result<bool> {
    let cpath = path_to_cstring(path)?;
    // SAFETY: `cpath` is a valid NUL-terminated path.
    if unsafe { libc::access(cpath.as_ptr(), libc::X_OK) } == 0 {
        Ok(true)
    } else {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::PermissionDenied { Ok(false) } else { Err(err) }
    }
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
fn user_temp_dir(prefix: &str) -> io::Result<PathBuf> {
    user_dir_in(&env::temp_dir(), prefix)
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
    /// after `prefix` inside the system temp dir that only the current user can
    /// write to (on Unix, falling back to the user cache directory when the
    /// temp dir is unusable).
    ///
    /// - On Unix, each candidate base directory hosts `{base}/{prefix}-{uid}`,
    ///   created with mode `0o700`, since temp dirs can be shared by every user
    ///   (`/tmp` on Linux). An existing path must be a directory, not a
    ///   symlink, owned by the current user, and not writable by anyone else;
    ///   otherwise it is rejected. Every candidate is also checked for `noexec`
    ///   mounts (`statvfs`/`statfs`) so both executables and dynamically loaded
    ///   shared libraries (`.node`, `.dylib`, `.so`) avoid `noexec` filesystems
    ///   and all artifacts share one directory. If [`env::temp_dir()`] is
    ///   unwritable, untrusted, or mounted `noexec`, this falls back to the
    ///   user cache directory (`$HOME/Library/Caches` on Apple platforms, or
    ///   `$XDG_CACHE_HOME` / `$HOME/.cache` on other Unix platforms). When
    ///   [`Self::executable`] is set, the materialized file is also verified
    ///   with `access(X_OK)`.
    /// - On Windows, the folder is `{temp_dir}\{prefix}`; the system temp dir
    ///   (`%LOCALAPPDATA%\Temp`) is already per-user.
    ///
    /// # Errors
    ///
    /// On Unix, returns an error if every candidate directory fails (cannot be
    /// created, fails per-user ownership/permission checks, is mounted
    /// `noexec`, or fails [`Self::at`]). On Windows, returns an error if the
    /// folder can't be created or [`Self::at`] fails.
    pub fn at_user_temp_dir(self, prefix: &str) -> io::Result<PathBuf> {
        #[cfg(unix)]
        {
            let candidates = candidate_bases();
            self.materialize_in_candidates(
                prefix,
                &candidates,
                is_dir_executable,
                is_file_executable,
            )
        }
        #[cfg(windows)]
        {
            self.at(user_temp_dir(prefix)?)
        }
    }

    #[cfg(unix)]
    fn try_materialize_in_candidate<FD, FF>(
        self,
        prefix: &str,
        candidate: &CandidateBase,
        is_dir_executable: &mut FD,
        is_file_executable: &mut FF,
    ) -> Result<PathBuf, CandidateFailure>
    where
        FD: FnMut(&Path) -> io::Result<bool>,
        FF: FnMut(&Path) -> io::Result<bool>,
    {
        use std::os::unix::fs::DirBuilderExt;

        let base = &candidate.path;

        // Check whether `base` exists and allows execution before creating any
        // user directory or writing any files on a `noexec` mount.
        match fs::symlink_metadata(base) {
            Ok(_) => check_dir_executable(base, base, is_dir_executable)?,
            Err(err) if err.kind() == io::ErrorKind::NotFound && candidate.create_if_missing => {
                let Some(ancestor) = nearest_existing_ancestor(base) else {
                    return Err(CandidateFailure::io(base, err));
                };
                check_dir_executable(ancestor, base, is_dir_executable)?;
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(base)
                    .map_err(|create_err| CandidateFailure::io(base, create_err))?;
            }
            Err(err) => return Err(CandidateFailure::io(base, err)),
        }

        let user_dir = user_dir_in(base, prefix).map_err(|err| CandidateFailure::io(base, err))?;

        // Re-check on the resolved user directory in case it is itself a
        // separate mount point (or `base` was just created above).
        check_dir_executable(&user_dir, &user_dir, is_dir_executable)?;

        let (path, created_by_us) =
            self.at_inner(&user_dir).map_err(|err| CandidateFailure::io(&user_dir, err))?;

        if !self.executable {
            return Ok(path);
        }

        let check = match is_file_executable(&path) {
            Ok(true) => Ok(true),
            Ok(false) if !created_by_us => {
                // An earlier non-executable materialization of the same content
                // may exist without `+x` in this user-owned directory. Restore
                // `0o755` without ever unlinking a shared pre-existing file.
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o755));
                is_file_executable(&path)
            }
            other => other,
        };

        match check {
            Ok(true) => Ok(path),
            Ok(false) => {
                if created_by_us {
                    let _ = fs::remove_file(&path);
                }
                Err(CandidateFailure::Noexec(format!(
                    "{}: materialized file is not executable",
                    path.display()
                )))
            }
            Err(err) => {
                if created_by_us {
                    let _ = fs::remove_file(&path);
                }
                Err(CandidateFailure::io(&path, err))
            }
        }
    }

    #[cfg(unix)]
    fn materialize_in_candidates<FD, FF>(
        self,
        prefix: &str,
        candidates: &[CandidateBase],
        mut is_dir_executable: FD,
        mut is_file_executable: FF,
    ) -> io::Result<PathBuf>
    where
        FD: FnMut(&Path) -> io::Result<bool>,
        FF: FnMut(&Path) -> io::Result<bool>,
    {
        let mut saw_noexec = false;
        let mut last_io_error: Option<io::Error> = None;
        let mut failures: Vec<String> = Vec::new();

        for candidate in candidates {
            match self.try_materialize_in_candidate(
                prefix,
                candidate,
                &mut is_dir_executable,
                &mut is_file_executable,
            ) {
                Ok(path) => return Ok(path),
                Err(CandidateFailure::Noexec(detail)) => {
                    saw_noexec = true;
                    failures.push(detail);
                }
                Err(CandidateFailure::Io { err, detail }) => {
                    failures.push(detail);
                    last_io_error = Some(err);
                }
            }
        }

        let details = failures.join("; ");
        if saw_noexec {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "failed to materialize artifact in any candidate directory (noexec or unwritable): {details}"
                ),
            ));
        }

        if let Some(err) = last_io_error {
            return Err(io::Error::new(
                err.kind(),
                format!("failed to materialize artifact in any candidate directory: {details}"),
            ));
        }

        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no candidate directories available for materialized artifact",
        ))
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
        self.at_inner(dir.as_ref()).map(|(path, _)| path)
    }

    fn at_inner(self, dir: &Path) -> io::Result<(PathBuf, bool)> {
        let path =
            dir.join(format!("{}_{}{}", self.artifact.name, self.artifact.hash, self.suffix));

        // Fast path: the content is assumed correct because the hash is in the
        // filename, so existence is all there is to check. Any stat failure
        // other than not-found (permission denied, I/O error, etc.) propagates.
        if fs::exists(&path)? {
            return Ok((path, false));
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
        #[cfg(unix)]
        if self.executable {
            use std::os::unix::fs::PermissionsExt;
            // `open(2)` masks `Builder::permissions` by the process umask;
            // `fchmod` on the temp file before publishing ensures `+x` even
            // under restrictive umasks (e.g. `0o177`).
            tmp.as_file().set_permissions(fs::Permissions::from_mode(0o755))?;
        }

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
            return Ok((path, false));
        }
        Ok((path, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    const TEST_EXEC_ARTIFACT: Artifact =
        Artifact::__new("helper", b"#!/bin/sh\necho ok\n", "deadbeef");
    #[cfg(unix)]
    const TEST_DATA_ARTIFACT: Artifact = Artifact::__new("addon", b"shared-lib-bytes", "cafebabe");

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

    #[cfg(unix)]
    #[test]
    fn candidate_bases_apple_uses_library_caches_and_ignores_relative() {
        let abs_home = PathBuf::from("/Users/alice");
        let tmp = PathBuf::from("/tmp");

        let candidates =
            candidate_bases_from_apple(tmp.clone(), Some(abs_home.clone().into_os_string()));
        assert_eq!(
            candidates,
            vec![
                CandidateBase { path: tmp.clone(), create_if_missing: false },
                CandidateBase {
                    path: abs_home.join("Library").join("Caches"),
                    create_if_missing: true,
                },
            ]
        );

        let relative_home =
            candidate_bases_from_apple(tmp.clone(), Some(OsString::from("relative/home")));
        assert_eq!(relative_home, vec![CandidateBase { path: tmp, create_if_missing: false }]);
    }

    #[cfg(unix)]
    #[test]
    fn candidate_bases_xdg_prefers_xdg_cache_home_falls_back_to_home_and_deduplicates() {
        let tmp = PathBuf::from("/tmp");
        let custom_cache = PathBuf::from("/custom/cache");
        let home = PathBuf::from("/home/alice");

        let with_xdg = candidate_bases_from_xdg(
            tmp.clone(),
            Some(custom_cache.clone().into_os_string()),
            Some(home.clone().into_os_string()),
        );
        assert_eq!(
            with_xdg,
            vec![
                CandidateBase { path: tmp.clone(), create_if_missing: false },
                CandidateBase { path: custom_cache, create_if_missing: true },
            ]
        );

        let with_home = candidate_bases_from_xdg(
            tmp.clone(),
            Some(OsString::from("relative/cache")),
            Some(home.clone().into_os_string()),
        );
        assert_eq!(
            with_home,
            vec![
                CandidateBase { path: tmp.clone(), create_if_missing: false },
                CandidateBase { path: home.join(".cache"), create_if_missing: true },
            ]
        );

        let deduped = candidate_bases_from_xdg(
            tmp.clone(),
            Some(tmp.clone().into_os_string()),
            Some(home.into_os_string()),
        );
        assert_eq!(deduped, vec![CandidateBase { path: tmp.clone(), create_if_missing: false }]);

        let all_relative =
            candidate_bases_from_xdg(tmp.clone(), None, Some(OsString::from("relative/home")));
        assert_eq!(all_relative, vec![CandidateBase { path: tmp, create_if_missing: false }]);
    }

    #[cfg(unix)]
    #[test]
    fn fallback_creates_missing_cache_dir_for_all_artifacts() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let missing_tmp = root.path().join("missing_tmp");
        let cache_dir = root.path().join("user_cache");
        let candidates = vec![
            CandidateBase { path: missing_tmp, create_if_missing: false },
            CandidateBase { path: cache_dir.clone(), create_if_missing: true },
        ];

        let data_path = TEST_DATA_ARTIFACT
            .materialize()
            .suffix(".node")
            .materialize_in_candidates(
                "vp_test",
                &candidates,
                is_dir_executable,
                is_file_executable,
            )
            .unwrap();

        let exec_path = TEST_EXEC_ARTIFACT
            .materialize()
            .executable()
            .materialize_in_candidates(
                "vp_test",
                &candidates,
                is_dir_executable,
                is_file_executable,
            )
            .unwrap();

        assert!(cache_dir.is_dir());
        assert!(data_path.starts_with(&cache_dir));
        assert!(exec_path.starts_with(&cache_dir));
        assert_eq!(data_path.parent(), exec_path.parent());
        assert_eq!(fs::read(&data_path).unwrap(), b"shared-lib-bytes");
        assert_eq!(fs::read(&exec_path).unwrap(), b"#!/bin/sh\necho ok\n");
        // Non-executable artifact keeps default temp file mode (no execute bit).
        assert_eq!(fs::metadata(&data_path).unwrap().permissions().mode() & 0o111, 0);
        // Executable artifact gets `+x`.
        assert_ne!(fs::metadata(&exec_path).unwrap().permissions().mode() & 0o100, 0);
    }

    #[cfg(unix)]
    #[test]
    fn non_executable_artifact_also_skips_noexec_primary_without_leaving_files() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let primary = root.path().join("noexec_tmp");
        let fallback = root.path().join("cache_fallback");
        fs::create_dir(&primary).unwrap();
        fs::create_dir(&fallback).unwrap();

        let candidates = vec![
            CandidateBase { path: primary.clone(), create_if_missing: false },
            CandidateBase { path: fallback.clone(), create_if_missing: true },
        ];

        let path = TEST_DATA_ARTIFACT
            .materialize()
            .suffix(".dylib")
            .materialize_in_candidates(
                "vp_test",
                &candidates,
                |p| Ok(!p.starts_with(&primary)),
                |_| panic!("non-executable artifact must not invoke access(X_OK)"),
            )
            .unwrap();

        assert!(path.starts_with(&fallback));
        assert_eq!(fs::read(&path).unwrap(), b"shared-lib-bytes");
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o111, 0);
        assert!(fs::read_dir(&primary).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn executable_skips_noexec_primary_without_leaving_files() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let primary = root.path().join("noexec_tmp");
        let fallback = root.path().join("exec_fallback");
        fs::create_dir(&primary).unwrap();
        fs::create_dir(&fallback).unwrap();

        let candidates = vec![
            CandidateBase { path: primary.clone(), create_if_missing: false },
            CandidateBase { path: fallback.clone(), create_if_missing: false },
        ];

        let path = TEST_EXEC_ARTIFACT
            .materialize()
            .executable()
            .materialize_in_candidates(
                "vp_test",
                &candidates,
                |p| Ok(!p.starts_with(&primary)),
                is_file_executable,
            )
            .unwrap();

        assert!(path.starts_with(&fallback));
        assert_eq!(fs::read(&path).unwrap(), b"#!/bin/sh\necho ok\n");
        assert_ne!(fs::metadata(&path).unwrap().permissions().mode() & 0o100, 0);
        // Primary dir was rejected before creating a user dir or writing any files.
        assert!(fs::read_dir(&primary).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn executable_removes_file_and_falls_back_when_file_exec_check_fails() {
        let root = tempfile::tempdir().unwrap();
        let primary = root.path().join("primary");
        let fallback = root.path().join("fallback");
        fs::create_dir(&primary).unwrap();
        fs::create_dir(&fallback).unwrap();

        let candidates = vec![
            CandidateBase { path: primary.clone(), create_if_missing: false },
            CandidateBase { path: fallback.clone(), create_if_missing: false },
        ];

        let path = TEST_EXEC_ARTIFACT
            .materialize()
            .executable()
            .materialize_in_candidates(
                "vp_test",
                &candidates,
                |_| Ok(true),
                |p| Ok(!p.starts_with(&primary)),
            )
            .unwrap();

        assert!(path.starts_with(&fallback));
        assert!(path.exists());
        // The file materialized inside `primary`'s user dir was cleaned up.
        let primary_user_dir = primary.join(format!("vp_test-{}", current_uid()));
        assert!(fs::read_dir(&primary_user_dir).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn executable_falls_back_past_missing_untrusted_and_unwritable_dirs() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("does_not_exist");
        let squatted = root.path().join("squatted");
        let elsewhere = root.path().join("elsewhere");
        let readonly = root.path().join("readonly");
        let fallback = root.path().join("fallback");
        fs::create_dir(&squatted).unwrap();
        fs::create_dir(&elsewhere).unwrap();
        fs::create_dir(&readonly).unwrap();
        fs::create_dir(&fallback).unwrap();

        // Plant a symlink where `user_dir_in` expects its per-user directory.
        std::os::unix::fs::symlink(&elsewhere, squatted.join(format!("vp_test-{}", current_uid())))
            .unwrap();

        let mut candidates = vec![
            CandidateBase { path: missing, create_if_missing: false },
            CandidateBase { path: squatted, create_if_missing: false },
        ];
        // Root bypasses mode 0o555 write restrictions, so only include the
        // read-only candidate when running unprivileged.
        if current_uid() != 0 {
            fs::set_permissions(&readonly, fs::Permissions::from_mode(0o555)).unwrap();
            candidates.push(CandidateBase { path: readonly.clone(), create_if_missing: false });
        }
        candidates.push(CandidateBase { path: fallback.clone(), create_if_missing: false });

        let res = TEST_EXEC_ARTIFACT.materialize().executable().materialize_in_candidates(
            "vp_test",
            &candidates,
            is_dir_executable,
            is_file_executable,
        );

        if current_uid() != 0 {
            fs::set_permissions(&readonly, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let path = res.unwrap();
        assert!(path.starts_with(&fallback));
        assert!(is_file_executable(&path).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn creates_missing_cache_dir_only_when_ancestor_is_executable() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let noexec_home = root.path().join("noexec_home");
        let exec_home = root.path().join("exec_home");
        fs::create_dir(&noexec_home).unwrap();
        fs::create_dir(&exec_home).unwrap();

        let noexec_cache = noexec_home.join(".cache");
        let exec_cache = exec_home.join(".cache");

        let candidates = vec![
            CandidateBase { path: noexec_cache.clone(), create_if_missing: true },
            CandidateBase { path: exec_cache.clone(), create_if_missing: true },
        ];

        let path = TEST_EXEC_ARTIFACT
            .materialize()
            .executable()
            .materialize_in_candidates(
                "vp_test",
                &candidates,
                |p| Ok(!p.starts_with(&noexec_home)),
                is_file_executable,
            )
            .unwrap();

        // `.cache` was NOT created under the `noexec` ancestor.
        assert!(!noexec_cache.exists());
        // `.cache` WAS created with mode 0o700 under the executable ancestor.
        assert!(exec_cache.is_dir());
        assert_eq!(fs::metadata(&exec_cache).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(path.starts_with(&exec_cache));
    }

    #[cfg(unix)]
    #[test]
    fn all_candidates_failed_reports_clear_errors() {
        let root = tempfile::tempdir().unwrap();
        let noexec_dir = root.path().join("noexec");
        fs::create_dir(&noexec_dir).unwrap();
        let missing_dir = root.path().join("missing");

        // When any candidate fails due to `noexec`, report `PermissionDenied`
        // even if a later candidate failed with `NotFound`.
        let err = TEST_EXEC_ARTIFACT
            .materialize()
            .executable()
            .materialize_in_candidates(
                "vp_test",
                &[
                    CandidateBase { path: noexec_dir, create_if_missing: false },
                    CandidateBase { path: missing_dir.clone(), create_if_missing: false },
                ],
                |_| Ok(false),
                is_file_executable,
            )
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("noexec"), "unexpected error: {err}");

        // When no candidate hit `noexec`, preserve the last I/O error kind.
        let io_err = TEST_EXEC_ARTIFACT
            .materialize()
            .executable()
            .materialize_in_candidates(
                "vp_test",
                &[CandidateBase { path: missing_dir, create_if_missing: false }],
                is_dir_executable,
                is_file_executable,
            )
            .unwrap_err();
        assert_eq!(io_err.kind(), io::ErrorKind::NotFound);
    }

    #[cfg(unix)]
    #[test]
    fn real_exec_checks_distinguish_executable_and_non_executable_files() {
        let temp = tempfile::tempdir().unwrap();
        assert!(is_dir_executable(temp.path()).unwrap());

        let exec_path = TEST_EXEC_ARTIFACT.materialize().executable().at(temp.path()).unwrap();
        assert!(is_file_executable(&exec_path).unwrap());

        let non_exec_path = TEST_DATA_ARTIFACT.materialize().at(temp.path()).unwrap();
        assert!(!is_file_executable(&non_exec_path).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn executable_check_failure_never_unlinks_preexisting_shared_file() {
        let root = tempfile::tempdir().unwrap();
        let primary = root.path().join("primary");
        let fallback = root.path().join("fallback");
        fs::create_dir(&primary).unwrap();
        fs::create_dir(&fallback).unwrap();

        let primary_user_dir = user_dir_in(&primary, "vp_test").unwrap();
        let preexisting =
            TEST_EXEC_ARTIFACT.materialize().executable().at(&primary_user_dir).unwrap();
        assert!(preexisting.exists());

        let candidates = vec![
            CandidateBase { path: primary.clone(), create_if_missing: false },
            CandidateBase { path: fallback.clone(), create_if_missing: false },
        ];

        let path = TEST_EXEC_ARTIFACT
            .materialize()
            .executable()
            .materialize_in_candidates(
                "vp_test",
                &candidates,
                |_| Ok(true),
                |p| Ok(!p.starts_with(&primary)),
            )
            .unwrap();

        assert!(path.starts_with(&fallback));
        // Pre-existing shared file in `primary` MUST remain intact.
        assert!(preexisting.exists());
    }
}
