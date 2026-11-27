//! Linux-specific syscall coverage tests for `fspy_preload_linux`:
//! - `execveat` variants (`AT_SYMLINK_NOFOLLOW`, non-CWD `dirfd`, `O_PATH` +
//!   `AT_EMPTY_PATH` on both ELF binaries and `#!` scripts, `AT_EMPTY_PATH` on
//!   non-`CLOEXEC` and `CLOEXEC` `#!` scripts)
//! - `newfstatat`/`fstatat`, `statx`, and `faccessat2` with valid `fd` + `AT_EMPTY_PATH`
//! - `access`/`faccessat`/`faccessat2` and `mkdirat`/`renameat`/`linkat`/`symlinkat`/`openat2`
//!   with non-CWD `dirfd` + relative path
//! - `openat2` invalid `how_ptr` / invalid `size` (`EFAULT` / `EINVAL`)
//! - `openat(O_RDONLY | O_CREAT)` / `openat(O_RDONLY | O_TRUNC)` recording `ReadWrite`,
//!   and `renameat2(RENAME_EXCHANGE)` recording both paths
//! - Legacy x86-64 `SYS_open`, `SYS_stat`, and `SYS_lstat`
#![cfg(target_os = "linux")]

mod test_utils;

use std::{
    fs::{self, File, Permissions},
    os::unix::fs::{PermissionsExt as _, symlink},
    path::Path,
};

use fspy::PathAccess;
use test_log::test;
#[cfg(target_arch = "x86_64")]
use test_utils::assert_includes;
use test_utils::{assert_contains, test_bin_path};

/// `STATX_BASIC_STATS`. Not defined in `libc` on every musl target.
const STATX_BASIC_STATS: libc::c_uint = 0x7ff;

/// `RENAME_EXCHANGE`. Not defined in `libc` on every musl target.
const RENAME_EXCHANGE: libc::c_uint = 2;

/// Kernel `struct statx` (256 bytes, 8-byte aligned).
#[repr(C, align(8))]
struct StatxBuf([u8; 256]);

/// Kernel `struct open_how` (24 bytes).
#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

fn errno() -> libc::c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap()
}

fn write_exec(path: &Path, content: &[u8]) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, Permissions::from_mode(0o755)).unwrap();
}

