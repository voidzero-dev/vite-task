#![cfg(target_os = "linux")]

use std::{process::Stdio, time::Duration};

use tokio::{io::AsyncReadExt as _, time::timeout};
use tokio_util::sync::CancellationToken;

#[test_log::test(tokio::test)]
async fn detached_descendant_does_not_block_collection_or_lose_syscalls() -> anyhow::Result<()> {
    let mut command = fspy::Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(r"(sleep 2; if test -r /dev/null; then printf ok; else printf failed; fi) &")
        .stdout(Stdio::piped());

    let mut child = command.spawn(CancellationToken::new()).await?;
    let mut stdout = child.stdout.take().expect("stdout should be piped");
    let termination = timeout(Duration::from_secs(1), child.wait_handle)
        .await
        .expect("detached descendant blocked root result collection")?;
    assert!(termination.status.success());

    let mut descendant_status = Vec::new();
    timeout(Duration::from_secs(5), stdout.read_to_end(&mut descendant_status))
        .await
        .expect("detached descendant did not finish its filesystem access")?;
    assert_eq!(descendant_status, b"ok", "detached descendant filesystem access failed");

    Ok(())
}
