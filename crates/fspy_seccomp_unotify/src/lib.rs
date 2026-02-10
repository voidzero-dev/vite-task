#![cfg(target_os = "linux")]
#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    reason = "non-vite crate"
)]

#[cfg(any(feature = "supervisor", feature = "target"))]
mod bindings;
pub mod payload;
#[cfg(feature = "target")]
pub mod target;

#[cfg(feature = "supervisor")]
pub mod supervisor;
