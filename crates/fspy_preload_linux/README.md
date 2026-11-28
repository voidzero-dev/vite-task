# `fspy_preload_linux`

Freestanding Linux ELF loader and `SIGSYS` syscall-tracking runtime for `fspy`.

## Overview

`fspy_preload_linux` is built as a fixed-address, `no_std`, `no_main` ELF64
executable targeting `x86_64-unknown-none` and `aarch64-unknown-none-softfloat`.
Instead of injecting a libc-bound `LD_PRELOAD` shared library or relying on
`seccomp_unotify`, `fspy` spawns `fspy_preload_linux`, which:

1. Decodes `__FSPY_BOOTSTRAP` from the initial Linux stack environment and
   attaches to the shared-memory IPC channel.
2. Resolves (when invoked as `Target::Unresolved`) or opens the target ELF64
   executable, strips `__FSPY_BOOTSTRAP`, and enters the target via
   `fspy_elf_loader::exec_elf` with its `SIGSYS` syscall interceptor installed.
3. Intercepts tracked filesystem syscalls and `execve`/`execveat` in-process
   inside the `SIGSYS` handler, reporting accessed paths to the IPC channel and
   chaining child `execve`/`execveat` calls through `fspy_preload_linux` while
   reusing the existing seccomp-BPF filter.

Because the runtime is completely freestanding and intercepts syscalls directly
via seccomp `SECCOMP_RET_TRAP`, it tracks dynamic glibc binaries, musl binaries,
static binaries, and custom-runtime binaries (such as Go, Bun, and Deno)
uniformly without `LD_PRELOAD`.
