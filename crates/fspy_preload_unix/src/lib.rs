#![cfg(unix)]
// required for defining inteposed `open`/`openat`(https://man7.org/linux/man-pages/man2/open.2.html)
#![feature(c_variadic)]
// non-vite crate, String/Path/PathBuf/format! etc. are allowed
#![allow(clippy::disallowed_types, clippy::disallowed_methods, clippy::disallowed_macros)]

mod client;
mod interceptions;
mod libc;
mod macros;
