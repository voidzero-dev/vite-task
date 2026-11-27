#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(not(target_os = "none"))]
const fn main() {}

#[cfg(target_os = "none")]
mod freestanding {
    use fspy_elf_loader::{Syscall, SyscallAction, SyscallInterceptor};
    use fspy_nostd::{
        CWD, Error, Result,
        fs::{Mode, OFlags, openat},
    };
    use syscalls::Sysno;

    fspy_nostd::main!(run);
    fspy_nostd::no_global_alloc!();

    const INTERCEPTOR_SHORT_CIRCUIT_ARG: usize = 0xdead_beef;
    const INTERCEPTOR_REWRITE_PROBE_MODE: usize = 0x7777;
    const INTERCEPTOR_EXEC_PROBE_ARGV: usize = 0x5555_1234;
    const INTERCEPTOR_EXEC_FAIL_CLEANUP_ARGV: usize = 0x5555_2345;
    const INTERCEPTOR_EXEC_FAIL_NO_CLEANUP_ARGV: usize = 0x5555_2346;

    static NONEXISTENT_EXEC_PATH: &[u8] = b"/nonexistent_fspy_execve_action_probe\0";
    static SIGSYS_IGNORED_PROBE_PATH: &[u8] = b"/fspy_probe_sigsys_ignored";
    static EMPTY_EXEC_PTRS: [usize; 1] = [0];

    unsafe fn test_execve_cleanup(ctx: *mut core::ffi::c_void) {
        let counter = ctx.cast::<u32>();
        if !counter.is_null() {
            // SAFETY: test passes a valid `*mut u32` counter in `envp` (`args[2]`).
            unsafe { counter.write_volatile(counter.read_volatile().wrapping_add(1)) };
        }
    }

    static TEST_INTERCEPTOR: SyscallInterceptor = SyscallInterceptor {
        syscalls: &[Sysno::getegid, Sysno::faccessat, Sysno::execve, Sysno::execveat],
        handle: test_handle,
        on_vfork_return: None,
    };