#[test(tokio::test)]
async fn execveat_nofollow_dirfd_o_path_and_empty_path_scripts() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let root = tmp_dir.path().canonicalize()?;

    // Copy the static test binary into `root` so we can exec it via non-CWD
    // `dirfd` + relative path and via `O_PATH` + `AT_EMPTY_PATH`.
    let rel_bin = root.join("rel_bin");
    fs::copy(test_bin_path(), &rel_bin)?;
    fs::set_permissions(&rel_bin, Permissions::from_mode(0o755))?;

    let bin_symlink = root.join("bin_link");
    symlink(&rel_bin, &bin_symlink)?;

    let probe_rel_bin = Path::new("/fspy-execveat-rel-bin-probe");
    let probe_rel_script = Path::new("/fspy-execveat-rel-script-probe");
    let probe_opath_bin = Path::new("/fspy-execveat-opath-bin-probe");
    let probe_opath_script = Path::new("/fspy-execveat-opath-script-probe");
    let probe_empty_script = Path::new("/fspy-execveat-empty-script-probe");

    let rel_script = root.join("rel_script.sh");
    write_exec(
        &rel_script,
        format!("#!/bin/sh\n{} open_read {}\n", rel_bin.display(), probe_rel_script.display())
            .as_bytes(),
    );

    let opath_script = root.join("opath_script.sh");
    write_exec(
        &opath_script,
        format!("#!/bin/sh\n{} open_read {}\n", rel_bin.display(), probe_opath_script.display())
            .as_bytes(),
    );

    let empty_script = root.join("empty_script.sh");
    write_exec(
        &empty_script,
        format!("#!/bin/sh\n{} open_read {}\n", rel_bin.display(), probe_empty_script.display())
            .as_bytes(),
    );

    let args = (
        root.to_str().unwrap().to_owned(),
        probe_rel_bin.to_str().unwrap().to_owned(),
        probe_opath_bin.to_str().unwrap().to_owned(),
    );

    let accesses =
        track_fn!(args, |(root_str, probe_rel_bin, probe_opath_bin): (String, String, String)| {
            use std::{ffi::CString, os::fd::AsRawFd as _};

            fn run_execveat_in_child(
                dirfd: libc::c_int,
                path: &std::ffi::CStr,
                argv: &[*const libc::c_char],
                flags: libc::c_int,
            ) -> libc::c_int {
                let envp = [std::ptr::null::<libc::c_char>()];
                // SAFETY: `fork()` followed by async-signal-safe `execveat` + `_exit`.
                let pid = unsafe { libc::fork() };
                assert!(pid >= 0, "fork failed");
                if pid == 0 {
                    // SAFETY: arguments are valid NUL/NULL-terminated C strings/arrays.
                    unsafe {
                        libc::syscall(
                            libc::SYS_execveat,
                            dirfd,
                            path.as_ptr(),
                            argv.as_ptr(),
                            envp.as_ptr(),
                            flags,
                        );
                        libc::_exit(127);
                    }
                }
                let mut status: libc::c_int = 0;
                // SAFETY: `pid` is our child and `status` is writable.
                assert_eq!(unsafe { libc::waitpid(pid, &raw mut status, 0) }, pid);
                assert!(
                    libc::WIFEXITED(status),
                    "execveat child terminated abnormally: status={status}"
                );
                libc::WEXITSTATUS(status)
            }

            // Ensure CWD is `/` so relative names MUST resolve against `dirfd`.
            std::env::set_current_dir("/").unwrap();
            let dir = File::open(&root_str).unwrap();
            let dirfd = dir.as_raw_fd();

            // 1. `AT_SYMLINK_NOFOLLOW` on a symlink -> `ELOOP`.
            let argv_link = [c"bin_link".as_ptr(), std::ptr::null()];
            let envp_empty = [std::ptr::null::<libc::c_char>()];
            // SAFETY: valid arguments to `execveat` expected to fail with `ELOOP`.
            let rc = unsafe {
                libc::syscall(
                    libc::SYS_execveat,
                    dirfd,
                    c"bin_link".as_ptr(),
                    argv_link.as_ptr(),
                    envp_empty.as_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            assert_eq!(rc, -1, "execveat AT_SYMLINK_NOFOLLOW on symlink should fail");
            assert_eq!(errno(), libc::ELOOP);

            // 2. Non-CWD `dirfd` + relative ELF executable.
            let probe_rel_c = CString::new(probe_rel_bin).unwrap();
            let argv_rel_bin = [
                c"rel_bin".as_ptr(),
                c"open_read".as_ptr(),
                probe_rel_c.as_ptr(),
                std::ptr::null(),
            ];
            assert_eq!(run_execveat_in_child(dirfd, c"rel_bin", &argv_rel_bin, 0), 0);

            // 3. Non-CWD `dirfd` + relative `#!` script.
            let argv_rel_script = [c"rel_script.sh".as_ptr(), std::ptr::null()];
            assert_eq!(run_execveat_in_child(dirfd, c"rel_script.sh", &argv_rel_script, 0), 0);

            // 4. `O_PATH` + `AT_EMPTY_PATH` on an ELF executable.
            // SAFETY: valid path relative to `dirfd`.
            let opath_bin_fd =
                unsafe { libc::openat(dirfd, c"rel_bin".as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
            assert!(opath_bin_fd >= 0, "openat O_PATH bin failed: {}", errno());
            let probe_opath_c = CString::new(probe_opath_bin).unwrap();
            let argv_opath_bin = [
                c"opath_bin".as_ptr(),
                c"open_read".as_ptr(),
                probe_opath_c.as_ptr(),
                std::ptr::null(),
            ];
            assert_eq!(
                run_execveat_in_child(opath_bin_fd, c"", &argv_opath_bin, libc::AT_EMPTY_PATH),
                0
            );
            // SAFETY: closing our owned descriptor.
            unsafe { libc::close(opath_bin_fd) };

            // 5. `O_PATH` (without `O_CLOEXEC`) + `AT_EMPTY_PATH` on a `#!` script.
            // SAFETY: valid path relative to `dirfd`.
            let opath_script_fd =
                unsafe { libc::openat(dirfd, c"opath_script.sh".as_ptr(), libc::O_PATH) };
            assert!(opath_script_fd >= 0, "openat O_PATH script failed: {}", errno());
            let argv_opath_script = [c"opath_script".as_ptr(), std::ptr::null()];
            assert_eq!(
                run_execveat_in_child(
                    opath_script_fd,
                    c"",
                    &argv_opath_script,
                    libc::AT_EMPTY_PATH
                ),
                0
            );
            // SAFETY: closing our owned descriptor.
            unsafe { libc::close(opath_script_fd) };

            // 6. `AT_EMPTY_PATH` on `#!` script: non-`CLOEXEC` succeeds, `CLOEXEC` fails `ENOENT`.
            // SAFETY: valid path relative to `dirfd`.
            let non_cloexec_fd =
                unsafe { libc::openat(dirfd, c"empty_script.sh".as_ptr(), libc::O_RDONLY) };
            assert!(non_cloexec_fd >= 0, "openat non-cloexec script failed: {}", errno());
            let argv_empty_script = [c"empty_script".as_ptr(), std::ptr::null()];
            assert_eq!(
                run_execveat_in_child(non_cloexec_fd, c"", &argv_empty_script, libc::AT_EMPTY_PATH),
                0
            );
            // SAFETY: closing our owned descriptor.
            unsafe { libc::close(non_cloexec_fd) };

            // `File::open` sets `O_CLOEXEC` -> `execveat(AT_EMPTY_PATH)` on `#!` returns `ENOENT`.
            let cloexec_script = File::open(format!("{root_str}/empty_script.sh")).unwrap();
            // SAFETY: valid arguments to `execveat` expected to fail with `ENOENT`.
            let rc_cloexec = unsafe {
                libc::syscall(
                    libc::SYS_execveat,
                    cloexec_script.as_raw_fd(),
                    c"".as_ptr(),
                    argv_empty_script.as_ptr(),
                    envp_empty.as_ptr(),
                    libc::AT_EMPTY_PATH,
                )
            };
            assert_eq!(rc_cloexec, -1, "AT_EMPTY_PATH on CLOEXEC script should fail");
            assert_eq!(errno(), libc::ENOENT);

            // 7. `execveat(AT_FDCWD, "", ..., AT_EMPTY_PATH)` refers to the current
            //    working directory and must return `EACCES` (matching Linux kernel).
            // SAFETY: valid arguments to `execveat` expected to fail with `EACCES`.
            let rc_cwd_empty = unsafe {
                libc::syscall(
                    libc::SYS_execveat,
                    libc::AT_FDCWD,
                    c"".as_ptr(),
                    argv_empty_script.as_ptr(),
                    envp_empty.as_ptr(),
                    libc::AT_EMPTY_PATH,
                )
            };
            assert_eq!(rc_cwd_empty, -1, "execveat(AT_FDCWD, \"\", AT_EMPTY_PATH) should fail");
            assert_eq!(errno(), libc::EACCES);
        })
        .await?;

    assert_contains(&accesses, &rel_bin, PathAccess::Read);
    assert_contains(&accesses, &rel_script, PathAccess::Read);
    assert_contains(&accesses, &opath_script, PathAccess::Read);
    assert_contains(&accesses, &empty_script, PathAccess::Read);
    assert_contains(&accesses, probe_rel_bin, PathAccess::Read);
    assert_contains(&accesses, probe_rel_script, PathAccess::Read);
    assert_contains(&accesses, probe_opath_bin, PathAccess::Read);
    assert_contains(&accesses, probe_opath_script, PathAccess::Read);
    assert_contains(&accesses, probe_empty_script, PathAccess::Read);
    Ok(())
}

#[test(tokio::test)]
async fn empty_path_fstatat_statx_and_faccessat2_record_fd_path() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let root = tmp_dir.path().canonicalize()?;
    let f_fstatat = root.join("empty_fstatat");
    let f_statx = root.join("empty_statx");
    let f_faccessat2 = root.join("empty_faccessat2");
    fs::write(&f_fstatat, b"a")?;
    fs::write(&f_statx, b"b")?;
    fs::write(&f_faccessat2, b"c")?;

    let args = (
        f_fstatat.to_str().unwrap().to_owned(),
        f_statx.to_str().unwrap().to_owned(),
        f_faccessat2.to_str().unwrap().to_owned(),
    );

    let accesses = track_fn!(args, |(p1, p2, p3): (String, String, String)| {
        use std::os::fd::AsRawFd as _;

        // Open each file via `O_PATH` before invoking the `AT_EMPTY_PATH`
        // syscalls (the `openat` itself also records Read, and we additionally
        // verify the kernel calls succeed / behave as expected under seccomp).
        let file1 = File::open(&p1).unwrap();
        let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `file1` is open and `st` is writable.
        let rc_fstatat = unsafe {
            libc::fstatat(file1.as_raw_fd(), c"".as_ptr(), st.as_mut_ptr(), libc::AT_EMPTY_PATH)
        };
        assert_eq!(rc_fstatat, 0);

        let file2 = File::open(&p2).unwrap();
        let mut stx = StatxBuf([0; 256]);
        // SAFETY: `file2` is open and `stx` is writable.
        let rc_statx = unsafe {
            libc::syscall(
                libc::SYS_statx,
                file2.as_raw_fd(),
                c"".as_ptr(),
                libc::AT_EMPTY_PATH,
                STATX_BASIC_STATS,
                core::ptr::from_mut(&mut stx),
            )
        };
        assert_eq!(rc_statx, 0);

        let file3 = File::open(&p3).unwrap();
        // SAFETY: `file3` is open and `c""` is a valid C string.
        // Kernels before 6.13 reject `AT_EMPTY_PATH` in `faccessat2` with
        // `EINVAL` (and pre-5.8 return `ENOSYS`), while `fspy_preload_linux`
        // still records the descriptor path before replaying.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_faccessat2,
                file3.as_raw_fd(),
                c"".as_ptr(),
                libc::F_OK,
                libc::AT_EMPTY_PATH,
            )
        };
        assert!(
            rc == 0 || matches!(errno(), libc::EINVAL | libc::ENOSYS),
            "unexpected faccessat2(AT_EMPTY_PATH) result: rc={rc}, errno={}",
            errno()
        );
    })
    .await?;

    assert_contains(&accesses, &f_fstatat, PathAccess::Read);
    assert_contains(&accesses, &f_statx, PathAccess::Read);
    assert_contains(&accesses, &f_faccessat2, PathAccess::Read);
    Ok(())
}

