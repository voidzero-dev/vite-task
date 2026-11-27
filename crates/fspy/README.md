# fspy

Run a command and capture all the paths it tries to access.

## Linux implementation

Linux uses `fspy_preload_linux` (`src/linux`), a freestanding fixed-address ELF64
loader (`x86_64-unknown-none` / `aarch64-unknown-none-softfloat`) built on
`fspy_elf_loader`. `fspy` spawns `fspy_preload_linux`, which maps the target
executable (dynamic glibc, musl, static, or custom runtime) in-process and
installs a `SECCOMP_RET_TRAP` (`SIGSYS`) filter to record filesystem syscalls
into the shared-memory IPC channel (`fspy_shared::ipc::channel`) and chain child
`execve`/`execveat` calls. When `seccomp` is unavailable (such as in a
restricted sandbox), `fspy` falls back to running the command untracked and
reports `TrackingIncomplete`.

## macOS implementation

macOS uses `DYLD_INSERT_LIBRARIES` with `fspy_preload_macos` (`src/macos`) to
interpose libc file-access and exec calls, substituting bundled shells and
coreutils for SIP-protected `/bin`/`/usr/bin` binaries.

## Windows implementation

Windows uses [Detours](https://github.com/microsoft/Detours) with
`fspy_preload_windows` (`src/windows`) to intercept Win32/NT file system calls.

## Unified interface

The unified interface of `Command` is in `src/command.rs`.
