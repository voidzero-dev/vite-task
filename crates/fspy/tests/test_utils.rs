use std::path::{Path, PathBuf, StripPrefixError};

use fspy::{AccessMode, PathAccessIterable};
#[doc(hidden)]
#[allow(unused)]
pub use fspy_test_utils::command_executing;

#[track_caller]
pub fn assert_contains(
    accesses: &PathAccessIterable,
    expected_path: &Path,
    expected_mode: AccessMode,
) {
    let found = accesses.iter().any(|access| {
        let Ok(stripped) =
            access.path.strip_path_prefix::<_, Result<PathBuf, StripPrefixError>, _>(
                expected_path,
                |strip_result| strip_result.map(Path::to_path_buf),
            )
        else {
            return false;
        };
        stripped.as_os_str().is_empty() && access.mode == expected_mode
    });
    if !found {
        panic!(
            "Expected to find access to path {:?} with mode {:?}, but it was not found in: {:?}",
            expected_path,
            expected_mode,
            accesses.iter().collect::<Vec<_>>()
        );
    }
}

#[macro_export]
macro_rules! track_child {
    ($body: block) => {{
        let std_cmd = $crate::test_utils::command_executing!((), |(): ()| {
            let _ = $body;
        });
        $crate::test_utils::spawn_std(std_cmd)
    }};
}

#[doc(hidden)]
#[allow(unused)]
pub async fn spawn_std(std_cmd: std::process::Command) -> anyhow::Result<PathAccessIterable> {
    let mut command = fspy::Command::new(std_cmd.get_program());
    command.args(std_cmd.get_args());

    let termination = command.spawn().await?.wait_handle.await?;
    assert!(termination.status.success());
    Ok(termination.path_accesses)
}
