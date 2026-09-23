//! On macOS, fspy runs `/bin/sh` commands under the bundled osh, which must not
//! lose the getpgid race against a fast child. Oils before 0.38.0 called
//! getpgid(child) after fork even when job control was disabled, and on macOS
//! getpgid of an already-exited child fails with ESRCH, killing the shell with
//! "oils I/O error (main): No such process" and exit 2
//! (oils-for-unix/oils#2689). Under CPU contention a few percent of runs died.
//!
//! A fixed osh never fails here, while an affected one fails a few percent of
//! runs, so 200 runs catch a bundled osh that reintroduces the race.
#![cfg(target_os = "macos")]

use std::{fs, path::Path, process::Stdio};

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
                    cmd.stdout(Stdio::null());
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
