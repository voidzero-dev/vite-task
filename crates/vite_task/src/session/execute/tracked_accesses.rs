//! Normalize raw fspy path accesses into workspace-relative, ordered form.
//!
//! Order is preserved because the classification rules depend on it: a task that
//! wrote a path before reading it is observing its own output, while one that
//! read before writing consumed something that existed. Collapsing accesses into
//! per-path read and write sets, as this module used to, throws that away.
//!
//! User-configured negative globs are NOT applied here. They are applied later,
//! separately for reads (input config) and writes (output config), since those
//! two configs are independent.
#![cfg(fspy)]

use fspy::PathAccessIterable;
use vite_path::{AbsolutePath, RelativePathBuf};

use super::classify::TrackedEvent;

/// Tracked file accesses from fspy, normalized to workspace-relative paths and
/// kept in observation order.
#[derive(Default, Debug)]
pub struct TrackedPathAccesses {
    pub events: Vec<TrackedEvent>,
}

impl TrackedPathAccesses {
    /// Build from fspy's raw iterable by stripping the workspace prefix and
    /// normalizing `..` components. `.git/*` paths are skipped. User-configured
    /// negatives are applied by the caller (see module docs).
    pub fn from_raw(raw: &PathAccessIterable, workspace_root: &AbsolutePath) -> Self {
        let mut events = Vec::new();
        for access in raw.iter() {
            // Strip workspace root and clean `..` components in one pass.
            // fspy may report paths like `packages/sub-pkg/../shared/dist/output.js`.
            let relative_path = access.path.strip_path_prefix(workspace_root, |strip_result| {
                let Ok(stripped_path) = strip_result else {
                    return None;
                };
                normalize_tracked_workspace_path(stripped_path)
            });

            let Some(relative_path) = relative_path else {
                continue;
            };

            events.push(TrackedEvent { path: relative_path, mode: access.mode });
        }
        Self { events }
    }
}

#[expect(
    clippy::disallowed_types,
    reason = "fspy strip_path_prefix exposes std::path::Path; convert to RelativePathBuf immediately"
)]
fn normalize_tracked_workspace_path(stripped_path: &std::path::Path) -> Option<RelativePathBuf> {
    // On Windows, paths are possible to be still absolute after stripping the workspace root.
    // For example: c:\workspace\subdir\c:\workspace\subdir
    // Just ignore those accesses.
    let relative = RelativePathBuf::new(stripped_path).ok()?;

    // Clean `..` components — fspy may report paths like
    // `packages/sub-pkg/../shared/dist/output.js`. Normalize them for
    // consistent behavior across platforms and clean user-facing messages.
    let relative = relative.clean().ok()?;

    // Skip .git directory accesses (workaround for tools like oxlint)
    if relative.as_path().strip_prefix(".git").is_ok() {
        return None;
    }

    Some(relative)
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::*;

    #[cfg(windows)]
    #[test]
    fn malformed_windows_drive_path_after_workspace_strip_is_ignored() {
        #[expect(
            clippy::disallowed_types,
            reason = "normalize_tracked_workspace_path requires std::path::Path for fspy strip_path_prefix output"
        )]
        let relative_path = normalize_tracked_workspace_path(std::path::Path::new(r"foo\C:\bar"));
        assert!(relative_path.is_none());
    }
}
