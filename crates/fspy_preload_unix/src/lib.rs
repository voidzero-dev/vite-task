// On musl targets, fspy_preload_unix is not usable (musl does not support cdylib/LD_PRELOAD).
// Compile as an empty crate to avoid build failures from missing libc symbols.
#![cfg_attr(not(target_env = "musl"), feature(c_variadic))]

#[cfg(all(unix, not(target_env = "musl")))]
mod client;
#[cfg(all(unix, not(target_env = "musl")))]
mod interceptions;
#[cfg(all(unix, not(target_env = "musl")))]
mod libc;
#[cfg(all(unix, not(target_env = "musl")))]
mod macros;