#[test(tokio::test)]
async fn access_and_dirfd_relative_fs_syscalls_are_tracked() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let root = tmp_dir.path().canonicalize()?;

    let access_file = root.join("via_access");
    let faccessat_file = root.join("via_faccessat");
    let faccessat2_file = root.join("via_faccessat2");
    let ren_src = root.join("ren_src");
    let ren_dst = root.join("ren_dst");
    let link_src = root.join("link_src");
    let link_dst = root.join("link_dst");
    let sym_dst = root.join("sym_dst");
    let mkdir_dst = root.join("mkdir_dst");
    let openat2_file = root.join("via_openat2");

    fs::write(&access_file, b"")?;
    fs::write(&faccessat_file, b"")?;
    fs::write(&faccessat2_file, b"")?;
    fs::write(&ren_src, b"")?;
    fs::write(&link_src, b"")?;
    fs::write(&openat2_file, b"")?;

    let root_str = root.to_str().unwrap().to_owned();
    let accesses = track_fn!(root_str, |root_str: String| {
        use std::{ffi::CString, os::fd::AsRawFd as _};

        // Change CWD to `/` so every relative path below MUST resolve against `dirfd`.
        std::env::set_current_dir("/").unwrap();
        let dir = File::open(&root_str).unwrap();
        let dirfd = dir.as_raw_fd();

        let abs_access = CString::new(format!("{root_str}/via_access")).unwrap();
        // SAFETY: `abs_access` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::access(abs_access.as_ptr(), libc::F_OK) }, 0);

        // SAFETY: `dirfd` is open and names are valid NUL-terminated strings.
        unsafe {
            assert_eq!(libc::faccessat(dirfd, c"via_faccessat".as_ptr(), libc::F_OK, 0), 0);

            let rc_fa2 = libc::syscall(
                libc::SYS_faccessat2,
                dirfd,
                c"via_faccessat2".as_ptr(),
                libc::F_OK,
                0,
            );
            assert!(
                rc_fa2 == 0 || errno() == libc::ENOSYS,
                "faccessat2 failed: rc={rc_fa2}, errno={}",
                errno()
            );

            assert_eq!(libc::mkdirat(dirfd, c"mkdir_dst".as_ptr(), 0o755), 0);
            assert_eq!(libc::renameat(dirfd, c"ren_src".as_ptr(), dirfd, c"ren_dst".as_ptr()), 0);
            assert_eq!(
                libc::linkat(dirfd, c"link_src".as_ptr(), dirfd, c"link_dst".as_ptr(), 0),
                0
            );
            assert_eq!(libc::symlinkat(c"target".as_ptr(), dirfd, c"sym_dst".as_ptr()), 0);

            let how =
                OpenHow { flags: u64::try_from(libc::O_RDONLY).unwrap(), mode: 0, resolve: 0 };
            let fd = libc::syscall(
                libc::SYS_openat2,
                dirfd,
                c"via_openat2".as_ptr(),
                &raw const how,
                size_of::<OpenHow>(),
            );
            assert!(fd >= 0, "openat2 failed: errno={}", errno());
            libc::close(libc::c_int::try_from(fd).unwrap());
        }
    })
    .await?;

    assert_contains(&accesses, &access_file, PathAccess::Read);
    assert_contains(&accesses, &faccessat_file, PathAccess::Read);
    assert_contains(&accesses, &faccessat2_file, PathAccess::Read);
    assert_contains(&accesses, &mkdir_dst, PathAccess::Mkdir);
    assert_contains(&accesses, &ren_dst, PathAccess::RenameTarget);
    assert_contains(&accesses, &link_dst, PathAccess::Write);
    assert_contains(&accesses, &sym_dst, PathAccess::Write);
    assert_contains(&accesses, &openat2_file, PathAccess::Read);
    Ok(())
}

