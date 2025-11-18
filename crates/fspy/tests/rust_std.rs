mod test_utils;

use std::{
    env::current_dir,
    fs::{File, OpenOptions},
    process::Stdio,
};

use fspy::AccessMode;
use test_log::test;
use test_utils::assert_contains;

#[test(tokio::test)]
async fn open_read() -> anyhow::Result<()> {
    let accesses = track_child!((), |(): ()| {
        let _ = File::open("hello");
    })
    .await?;
    assert_contains(&accesses, current_dir().unwrap().join("hello").as_path(), AccessMode::READ);

    Ok(())
}

#[test(tokio::test)]
async fn open_write() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let tmp_path = tmp_dir.path().join("hello");
    let tmp_path_str = tmp_path.to_str().unwrap().to_owned();
    let accesses = track_child!(tmp_path_str, |tmp_path_str: String| {
        let _ = OpenOptions::new().write(true).open(tmp_path_str);
    })
    .await?;
    assert_contains(&accesses, tmp_path.as_path(), AccessMode::WRITE);

    Ok(())
}

#[test(tokio::test)]
async fn readdir() -> anyhow::Result<()> {
    let accesses = track_child!((), |(): ()| {
        let _ = std::fs::read_dir("hello_dir");
    })
    .await?;
    assert_contains(&accesses, current_dir()?.join("hello_dir").as_path(), AccessMode::READ_DIR);

    Ok(())
}

#[test(tokio::test)]
async fn subprocess() -> anyhow::Result<()> {
    let accesses = track_child!((), |(): ()| {
        let mut command = if cfg!(windows) {
            let mut command = std::process::Command::new("cmd");
            command.arg("/c").arg("type hello");
            command
        } else {
            let mut command = std::process::Command::new("/bin/sh");
            command.arg("-c").arg("cat hello");
            command
        };
        command
            .stdout(Stdio::null())
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
            .wait()
            .unwrap();
    })
    .await?;
    assert_contains(&accesses, current_dir().unwrap().join("hello").as_path(), AccessMode::READ);

    Ok(())
}
