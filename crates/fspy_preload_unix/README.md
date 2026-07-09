## fspy_preload_unix

The shared library injected by `DYLD_INSERT_LIBRARIES` on macOS and `LD_PRELOAD` on Linux to intercept file system calls.

This crate only contains code for the shared library itself. The injection process is implemented by the `fspy` crate.

### Linux raw-syscall acceleration

On Linux x86-64 and AArch64, this crate registers a callback with [`fspy_syscall_intercept`](../fspy_syscall_intercept/README.md). The callback decodes filesystem syscalls, safely resolves paths, and publishes complete shared-memory records before allowing a syscall through the exact seccomp-allowlisted gate. Unsupported or suppressed paths, invalid pointers, unavailable IPC, and nested or concurrent callback contention use the non-allowlisted gate and remain covered by the seccomp listener.

The binary scanning, text patching, generated gates, architecture state preservation, runtime capability checks, and trusted-workload caveats are owned and documented by `fspy_syscall_intercept`.
