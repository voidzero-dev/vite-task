#![cfg(target_os = "linux")]

use fspy::AccessMode;
use tokio_util::sync::CancellationToken;

use crate::test_utils::assert_contains;

mod test_utils;

#[test_log::test(tokio::test)]
async fn undecodable_notification_marks_tracing_incomplete_and_continues() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let before_path = temp_dir.path().join("before");
    let after_path = temp_dir.path().join("after");
    let before_arg = before_path.to_string_lossy().into_owned();
    let after_arg = after_path.to_string_lossy().into_owned();
    let cmd = subprocess_test::command_for_fn!(
        (before_arg.clone(), after_arg.clone()),
        |(before_arg, after_arg): (String, String)| {
            let before = std::ffi::CString::new(before_arg).unwrap();
            let after = std::ffi::CString::new(after_arg).unwrap();

            // SAFETY: each caller provides a valid NUL-terminated path and the
            // remaining arguments follow the openat syscall ABI.
            let open = |path: &std::ffi::CStr| unsafe {
                libc::syscall(libc::SYS_openat, libc::AT_FDCWD, path.as_ptr(), libc::O_RDONLY, 0)
            };
            assert_eq!(open(&before), -1);
            assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ENOENT));

            // SAFETY: issuing openat with an invalid userspace pointer is safe; the
            // kernel rejects it with EFAULT after the seccomp supervisor continues it.
            let result = unsafe {
                libc::syscall(libc::SYS_openat, libc::AT_FDCWD, 1usize, libc::O_RDONLY, 0)
            };
            assert_eq!(result, -1);
            assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EFAULT));

            // The handler loop must remain alive after recording the error.
            assert_eq!(open(&after), -1);
            assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ENOENT));
        }
    );

    let termination =
        fspy::Command::from(cmd).spawn(CancellationToken::new()).await?.wait_handle.await?;

    assert!(termination.status.success());
    assert_contains(&termination.path_accesses, &before_path, AccessMode::READ);
    assert_contains(&termination.path_accesses, &after_path, AccessMode::READ);
    let tracing_error = termination.tracing_error.expect("tracing should be incomplete");
    assert_eq!(tracing_error.raw_os_error(), Some(libc::EFAULT));
    Ok(())
}
