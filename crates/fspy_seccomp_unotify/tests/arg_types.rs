#![cfg(target_os = "linux")]
use std::{
    env::{current_dir, set_current_dir},
    error::Error,
    ffi::{CString, OsString},
    io,
    os::unix::ffi::OsStringExt,
    time::Duration,
};

use assertables::assert_contains;
use fspy_seccomp_unotify::{
    impl_handler,
    supervisor::{
        handler::arg::{CStrPtr, Caller, Fd},
        supervise,
    },
    target::install_target,
};
use nix::{
    fcntl::{AT_FDCWD, OFlag, openat},
    sys::stat::Mode,
};
use test_log::test;
use tokio::{process::Command, task::spawn_blocking, time::timeout};
use tracing::{Level, span, trace};

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".global fspy_test_openat_gate",
    "fspy_test_openat_gate:",
    "movq %rcx, %r10",
    "movq $257, %rax",
    "syscall",
    ".global fspy_test_openat_gate_post_syscall",
    "fspy_test_openat_gate_post_syscall:",
    "retq",
    options(att_syntax)
);

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".global fspy_test_openat_gate",
    "fspy_test_openat_gate:",
    "mov x8, #56",
    "svc #0",
    ".global fspy_test_openat_gate_post_syscall",
    "fspy_test_openat_gate_post_syscall:",
    "ret",
);

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
unsafe extern "C" {
    fn fspy_test_openat_gate(
        dirfd: libc::c_int,
        path: *const libc::c_char,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) -> libc::c_long;
    static fspy_test_openat_gate_post_syscall: u8;
}

#[derive(Debug, PartialEq, Eq, Clone)]
enum Syscall {
    Openat { at_dir: OsString, path: Option<OsString> },
}

#[derive(Default, Clone, Debug)]
struct SyscallRecorder(Vec<Syscall>);
impl SyscallRecorder {
    fn openat(&mut self, caller: Caller<'_>, (fd, path): (Fd, CStrPtr)) -> io::Result<()> {
        let at_dir = fd.get_path(caller)?;
        let mut buf = vec![0u8; 40000];
        let path = path
            .read(caller, &mut buf)?
            .map(|null_pos| OsString::from_vec(buf[..null_pos].to_vec()));
        self.0.push(Syscall::Openat { at_dir, path });
        Ok(())
    }
}

impl_handler!(SyscallRecorder: openat,);

async fn run_in_pre_exec(
    post_syscall_pc: u64,
    mut f: impl FnMut() -> io::Result<()> + Send + Sync + 'static,
) -> Result<Vec<Syscall>, Box<dyn Error>> {
    Ok(timeout(Duration::from_secs(5), async move {
        let mut cmd = Command::new("/bin/echo");
        let supervisor = supervise::<SyscallRecorder>(post_syscall_pc)?;

        let payload = supervisor.payload().clone();

        // SAFETY: `pre_exec` closure runs in the forked child process before exec.
        // It installs the seccomp filter and runs the user-provided closure, both of
        // which are safe in a pre-exec context (no async, no locks held).
        unsafe {
            cmd.pre_exec(move || {
                install_target(&payload)?;
                f()?;
                Ok(())
            });
        }
        let child_fut = spawn_blocking(move || {
            let _span = span!(Level::TRACE, "spawn test child process");
            cmd.spawn()
        });

        let exit_status = child_fut.await.unwrap()?.wait().await?;
        trace!("test child process exited with status: {:?}", exit_status);

        trace!("waiting for handler to finish and test child process to exit");

        assert!(exit_status.success());

        let outcome = supervisor.stop().await;
        if let Some(error) = outcome.error {
            return Err(error);
        }
        let recorders = outcome.handlers;
        trace!("{} recorders awaited", recorders.len());

        let syscalls = recorders.into_iter().flat_map(|recorder| recorder.0);
        io::Result::Ok(syscalls.collect())
    })
    .await??)
}

#[test(tokio::test)]
async fn fd_and_path() -> Result<(), Box<dyn Error>> {
    let syscalls = run_in_pre_exec(0, || {
        set_current_dir("/")?;
        let home_fd = openat(AT_FDCWD, c"/home", OFlag::O_PATH, Mode::empty())?;
        let _ = openat(home_fd, c"open_at_home", OFlag::O_RDONLY, Mode::empty());
        let _ = openat(AT_FDCWD, c"openat_cwd", OFlag::O_RDONLY, Mode::empty());
        Ok(())
    })
    .await?;
    assert_contains!(syscalls, &Syscall::Openat { at_dir: "/".into(), path: Some("/home".into()) });
    assert_contains!(
        syscalls,
        &Syscall::Openat { at_dir: "/home".into(), path: Some("open_at_home".into()) }
    );
    assert_contains!(
        syscalls,
        &Syscall::Openat { at_dir: "/".into(), path: Some("openat_cwd".into()) }
    );
    Ok(())
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[test(tokio::test)]
async fn exact_gate_pc_bypasses_notification() -> Result<(), Box<dyn Error>> {
    let gate_path = CString::new("/fspy-gate-bypassed")?;
    let notified_path = CString::new("/fspy-gate-notified")?;
    let post_syscall_pc =
        u64::try_from((&raw const fspy_test_openat_gate_post_syscall).addr()).unwrap();
    let syscalls = run_in_pre_exec(post_syscall_pc, move || {
        // SAFETY: the assembly gate implements the Linux openat syscall ABI and receives a valid
        // C string for the duration of the call.
        let _ = unsafe {
            fspy_test_openat_gate(
                libc::AT_FDCWD,
                gate_path.as_ptr(),
                libc::O_RDONLY,
                libc::mode_t::default(),
            )
        };
        let _ = openat(AT_FDCWD, notified_path.as_c_str(), OFlag::O_RDONLY, Mode::empty());
        Ok(())
    })
    .await?;

    assert!(!syscalls.iter().any(|syscall| matches!(
        syscall,
        Syscall::Openat { path: Some(path), .. } if path == "/fspy-gate-bypassed"
    )));
    assert_contains!(
        syscalls,
        &Syscall::Openat {
            at_dir: current_dir().unwrap().into(),
            path: Some("/fspy-gate-notified".into()),
        }
    );
    Ok(())
}

#[tokio::test]
async fn path_long() -> Result<(), Box<dyn Error>> {
    let long_path = b"a".repeat(30000);
    let long_path_cstr = CString::new(long_path.as_slice()).unwrap();
    let syscalls = run_in_pre_exec(0, move || {
        let _ = openat(AT_FDCWD, long_path_cstr.as_c_str(), OFlag::O_RDONLY, Mode::empty());
        Ok(())
    })
    .await?;
    assert_contains!(
        syscalls,
        &Syscall::Openat {
            at_dir: current_dir().unwrap().into(),
            path: Some(OsString::from_vec(long_path)),
        }
    );
    Ok(())
}

#[tokio::test]
async fn path_overflow() -> Result<(), Box<dyn Error>> {
    let long_path = b"a".repeat(40000);
    let long_path_cstr = CString::new(long_path.as_slice()).unwrap();
    let syscalls = run_in_pre_exec(0, move || {
        let _ = openat(AT_FDCWD, long_path_cstr.as_c_str(), OFlag::O_RDONLY, Mode::empty());
        Ok(())
    })
    .await?;
    assert_contains!(
        syscalls,
        &Syscall::Openat { at_dir: current_dir().unwrap().into(), path: None }
    );
    Ok(())
}
