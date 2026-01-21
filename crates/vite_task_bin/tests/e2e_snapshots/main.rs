mod redact;

use std::{
    env::{self, join_paths, split_paths},
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use copy_dir::copy_dir;
use expectrl::Expect;
use redact::redact_e2e_output;
use tokio::{io::AsyncReadExt, process::Command};
use vite_path::{AbsolutePath, AbsolutePathBuf, RelativePathBuf};
use vite_str::Str;
use vite_workspace::find_workspace_root;

/// Timeout for each step in e2e tests
const STEP_TIMEOUT: Duration = Duration::from_secs(10);

/// Get the shell executable for running e2e test steps.
/// On Unix, uses /bin/sh.
/// On Windows, uses BASH env var or falls back to Git Bash.
fn get_shell_exe() -> PathBuf {
    if cfg!(windows) {
        if let Some(bash) = std::env::var_os("BASH") {
            PathBuf::from(bash)
        } else {
            let git_bash = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
            if git_bash.exists() {
                git_bash
            } else {
                panic!(
                    "Could not find bash executable for e2e tests.\n\
                     Please set the BASH environment variable to point to a bash executable,\n\
                     or install Git for Windows which provides bash at:\n\
                     C:\\Program Files\\Git\\bin\\bash.exe"
                );
            }
        }
    } else {
        PathBuf::from("/bin/sh")
    }
}

#[derive(serde::Deserialize, Debug)]
#[serde(untagged)]
enum Step {
    Simple(Str),
    Interactive { cmd: Str, interactive: bool },
}

impl Step {
    fn cmd(&self) -> &str {
        match self {
            Step::Simple(s) => s.as_str(),
            Step::Interactive { cmd, .. } => cmd.as_str(),
        }
    }

    fn is_interactive(&self) -> bool {
        match self {
            Step::Simple(_) => false,
            Step::Interactive { interactive, .. } => *interactive,
        }
    }
}

#[derive(serde::Deserialize, Debug)]
struct E2e {
    pub name: Str,
    #[serde(default)]
    pub cwd: RelativePathBuf,
    pub steps: Vec<Step>,
    /// Optional platform filter: "unix" or "windows". If set, test only runs on that platform.
    #[serde(default)]
    pub platform: Option<Str>,
}

#[derive(serde::Deserialize, Default)]
struct SnapshotsFile {
    #[serde(rename = "e2e", default)] // toml usually uses singular for arrays
    pub e2e_cases: Vec<E2e>,
}

struct InteractiveResult {
    output: String,
    exit_code: Option<i32>,
}

/// Run a step interactively using expectrl with PTY.
///
/// Watches for `[write-stdin:...]` patterns in stdout:
/// - `[write-stdin:content]` - writes "content\n" to stdin
/// - `[write-stdin:]` - signals EOF (closes stdin)
async fn run_interactive_step(
    #[cfg_attr(windows, allow(unused_variables))] shell_exe: &Path,
    cmd: &str,
    cwd: &Path,
    env_path: &OsStr,
) -> std::io::Result<InteractiveResult> {
    // Build the command - use PowerShell on Windows for ConPTY support and UNC path handling
    #[cfg(windows)]
    let command = {
        let mut ps = std::process::Command::new("powershell.exe");
        // -NoProfile: don't load user profile (faster startup)
        // -NonInteractive: no interactive prompts
        // -Command: run the specified command
        ps.args(["-NoProfile", "-NonInteractive", "-Command", cmd]).current_dir(cwd);
        ps.env_clear().env("PATH", env_path).env("NO_COLOR", "1");
        if let Ok(pathext) = std::env::var("PATHEXT") {
            ps.env("PATHEXT", pathext);
        }
        ps
    };

    #[cfg(unix)]
    let command = {
        let mut bash = std::process::Command::new(shell_exe);
        bash.arg("-c").arg(cmd).current_dir(cwd);
        bash.env_clear().env("PATH", env_path).env("NO_COLOR", "1");
        bash
    };

    // Run the synchronous expectrl code in a blocking task
    tokio::task::spawn_blocking(move || {
        // Spawn with PTY using expectrl's sync API
        let mut session = expectrl::session::Session::spawn(command)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        // Set a timeout for expect operations (8 seconds to leave buffer for outer timeout)
        session.set_expect_timeout(Some(std::time::Duration::from_secs(8)));

        let mut output = String::new();
        let write_stdin_pattern = expectrl::Regex(r"\[write-stdin:([^\]]*)\]");

        loop {
            // Use expectrl's expect() to wait for the pattern
            let expect_result = session.expect(&write_stdin_pattern);

            match expect_result {
                Ok(found) => {
                    // Append any output before the match
                    let before = String::from_utf8_lossy(found.before());
                    output.push_str(&before);

                    // Append the matched pattern itself (keep it visible in output)
                    let matched = String::from_utf8_lossy(found.as_bytes());
                    output.push_str(&matched);

                    // Extract the content from the capture group
                    let content = found
                        .get(1)
                        .map(|m| String::from_utf8_lossy(m).to_string())
                        .unwrap_or_default();

                    if content.is_empty() {
                        // Small delay to let the PTY process any pending data before EOF
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        // EOF signal - send Ctrl-D (EOF character) to close stdin
                        session
                            .send(&[4]) // ASCII 4 = Ctrl-D = EOF
                            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                        // Read any remaining output until process ends
                        let remaining = session.expect(expectrl::Eof);
                        if let Ok(eof_found) = remaining {
                            output.push_str(&String::from_utf8_lossy(eof_found.before()));
                        }
                        break;
                    } else {
                        // Write content to stdin
                        let to_write = format!("{}\n", content);
                        session
                            .send(to_write.as_bytes())
                            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                        // Small delay to let the PTY process the input
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
                Err(expectrl::Error::Eof) => {
                    // Process ended without matching pattern - collect what we have
                    use std::io::Read;
                    let mut remaining = Vec::new();
                    let _ = session.read_to_end(&mut remaining);
                    output.push_str(&String::from_utf8_lossy(&remaining));
                    break;
                }
                Err(expectrl::Error::ExpectTimeout) => {
                    // Timeout waiting for pattern - collect what we have
                    use std::io::Read;
                    let mut remaining = Vec::new();
                    let _ = session.read_to_end(&mut remaining);
                    let remaining_str = String::from_utf8_lossy(&remaining);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!(
                            "expectrl timeout waiting for [write-stdin:] pattern. Output so far: '{}'. Remaining: '{}'",
                            output, remaining_str
                        ),
                    ));
                }
                Err(e) => {
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, e));
                }
            }
        }

        // Get exit status (platform-specific)
        #[cfg(unix)]
        let exit_code = {
            use expectrl::process::unix::WaitStatus;
            session.get_process().wait().ok().and_then(|status| match status {
                WaitStatus::Exited(_, code) => Some(code),
                WaitStatus::Signaled(_, _, _) => None,
                _ => None,
            })
        };

        #[cfg(windows)]
        let exit_code = {
            // conpty's wait(timeout) returns Result<u32> directly
            session
                .get_process()
                .wait(None)
                .ok()
                .map(|code| code as i32)
        };

        Ok(InteractiveResult { output, exit_code })
    })
    .await
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
}

