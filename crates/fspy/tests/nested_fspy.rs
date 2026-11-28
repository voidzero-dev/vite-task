//! Cross-platform integration tests for nested `fspy` detection.
//!
//! When a process already running inside an active `fspy` tracker invokes
//! `fspy::Command` itself, the inner `fspy` must detect the outer tracker via
//! the null-device sentinel probe, run the grandchild untracked, report
//! `Err(UntrackedReason::Nested)`, and let the outer tracker capture the
//! grandchild's file accesses without recording the sentinel probe path.

mod test_utils;

use std::fs::File;

use fspy::{PathAccess, UntrackedReason};
use test_log::test;

use crate::test_utils::assert_contains;

#[test(tokio::test)]
async fn nested_fspy_delegates_to_outer_tracker_without_recording_probe() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let tmp_path = tmp_dir.path().canonicalize()?;
    let probe_file = tmp_path.join("grandchild_input.txt");
    File::create(&probe_file)?;
    let probe_file_str = probe_file.to_str().unwrap().to_owned();

    let accesses = track_fn!(probe_file_str, |probe_file_str: String| {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            let grandchild_cmd =
                vt_subprocess_test::command_for_fn!(probe_file_str, |probe_file_str: String| {
                    let _ = std::fs::read(&probe_file_str).unwrap();
                });
            let termination = fspy::Command::from(grandchild_cmd)
                .spawn(tokio_util::sync::CancellationToken::new())
                .await
                .expect("nested fspy spawn must succeed via untracked fallback")
                .wait_handle
                .await
                .expect("nested fspy child wait failed");
            assert!(
                termination.status.success(),
                "nested grandchild exited unsuccessfully: {:?}",
                termination.status
            );
            assert_eq!(
                termination.path_accesses.err(),
                Some(UntrackedReason::Nested),
                "nested fspy must report UntrackedReason::Nested while delegating to outer tracker"
            );
        });
    })
    .await?;

    assert_contains(&accesses, &probe_file, PathAccess::Read);

    let leaked_probe: Vec<_> = accesses
        .iter()
        .filter(|access| {
            access.paths().any(|p| {
                format!("{p:?}").contains("fspy_probe_92e41940-369e-42f1-876f-a8e2a614fbf2")
            })
        })
        .collect();
    assert!(
        leaked_probe.is_empty(),
        "outer tracker must not record the nested-detection sentinel probe path: {leaked_probe:?}"
    );

    Ok(())
}
