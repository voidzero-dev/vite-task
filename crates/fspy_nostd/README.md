# fspy_nostd

Low-level operations for fspy code that runs before a process runtime is ready or in a context where normal runtime code can deadlock.

The current implementation supports Linux, `target_os = "none"` code injected
into Linux, macOS, and Windows.

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

For this crate, `target_os = "none"` means code that runs inside a Linux
process. It uses the same Linux kernel ABI as the normal Linux build.

The final injected artifact must also reject libc references introduced by
other dependencies or compiler-generated memory operations. This crate's
Linux operations alone cannot enforce the complete artifact link.

## API rules

Every exported operation follows these rules:

1. It uses no hidden allocation, locks, or lazy global state.
2. It takes caller-owned storage when an operation needs memory.
3. It satisfies the platform requirement in the table above.

Code that needs allocation uses an explicit allocator. [`fspy_nostd_alloc`](../fspy_nostd_alloc) provides one based on memory mappings.

## Platform boundaries

- Linux and `none` issue syscalls directly and obtain ABI constants and
  structures from `linux-raw-sys`.
- macOS calls libSystem through `libc`, as required by the platform.
- Windows calls Win32 directly.

`Error` owns the raw platform error code. On macOS and Windows it additionally
provides `Error::last_os_error()` because their APIs report failure through a
sentinel and put the reason in thread-local state. Linux and `none` syscalls
return their error directly, so that method deliberately does not exist there.

## Modules

- `mm`: memory mapping and protection operations.
- `env`: allocation-free process argument and environment iteration.
- `fs`: filesystem operations with caller-owned paths and buffers.
- `param`: page-size access.
- `get_module_handle`: allocation-free lookup of an already-loaded Windows module.
