# fspy

Run a command and capture all the paths it tries to access.

## macOS implementation

It uses `DYLD_INSERT_LIBRARIES` to inject a shared library that intercepts file system calls.

## Linux implementation

Linux installs one inherited `seccomp_unotify` filter for the complete task process tree. Dynamically linked executables also receive an `LD_PRELOAD` library that accelerates safely patchable syscall sites. Static binaries, direct syscalls, and every declined acceleration case continue through seccomp.

## Linux musl implementation

On musl targets, only `seccomp_unotify`-based tracking is used (no preload library).

## Windows implementation

It uses [Detours](https://github.com/microsoft/Detours) to intercept file system calls. The implementation is in `src/windows`.

## Unified interface

The unified interface of `Command` is in `src/command.rs`.

## Preload Libraries

`DYLD_INSERT_LIBRARIES`, `LD_PRELOAD`, `Detours` all require a shared library to be injected. The shared libraries of macOS/Linux are in the `fspy_preload_unix` crate, and the shared library of Windows is in the `fspy_preload_windows` crate.
