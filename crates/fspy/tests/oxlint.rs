mod test_utils;

use std::env::vars_os;

use fspy::{AccessMode, PathAccessIterable};
use test_log::test;

/// Find the oxlint executable in test_bins
fn find_oxlint() -> std::path::PathBuf {
    let test_bins_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("vite_task_bin")
        .join("test_bins")
        .join("node_modules")
        .join(".bin");

    which::which_in("oxlint", Some(&test_bins_dir), std::env::current_dir().unwrap())
        .expect("oxlint not found in test_bins/node_modules/.bin")
}

async fn track_oxlint(dir: &std::path::Path, args: &[&str]) -> anyhow::Result<PathAccessIterable> {
    let oxlint_path = find_oxlint();
    let mut command = fspy::Command::new(&oxlint_path);
    command.args(args).envs(vars_os()).current_dir(dir);

    let child = command.spawn().await?;
    let termination = child.wait_handle.await?;
    // oxlint may return non-zero if it finds lint errors, that's OK
    Ok(termination.path_accesses)
}

#[test(tokio::test)]
async fn oxlint_reads_js_file() -> anyhow::Result<()> {
    let tmpdir = tempfile::tempdir()?;
    let js_file = tmpdir.path().join("test.js");
    std::fs::write(&js_file, "console.log('hello');")?;

    let accesses = track_oxlint(tmpdir.path(), &[]).await?;

    // Check that oxlint read the JS file
    test_utils::assert_contains(&accesses, &js_file, AccessMode::READ);

    Ok(())
}

#[test(tokio::test)]
async fn oxlint_reads_directory() -> anyhow::Result<()> {
    let tmpdir = tempfile::tempdir()?;
    let js_file = tmpdir.path().join("test.js");
    std::fs::write(&js_file, "console.log('hello');")?;

    let accesses = track_oxlint(tmpdir.path(), &[]).await?;

    // Check that oxlint read the directory to find JS files
    // This is the key check - if READ_DIR is not tracked, cache won't detect new files
    test_utils::assert_contains(&accesses, tmpdir.path(), AccessMode::READ_DIR);

    Ok(())
}

#[test(tokio::test)]
async fn oxlint_empty_directory_reads_dir() -> anyhow::Result<()> {
    let tmpdir = tempfile::tempdir()?;
    // No files in directory

    let accesses = track_oxlint(tmpdir.path(), &[]).await?;

    // Even with no JS files, oxlint should read the directory to discover files
    // Print all accesses for debugging
    println!("All accesses in empty directory:");
    for access in accesses.iter() {
        println!("  {:?}", access);
    }

    // This assertion may fail - that would indicate the bug
    test_utils::assert_contains(&accesses, tmpdir.path(), AccessMode::READ_DIR);

    Ok(())
}