#[test(tokio::test)]
async fn openat2_invalid_how_ptr_and_size_return_efault_and_einval() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let file_path = tmp_dir.path().canonicalize()?.join("probe");
    fs::write(&file_path, b"")?;

    let path_str = file_path.to_str().unwrap().to_owned();
    let _accesses = track_fn!(path_str, |path_str: String| {
        use std::ffi::CString;

        let c_path = CString::new(path_str).unwrap();

        // 1. NULL `how_ptr` -> `EFAULT`.
        // SAFETY: kernel validates `how_ptr` and returns `-EFAULT`.
        let rc_null = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                libc::AT_FDCWD,
                c_path.as_ptr(),
                0_usize,
                size_of::<OpenHow>(),
            )
        };
        assert_eq!(rc_null, -1);
        assert_eq!(errno(), libc::EFAULT);

        // 2. First-page bad `how_ptr` -> `EFAULT` without crashing `SIGSYS`.
        // SAFETY: `fspy_elf_loader` rejects first-page pointers and kernel returns `-EFAULT`.
        let rc_low = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                libc::AT_FDCWD,
                c_path.as_ptr(),
                0x10_usize,
                size_of::<OpenHow>(),
            )
        };
        assert_eq!(rc_low, -1);
        assert_eq!(errno(), libc::EFAULT);

        // 3. Undersized `size` (< `size_of::<open_how>()`) -> `EINVAL`.
        let how = OpenHow { flags: u64::try_from(libc::O_RDONLY).unwrap(), mode: 0, resolve: 0 };
        // SAFETY: valid `how` pointer with intentionally too-small `size`.
        let rc_small = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                libc::AT_FDCWD,
                c_path.as_ptr(),
                &raw const how,
                8_usize,
            )
        };
        assert_eq!(rc_small, -1);
        assert_eq!(errno(), libc::EINVAL);
    })
    .await?;

    Ok(())
}

