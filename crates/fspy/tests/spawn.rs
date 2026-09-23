use fspy::error::SpawnError;
use tokio_util::sync::CancellationToken;

#[test_log::test(tokio::test)]
async fn invalid_working_directory_is_spawn_error() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let command = subprocess_test::command_for_fn!((), |()| {});
    let mut command = fspy::Command::from(command);
    command.current_dir(dir.path().join("missing"));
    let result = command.spawn(CancellationToken::new()).await;
    assert!(matches!(result, Err(SpawnError::OsSpawn(_))));
    Ok(())
}
