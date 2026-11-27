//! Linux exec resolution and bootstrap serialization shared between `fspy` and
//! `fspy_preload_linux`.

use allocator_api2::{alloc::Allocator, vec::Vec};
use bitflags::bitflags;
use bstr::BStr;
use fspy_nostd::{
    BorrowedFd, CStr, CWD, Error, OwnedFd, RawFd, Result, Thin,
    fs::{
        AccessFlags, Mode, OFlags, faccessat, fcntl_dupfd, fcntl_dupfd_cloexec, fcntl_is_cloexec,
        fstat, openat,
    },
};
use fspy_nostd_alloc::{
    CString,
    fs::{fd_path, format_proc_self_fd, resolve_path_at},
};
use fspy_shared::ipc::{IpcStr, channel::ChannelConf};
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    codec::{decode_b64, encode_b64},
    nostd_exec::{
        Exec,
        open::{open_executable, pread_header},
        shebang::{
            LINUX_MAX_LINE_LEN, LINUX_MAX_RECURSION, ParseShebangOptions, apply_shebang,
            parse_shebang_bytes,
        },
        to_vec_in,
    },
};

/// Environment variable carrying the base64-encoded [`Bootstrap`].
///
/// Passed from `fspy` (or a parent `fspy_preload_linux` process) into a fresh
/// `fspy_preload_linux` image, and stripped before entering the target.
pub const BOOTSTRAP_ENV_NAME: &str = "__FSPY_BOOTSTRAP";

bitflags! {
    /// Flags accepted by Linux `execveat(2)` and [`resolve_exec`].
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct ExecAtFlags: u32 {
        /// Execute the file referred to by `dirfd` when `exec.program` is empty.
        const EMPTY_PATH = linux_raw_sys::general::AT_EMPTY_PATH;
        /// Refuse to follow a trailing symbolic link on the initial lookup.
        const SYMLINK_NOFOLLOW = linux_raw_sys::general::AT_SYMLINK_NOFOLLOW;
    }
}

/// Output of [`resolve_exec`].
///
/// Contains the open final binary descriptor, the original execution name for
/// `AT_EXECFN`, and any extra non-`CLOEXEC` readable script descriptor opened
/// when `execveat(..., AT_EMPTY_PATH)` targeted an `O_PATH` `#!` script.
pub struct ResolvedExec<A: Allocator> {
    pub file: OwnedFd,
    pub execfn: Vec<u8, A>,
    /// Readable non-`CLOEXEC` duplicate of an `O_PATH` shebang script passed to
    /// the interpreter as `/dev/fd/<fd>`. Kept open across `SYS_execve` and
    /// closed on exec failure.
    pub extra_fd: Option<OwnedFd>,
}

/// What the freshly exec'd `fspy_preload_linux` binary should load and run.
#[derive(Debug, SchemaWrite, SchemaRead)]
pub enum Target<'a> {
    /// Initial spawn from `fspy`: `fspy_preload_linux` performs [`resolve_exec`]
    /// and ELF validation itself, reporting the result errno (or `0`) over
    /// `error_pipe_fd`.
    Unresolved { path: &'a IpcStr, error_pipe_fd: RawFd },
    /// Child `execve`/`execveat` handed off as the already-validated open
    /// descriptor, so the new image loads the exact file the call resolved.
    /// `sigmask` is the calling thread's virtualized signal mask prior to exec.
    ResolvedFd { fd: RawFd, execfn: &'a IpcStr, sigmask: u64 },
}

/// Bootstrap configuration passed into `fspy_preload_linux` via
/// [`BOOTSTRAP_ENV_NAME`].
#[derive(Debug, SchemaWrite, SchemaRead)]
pub struct Bootstrap<'a> {
    pub ipc_channel_conf: ChannelConf<'a>,
    pub preload_path: &'a IpcStr,
    pub target: Target<'a>,
}

// Seals the view-only design: every borrowed field points into leaked storage.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Bootstrap<'static>>();
};

