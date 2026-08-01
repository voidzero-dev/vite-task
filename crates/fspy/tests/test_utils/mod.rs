use std::path::{Path, PathBuf, StripPrefixError};

use fspy::{AccessMode, PathAccessIterable};
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
pub use subprocess_test::command_for_fn;

/// Flags describing how an access was requested rather than what kind of access
/// it was. They differ between runtimes for the same API: `writeFileSync`
/// truncates on Node and Deno but not on Bun. Spelling them out in every test
/// would assert the runtime's open flags rather than the behaviour under test, so
/// they only participate when a test names one explicitly.
const MODIFIERS: AccessMode = AccessMode::CREATE
    .union(AccessMode::TRUNCATE)
    .union(AccessMode::EXCLUSIVE)
    .union(AccessMode::IS_DIR);

/// # Panics
///
/// Panics if the expected path access is not found or has the wrong mode.
#[track_caller]
pub fn assert_contains(
    accesses: &PathAccessIterable,
    expected_path: &Path,
    expected_mode: AccessMode,
) {
    let mut actual_mode: AccessMode = AccessMode::empty();
    for access in accesses.iter() {
        let Ok(stripped) =
            access.path.strip_path_prefix::<_, Result<PathBuf, StripPrefixError>, _>(
                expected_path,
                |strip_result| strip_result.map(Path::to_path_buf),
            )
        else {
            continue;
        };
        if stripped.as_os_str().is_empty() {
            actual_mode.insert(access.mode);
        }
    }

    if actual_mode.contains(AccessMode::READ_DIR) {
        // READ_DIR already implies READ.
        actual_mode.remove(AccessMode::READ);
    }

    if !expected_mode.intersects(MODIFIERS) {
        actual_mode.remove(MODIFIERS);
    }

    assert_eq!(
        expected_mode,
        actual_mode,
        "Expected to find access to path {} with mode {:?}, but it was not found in: {:?}",
        expected_path.display(),
        expected_mode,
        accesses.iter().collect::<Vec<_>>()
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
pub async fn spawn_command(cmd: subprocess_test::Command) -> anyhow::Result<PathAccessIterable> {
    let termination = fspy::Command::from(cmd)
        .spawn(tokio_util::sync::CancellationToken::new())
        .await?
        .wait_handle
        .await?;
    assert!(termination.status.success());
    Ok(termination.path_accesses)
}
