mod test_utils;

use std::{
    env::var_os,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use fspy::PathAccess;
use test_log::test;
use test_utils::assert_contains;

fn run(command: &mut Command) -> anyhow::Result<Vec<u8>> {
    let output = command.output()?;
    anyhow::ensure!(
        output.status.success(),
        "{command:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

/// Builds a Node.js single executable application that reads the file named
/// by its first argument.
/// <https://nodejs.org/api/single-executable-applications.html>
fn build_node_sea(dir: &Path) -> anyhow::Result<PathBuf> {
    let manifest_dir = PathBuf::from(var_os("CARGO_MANIFEST_DIR").unwrap());
    let postject = manifest_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("packages/tools/node_modules/postject/dist/cli.js");

    // `node` on PATH may be a version manager shim; copy the real binary.
    let node = PathBuf::from(
        String::from_utf8(run(Command::new("node").args(["-p", "process.execPath"]))?)?.trim_end(),
    );

    fs::write(dir.join("main.js"), "require('node:fs').readFileSync(process.argv[2]);")?;
    fs::write(
        dir.join("sea-config.json"),
        r#"{ "main": "main.js", "output": "sea-prep.blob", "disableExperimentalSEAWarning": true }"#,
    )?;
    run(Command::new(&node)
        .arg("--experimental-sea-config")
        .arg("sea-config.json")
        .current_dir(dir))?;

    let sea = dir.join(if cfg!(windows) { "sea.exe" } else { "sea" });
    fs::copy(&node, &sea)?;
    #[cfg(target_os = "macos")]
    run(Command::new("codesign").arg("--remove-signature").arg(&sea))?;

    let mut inject = Command::new(&node);
    inject.arg(postject).arg(&sea).args([
        "NODE_SEA_BLOB",
        "sea-prep.blob",
        "--sentinel-fuse",
        "NODE_SEA_FUSE_fce680ab2cc467b6e072b8b5df1996b2",
    ]);
    #[cfg(target_os = "macos")]
    inject.args(["--macho-segment-name", "NODE_SEA"]);
    run(inject.current_dir(dir))?;

    #[cfg(target_os = "macos")]
    run(Command::new("codesign").args(["--sign", "-"]).arg(&sea))?;
    Ok(sea)
}

// On macOS, the injected preload must not hide the embedded application from
// Node's lookup.
#[test(tokio::test)]
#[ignore = "requires node and `pnpm install` (workspace root)"]
async fn node_sea() -> anyhow::Result<()> {
    let tmpdir = tempfile::tempdir()?;
    // on macOS, tmpdir.path() may be a symlink, so we need to canonicalize it
    let tmpdir_path = fs::canonicalize(tmpdir.path())?;
    let sea = build_node_sea(&tmpdir_path)?;
    let input = tmpdir_path.join("input.txt");
    fs::write(&input, "")?;

    let mut command = fspy::Command::new(&sea);
    command.arg(&input);
    let termination =
        command.spawn(tokio_util::sync::CancellationToken::new()).await?.wait_handle.await?;
    assert!(termination.status.success(), "{:?}", termination.status);
    let accesses = termination.path_accesses.expect("the test region holds every record");
    assert_contains(&accesses, &input, PathAccess::Read);
    Ok(())
}
