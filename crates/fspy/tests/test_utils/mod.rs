use std::path::Path;

use fspy::{IpcPath, PathAccess, PathAccessIterable};
// Used by the track_child! macro; not all test files use this macro
#[doc(hidden)]
#[expect(
    clippy::useless_attribute,
    reason = "allow attribute on re-export required for macro usage"
)]
#[expect(
    clippy::allow_attributes,
    reason = "allow attribute on re-export required for macro usage"
)]
#[allow(
    unused_imports,
    reason = "used by track_child! macro; not all test files use this macro"
)]
pub use vt_subprocess_test::command_for_fn;

/// Asserts that `expected_path` was accessed, and only as `expected` (a
/// variant such as `PathAccess::Read`).
///
/// # Panics
///
/// Panics if the path was not accessed, or was accessed in another way.
#[track_caller]
pub fn assert_contains<'a>(
    accesses: &'a PathAccessIterable,
    expected_path: &Path,
    expected: fn(&'a IpcPath) -> PathAccess<'a>,
) {
    // Each access to the path, paired with what it was expected to be.
    let mut path_accesses = Vec::new();
    for access in accesses.iter() {
        for path in access.paths() {
            let is_expected_path = path.strip_path_prefix(expected_path, |strip_result| {
                strip_result.is_ok_and(|stripped| stripped.as_os_str().is_empty())
            });
            if is_expected_path {
                path_accesses.push((access, expected(path)));
            }
        }
    }

    if path_accesses.iter().any(|(access, _)| matches!(access, PathAccess::ReadDir(_))) {
        // ReadDir already implies Read.
        path_accesses.retain(|(access, _)| !matches!(access, PathAccess::Read(_)));
    }

    assert!(
        !path_accesses.is_empty()
            && path_accesses.iter().all(|(access, expected)| access == expected),
        "Expected accesses to {} to match, as (actual, expected): {:?}. All accesses: {:?}",
        expected_path.display(),
        path_accesses,
        accesses.iter().collect::<Vec<_>>()
    );
}

/// Asserts that `unexpected_path` was not accessed in any way.
///
/// # Panics
///
/// Panics if the path was accessed.
#[expect(
    clippy::allow_attributes,
    reason = "allow attribute required for conditionally-used helper"
)]
#[allow(dead_code, reason = "not all test files use this helper")]
#[track_caller]
pub fn assert_not_contains(accesses: &PathAccessIterable, unexpected_path: &Path) {
    let matching = accesses
        .iter()
        .filter(|access| {
            access.paths().any(|path| {
                path.strip_path_prefix(unexpected_path, |strip_result| {
                    strip_result.is_ok_and(|stripped| stripped.as_os_str().is_empty())
                })
            })
        })
        .collect::<Vec<_>>();
    assert!(
        matching.is_empty(),
        "Expected no accesses to {}, found: {:?}",
        unexpected_path.display(),
        matching
    );
}

/// Spawns a subprocess that executes the given function with file access tracking.
///
/// - $arg: The argument to pass to the function
/// - $body: The function to run in the subprocess
///
/// Returns the tracked file accesses from the subprocess.
#[macro_export]
macro_rules! track_fn {
    ($arg: expr, $body: expr) => {{
        let cmd = $crate::test_utils::command_for_fn!($arg, $body);
        $crate::test_utils::spawn_command(cmd)
    }};
}

// Used by the track_child! macro; not all test files use this macro
#[doc(hidden)]
#[expect(
    clippy::allow_attributes,
    reason = "allow attribute required for conditionally-used helper"
)]
#[allow(dead_code, reason = "used by track_fn! macro; not all test files use this macro")]
pub async fn spawn_command(cmd: vt_subprocess_test::Command) -> anyhow::Result<PathAccessIterable> {
    let termination = fspy::Command::from(cmd)
        .spawn(tokio_util::sync::CancellationToken::new())
        .await?
        .wait_handle
        .await?;
    assert!(termination.status.success());
    Ok(termination.path_accesses.expect("the test region holds every record"))
}
