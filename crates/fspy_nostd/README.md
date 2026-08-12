# fspy_nostd

Low-level operations for fspy code that runs before a process runtime is ready or in a context where normal runtime code can deadlock.

The current implementation supports Linux and macOS. The crate has no Windows backend yet.

## Execution contexts

| Context                 | Required properties                      |
| ----------------------- | ---------------------------------------- |
| Windows preload         | Loader-lock-safe and safe before `main`  |
| Linux and macOS preload | Async-signal-safe and safe before `main` |
| Linux injected runtime  | No libc dependency                       |

Safe before `main` means that code cannot depend on target-runtime constructors or initialized global state.

## Windows preload

Windows calls `DllMain` while holding the loader lock and may call it before the executable entry point. Rust `std` initializes some global state on first use. That initialization can acquire locks or call into another DLL.

[`rust-lang/rust#99341`](https://github.com/rust-lang/rust/issues/99341) records a `DllMain` deadlock caused by lazy `HashSet` initialization. Rust treats libstd support in `DllMain` as best-effort.

A Windows `fspy_nostd` backend must use state initialized at compile time and Windows operations that Microsoft documents as safe in `DllMain`.

## Linux and macOS preload

fspy interposes functions that POSIX marks async-signal-safe. Programs may call those functions from a signal handler or from the post-`fork()` child of a multithreaded process. The interceptor must preserve that safety.

`malloc` is not async-signal-safe. A signal can interrupt a thread while it holds the allocator lock, and `fork()` can copy that lock without the thread that would release it. Calling `malloc` from the interceptor can then deadlock.

Preload code can also run before `main`, while libc and the target runtime are still initializing.

## Linux injected runtime

The injected runtime cannot assume that the target process has libc. Linux kernel operations in `fspy_nostd` use raw syscalls, so injected code can reuse them without linking or calling libc.

The final injected artifact must also reject libc references that dependencies or compiler-generated memory operations introduce. The raw-syscall check covers the `rustix` backend, not the complete artifact link.

## API rules

Every exported operation follows these rules:

1. It uses no hidden allocation, locks, or lazy global state.
2. It takes caller-owned storage when an operation needs memory.
3. It satisfies the platform requirement in the table above.

Code that needs allocation uses an explicit allocator. [`fspy_nostd_alloc`](../fspy_nostd_alloc) provides one based on memory mappings.

## Linux raw-syscall enforcement

`rustix` can use libc instead of raw syscalls. A dependency can select that backend through feature unification or `RUSTFLAGS`.

[`lib.rs`](src/lib.rs) references `rustix::runtime`, which exists only with the raw Linux backend. Selecting the libc backend makes `fspy_nostd` fail to compile.

## Modules

- `mm`: anonymous memory mapping and protection operations.
- `env`: allocation-free process argument and environment iteration.
- `fs`: filesystem operations with caller-owned buffers.
- `param`: page-size access.
