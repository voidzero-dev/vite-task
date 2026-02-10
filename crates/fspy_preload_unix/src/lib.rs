#![cfg(unix)]
// required for defining inteposed `open`/`openat`(https://man7.org/linux/man-pages/man2/open.2.html)
#![feature(c_variadic)]
#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    reason = "non-vite crate"
)]

mod client;
mod interceptions;
mod libc;
mod macros;
