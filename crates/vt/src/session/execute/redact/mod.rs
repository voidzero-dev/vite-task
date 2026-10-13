//! Files whose content carries noise — timestamps, machine-specific paths —
//! that would otherwise cause cache misses without any meaningful change.
//! Only whether they exist is fingerprinted.
//!
//! To add another file, add a module here with a `matches` condition, and
//! check it in [`hash_file_content`](super::hash::hash_file_content).

pub(super) mod node_modules_bin;
pub(super) mod pnpm_modules_manifest;
