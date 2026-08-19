//! Allocation-free process argument and environment iteration.
//!
//! [`current`] snapshots both collections at once. Linux obtains their string
//! bounds from one read of `/proc/self/stat`; macOS snapshots the pointer
//! arrays exposed by `_NSGetArgv` and `_NSGetEnviron`.
//!
//! macOS additionally exposes `args` and `envs` as direct thin C-string
//! views. Neither platform interface can attach a lifetime to its
//! process-global storage, so construction is unsafe and yielded views
//! deliberately carry a `'static` lifetime.

use bstr::BStr;

use crate::{CStr, Fat};

#[cfg(any(target_os = "linux", target_os = "none"))]
mod linux;
#[cfg(target_os = "macos")]
mod mac;

#[cfg(any(target_os = "linux", target_os = "none"))]
pub use linux::{Current, FatArgs, FatEnvs, current};
#[cfg(target_os = "macos")]
pub use mac::{Current, FatArgs, FatEnvs, ThinArgs, ThinEnvs, args, current, envs};

/// One process environment entry, split at its first `=` byte.
///
/// Entries without `=` retain their complete bytes as the name and have no
/// value. An entry ending in `=` has a present, empty C string value.
pub type Entry<R = Fat> = (&'static BStr, Option<CStr<'static, R>>);
