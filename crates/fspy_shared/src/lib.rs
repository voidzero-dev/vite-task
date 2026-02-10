// non-vite crate, String/Path/PathBuf/format! etc. are allowed
#![allow(clippy::disallowed_types, clippy::disallowed_methods, clippy::disallowed_macros)]

pub mod ipc;

#[cfg(windows)]
pub mod windows;
