//! Files whose content carries noise — timestamps, machine-specific paths —
//! that would otherwise cause cache misses without any meaningful change.
//!
//! To redact another file, add a module here with a `matches` condition and a
//! `redact` action, and call them from
//! [`hash_file_content`](super::hash::hash_file_content).

pub(super) mod node_modules_bin;
pub(super) mod pnpm_modules_manifest;

use std::io;

use serde_json::Value;

/// The content is not in the format a redaction expects, so the raw content is
/// hashed instead.
#[derive(Debug, PartialEq, Eq)]
pub struct UnexpectedFormat;

/// Writes `value` as JSON with every object's keys sorted, so the output does
/// not depend on the file's key order.
///
/// Serializing a JSON value fails only when `out` does, which the in-memory
/// writers used here never do.
fn write_canonical_json<W: io::Write>(
    mut value: Value,
    out: &mut W,
) -> Result<(), UnexpectedFormat> {
    // serde_json's `preserve_order` feature is enabled in this workspace, so
    // objects keep the file's key order until sorted.
    value.sort_all_objects();
    serde_json::to_writer(out, &value).map_err(|_| UnexpectedFormat)
}
