mod test_utils;

use std::{
    env::{current_dir, join_paths, split_paths, var_os, vars_os},
    ffi::{OsStr, OsString},
    iter,
    path::PathBuf,
};

use fspy::{AccessMode, PathAccessIterable};
use ntest::test_case;
use test_utils::assert_contains;

fn resolve_runtime(runtime: &str) -> anyhow::Result<(PathBuf, OsString)> {
    let manifest_dir = PathBuf::from(var_os("CARGO_MANIFEST_DIR").unwrap());
    let tools_bin =
        manifest_dir.parent().unwrap().parent().unwrap().join("packages/tools/node_modules/.bin");
    let path =
        join_paths(iter::once(tools_bin).chain(var_os("PATH").iter().flat_map(split_paths)))?;
    let program = which::which_in(runtime, Some(&path), current_dir()?)?;
    Ok((program, path))
}

fn track_script(
    runtime: &str,
    script: &str,
    args: &[&OsStr],
) -> anyhow::Result<PathAccessIterable> {
    let (program, path) = resolve_runtime(runtime)?;

    let mut command = fspy::Command::new(program);
    command
        .envs(vars_os().filter(|(name, _)| !name.eq_ignore_ascii_case("PATH")))
        .env("PATH", path); // https://github.com/jdx/mise/discussions/5968
    let script = format!(
        "const fs = require('node:fs'); \
         const child_process = require('node:child_process'); \
         {script}"
    );
    if runtime == "deno" {
        command.args(["eval", "--ext=cjs"]).arg(script).args(args);
    } else {
        command.arg("-e").arg(script).args(args);
    }

    // `ntest::test_case` generates synchronous `#[test]` functions, so drive
    // fspy's asynchronous process tracking from this shared helper.
    tokio::runtime::Runtime::new()?.block_on(async {
        let child = command.spawn(tokio_util::sync::CancellationToken::new()).await?;
        let termination = child.wait_handle.await?;
        assert!(termination.status.success());
        Ok(termination.path_accesses)
    })
}

// Bun's Linux runtime uses direct syscalls, which require universal seccomp
// tracing instead of preload interception. Deno does not distribute a musl
// runtime. Node remains covered on every target.
#[test_case("node")]
#[ignore = "requires node"]
#[cfg_attr(not(target_os = "linux"), test_case("bun"))]
#[cfg_attr(not(target_os = "linux"), ignore = "requires node")]
#[cfg_attr(not(target_env = "musl"), test_case("deno"))]
#[cfg_attr(not(target_env = "musl"), ignore = "requires node")]
fn read_sync(runtime: &str) -> anyhow::Result<()> {
    let accesses = track_script(runtime, "try { fs.readFileSync('hello') } catch {}", &[])?;
    assert_contains(&accesses, current_dir().unwrap().join("hello").as_path(), AccessMode::READ);
    Ok(())
}

#[test_case("node")]
#[ignore = "requires node"]
#[cfg_attr(not(target_os = "linux"), test_case("bun"))]
#[cfg_attr(not(target_os = "linux"), ignore = "requires node")]
#[cfg_attr(not(target_env = "musl"), test_case("deno"))]
#[cfg_attr(not(target_env = "musl"), ignore = "requires node")]
fn exist_sync(runtime: &str) -> anyhow::Result<()> {
    let accesses = track_script(runtime, "try { fs.existsSync('hello') } catch {}", &[])?;
    assert_contains(&accesses, current_dir().unwrap().join("hello").as_path(), AccessMode::READ);
    Ok(())
}

#[test_case("node")]
#[ignore = "requires node"]
#[cfg_attr(not(target_os = "linux"), test_case("bun"))]
#[cfg_attr(not(target_os = "linux"), ignore = "requires node")]
#[cfg_attr(not(target_env = "musl"), test_case("deno"))]
#[cfg_attr(not(target_env = "musl"), ignore = "requires node")]
fn stat_sync(runtime: &str) -> anyhow::Result<()> {
    let accesses = track_script(runtime, "try { fs.statSync('hello') } catch {}", &[])?;
    assert_contains(&accesses, current_dir().unwrap().join("hello").as_path(), AccessMode::READ);
    Ok(())
}

