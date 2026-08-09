// Compile as an empty crate on non-unix targets and on musl (where seccomp
// alone handles access tracking).

/// This library interposes libc functions that POSIX declares
/// async-signal-safe (`open`, `stat`, `execve`, ...), so its own code may run
/// inside signal handlers and in the child of `fork()` in a multithreaded
/// process — contexts where taking the libc allocator's locks can deadlock.
/// Route every Rust allocation in this cdylib through fspy's lock-free,
/// mmap-backed allocator instead.
#[cfg(all(unix, not(target_env = "musl")))]
#[global_allocator]
static GLOBAL_ALLOCATOR: fspy_alloc::FspyAlloc = fspy_alloc::FspyAlloc::new();

#[cfg(all(unix, not(target_env = "musl")))]
mod client;
#[cfg(all(unix, not(target_env = "musl")))]
mod interceptions;
#[cfg(all(unix, not(target_env = "musl")))]
mod libc;
#[cfg(all(unix, not(target_env = "musl")))]
mod macros;
