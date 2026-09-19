use std::process::Command;

#[test]
fn report_cli_without_workspace_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let output =
        Command::new(std::env::var_os("CARGO_BIN_EXE_vt").expect("CARGO_BIN_EXE_vt not set"))
            .args(["run", "--report-unchanged"])
            .env_remove("VP_RUN_IPC_NAME")
            .env_remove("VP_RUN_NODE_CLIENT_PATH")
            .current_dir(dir.path())
            .output()
            .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn report_cli_with_unreachable_runner_fails() {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    let address = dir.path().join("missing.sock").into_os_string();
    #[cfg(windows)]
    let address = std::ffi::OsString::from(r"\\.\pipe\vp-report-no-such-server");
    let output =
        Command::new(std::env::var_os("CARGO_BIN_EXE_vt").expect("CARGO_BIN_EXE_vt not set"))
            .args(["run", "--report-unchanged"])
            .env("VP_RUN_IPC_NAME", address)
            .current_dir(dir.path())
            .output()
            .unwrap();
    assert!(!output.status.success());
    assert!(!output.stderr.is_empty());
}
