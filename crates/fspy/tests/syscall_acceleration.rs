#![cfg(all(target_os = "linux", not(target_env = "musl")))]

use std::{ffi::CString, os::unix::ffi::OsStrExt as _};

use tokio_util::sync::CancellationToken;

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".global fspy_test_accelerated_openat",
    ".type fspy_test_accelerated_openat, @function",
    "fspy_test_accelerated_openat:",
    "movq %rcx, %r10",
    "movq $257, %rax",
    ".global fspy_test_accelerated_openat_site",
    "fspy_test_accelerated_openat_site:",
    "syscall",
    "testq %rax, %rax",
    "retq",
    ".size fspy_test_accelerated_openat, .-fspy_test_accelerated_openat",
    options(att_syntax)
);

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".global fspy_test_accelerated_openat",
    ".type fspy_test_accelerated_openat, %function",
    "fspy_test_accelerated_openat:",
    "mov x8, #56",
    ".global fspy_test_accelerated_openat_site",
    "fspy_test_accelerated_openat_site:",
    "svc #0",
    "ret",
    ".size fspy_test_accelerated_openat, .-fspy_test_accelerated_openat",
);

unsafe extern "C" {
    fn fspy_test_accelerated_openat(
        dirfd: libc::c_int,
        path: *const libc::c_char,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) -> libc::c_long;
    static fspy_test_accelerated_openat_site: u8;
}

#[test_log::test(tokio::test)]
async fn raw_syscall_uses_acceleration_or_seccomp_fallback() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let path = temp_dir.path().join("accelerated-missing-input");
    let path_arg = path.to_string_lossy().into_owned();

    let command = subprocess_test::command_for_fn!(path_arg, |path_arg: String| {
        let site = (&raw const fspy_test_accelerated_openat_site).cast::<u8>();
        #[cfg(target_arch = "x86_64")]
        let patched = {
            // SAFETY: the label points into this executable's readable text mapping.
            (unsafe { site.read() }) == 0xe9
        };
        #[cfg(target_arch = "aarch64")]
        let patched = {
            // SAFETY: AArch64 instructions are aligned four-byte words in readable text.
            let instruction = unsafe { site.cast::<u32>().read_unaligned() };
            instruction & 0xfc00_0000 == 0x1400_0000
        };
        if std::env::var_os("FSPY_TEST_REQUIRE_ACCELERATION").is_some() {
            assert!(patched, "syscall site was not accelerated");
        }

        let path = CString::new(path_arg).unwrap();
        // SAFETY: the assembly function implements the native openat syscall ABI.
        let result = unsafe {
            fspy_test_accelerated_openat(
                libc::AT_FDCWD,
                path.as_ptr(),
                libc::O_RDONLY,
                libc::mode_t::default(),
            )
        };
        assert_eq!(result, libc::c_long::from(-libc::ENOENT));
    });

    let termination =
        fspy::Command::from(command).spawn(CancellationToken::new()).await?.wait_handle.await?;
    assert!(termination.status.success(), "child exited with {:?}", termination.status);
    assert!(termination.tracing_error.is_none());

    let matching_accesses = termination
        .path_accesses
        .iter()
        .filter(|access| {
            access.path.strip_path_prefix(&path, |stripped| {
                stripped.is_ok_and(|stripped| stripped.as_os_str().is_empty())
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(matching_accesses.len(), 1, "raw syscall should be recorded exactly once");
    assert_eq!(matching_accesses[0].mode, fspy::AccessMode::READ);

    let control_accesses = termination
        .path_accesses
        .iter()
        .filter(|access| {
            access.path.strip_path_prefix("/tmp", |stripped| {
                stripped.is_ok_and(|path| {
                    path.file_name().is_some_and(|name| name.as_bytes().starts_with(b"fspy_ipc_"))
                })
            })
        })
        .count();
    assert_eq!(
        control_accesses, 1,
        "only preload initialization should notify seccomp about the channel lock"
    );
    Ok(())
}