#[test(tokio::test)]
async fn openat_rdonly_creat_trunc_and_renameat2_exchange() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let root = tmp_dir.path().canonicalize()?;

    let created = root.join("rdonly_creat");
    let truncated = root.join("rdonly_trunc");
    fs::write(&truncated, b"old content")?;
    let ex_a = root.join("exchange_a");
    let ex_b = root.join("exchange_b");
    fs::write(&ex_a, b"a")?;
    fs::write(&ex_b, b"b")?;

    let root_str = root.to_str().unwrap().to_owned();
    let accesses = track_fn!(root_str, |root_str: String| {
        use std::os::fd::AsRawFd as _;

        let dir = File::open(&root_str).unwrap();
        let dirfd = dir.as_raw_fd();

        // 1. `O_RDONLY | O_CREAT` creates the file and must record `ReadWrite`.
        // SAFETY: `dirfd` is open and filename is a valid C string.
        let fd_creat = unsafe {
            libc::openat(dirfd, c"rdonly_creat".as_ptr(), libc::O_RDONLY | libc::O_CREAT, 0o644)
        };
        assert!(fd_creat >= 0, "openat O_RDONLY|O_CREAT failed: {}", errno());
        // SAFETY: closing our owned descriptor.
        unsafe { libc::close(fd_creat) };

        // 2. `O_RDONLY | O_TRUNC` truncates the file and must record `ReadWrite`.
        // SAFETY: `dirfd` is open and filename is a valid C string.
        let fd_trunc = unsafe {
            libc::openat(dirfd, c"rdonly_trunc".as_ptr(), libc::O_RDONLY | libc::O_TRUNC)
        };
        assert!(fd_trunc >= 0, "openat O_RDONLY|O_TRUNC failed: {}", errno());
        // SAFETY: closing our owned descriptor.
        unsafe { libc::close(fd_trunc) };

        // 3. `renameat2(..., RENAME_EXCHANGE)` swaps both paths and must record both.
        // SAFETY: `dirfd` is open and both filenames are valid C strings.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                dirfd,
                c"exchange_a".as_ptr(),
                dirfd,
                c"exchange_b".as_ptr(),
                RENAME_EXCHANGE,
            )
        };
        assert_eq!(rc, 0, "renameat2(RENAME_EXCHANGE) failed: {}", errno());
    })
    .await?;

    assert_contains(&accesses, &created, PathAccess::ReadWrite);
    assert_contains(&accesses, &truncated, PathAccess::ReadWrite);
    assert_contains(&accesses, &ex_a, PathAccess::RenameTarget);
    assert_contains(&accesses, &ex_b, PathAccess::RenameTarget);
    assert_eq!(fs::read(&ex_a)?, b"b");
    assert_eq!(fs::read(&ex_b)?, b"a");
    Ok(())
}

