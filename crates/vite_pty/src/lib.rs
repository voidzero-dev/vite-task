#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    reason = "vite_pty is a standalone PTY crate, not using vite_str/vite_path"
)]

pub mod geo;
pub mod terminal;

pub use portable_pty::ExitStatus;
