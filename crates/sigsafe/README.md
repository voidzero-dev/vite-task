# sigsafe

Unix syscall wrappers that are safe to call where libc is not.

## Why this crate exists

The fspy preload library injects itself into traced programs and intercepts their libc calls (`open`, `stat`, `execve`, ...). POSIX declares those functions async-signal-safe, so programs are allowed to call them:

- inside a signal handler,
- in the child of `fork()` of a multithreaded program,
- while the process is still starting up, before libc is fully initialized.

Most of libc is off limits in those places. `malloc` is the classic trap: a signal can pause a thread while it holds malloc's lock, and `fork()` copies a locked lock into a child that has no thread left to unlock it — the next `malloc` waits forever. Interception code runs exactly there, so anything it calls must work without libc's machinery, or the traced program can hang.

## The rules

Every function in this crate follows three rules:

1. **Syscalls only.** On Linux, nothing goes through libc — the syscall instructions are emitted directly (rustix's raw backend). On macOS there is no stable syscall interface, so calls go through libSystem's wrappers; for the calls exposed here those are thin stubs with no locks and no state.
2. **No locks, no hidden state.** Nothing a signal or a `fork()` could catch locked or half-written. Where shared state is unavoidable it is a fixed set of atomics, each touched by single complete operations.
3. **No global allocation.** No function touches a heap behind the caller's back. Code that needs memory gets it from an explicit allocator — [`sigsafe_alloc`](../sigsafe_alloc) provides one built on `mmap`.

## How rule 1 is enforced on Linux

rustix can be built with a libc backend instead of raw syscalls, and anything in the dependency graph — including crates outside this repository — can select it (the `rustix/use-libc` feature, or `RUSTFLAGS=--cfg=rustix_use_libc`). No build script can detect that reliably, so [`lib.rs`](src/lib.rs) checks at compile time instead: it references `rustix::runtime`, a module that exists only in rustix's raw-syscall build. Selecting the libc backend makes this crate fail to compile, rather than silently losing the guarantee.

## What's inside

Functions whose rustix implementation already meets the rules are re-exposed as-is; being listed in a module here is what marks a call as allowed, and the backend check above is what keeps that true.

- `mm` — anonymous memory mappings: `mmap_anonymous`, `munmap`.
- `fs` — caller-buffer filesystem operations: `getcwd`, plus macOS `fcntl_getpath`.
- `param` — `page_size`.

Allocation without malloc lives in [`sigsafe_alloc`](../sigsafe_alloc).
