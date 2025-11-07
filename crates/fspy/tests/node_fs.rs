mod test_utils;

use std::env::{current_dir, vars_os};

use fspy::{AccessMode, PathAccessIterable};
use test_utils::assert_contains;

async fn track_node_script(script: &str) -> anyhow::Result<PathAccessIterable> {
    let mut command = fspy::Spy::global()?.new_command("node");
    command
        .arg("-e")
        .envs(vars_os()) // https://github.com/jdx/mise/discussions/5968
        .arg(script);
    let child = command.spawn().await?;
    let termination = child.wait_handle.await?;
    assert!(termination.status.success());
    Ok(termination.path_accesses)
}

#[tokio::test]
async fn read_sync() -> anyhow::Result<()> {
    let accesses = track_node_script("try { fs.readFileSync('hello') } catch {}").await?;
    assert_contains(&accesses, current_dir().unwrap().join("hello").as_path(), AccessMode::Read);
    Ok(())
}

#[tokio::test]
async fn read_dir_sync() -> anyhow::Result<()> {
    let accesses = track_node_script("try { fs.readdirSync('.') } catch {}").await?;
    assert_contains(&accesses, &current_dir().unwrap(), AccessMode::ReadDir);
    Ok(())
}

#[tokio::test]
async fn subprocess() -> anyhow::Result<()> {
    let cmd = if cfg!(windows) {
        r"'cmd', ['/c', 'type hello']"
    } else {
        r"'/bin/sh', ['-c', 'cat hello']"
    };
    let accesses = track_node_script(&format!(
        "try {{ child_process.spawnSync({cmd}, {{ stdio: 'ignore' }}) }} catch {{}}"
    ))
    .await?;
    assert_contains(&accesses, current_dir().unwrap().join("hello").as_path(), AccessMode::Read);
    Ok(())
}
