#![cfg(unix)]
mod test_utils;

use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
};

use fspy::AccessMode;
use test_log::test;
use test_utils::assert_contains;
use tokio::fs;

#[test(tokio::test)]
async fn spawn_sh_shebang() -> anyhow::Result<()> {
    let tmp_dir = tempfile::TempDir::new()?;

    let shebang_script_path = tmp_dir.path().join("fspy_test_shebang_script.sh");
    let shebang_script_path = shebang_script_path.into_os_string().into_string().unwrap();

    fs::write(&shebang_script_path, "#!/bin/sh\ncat hello\n").await?;

    let mut perms = fs::metadata(&shebang_script_path).await?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&shebang_script_path, perms).await?;

    let accesses = track_fn!(shebang_script_path.clone(), |shebang_script_path: String| {
        let _ignored = Command::new(&shebang_script_path)
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .status()
            .expect("Failed to execute shebang script");
    })
    .await?;

    assert_contains(&accesses, Path::new(&shebang_script_path), AccessMode::READ);
    assert_contains(&accesses, Path::new("/hello"), AccessMode::READ);

    Ok(())
}

#[test(tokio::test)]
async fn spawn_shebang_with_long_interpreter_path() -> anyhow::Result<()> {
    let tmp_dir = tempfile::TempDir::new()?;

    // Kernels read 256 (Linux) or 512 (macOS) bytes of the shebang line, so a
    // 200-byte interpreter path must be kept whole.
    let mut interpreter = tmp_dir.path().to_path_buf();
    while interpreter.as_os_str().len() < 150 {
        interpreter.push("d".repeat(40));
    }
    fs::create_dir_all(&interpreter).await?;
    interpreter.push("x".repeat(200 - interpreter.as_os_str().len() - 1));
    assert_eq!(interpreter.as_os_str().len(), 200);
    fs::symlink("/bin/sh", &interpreter).await?;

    let script_path = tmp_dir.path().join("long_shebang.sh");
    fs::write(&script_path, format!("#!{}\necho ok\n", interpreter.display())).await?;
    fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).await?;
    let script_path = script_path.into_os_string().into_string().unwrap();

    let accesses = track_fn!(script_path, |script_path: String| {
        let output = Command::new(&script_path)
            .stdin(Stdio::null())
            .output()
            .expect("Failed to execute shebang script");
        assert_eq!(output.stdout, b"ok\n", "stderr: {}", String::from_utf8_lossy(&output.stderr));
    })
    .await?;

    assert_contains(&accesses, &interpreter, AccessMode::READ);

    Ok(())
}
