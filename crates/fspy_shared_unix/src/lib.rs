#![cfg(any(unix, target_os = "none"))]
#![cfg_attr(not(test), no_std)]

mod codec;
#[cfg(any(target_os = "linux", target_os = "none"))]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
mod nostd_exec;

pub use nostd_exec::{Env, Exec, RawExec};
