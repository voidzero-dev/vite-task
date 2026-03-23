use std::process::Stdio;

use tokio::io::AsyncReadExt as _;
use tokio_util::sync::CancellationToken;

#[test_log::test(tokio::test)]
async fn cancellation_kills_tracked_child() -> anyhow::Result<()> {
    let cmd = subprocess_test::command_for_fn!((), |()| {
        use std::io::Write as _;
        // Signal readiness via stdout
        std::io::stdout().write_all(b"ready\n").unwrap();
        std::io::stdout().flush().unwrap();
        // Block on stdin — will be killed by cancellation
        let _ = std::io::stdin().read_line(&mut String::new());
    });
    let token = CancellationToken::new();
    let mut fspy_cmd = fspy::Command::from(cmd);
    fspy_cmd.stdout(Stdio::piped()).stdin(Stdio::piped());
    let mut child = fspy_cmd.spawn(token.clone()).await?;

    // Wait for child to signal readiness
    let mut stdout = child.stdout.take().unwrap();
    let mut buf = vec![0u8; 64];
    let n = stdout.read(&mut buf).await?;
    assert!(std::str::from_utf8(&buf[..n])?.contains("ready"));

    // Cancel — fspy background task calls start_kill
    token.cancel();
    let termination = child.wait_handle.await?;
    assert!(!termination.status.success());
    Ok(())
}