#[test_case("node")]
#[ignore = "requires node"]
#[cfg_attr(not(target_os = "linux"), test_case("bun"))]
#[cfg_attr(not(target_os = "linux"), ignore = "requires node")]
#[cfg_attr(not(target_env = "musl"), test_case("deno"))]
#[cfg_attr(not(target_env = "musl"), ignore = "requires node")]
fn create_read_stream(runtime: &str) -> anyhow::Result<()> {
    let accesses = track_script(
        runtime,
        "try { fs.createReadStream('hello').on('error', () => {}) } catch {}",
        &[],
    )?;
    assert_contains(&accesses, current_dir().unwrap().join("hello").as_path(), AccessMode::READ);
    Ok(())
}

#[test_case("node")]
#[ignore = "requires node"]
#[cfg_attr(not(target_os = "linux"), test_case("bun"))]
#[cfg_attr(not(target_os = "linux"), ignore = "requires node")]
#[cfg_attr(not(target_env = "musl"), test_case("deno"))]
#[cfg_attr(not(target_env = "musl"), ignore = "requires node")]
fn create_write_stream(runtime: &str) -> anyhow::Result<()> {
    let tmpdir = tempfile::tempdir()?;
    let file_path = tmpdir.path().join("hello");
    let accesses = track_script(
        runtime,
        "try { fs.createWriteStream(process.argv.at(-1)).on('error', () => {}) } catch {}",
        &[file_path.as_os_str()],
    )?;
    assert_contains(&accesses, file_path.as_path(), AccessMode::WRITE);
    Ok(())
}

#[test_case("node")]
#[ignore = "requires node"]
#[cfg_attr(not(target_os = "linux"), test_case("bun"))]
#[cfg_attr(not(target_os = "linux"), ignore = "requires node")]
#[cfg_attr(not(target_env = "musl"), test_case("deno"))]
#[cfg_attr(not(target_env = "musl"), ignore = "requires node")]
fn write_sync(runtime: &str) -> anyhow::Result<()> {
    let tmpdir = tempfile::tempdir()?;
    let file_path = tmpdir.path().join("hello");
    let accesses = track_script(
        runtime,
        "try { fs.writeFileSync(process.argv.at(-1), '') } catch {}",
        &[file_path.as_os_str()],
    )?;
    assert_contains(&accesses, &file_path, AccessMode::WRITE);
    Ok(())
}

#[test_case("node")]
#[ignore = "requires node"]
#[cfg_attr(not(target_os = "linux"), test_case("bun"))]
#[cfg_attr(not(target_os = "linux"), ignore = "requires node")]
#[cfg_attr(not(target_env = "musl"), test_case("deno"))]
#[cfg_attr(not(target_env = "musl"), ignore = "requires node")]
fn read_dir_sync(runtime: &str) -> anyhow::Result<()> {
    let accesses = track_script(runtime, "try { fs.readdirSync('.') } catch {}", &[])?;
    assert_contains(&accesses, &current_dir().unwrap(), AccessMode::READ_DIR);
    Ok(())
}

#[test_case("node")]
#[ignore = "requires node"]
#[cfg_attr(not(target_os = "linux"), test_case("bun"))]
#[cfg_attr(not(target_os = "linux"), ignore = "requires node")]
#[cfg_attr(not(target_env = "musl"), test_case("deno"))]
#[cfg_attr(not(target_env = "musl"), ignore = "requires node")]
fn subprocess(runtime: &str) -> anyhow::Result<()> {
    let cmd = if cfg!(windows) {
        r"'cmd', ['/c', 'type hello']"
    } else {
        r"'/bin/sh', ['-c', 'cat hello']"
    };
    let accesses = track_script(
        runtime,
        &format!("try {{ child_process.spawnSync({cmd}, {{ stdio: 'ignore' }}) }} catch {{}}"),
        &[],
    )?;
    assert_contains(&accesses, current_dir().unwrap().join("hello").as_path(), AccessMode::READ);
    Ok(())
}