    /// Test interceptor exercising:
    /// - `getegid(0xdead_beef)` -> short-circuits with `Err(Error::ACCES)`
    /// - `getegid(_)` -> replays normally (`Ok(SyscallAction::Replay)`)
    /// - `faccessat(_, path, 0x7777)` -> checks `/proc/self/exe` was rewritten
    ///   to an absolute non-`/proc/` path, returning `Err(Error::LOOP)` on
    ///   match or `Err(Error::NOENT)` on mismatch
    /// - `execve(path, 0x5555_1234, _)` / `execveat(dirfd, path, 0x5555_1234, _, _)`
    ///   -> checks `/proc/self/exe` was rewritten to an absolute non-`/proc/`
    ///   path (and `dirfd == AT_FDCWD` for `execveat`)
    /// - `execve(_, 0x5555_2345, counter_ptr)` -> returns
    ///   `SyscallAction::Execve` to a nonexistent path with a cleanup callback
    ///   that increments `*counter_ptr`
    /// - `execve(_, 0x5555_2346, _)` -> returns `SyscallAction::Execve` to a
    ///   nonexistent path with `cleanup: None`
    unsafe fn test_handle(call: &Syscall) -> Result<SyscallAction> {
        let check_rewritten_path = |path_ptr: usize| -> Result<SyscallAction> {
            // SAFETY: `path_ptr` is the trapped syscall's path argument.
            let Ok(cstr) = (unsafe { fspy_elf_loader::user_cstr(path_ptr) }) else {
                return Err(Error::NOENT);
            };
            let path = cstr.count().as_units();
            if path.starts_with(b"/") && !path.starts_with(b"/proc/") {
                Err(Error::LOOP)
            } else {
                Err(Error::NOENT)
            }
        };

        match call.sysno {
            Sysno::getegid => {
                if call.args[0] == INTERCEPTOR_SHORT_CIRCUIT_ARG {
                    return Err(Error::ACCES);
                }
                Ok(SyscallAction::Replay)
            }
            Sysno::faccessat => {
                if call.args[2] == INTERCEPTOR_REWRITE_PROBE_MODE {
                    return check_rewritten_path(call.args[1]);
                }
                Ok(SyscallAction::Replay)
            }
            Sysno::execve => {
                if call.args[1] == INTERCEPTOR_EXEC_PROBE_ARGV {
                    return check_rewritten_path(call.args[0]);
                }
                if call.args[1] == INTERCEPTOR_EXEC_FAIL_CLEANUP_ARGV {
                    return Ok(SyscallAction::Execve {
                        prog: NONEXISTENT_EXEC_PATH.as_ptr(),
                        argv: EMPTY_EXEC_PTRS.as_ptr().cast(),
                        envp: EMPTY_EXEC_PTRS.as_ptr().cast(),
                        cleanup: Some((
                            test_execve_cleanup,
                            core::ptr::with_exposed_provenance_mut(call.args[2]),
                        )),
                    });
                }
                if call.args[1] == INTERCEPTOR_EXEC_FAIL_NO_CLEANUP_ARGV {
                    return Ok(SyscallAction::Execve {
                        prog: NONEXISTENT_EXEC_PATH.as_ptr(),
                        argv: EMPTY_EXEC_PTRS.as_ptr().cast(),
                        envp: EMPTY_EXEC_PTRS.as_ptr().cast(),
                        cleanup: None,
                    });
                }
                // SAFETY: `call.args[0]` is the trapped `execve` pathname pointer.
                if let Ok(cstr) = unsafe { fspy_elf_loader::user_cstr(call.args[0]) }
                    && cstr.count().as_units() == SIGSYS_IGNORED_PROBE_PATH
                {
                    return Err(if call.sigsys_ignored() { Error::LOOP } else { Error::NOENT });
                }
                Ok(SyscallAction::Replay)
            }
            Sysno::execveat => {
                if call.args[2] == INTERCEPTOR_EXEC_PROBE_ARGV {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_possible_wrap,
                        reason = "Linux dirfd lives in low 32 bits"
                    )]
                    let dirfd = call.args[0] as i32;
                    if dirfd != CWD.as_raw_fd() {
                        return Err(Error::NOENT);
                    }
                    return check_rewritten_path(call.args[1]);
                }
                Ok(SyscallAction::Replay)
            }
            _ => Ok(SyscallAction::Replay),
        }
    }

    // The kernel enters with argc, argv, envp, and auxv on the initial stack.
    // Consume those views before exec_elf replaces the stack.
    #[expect(clippy::needless_pass_by_value, reason = "signature required by main!")]
    fn run(stack: fspy_nostd::env::InitialStack<'_>) -> ! {
        // Invocation: launcher [--test-interceptor] ELF_PATH TARGET_ARGV0 [TARGET_ARGS...].
        // The kernel supplied this stack for the launcher; exec_elf will build
        // another one for the target, with the launcher arguments removed.
        let mut args = stack.args().iter().copied();
        let _launcher = args.next().expect("missing launcher argv[0]");
        let first = args.next().expect("missing target path");
        let (interceptor, path) = if first.count().as_units() == b"--test-interceptor" {
            (Some(TEST_INTERCEPTOR), args.next().expect("missing target path"))
        } else {
            (None, first)
        };
        let file = openat(CWD, path, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())
            .expect("cannot open target");
        // SAFETY: this is a fresh single-threaded freestanding process. The
        // parent selects a trusted fixture and kernel auxv references stay
        // valid until copied. There is no runtime requiring stack cleanup.
        unsafe {
            fspy_elf_loader::exec_elf(
                file,
                path,
                args,
                stack.envs().iter().copied(),
                stack.auxv(),
                interceptor,
            )
        }
    }
}