/// Serializes `bootstrap` into its unpadded base64 representation, leaked into
/// `allocator`.
///
/// # Panics
///
/// Panics if serialization or base64 encoding fails, which cannot happen for a
/// valid [`Bootstrap`].
#[must_use]
pub fn encode_bootstrap<'a, A: Allocator + Clone + 'a>(
    bootstrap: &Bootstrap<'_>,
    allocator: A,
) -> &'a str {
    encode_b64(bootstrap, allocator)
}

impl<'a> Bootstrap<'a> {
    /// Decodes the [`Bootstrap`] stored in [`BOOTSTRAP_ENV_NAME`] from `envs`,
    /// leaking decoded backing bytes into `allocator`.
    ///
    /// # Panics
    ///
    /// Panics when [`BOOTSTRAP_ENV_NAME`] is missing, malformed base64, or
    /// fails deserialization.
    #[must_use]
    pub fn from_env<A: Allocator + Clone + 'a>(envs: &[CStr<'_, Thin>], allocator: A) -> Self {
        let prefix = BOOTSTRAP_ENV_NAME.as_bytes();
        let encoded = envs
            .iter()
            .find_map(|entry| {
                let bytes = entry.count().as_units();
                bytes.strip_prefix(prefix).and_then(|rest| rest.strip_prefix(b"="))
            })
            .expect("environment variable '__FSPY_BOOTSTRAP' not found");

        decode_b64(encoded, allocator).expect("invalid '__FSPY_BOOTSTRAP' payload")
    }
}

/// Ensures [`BOOTSTRAP_ENV_NAME`] is set to `encoded` on `envs`, overwriting
/// and deduplicating any existing entries.
///
/// # Errors
///
/// Returns [`Error::NOMEM`] if storage cannot be allocated.
pub fn ensure_bootstrap_env<A: Allocator + Clone>(
    envs: &mut Vec<crate::Env<A>, A>,
    encoded: &str,
) -> Result<()> {
    crate::nostd_exec::ensure_env(envs, BOOTSTRAP_ENV_NAME.as_bytes(), encoded.as_bytes())
}

