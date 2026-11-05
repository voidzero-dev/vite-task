//! Shell behavior that tracked tasks depend on. On macOS, fspy runs the
//! bundled Oils shell in place of `/bin/sh` and `/bin/bash`, so these tests
//! check that it behaves like the real shells, which run on Linux.
#![cfg(unix)]
mod test_utils;

use std::process::{Command, Stdio};

use test_log::test;

/// Runs `script` with `shell -c` and returns its exit code and stdout.
fn run(shell: &str, script: &str) -> (Option<i32>, String) {
    let output = Command::new(shell)
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .output()
        .expect("failed to run the shell");
    (output.status.code(), String::from_utf8(output.stdout).expect("stdout is not UTF-8"))
}

/// Under `set -e`, a loop with a redirect must not exit when its last
/// `test ... && ...` fails.
#[test(tokio::test)]
async fn errexit_ignores_status_of_compound_command_with_redirect() -> anyhow::Result<()> {
    track_fn!((), |(): ()| {
        let script = "set -e
found=0
while read -r line; do
  test \"$line\" = key && found=1
done <<EOF
key
other
EOF
echo reached $found";
        assert_eq!(run("/bin/sh", script), (Some(0), "reached 1\n".to_owned()));
    })
    .await?;
    Ok(())
}

/// An EXIT trap that saves `$?` and exits with it keeps the exit status.
#[test(tokio::test)]
async fn exit_trap_sees_exit_status() -> anyhow::Result<()> {
    track_fn!((), |(): ()| {
        let script = "trap 'st=$?; trap - EXIT; true; exit $st' EXIT; exit 7";
        assert_eq!(run("/bin/sh", script), (Some(7), String::new()));
    })
    .await?;
    Ok(())
}

/// A bare `exit` in an EXIT trap keeps the status from before the trap.
#[test(tokio::test)]
async fn bare_exit_in_exit_trap_keeps_status() -> anyhow::Result<()> {
    track_fn!((), |(): ()| {
        assert_eq!(run("/bin/sh", "trap exit EXIT; exit 3"), (Some(3), String::new()));
    })
    .await?;
    Ok(())
}

/// `$BASH` is set when the shell runs as `bash`.
#[test(tokio::test)]
async fn bash_variable_is_set() -> anyhow::Result<()> {
    track_fn!((), |(): ()| {
        let (code, stdout) = run("/bin/bash", "echo \"$BASH\"");
        assert_eq!(code, Some(0));
        assert_ne!(stdout.trim(), "", "$BASH is empty");
    })
    .await?;
    Ok(())
}