#[cfg(target_arch = "x86_64")]
#[test(tokio::test)]
async fn legacy_x86_64_open_creat_stat_lstat_syscalls_are_tracked() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let root = tmp_dir.path().canonicalize()?;

    let open_target = root.join("legacy_open");
    let creat_target = root.join("legacy_creat");
    let stat_target = root.join("legacy_stat");
    let lstat_target = root.join("legacy_lstat_link");
    fs::write(&open_target, b"open")?;
    fs::write(&stat_target, b"stat")?;
    symlink(&stat_target, &lstat_target)?;

    let args = (
        open_target.to_str().unwrap().to_owned(),
        creat_target.to_str().unwrap().to_owned(),
        stat_target.to_str().unwrap().to_owned(),
        lstat_target.to_str().unwrap().to_owned(),
    );

    let accesses =
        track_fn!(args, |(p_open, p_creat, p_stat, p_lstat): (String, String, String, String)| {
            use std::ffi::CString;

            #[repr(C, align(8))]
            struct RawStatBuf([u8; 256]);

            let c_open = CString::new(p_open).unwrap();
            let c_creat = CString::new(p_creat).unwrap();
            let c_stat = CString::new(p_stat).unwrap();
            let c_lstat = CString::new(p_lstat).unwrap();

            // SAFETY: valid C string; `SYS_open` takes `(path, flags, mode)`.
            let fd = unsafe { libc::syscall(libc::SYS_open, c_open.as_ptr(), libc::O_RDONLY, 0) };
            assert!(fd >= 0, "SYS_open failed: {}", errno());
            // SAFETY: closing our owned descriptor.
            unsafe { libc::close(libc::c_int::try_from(fd).unwrap()) };

            // SAFETY: valid C string; `SYS_creat` takes `(path, mode)`.
            let creat_fd = unsafe { libc::syscall(libc::SYS_creat, c_creat.as_ptr(), 0o644) };
            assert!(creat_fd >= 0, "SYS_creat failed: {}", errno());
            // SAFETY: closing our owned descriptor.
            unsafe { libc::close(libc::c_int::try_from(creat_fd).unwrap()) };

            let mut stat_buf = RawStatBuf([0; 256]);
            // SAFETY: valid C string and 256-byte aligned buffer (>= x86_64 `struct stat`).
            let rc_stat =
                unsafe { libc::syscall(libc::SYS_stat, c_stat.as_ptr(), &raw mut stat_buf) };
            assert_eq!(rc_stat, 0);
            // SAFETY: valid C string and 256-byte aligned buffer (>= x86_64 `struct stat`).
            let rc_lstat =
                unsafe { libc::syscall(libc::SYS_lstat, c_lstat.as_ptr(), &raw mut stat_buf) };
            assert_eq!(rc_lstat, 0);
        })
        .await?;

    assert_includes(&accesses, &open_target, PathAccess::Read);
    assert_includes(&accesses, &creat_target, PathAccess::Write);
    assert_includes(&accesses, &stat_target, PathAccess::Read);
    assert_includes(&accesses, &lstat_target, PathAccess::Read);
    Ok(())
}