/// Resolves a Linux `execve`/`execveat` target through optional `AT_EMPTY_PATH`
/// handling and recursive `#!` shebang expansion, reporting every read path via
/// `on_path_read`.
///
/// On success, `exec.program` and `exec.args` reflect the final binary, and
/// [`ResolvedExec::file`] holds an open `O_CLOEXEC` descriptor to that binary
/// ready for ELF validation or loading.
///
/// # Errors
///
/// Returns Linux-compatible `errno` values:
/// - [`Error::NOENT`] for an empty pathname without [`ExecAtFlags::EMPTY_PATH`],
///   missing files, or `FD_CLOEXEC` script descriptors under `AT_EMPTY_PATH`.
/// - [`Error::BADF`] for an invalid or missing `dirfd`.
/// - [`Error::ACCES`] for non-regular files or files lacking execute permission.
/// - [`Error::LOOP`] for `AT_SYMLINK_NOFOLLOW` on a symlink or exceeding
///   `LINUX_MAX_RECURSION` (`5`) `#!` hops.
/// - [`Error::NOEXEC`] for malformed `#!` lines.
/// - [`Error::NOMEM`] if allocation fails.
pub fn resolve_exec<A: Allocator + Clone>(
    exec: &mut Exec<A>,
    dirfd: Option<BorrowedFd<'_>>,
    flags: ExecAtFlags,
    mut on_path_read: impl FnMut(&BStr),
) -> Result<ResolvedExec<A>> {
    let allocator = exec.allocator().clone();
    let mut peek_buf = [0u8; LINUX_MAX_LINE_LEN];

    let (execfn, extra_fd, mut depth, mut cur_dirfd, mut cur_nofollow) = if exec.program.is_empty()
    {
        if !flags.contains(ExecAtFlags::EMPTY_PATH) {
            return Err(Error::NOENT);
        }
        match resolve_empty_path(exec, dirfd, &mut peek_buf, &mut on_path_read)? {
            EmptyPathStep::Resolved(resolved) => return Ok(resolved),
            EmptyPathStep::Shebang { execfn, extra_fd } => {
                (execfn, extra_fd, 1usize, Some(CWD), false)
            }
        }
    } else {
        let execfn = to_vec_in(&exec.program, allocator.clone());
        (execfn, None, 0usize, dirfd, flags.contains(ExecAtFlags::SYMLINK_NOFOLLOW))
    };

    loop {
        if exec.program.is_empty() {
            return Err(Error::NOENT);
        }
        let is_abs = exec.program.starts_with(b"/");
        let c_path = bytes_to_cstring(&exec.program, allocator.clone())?;

        // Resolve absolute path for `on_path_read` and final-hop normalization.
        // Tolerate `getcwd` failures (e.g. deleted cwd) so relative execs can
        // still fall back to fd-backed handoff, matching kernel behavior.
        let abs_path =
            resolve_path_at(&allocator, cur_dirfd, c_path.as_c_str().as_thin()).ok().flatten();
        if let Some(resolved) = abs_path {
            on_path_read(BStr::new(resolved.as_units()));
        }

        let open_dirfd = if is_abs { CWD } else { cur_dirfd.ok_or(Error::BADF)? };
        let file = open_executable(open_dirfd, c_path.as_c_str().as_thin(), cur_nofollow)?;
        let n = pread_header(file.as_fd(), &mut peek_buf)?;

        if let Some(shebang) =
            parse_shebang_bytes(&peek_buf[..n], ParseShebangOptions::linux(), allocator.clone())?
        {
            depth += 1;
            if depth > LINUX_MAX_RECURSION {
                return Err(Error::LOOP);
            }
            drop(file);

            // Preserve relative path as `script_arg` when relative to CWD (matching
            // kernel `execve`), or substitute the resolved path for non-CWD `dirfd`.
            let script_arg =
                if is_abs || cur_dirfd.is_some_and(|d| d.as_raw_fd() == CWD.as_raw_fd()) {
                    to_vec_in(&exec.program, allocator.clone())
                } else if let Some(resolved) = abs_path {
                    to_vec_in(resolved.as_units(), allocator.clone())
                } else {
                    to_vec_in(&exec.program, allocator.clone())
                };

            apply_shebang(exec, shebang, script_arg);
            cur_dirfd = Some(CWD);
            cur_nofollow = false;
            continue;
        }

        return Ok(ResolvedExec { file, execfn, extra_fd });
    }
}

enum EmptyPathStep<A: Allocator> {
    Resolved(ResolvedExec<A>),
    Shebang { execfn: Vec<u8, A>, extra_fd: Option<OwnedFd> },
}