fn run_case(
    runtime: &tokio::runtime::Runtime,
    tmpdir: &AbsolutePath,
    fixture_path: &Path,
    filter: Option<&str>,
) {
    let fixture_name = fixture_path.file_name().unwrap().to_str().unwrap();
    if fixture_name.starts_with(".") {
        return; // skip hidden files like .DS_Store
    }

    // Skip if filter doesn't match
    if let Some(f) = filter {
        if !fixture_name.contains(f) {
            return;
        }
    }
    println!("{}", fixture_name);
    // Configure insta to write snapshots to fixture directory
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(fixture_path.join("snapshots"));
    settings.set_prepend_module_to_snapshot(false);
    settings.remove_snapshot_suffix();

    // Use block_on inside bind to run async code with insta settings applied
    settings.bind(|| runtime.block_on(run_case_inner(tmpdir, fixture_path, fixture_name)));
}

async fn run_case_inner(tmpdir: &AbsolutePath, fixture_path: &Path, fixture_name: &str) {
    // Copy the case directory to a temporary directory to avoid discovering workspace outside of the test case.
    let stage_path = tmpdir.join(fixture_name);
    copy_dir(fixture_path, &stage_path).unwrap();

    let (workspace_root, _cwd) = find_workspace_root(&stage_path).unwrap();

    assert_eq!(
        &stage_path, &*workspace_root.path,
        "folder '{}' should be a workspace root",
        fixture_name
    );

    let cases_toml_path = fixture_path.join("snapshots.toml");
    let cases_file: SnapshotsFile = match std::fs::read(&cases_toml_path) {
        Ok(content) => toml::from_slice(&content).unwrap(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(err) => panic!("Failed to read cases.toml for fixture {}: {}", fixture_name, err),
    };

    // Navigate from CARGO_MANIFEST_DIR to packages/tools at the repo root
    let repo_root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    let test_bin_path = Arc::<OsStr>::from(
        repo_root.join("packages").join("tools").join("node_modules").join(".bin").into_os_string(),
    );

    // Get shell executable for running steps
    let shell_exe = get_shell_exe();

    // Prepare PATH for e2e tests
    let e2e_env_path = join_paths(
        [
            // Include vite binary path to PATH so that e2e tests can run "vite ..." commands.
            {
                let vite_path = AbsolutePath::new(env!("CARGO_BIN_EXE_vite")).unwrap();
                let vite_dir = vite_path.parent().unwrap();
                vite_dir.as_path().as_os_str().into()
            },
            // Include packages/tools to PATH so that e2e tests can run utilities such as replace-file-content.
            test_bin_path,
        ]
        .into_iter()
        .chain(
            // the existing PATH
            split_paths(&env::var_os("PATH").unwrap())
                .map(|path| Arc::<OsStr>::from(path.into_os_string())),
        ),
    )
    .unwrap();

    let mut e2e_count = 0u32;
    for e2e in cases_file.e2e_cases {
        // Skip test if platform doesn't match
        if let Some(platform) = &e2e.platform {
            let should_run = match platform.as_str() {
                "unix" => cfg!(unix),
                "windows" => cfg!(windows),
                other => panic!("Unknown platform '{}' in test '{}'", other, e2e.name),
            };
            if !should_run {
                continue;
            }
        }

        let e2e_stage_path = tmpdir.join(format!("{}_e2e_stage_{}", fixture_name, e2e_count));
        e2e_count += 1;
        assert!(copy_dir(fixture_path, &e2e_stage_path).unwrap().is_empty());

        let e2e_stage_path_str = e2e_stage_path.as_path().to_str().unwrap();

        let mut e2e_outputs = String::new();
        for step in e2e.steps {
            enum TerminationState {
                Exited { exit_code: Option<i32> },
                TimedOut,
            }

            let (termination_state, stdout, stderr) = if step.is_interactive() {
                // Interactive mode: use expectrl with PTY
                let timeout = tokio::time::sleep(STEP_TIMEOUT);
                tokio::pin!(timeout);

                let step_cwd = e2e_stage_path.join(&e2e.cwd);
                let interactive_fut =
                    run_interactive_step(&shell_exe, step.cmd(), step_cwd.as_path(), &e2e_env_path);

                tokio::select! {
                    result = interactive_fut => {
                        match result {
                            Ok(interactive_result) => {
                                (
                                    TerminationState::Exited { exit_code: interactive_result.exit_code },
                                    interactive_result.output,
                                    String::new(), // PTY combines stdout/stderr
                                )
                            }
                            Err(e) => {
                                panic!("Interactive step failed: {}", e);
                            }
                        }
                    }
                    _ = &mut timeout => {
                        (TerminationState::TimedOut, String::new(), String::new())
                    }
                }
            } else {
                // Non-interactive mode: use tokio::process::Command
                let mut cmd = Command::new(&shell_exe);
                cmd.arg("-c")
                    .arg(step.cmd())
                    .env_clear()
                    .env("PATH", &e2e_env_path)
                    .env("NO_COLOR", "1")
                    .current_dir(e2e_stage_path.join(&e2e.cwd));

                // On Windows, inherit PATHEXT for executable lookup
                if cfg!(windows) {
                    if let Ok(pathext) = std::env::var("PATHEXT") {
                        cmd.env("PATHEXT", pathext);
                    }
                }

                // Spawn the child process
                cmd.stdin(Stdio::null());
                cmd.stdout(Stdio::piped());
                cmd.stderr(Stdio::piped());

                let mut child = cmd.spawn().unwrap();

                // Take stdout/stderr handles
                let mut stdout_handle = child.stdout.take().unwrap();
                let mut stderr_handle = child.stderr.take().unwrap();

                // Buffers for accumulating output
                let mut stdout_buf = Vec::new();
                let mut stderr_buf = Vec::new();

                // Read chunks concurrently with process wait, using select! with timeout
                let mut stdout_done = false;
                let mut stderr_done = false;

                // Initial state is running
                let mut term_state: Option<TerminationState> = None;

                let timeout = tokio::time::sleep(STEP_TIMEOUT);
                tokio::pin!(timeout);

                loop {
                    let mut stdout_chunk = [0u8; 8192];
                    let mut stderr_chunk = [0u8; 8192];

                    tokio::select! {
                        result = stdout_handle.read(&mut stdout_chunk), if !stdout_done => {
                            match result {
                                Ok(0) => stdout_done = true,
                                Ok(n) => stdout_buf.extend_from_slice(&stdout_chunk[..n]),
                                Err(_) => stdout_done = true,
                            }
                        }
                        result = stderr_handle.read(&mut stderr_chunk), if !stderr_done => {
                            match result {
                                Ok(0) => stderr_done = true,
                                Ok(n) => stderr_buf.extend_from_slice(&stderr_chunk[..n]),
                                Err(_) => stderr_done = true,
                            }
                        }
                        result = child.wait(), if term_state.is_none() => {
                            let status = result.unwrap();
                            term_state = Some(TerminationState::Exited { exit_code: status.code() });
                        }
                        _ = &mut timeout, if term_state.is_none() => {
                            // Timeout - kill the process
                            let _ = child.kill().await;
                            term_state = Some(TerminationState::TimedOut);
                        }
                    }

                    // Exit conditions:
                    // 1. Process exited and all output drained
                    // 2. Timed out and all output drained (after kill, pipes close)
                    if term_state.is_some() && stdout_done && stderr_done {
                        break;
                    }
                }

                let stdout = String::from_utf8_lossy(&stdout_buf).into_owned();
                let stderr = String::from_utf8_lossy(&stderr_buf).into_owned();

                // term_state is guaranteed to be Some here due to the break condition
                (term_state.unwrap(), stdout, stderr)
            };

            // Format output
            match &termination_state {
                TerminationState::TimedOut => {
                    e2e_outputs.push_str("[timeout]");
                }
                TerminationState::Exited { exit_code } => {
                    let code = exit_code.unwrap_or(-1);
                    if code != 0 {
                        e2e_outputs.push_str(format!("[{}]", code).as_str());
                    }
                }
            }

            e2e_outputs.push_str("> ");
            e2e_outputs.push_str(step.cmd());
            e2e_outputs.push('\n');

            e2e_outputs.push_str(&redact_e2e_output(stdout, e2e_stage_path_str));
            e2e_outputs.push_str(&redact_e2e_output(stderr, e2e_stage_path_str));
            e2e_outputs.push('\n');

            // Skip remaining steps if timed out
            if matches!(termination_state, TerminationState::TimedOut) {
                break;
            }
        }
        insta::assert_snapshot!(e2e.name.as_str(), e2e_outputs);
    }
}

fn main() {
    let filter = std::env::args().nth(1);

    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp_dir_path = AbsolutePathBuf::new(tmp_dir.path().canonicalize().unwrap()).unwrap();

    let tests_dir = std::env::current_dir().unwrap().join("tests");

    // Create tokio runtime for async operations
    let runtime = tokio::runtime::Runtime::new().unwrap();

    insta::glob!(tests_dir, "e2e_snapshots/fixtures/*", |case_path| {
        run_case(&runtime, &tmp_dir_path, case_path, filter.as_deref())
    });
}
