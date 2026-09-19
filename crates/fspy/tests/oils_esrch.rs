//! The bundled osh substitute must not lose the getpgid race against a fast
//! child: stock oils 0.37.0 calls getpgid(child) after fork even when job
//! control is disabled, and on macOS getpgid of an already-exited child fails
//! with ESRCH, killing the shell with "oils I/O error (main)" and exit 2
//! (oils-for-unix/oils#2689). Under CPU contention a few percent of runs died.
//!
//! The race is gone (not just rarer) with a fixed osh, so this fails
//! deterministically if the bundled artifact regresses to a stock build.
#![cfg(target_os = "macos")]

use std::{fs, path::Path};

use test_log::test;

#[test(tokio::test(flavor = "multi_thread", worker_threads = 8))]
async fn fast_external_commands_under_contention() -> anyhow::Result<()> {
    let input = Path::new(env!("CARGO_TARGET_TMPDIR")).join("fspy-oils-esrch-input.txt");
    fs::write(&input, "hello\n")?;

    let mut failures = Vec::new();
    for _round in 0..25 {
        let mut handles = Vec::new();
        for _ in 0..8 {
            handles.push(tokio::spawn({
                let input = input.clone();
                async move {
                    let mut cmd = fspy::Command::new("/bin/sh");
                    cmd.arg("-c").arg(format!("cat {}", input.display()));
                    cmd.env("PATH", "/usr/bin:/bin");
                    let child = cmd.spawn(tokio_util::sync::CancellationToken::new()).await?;
                    let termination = child.wait_handle.await?;
                    anyhow::Ok(termination.status.code())
                }
            }));
        }
        for h in handles {
            let code = h.await??;
            if code != Some(0) {
                failures.push(code);
            }
        }
    }
    assert!(failures.is_empty(), "osh exited non-zero {} times: {failures:?}", failures.len());
    Ok(())
}