fn resolve_empty_path<A: Allocator + Clone>(
    exec: &mut Exec<A>,
    dirfd: Option<BorrowedFd<'_>>,
    peek_buf: &mut [u8; LINUX_MAX_LINE_LEN],
    on_path_read: &mut impl FnMut(&BStr),
) -> Result<EmptyPathStep<A>> {
    let allocator = exec.allocator().clone();
    let Some(fd) = dirfd else {
        return Err(Error::BADF);
    };
    if !fstat(fd)?.is_regular_file() {
        return Err(Error::ACCES);
    }
    let raw_fd = u32::try_from(fd.as_raw_fd()).map_err(|_| Error::BADF)?;
    let mut proc_buf = [0u8; 32];
    let proc_fd = format_proc_self_fd(&mut proc_buf, raw_fd);
    faccessat(CWD, proc_fd, AccessFlags::X_OK)?;

    let resolved_fd_path = fd_path(allocator.clone(), fd)?;
    if let Some(ref p) = resolved_fd_path {
        on_path_read(BStr::new(p.as_units()));
    }

    let (file, n, reopened_from_o_path) = match pread_header(fd, peek_buf) {
        Ok(n) => (fcntl_dupfd_cloexec(fd, 0)?, n, false),
        Err(Error::BADF) => {
            // `fd` may have been opened with `O_PATH`; reopen read-only via `/proc/self/fd`.
            let reopened = openat(
                CWD,
                proc_fd,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )?;
            let n = pread_header(reopened.as_fd(), peek_buf)?;
            (reopened, n, true)
        }
        Err(err) => return Err(err),
    };

    let execfn = resolved_fd_path.map_or_else(
        || format_dev_fd(raw_fd, allocator.clone()),
        fspy_nostd_alloc::CString::into_units,
    );

    let Some(shebang) =
        parse_shebang_bytes(&peek_buf[..n], ParseShebangOptions::linux(), allocator.clone())?
    else {
        return Ok(EmptyPathStep::Resolved(ResolvedExec { file, execfn, extra_fd: None }));
    };

    // Matches Linux `binfmt_script`: an `FD_CLOEXEC` script fd cannot be
    // passed to the interpreter as `/dev/fd/<fd>`.
    if fcntl_is_cloexec(fd)? {
        return Err(Error::NOENT);
    }

    // If `fd` was opened with `O_PATH`, the interpreter cannot read through
    // `/dev/fd/<raw_fd>` (`EBADF`); hand it a non-`CLOEXEC` duplicate of the
    // readable descriptor we reopened above instead.
    let (script_fd_num, extra_fd) = if reopened_from_o_path {
        let readable = fcntl_dupfd(file.as_fd(), 3)?;
        drop(file);
        let readable_num = u32::try_from(readable.as_raw_fd()).map_err(|_| Error::BADF)?;
        (readable_num, Some(readable))
    } else {
        drop(file);
        (raw_fd, None)
    };
    let script_arg = format_dev_fd(script_fd_num, allocator);
    apply_shebang(exec, shebang, script_arg);
    Ok(EmptyPathStep::Shebang { execfn, extra_fd })
}

/// Copies `bytes` into a NUL-terminated [`CString`], rejecting interior NULs
/// with [`Error::NOENT`].
fn bytes_to_cstring<A: Allocator>(
    bytes: &[u8],
    allocator: A,
) -> Result<CString<fspy_nostd::Fat, A>> {
    let mut owned = Vec::new_in(allocator);
    owned.try_reserve_exact(bytes.len() + 1).map_err(|_| Error::NOMEM)?;
    owned.extend_from_slice(bytes);
    owned.push(0);
    CString::from_vec_with_nul(owned).ok_or(Error::NOENT)
}