#[test(tokio::test)]
async fn execveat_sets_kernel_matching_at_execfn_while_preserving_proc_self_exe()
-> anyhow::Result<()> {
    let cmd = vt_subprocess_test::command_for_fn!((), |()| {
        use std::{
            ffi::{CStr, CString},
            os::{fd::AsRawFd as _, unix::ffi::OsStrExt as _},
        };

        if let Some(expected_execfn) = std::env::var_os("FSPY_TEST_EXPECT_EXECFN") {
            let expected_exe = std::env::var_os("FSPY_TEST_EXPECT_EXE").unwrap();
            // SAFETY: `getauxval(AT_EXECFN)` returns a pointer to the NUL-terminated
            // `AT_EXECFN` string on the initial stack (or 0 if absent).
            let raw = unsafe { libc::getauxval(libc::AT_EXECFN) };
            assert_ne!(raw, 0, "AT_EXECFN missing in execveat child");
            // SAFETY: non-zero `AT_EXECFN` points to a valid NUL-terminated C string.
            let actual_execfn = unsafe { CStr::from_ptr(raw as *const libc::c_char) };
            assert_eq!(
                actual_execfn.to_bytes(),
                expected_execfn.as_bytes(),
                "AT_EXECFN did not match Linux execveat format"
            );
            let actual_exe = std::fs::read_link("/proc/self/exe").unwrap();
            assert_eq!(
                actual_exe.as_os_str().as_bytes(),
                expected_exe.as_bytes(),
                "/proc/self/exe did not preserve real binary path"
            );
            return;
        }

        let exe = std::fs::read_link("/proc/self/exe").unwrap();
        let exe_dir = exe.parent().unwrap();
        let exe_name = exe.file_name().unwrap();

        let mut args: Vec<CString> =
            std::env::args_os().map(|a| CString::new(a.as_bytes()).unwrap()).collect();
        if args.is_empty() {
            let cmdline = std::fs::read("/proc/self/cmdline").unwrap();
            let mut parts: Vec<&[u8]> = cmdline.split(|&b| b == 0).collect();
            if parts.last().is_some_and(|s| s.is_empty()) {
                parts.pop();
            }
            args = parts.into_iter().map(|p| CString::new(p).unwrap()).collect();
        }
        let mut argv: Vec<*const libc::c_char> = args.iter().map(|s| s.as_ptr()).collect();
        argv.push(std::ptr::null());

        let base_envs: Vec<CString> = std::env::vars_os()
            .filter(|(k, _)| k != "FSPY_TEST_EXPECT_EXECFN" && k != "FSPY_TEST_EXPECT_EXE")
            .map(|(k, v)| {
                let mut bytes = k.as_bytes().to_vec();
                bytes.push(b'=');
                bytes.extend_from_slice(v.as_bytes());
                CString::new(bytes).unwrap()
            })
            .collect();

        let run_case = |dirfd: libc::c_int,
                        path: &CStr,
                        flags: libc::c_int,
                        expect_execfn: &str,
                        chdir_root: bool| {
            let mut envs = base_envs.clone();
            envs.push(CString::new(format!("FSPY_TEST_EXPECT_EXECFN={expect_execfn}")).unwrap());
            let mut exe_env = b"FSPY_TEST_EXPECT_EXE=".to_vec();
            exe_env.extend_from_slice(exe.as_os_str().as_bytes());
            envs.push(CString::new(exe_env).unwrap());
            let mut envp: Vec<*const libc::c_char> = envs.iter().map(|s| s.as_ptr()).collect();
            envp.push(std::ptr::null());

            // SAFETY: `fork()` followed by async-signal-safe `chdir`/`execveat`/`_exit`.
            let pid = unsafe { libc::fork() };
            assert!(pid >= 0, "fork failed");
            if pid == 0 {
                // SAFETY: all pointers are valid NUL/NULL-terminated C strings/arrays.
                unsafe {
                    if chdir_root {
                        let _ = libc::chdir(c"/".as_ptr());
                    }
                    libc::syscall(
                        libc::SYS_execveat,
                        dirfd,
                        path.as_ptr(),
                        argv.as_ptr(),
                        envp.as_ptr(),
                        flags,
                    );
                    libc::_exit(127);
                }
            }
            let mut status: libc::c_int = 0;
            // SAFETY: `pid` is our child and `status` is writable.
            assert_eq!(unsafe { libc::waitpid(pid, &raw mut status, 0) }, pid);
            assert!(
                libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
                "execveat AT_EXECFN check failed for {expect_execfn} with status {status}"
            );
        };

        // 1. `execveat(bin_fd, "", ..., AT_EMPTY_PATH)` -> `/dev/fd/<bin_fd>`.
        let bin_file = File::open(&exe).unwrap();
        let expect_empty = format!("/dev/fd/{}", bin_file.as_raw_fd());
        run_case(bin_file.as_raw_fd(), c"", libc::AT_EMPTY_PATH, &expect_empty, false);

        // 2. `execveat(dirfd, basename, ..., 0)` -> `/dev/fd/<dirfd>/<basename>`.
        let dir_file = File::open(exe_dir).unwrap();
        let name_c = CString::new(exe_name.as_bytes()).unwrap();
        let expect_rel = format!("/dev/fd/{}/{}", dir_file.as_raw_fd(), exe_name.to_str().unwrap());
        run_case(dir_file.as_raw_fd(), &name_c, 0, &expect_rel, true);
    });

    let _accesses = test_utils::spawn_command(cmd).await?;
    Ok(())
}
