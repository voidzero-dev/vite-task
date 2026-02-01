mod test_utils;

use std::{env::vars_os, ffi::OsString};

use fspy::{AccessMode, PathAccessIterable};
use test_log::test;

/// Get the packages/tools directory path
/// Uses runtime path resolution to work across platforms (macOS/Windows shared folders)
fn tools_dir() -> std::path::PathBuf {
    // Navigate from current working directory (workspace root) to packages/tools
    // This works because cargo test runs with cwd set to the workspace root
    let cwd = std::env::current_dir().expect("Failed to get current directory");

    // Try to find packages/tools from the workspace root
    let from_workspace = cwd.join("packages/tools");
    if from_workspace.exists() {
        return from_workspace;
    }

    // Fallback: navigate up from crates/fspy to find the workspace root
    // This handles cases where cwd might be different
    let mut dir = cwd.clone();
    loop {
        let candidate = dir.join("packages/tools");
        if candidate.exists() {
            return candidate;
        }
        if !dir.pop() {
            break;
        }
    }

    panic!(
        "packages/tools not found. Searched from: {}. Make sure to run 'pnpm install' in packages/tools first.",
        cwd.display()
    );
}

/// Get the oxlint cli.js path in packages/tools/node_modules
fn oxlint_cli_js() -> std::path::PathBuf {
    tools_dir().join("node_modules/oxlint/dist/cli.js")
}

/// Get the node_modules/.bin directory path
fn tools_bin_dir() -> std::path::PathBuf {
    tools_dir().join("node_modules/.bin")
}

/// Find node executable
fn find_node() -> std::path::PathBuf {
    which::which("node").expect("node not found in PATH")
}

async fn track_oxlint(dir: &std::path::Path, args: &[&str]) -> anyhow::Result<PathAccessIterable> {
    let node_path = find_node();
    let oxlint_cli = oxlint_cli_js();
    let bin_dir = tools_bin_dir();

    // Build PATH with packages/tools/node_modules/.bin prepended so oxlint can find tsgolint
    let new_path = if let Some(existing_path) = std::env::var_os("PATH") {
        let mut paths = vec![bin_dir.as_os_str().to_owned()];
        paths.extend(std::env::split_paths(&existing_path).map(|p| p.into_os_string()));
        std::env::join_paths(paths)?
    } else {
        OsString::from(&bin_dir)
    };

    let mut command = fspy::Command::new(&node_path);

    // Run oxlint cli.js directly via node
    // Pass the target directory as the last argument to oxlint
    command
        .arg(&oxlint_cli)
        .args(args.iter().filter(|a| !a.is_empty()))
        .arg(dir)
        .envs(vars_os().filter(|(k, _)| !k.eq_ignore_ascii_case("PATH")))
        .env("PATH", new_path);

    let child = command.spawn().await?;
    let termination = child.wait_handle.await?;
    // oxlint may return non-zero if it finds lint errors, that's OK
    Ok(termination.path_accesses)
}

#[test(tokio::test)]
async fn oxlint_reads_js_file() -> anyhow::Result<()> {
    let tmpdir = tempfile::tempdir()?;
    // on macOS, tmpdir.path() may be a symlink, so we need to canonicalize it
    let tmpdir_path = std::fs::canonicalize(tmpdir.path())?;

    let js_file = tmpdir_path.join("test.js");
    std::fs::write(&js_file, "console.log('hello');")?;

    let accesses = track_oxlint(&tmpdir_path, &[]).await?;

    // Check that oxlint read the JS file
    test_utils::assert_contains(&accesses, &js_file, AccessMode::READ);

    Ok(())
}

#[test(tokio::test)]
async fn oxlint_reads_directory() -> anyhow::Result<()> {
    let tmpdir = tempfile::tempdir()?;

    // on macOS, tmpdir.path() may be a symlink, so we need to canonicalize it
    let tmpdir_path = std::fs::canonicalize(tmpdir.path())?;

    let accesses = track_oxlint(&tmpdir_path, &[]).await?;

    // Check that oxlint read the directory to find JS files
    // This is the key check - if READ_DIR is not tracked, cache won't detect new files
    test_utils::assert_contains(&accesses, &tmpdir_path, AccessMode::READ_DIR);
    Ok(())
}

// Skip on Windows: tsgolint panics with "Expected file to be in inferred program"
// when running on temp directories. This is a tsgolint bug, not an fspy issue.
#[cfg_attr(windows, ignore)]
#[test(tokio::test)]
async fn oxlint_type_aware() -> anyhow::Result<()> {
    let tmpdir = tempfile::tempdir()?;
    // on macOS, tmpdir.path() may be a symlink, so we need to canonicalize it
    let tmpdir_path = std::fs::canonicalize(tmpdir.path())?;

    // Create a simple TypeScript file
    let ts_file = tmpdir_path.join("index.ts");
    std::fs::write(
        &ts_file,
        r#"
import type { Foo } from './types';
declare const _foo: Foo;
"#,
    )?;

    // Run oxlint without --type-aware first
    let accesses = track_oxlint(&tmpdir_path, &[""]).await?;
    let access_to_types_ts = accesses.iter().find(|access| {
        let os_str = access.path.to_cow_os_str();
        os_str.as_encoded_bytes().ends_with(b"\\types.ts")
            || os_str.as_encoded_bytes().ends_with(b"/types.ts")
    });
    assert_eq!(access_to_types_ts, None, "oxlint should not read types.ts without --type-aware");

    // Run oxlint with --type-aware to enable type-aware linting
    let accesses = track_oxlint(&tmpdir_path, &["--type-aware"]).await?;

    // Check that oxlint read types.ts
    test_utils::assert_contains(&accesses, &tmpdir_path.join("types.ts"), AccessMode::READ);

    Ok(())
}