fn format_dev_fd<A: Allocator>(fd: u32, allocator: A) -> Vec<u8, A> {
    let mut buf = [0u8; 32];
    let proc_fd = format_proc_self_fd(&mut buf, fd).count().as_units();
    let digits = proc_fd.strip_prefix(b"/proc/self/fd/").unwrap_or(proc_fd);
    let mut out = Vec::with_capacity_in(b"/dev/fd/".len() + digits.len(), allocator);
    out.extend_from_slice(b"/dev/fd/");
    out.extend_from_slice(digits);
    out
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::{
        fs::{self, File, Permissions},
        os::unix::{
            ffi::OsStrExt as _,
            fs::{OpenOptionsExt as _, PermissionsExt as _, symlink},
            io::AsRawFd as _,
        },
        path::Path,
    };

    use allocator_api2::alloc::Global;
    use fspy_shared::ipc::channel::channel;

    use super::*;

    fn write_exec(path: &Path, content: &[u8]) {
        fs::write(path, content).unwrap();
        fs::set_permissions(path, Permissions::from_mode(0o755)).unwrap();
    }

    fn make_exec(prog: &[u8], args: &[&[u8]]) -> Exec<Global> {
        let mut a = Vec::new_in(Global);
        for arg in args {
            a.push(to_vec_in(arg, Global));
        }
        Exec { program: to_vec_in(prog, Global), args: a, envs: Vec::new_in(Global) }
    }

    #[test]
    fn bootstrap_round_trips_all_target_variants() {
        let receiver = channel(1 << 30, Global).unwrap();
        let preload = IpcStr::from_bytes(b"/tmp/fspy_preload");
        let target_path = IpcStr::from_bytes(b"/usr/bin/env");
        let execfn = IpcStr::from_bytes(b"env");

        let cases = [
            Target::Unresolved { path: target_path, error_pipe_fd: 7 },
            Target::ResolvedFd { fd: 9, execfn, sigmask: 0x5678 },
        ];

        for target in cases {
            let bump = fspy_nostd_alloc::pooled_bump();
            let bootstrap =
                Bootstrap { ipc_channel_conf: receiver.conf(), preload_path: preload, target };
            let encoded = encode_bootstrap(&bootstrap, &bump);

            let mut env_bytes = std::vec::Vec::new();
            env_bytes.extend_from_slice(BOOTSTRAP_ENV_NAME.as_bytes());
            env_bytes.push(b'=');
            env_bytes.extend_from_slice(encoded.as_bytes());
            env_bytes.push(0);
            let other = b"PATH=/usr/bin\0";
            // SAFETY: both slices are NUL-terminated for the duration of `from_env`.
            let envs = unsafe {
                [CStr::<Thin>::from_ptr(other.as_ptr()), CStr::<Thin>::from_ptr(env_bytes.as_ptr())]
            };

            let decoded = Bootstrap::from_env(&envs, &bump);
            assert_eq!(decoded.preload_path.as_bytes(), b"/tmp/fspy_preload");
            match (&bootstrap.target, &decoded.target) {
                (
                    Target::Unresolved { path: p1, error_pipe_fd: f1 },
                    Target::Unresolved { path: p2, error_pipe_fd: f2 },
                ) => {
                    assert_eq!(p1.as_bytes(), p2.as_bytes());
                    assert_eq!(f1, f2);
                }
                (
                    Target::ResolvedFd { fd: f1, execfn: e1, sigmask: m1 },
                    Target::ResolvedFd { fd: f2, execfn: e2, sigmask: m2 },
                ) => {
                    assert_eq!(f1, f2);
                    assert_eq!(e1.as_bytes(), e2.as_bytes());
                    assert_eq!(m1, m2);
                }
                _ => panic!("mismatched target variant"),
            }
        }
    }

    #[test]
    fn resolve_exec_handles_chained_shebangs_and_loop_limit() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        write_exec(&bin, b"\x7fELFnot-a-script");

        // Chain of 5 shebangs -> bin (allowed: depth 5 <= LINUX_MAX_RECURSION).
        let s5 = dir.path().join("s5");
        let s4 = dir.path().join("s4");
        let s3 = dir.path().join("s3");
        let s2 = dir.path().join("s2");
        let s1 = dir.path().join("s1");
        let s0 = dir.path().join("s0");
        write_exec(&s5, format!("#!{} a5\n", bin.display()).as_bytes());
        write_exec(&s4, format!("#!{} a4\n", s5.display()).as_bytes());
        write_exec(&s3, format!("#!{} a3\n", s4.display()).as_bytes());
        write_exec(&s2, format!("#!{} a2\n", s3.display()).as_bytes());
        write_exec(&s1, format!("#!{} a1\n", s2.display()).as_bytes());
        write_exec(&s0, format!("#!{} a0\n", s1.display()).as_bytes());

        let mut seen = std::vec::Vec::new();
        let mut exec = make_exec(s1.as_os_str().as_bytes(), &[b"orig0", b"user"]);
        let resolved =
            resolve_exec(&mut exec, Some(CWD), ExecAtFlags::empty(), |p| seen.push(p.to_vec()))
                .unwrap();
        assert_eq!(resolved.execfn.as_slice(), s1.as_os_str().as_bytes());
        assert_eq!(exec.program.as_slice(), bin.as_os_str().as_bytes());
        assert_eq!(seen.len(), 6);

        // Chain of 6 shebangs exceeds LINUX_MAX_RECURSION -> ELOOP.
        let mut exec = make_exec(s0.as_os_str().as_bytes(), &[b"orig0"]);
        let err = resolve_exec(&mut exec, Some(CWD), ExecAtFlags::empty(), |_| {}).err().unwrap();
        assert_eq!(err, Error::LOOP);
    }

    #[test]
    fn resolve_exec_handles_empty_path_and_nofollow() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        write_exec(&bin, b"\x7fELFdummy");
        let link = dir.path().join("link");
        symlink(&bin, &link).unwrap();

        // Empty path without EMPTY_PATH -> ENOENT without reading anything.
        let mut reads = 0usize;
        let mut exec = make_exec(b"", &[]);
        assert_eq!(
            resolve_exec(&mut exec, Some(CWD), ExecAtFlags::empty(), |_| reads += 1).err(),
            Some(Error::NOENT)
        );
        assert_eq!(reads, 0);

        // SYMLINK_NOFOLLOW on symlink -> ELOOP.
        let mut exec = make_exec(link.as_os_str().as_bytes(), &[b"link"]);
        assert_eq!(
            resolve_exec(&mut exec, Some(CWD), ExecAtFlags::SYMLINK_NOFOLLOW, |_| {}).err(),
            Some(Error::LOOP)
        );

        // AT_EMPTY_PATH on open executable fd resolves.
        let file = File::open(&bin).unwrap();
        // SAFETY: `file` remains open for the call.
        let fd = unsafe { BorrowedFd::borrow_raw(file.as_raw_fd()) };
        let mut exec = make_exec(b"", &[b"bin"]);
        assert!(resolve_exec(&mut exec, Some(fd), ExecAtFlags::EMPTY_PATH, |_| {}).is_ok());

        // AT_EMPTY_PATH on CLOEXEC shebang script -> ENOENT.
        let script = dir.path().join("script.sh");
        write_exec(&script, format!("#!{}\n", bin.display()).as_bytes());
        let script_file = File::open(&script).unwrap(); // Rust opens with O_CLOEXEC.
        // SAFETY: `script_file` remains open for the call.
        let script_fd = unsafe { BorrowedFd::borrow_raw(script_file.as_raw_fd()) };
        let mut exec = make_exec(b"", &[]);
        assert_eq!(
            resolve_exec(&mut exec, Some(script_fd), ExecAtFlags::EMPTY_PATH, |_| {}).err(),
            Some(Error::NOENT)
        );

        // AT_EMPTY_PATH on non-CLOEXEC O_PATH shebang script reopens readable
        // and passes `/dev/fd/<extra_fd>` (non-CLOEXEC) to the interpreter.
        let o_path_cloexec = fs::OpenOptions::new()
            .read(true)
            .custom_flags(linux_raw_sys::general::O_PATH.cast_signed())
            .open(&script)
            .unwrap();
        // SAFETY: `o_path_cloexec` remains open for the `fcntl_dupfd` call.
        let o_path_borrowed = unsafe { BorrowedFd::borrow_raw(o_path_cloexec.as_raw_fd()) };
        let o_path_owned = fcntl_dupfd(o_path_borrowed, 3).unwrap();
        let mut exec = make_exec(b"", &[]);
        let resolved =
            resolve_exec(&mut exec, Some(o_path_owned.as_fd()), ExecAtFlags::EMPTY_PATH, |_| {})
                .unwrap();
        let extra = resolved.extra_fd.as_ref().expect("expected reopened readable script fd");
        assert!(!fcntl_is_cloexec(extra.as_fd()).unwrap());
        let expected_dev_fd = format!("/dev/fd/{}", extra.as_raw_fd());
        assert_eq!(exec.args.last().unwrap().as_slice(), expected_dev_fd.as_bytes());
    }
}
