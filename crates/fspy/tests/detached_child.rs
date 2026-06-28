#![cfg(all(unix, not(target_env = "musl")))]

use std::{process::Stdio, time::Duration};

use tokio::{io::AsyncReadExt as _, time::timeout};
use tokio_util::sync::CancellationToken;

#[test_log::test(tokio::test)]
async fn detached_descendant_sender_does_not_block_wait() -> anyhow::Result<()> {
    let mut command = fspy::Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("sleep 5 >/dev/null 2>&1 & echo $!")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = command.spawn(CancellationToken::new()).await?;
    let mut stdout = child.stdout.take().unwrap();
    let mut child_pid = String::new();
    stdout.read_to_string(&mut child_pid).await?;

    let termination = timeout(Duration::from_secs(1), child.wait_handle).await??;
    assert!(termination.status.success());
    assert!(
        !termination.path_accesses.is_complete(),
        "detached descendant should keep IPC sender alive past the root child"
    );

    let _ = std::process::Command::new("kill").arg(child_pid.trim()).status();
    Ok(())
}
