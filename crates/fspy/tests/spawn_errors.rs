//! Top-level spawn error handling across Unix platforms (Linux `Target::Unresolved`
//! and macOS `EncodedPayload::prepare_exec`).
#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt as _};

use fspy::error::SpawnError;
use test_log::test;
use tokio_util::sync::CancellationToken;

fn assert_os_spawn_errno(result: Result<fspy::TrackedChild, SpawnError>, expected_errno: i32) {
    match result {
        Err(SpawnError::OsSpawn(err)) => {
            assert_eq!(
                err.raw_os_error(),
                Some(expected_errno),
                "expected errno {expected_errno}, got {err:?}"
            );
        }
        Err(other) => panic!("expected SpawnError::OsSpawn({expected_errno}), got {other:?}"),
        Ok(_) => panic!("expected SpawnError::OsSpawn({expected_errno}), but spawn succeeded"),
    }
}

#[test(tokio::test)]
async fn top_level_spawn_exec_errors_return_os_spawn_with_expected_errno() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let root = tmp_dir.path().canonicalize()?;

    // 1a. Malformed/empty shebang and overlong shebang -> ENOEXEC (rejected during
    // exec preparation on both Linux and macOS).
    let malformed_shebang = root.join("malformed_shebang.sh");
    fs::write(&malformed_shebang, "#!\n")?;
    fs::set_permissions(&malformed_shebang, fs::Permissions::from_mode(0o755))?;
    assert_os_spawn_errno(
        fspy::Command::new(&malformed_shebang).spawn(CancellationToken::new()).await,
        libc::ENOEXEC,
    );

    let overlong_shebang = root.join("overlong_shebang.sh");
    fs::write(&overlong_shebang, format!("#!/{}\n", "a".repeat(520)))?;
    fs::set_permissions(&overlong_shebang, fs::Permissions::from_mode(0o755))?;
    assert_os_spawn_errno(
        fspy::Command::new(&overlong_shebang).spawn(CancellationToken::new()).await,
        libc::ENOEXEC,
    );

    // 1b. Shebang-less executable -> ENOEXEC on Linux (`fspy_preload_linux` uses
    // raw `execve`, which does not fall back to `/bin/sh`, unlike macOS
    // `posix_spawnp` / `execvp` used by `tokio::process::Command`).
    #[cfg(target_os = "linux")]
    {
        let shebangless = root.join("shebangless");
        fs::write(&shebangless, "not a binary or shebang script\n")?;
        fs::set_permissions(&shebangless, fs::Permissions::from_mode(0o755))?;
        assert_os_spawn_errno(
            fspy::Command::new(&shebangless).spawn(CancellationToken::new()).await,
            libc::ENOEXEC,
        );
    }

    // 2. Script with missing interpreter -> ENOENT
    let missing_interp = root.join("missing_interp.sh");
    fs::write(&missing_interp, "#!/nonexistent/fspy_missing_interpreter\n")?;
    fs::set_permissions(&missing_interp, fs::Permissions::from_mode(0o755))?;
    assert_os_spawn_errno(
        fspy::Command::new(&missing_interp).spawn(CancellationToken::new()).await,
        libc::ENOENT,
    );

    // 3. Non-executable (0o644) file as shebang interpreter -> EACCES
    let non_exec = root.join("non_exec_file");
    fs::write(&non_exec, "plain file\n")?;
    fs::set_permissions(&non_exec, fs::Permissions::from_mode(0o644))?;
    let script_non_exec = root.join("via_non_exec.sh");
    fs::write(&script_non_exec, format!("#!{}\n", non_exec.display()))?;
    fs::set_permissions(&script_non_exec, fs::Permissions::from_mode(0o755))?;
    assert_os_spawn_errno(
        fspy::Command::new(&script_non_exec).spawn(CancellationToken::new()).await,
        libc::EACCES,
    );

    // 4. Directory as shebang interpreter -> EACCES
    let dir_target = root.join("a_directory");
    fs::create_dir(&dir_target)?;
    let script_dir = root.join("via_dir.sh");
    fs::write(&script_dir, format!("#!{}\n", dir_target.display()))?;
    fs::set_permissions(&script_dir, fs::Permissions::from_mode(0o755))?;
    assert_os_spawn_errno(
        fspy::Command::new(&script_dir).spawn(CancellationToken::new()).await,
        libc::EACCES,
    );

    // Direct top-level spawns of a non-executable 0o644 file or a directory are
    // rejected during `resolve_program()` with `SpawnError::Which`.
    assert!(matches!(
        fspy::Command::new(&non_exec).spawn(CancellationToken::new()).await,
        Err(SpawnError::Which { .. })
    ));
    assert!(matches!(
        fspy::Command::new(&dir_target).spawn(CancellationToken::new()).await,
        Err(SpawnError::Which { .. })
    ));

    Ok(())
}
