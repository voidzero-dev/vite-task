use std::path::{Path, PathBuf, StripPrefixError};

use fspy::{AccessMode, PathAccessIterable};
#[doc(hidden)]
#[expect(unused)]
pub use fspy_test_utils::command_executing;

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

    assert_eq!(
        expected_mode,
        actual_mode,
        "Expected to find access to path {:?} with mode {:?}, but it was not found in: {:?}",
        expected_path,
        expected_mode,
        accesses.iter().collect::<Vec<_>>()
    );
}

#[macro_export]
macro_rules! track_child {
    ($arg: expr, $body: expr) => {{
        let std_cmd = $crate::test_utils::command_executing!($arg, $body);
        $crate::test_utils::spawn_std(std_cmd)
    }};
}

#[doc(hidden)]
#[expect(unused)]
pub async fn spawn_std(std_cmd: std::process::Command) -> anyhow::Result<PathAccessIterable> {
    let mut command = fspy::Command::new(std_cmd.get_program());
    command
        .args(std_cmd.get_args())
        .envs(std_cmd.get_envs().filter_map(|(name, value)| Some((name, value?))));

    let termination = command.spawn().await?.wait_handle.await?;
    assert!(termination.status.success());
    Ok(termination.path_accesses)
}
