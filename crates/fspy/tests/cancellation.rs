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

#[test_log::test(tokio::test)]
async fn observes_reads_before_the_process_exits_without_consuming_final_trace()
-> anyhow::Result<()> {
    use std::sync::Arc;

    use tokio::io::AsyncWriteExt as _;
    let directory = tempfile::tempdir()?;
    let path = directory.path().canonicalize()?.join("input");
    std::fs::write(&path, "contents")?;
    let cmd =
        subprocess_test::command_for_fn!(path.to_str().unwrap().to_owned(), |path: String| {
            let _ = std::fs::read(path).unwrap();
            let _ = std::io::stdin().read_line(&mut String::new());
        });
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let observed_path = path.clone();
    let mut command = fspy::Command::from(cmd);
    command.stdin(Stdio::piped()).observe_accesses(Arc::new(move |access| {
        let access = access.expect("live trace must remain complete");
        let matched = access.path.strip_path_prefix(&observed_path, |path| {
            path.is_ok_and(|path| path.as_os_str().is_empty())
        });
        if matched && access.mode.contains(fspy::AccessMode::READ) {
            let _ = tx.send(());
        }
    }));
    let mut child = command.spawn(CancellationToken::new()).await?;
    tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await?
        .expect("observer remains connected");
    child.stdin.take().unwrap().write_all(b"exit\n").await?;
    let termination = child.wait_handle.await?;
    assert!(termination.status.success());
    assert!(termination.path_accesses?.iter().any(|access| {
        access
            .path
            .strip_path_prefix(&path, |path| path.is_ok_and(|path| path.as_os_str().is_empty()))
    }));
    Ok(())
}
