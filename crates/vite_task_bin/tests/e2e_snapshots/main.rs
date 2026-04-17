mod redact;

use std::{
    env::{self, join_paths, split_paths},
    ffi::OsStr,
    io::Write,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use cp_r::CopyOptions;
use pty_terminal::{geo::ScreenSize, terminal::CommandBuilder};
use pty_terminal_test::TestTerminal;
use redact::redact_e2e_output;
use vec1::Vec1;
use vite_path::{AbsolutePath, AbsolutePathBuf, RelativePathBuf};
use vite_str::Str;
use vite_workspace::find_workspace_root;

/// Timeout for each step in e2e tests.
/// Windows CI needs a longer timeout due to Git Bash startup overhead and slower I/O.
const STEP_TIMEOUT: Duration =
    if cfg!(windows) { Duration::from_secs(60) } else { Duration::from_secs(20) };

/// Screen size for the PTY terminal. Large enough to avoid line wrapping.
const SCREEN_SIZE: ScreenSize = ScreenSize { rows: 500, cols: 500 };

#[derive(serde::Deserialize, Debug)]
#[serde(untagged)]
enum Step {
    /// Shorthand: `["vt", "run", "build"]`
    Simple(Vec1<Str>),
    /// Detailed: `{ argv = ["vt", "run"], comment = "cache miss", ... }`
    Detailed(StepConfig),
}

#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct StepConfig {
    argv: Vec1<Str>,
    /// Appended as `# comment` in the snapshot display line.
    #[serde(default)]
    comment: Option<Str>,
    /// Extra environment variables set for this step.
    #[serde(default)]
    envs: Vec<(Str, Str)>,
    #[serde(default)]
    interactions: Vec<Interaction>,
}

impl Step {
    fn argv(&self) -> &[Str] {
        match self {
            Self::Simple(argv) => argv,
            Self::Detailed(config) => &config.argv,
        }
    }

    /// Format as a shell-like display string for snapshots (e.g. `MY_ENV=1 vt run test # cache miss`).
    #[expect(clippy::disallowed_types, reason = "String required by join/format")]
    fn display_command(&self) -> String {
        let argv_str = self
            .argv()
            .iter()
            .map(|a| {
                let s = a.as_str();
                if s.contains(|c: char| c.is_whitespace() || c == '"') {
                    shell_escape::escape(s.into())
                } else {
                    s.into()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");

        match self {
            Self::Simple(_) => argv_str,
            Self::Detailed(config) => {
                let mut parts = String::new();
                for (k, v) in &config.envs {
                    parts.push_str(vite_str::format!("{k}={v} ").as_str());
                }
                parts.push_str(&argv_str);
                if let Some(comment) = &config.comment {
                    parts.push_str(vite_str::format!(" # {comment}").as_str());
                }
                parts
            }
        }
    }

    fn interactions(&self) -> &[Interaction] {
        match self {
            Self::Detailed(config) => &config.interactions,
            Self::Simple(_) => &[],
        }
    }

    fn envs(&self) -> &[(Str, Str)] {
        match self {
            Self::Detailed(config) => &config.envs,
            Self::Simple(_) => &[],
        }
    }
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(untagged)]
enum Interaction {
    ExpectMilestone(ExpectMilestoneInteraction),
    Write(WriteInteraction),
    WriteLine(WriteLineInteraction),
    WriteKey(WriteKeyInteraction),
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct ExpectMilestoneInteraction {
    #[serde(rename = "expect-milestone")]
    expect_milestone: Str,
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct WriteInteraction {
    write: Str,
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct WriteLineInteraction {
    #[serde(rename = "write-line")]
    write_line: Str,
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct WriteKeyInteraction {
    #[serde(rename = "write-key")]
    write_key: WriteKey,
}

#[derive(serde::Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum WriteKey {
    Up,
    Down,
    Enter,
    Escape,
    Backspace,
    CtrlC,
}

impl WriteKey {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Enter => "enter",
            Self::Escape => "escape",
            Self::Backspace => "backspace",
            Self::CtrlC => "ctrl-c",
        }
    }

    const fn bytes(self) -> &'static [u8] {
        match self {
            Self::Up => b"\x1b[A",
            Self::Down => b"\x1b[B",
            Self::Enter => b"\r",
            Self::Escape => b"\x1b",
            Self::Backspace => b"\x7f",
            Self::CtrlC => b"\x03",
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

#[expect(clippy::disallowed_types, reason = "Path required for fixture path handling")]
fn run_case(tmpdir: &AbsolutePath, fixture_path: &std::path::Path) -> Result<(), String> {
    let fixture_name = fixture_path.file_name().unwrap().to_str().unwrap();
    let snapshots = snapshot_test::Snapshots::new(fixture_path.join("snapshots"));
    run_case_inner(tmpdir, fixture_path, fixture_name, &snapshots)
}

enum TerminationState {
    Exited(i64),
    TimedOut,
}

#[expect(
    clippy::too_many_lines,
    reason = "e2e test runner with process management necessarily has many lines"
)]
#[expect(
    clippy::disallowed_types,
    reason = "Path required for fixture handling; String required by from_utf8_lossy and string accumulation"
)]
fn run_case_inner(
    tmpdir: &AbsolutePath,
    fixture_path: &std::path::Path,
    fixture_name: &str,
    snapshots: &snapshot_test::Snapshots,
) -> Result<(), String> {
    // Copy the case directory to a temporary directory to avoid discovering workspace outside of the test case.
    let stage_path = tmpdir.join(fixture_name);
    CopyOptions::new().copy_tree(fixture_path, stage_path.as_path()).unwrap();

    let (workspace_root, _cwd) = find_workspace_root(&stage_path).unwrap();

    assert_eq!(
        &stage_path, &*workspace_root.path,
        "folder '{fixture_name}' should be a workspace root"
    );

    let cases_toml_path = fixture_path.join("snapshots.toml");
    let cases_file: SnapshotsFile = match std::fs::read(&cases_toml_path) {
        Ok(content) => toml::from_slice(&content).unwrap(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => SnapshotsFile::default(),
        Err(err) => panic!("Failed to read cases.toml for fixture {fixture_name}: {err}"),
    };

    // Prepare PATH for e2e tests: include vt and vtt binary directories.
    let bin_dirs: [Arc<OsStr>; 2] = ["CARGO_BIN_EXE_vt", "CARGO_BIN_EXE_vtt"].map(|var| {
        let bin_path = env::var_os(var).unwrap_or_else(|| panic!("{var} not set"));
        let bin = AbsolutePathBuf::new(std::path::PathBuf::from(bin_path)).unwrap();
        Arc::<OsStr>::from(bin.parent().unwrap().as_path().as_os_str())
    });
    let e2e_env_path = join_paths(
        bin_dirs.iter().cloned().chain(
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

        let e2e_stage_path = tmpdir.join(vite_str::format!("{fixture_name}_e2e_stage_{e2e_count}"));
        e2e_count += 1;
        CopyOptions::new().copy_tree(fixture_path, e2e_stage_path.as_path()).unwrap();

        let e2e_stage_path_str = e2e_stage_path.as_path().to_str().unwrap();

        let mut e2e_outputs = String::new();
        for step in &e2e.steps {
            let step_display = step.display_command();

            let argv = step.argv();

            // Only vt and vtt are allowed as step programs.
            let program = argv[0].as_str();
            assert!(
                program == "vt" || program == "vtt",
                "step program must be 'vt' or 'vtt', got '{program}'"
            );
            let exe_env = vite_str::format!("CARGO_BIN_EXE_{program}");
            let resolved =
                env::var_os(exe_env.as_str()).unwrap_or_else(|| panic!("{exe_env} not set"));
            let mut cmd = CommandBuilder::new(resolved);
            for arg in &argv[1..] {
                cmd.arg(arg.as_str());
            }
            cmd.env_clear();
            cmd.env("PATH", &e2e_env_path);
            cmd.env("NO_COLOR", "1");
            cmd.env("TERM", "dumb");
            // On Windows, ensure common executable extensions are included in PATHEXT for command resolution in subprocesses.
            if cfg!(windows) {
                cmd.env("PATHEXT", ".COM;.EXE;.BAT;.CMD;.VBS;.VBE;.JS;.JSE;.WSF;.WSH;.MSC");
            }
            for (k, v) in step.envs() {
                cmd.env(k.as_str(), v.as_str());
            }
            cmd.cwd(e2e_stage_path.join(&e2e.cwd).as_path());

            let terminal = TestTerminal::spawn(SCREEN_SIZE, cmd).unwrap();
            let mut killer = terminal.child_handle.clone();
            let interactions = step.interactions().to_vec();
            let output = Arc::new(Mutex::new(String::new()));
            let output_for_thread = Arc::clone(&output);
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let mut terminal = terminal;

                for interaction in interactions {
                    match interaction {
                        Interaction::ExpectMilestone(expect) => {
                            output_for_thread.lock().unwrap().push_str(
                                vite_str::format!(
                                    "@ expect-milestone: {}\n",
                                    expect.expect_milestone
                                )
                                .as_str(),
                            );
                            let milestone_screen =
                                terminal.reader.expect_milestone(expect.expect_milestone.as_str());
                            let mut output = output_for_thread.lock().unwrap();
                            output.push_str(&milestone_screen);
                            output.push('\n');
                        }
                        Interaction::Write(write) => {
                            output_for_thread
                                .lock()
                                .unwrap()
                                .push_str(vite_str::format!("@ write: {}\n", write.write).as_str());
                            terminal.writer.write_all(write.write.as_str().as_bytes()).unwrap();
                            terminal.writer.flush().unwrap();
                        }
                        Interaction::WriteLine(write_line) => {
                            output_for_thread.lock().unwrap().push_str(
                                vite_str::format!("@ write-line: {}\n", write_line.write_line)
                                    .as_str(),
                            );
                            terminal
                                .writer
                                .write_line(write_line.write_line.as_str().as_bytes())
                                .unwrap();
                        }
                        Interaction::WriteKey(write_key) => {
                            let key_name = write_key.write_key.as_str();
                            output_for_thread
                                .lock()
                                .unwrap()
                                .push_str(vite_str::format!("@ write-key: {key_name}\n").as_str());
                            terminal.writer.write_all(write_key.write_key.bytes()).unwrap();
                            terminal.writer.flush().unwrap();
                        }
                    }
                }

                let status = terminal.reader.wait_for_exit().unwrap();
                let screen = terminal.reader.screen_contents();

                {
                    let mut output = output_for_thread.lock().unwrap();
                    if !output.is_empty() && !output.ends_with('\n') {
                        output.push('\n');
                    }
                    output.push_str(&screen);
                }

                let _ = tx.send(i64::from(status.exit_code()));
            });

            let (termination_state, output) = match rx.recv_timeout(STEP_TIMEOUT) {
                Ok(exit_code) => {
                    let output = output.lock().unwrap().clone();
                    (TerminationState::Exited(exit_code), output)
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = killer.kill();
                    let output = output.lock().unwrap().clone();
                    (TerminationState::TimedOut, output)
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("Terminal thread panicked");
                }
            };

            // Format output
            match &termination_state {
                TerminationState::TimedOut => {
                    e2e_outputs.push_str("[timeout]");
                }
                TerminationState::Exited(exit_code) => {
                    if *exit_code != 0 {
                        e2e_outputs.push_str(vite_str::format!("[{exit_code}]").as_str());
                    }
                }
            }

            e2e_outputs.push_str("> ");
            e2e_outputs.push_str(&step_display);
            e2e_outputs.push('\n');

            e2e_outputs.push_str(&redact_e2e_output(output, e2e_stage_path_str));
            e2e_outputs.push('\n');

            // Skip remaining steps if timed out
            if matches!(termination_state, TerminationState::TimedOut) {
                break;
            }
        }
        snapshots.check_snapshot(vite_str::format!("{}.snap", e2e.name).as_str(), &e2e_outputs)?;
    }
    Ok(())
}

#[expect(clippy::disallowed_types, reason = "Path required for CARGO_MANIFEST_DIR path traversal")]
fn main() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp_dir_path = AbsolutePathBuf::new(tmp_dir.path().canonicalize().unwrap()).unwrap();

    let manifest_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());

    // Copy .node-version to the tmp dir so version manager shims can resolve the correct
    // Node.js binary when running task commands.
    let repo_root = manifest_dir.parent().unwrap().parent().unwrap();
    std::fs::copy(repo_root.join(".node-version"), tmp_dir.path().join(".node-version")).unwrap();

    let fixtures_dir = manifest_dir.join("tests/e2e_snapshots/fixtures");

    let mut fixture_paths = std::fs::read_dir(fixtures_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| !n.starts_with('.')))
        .collect::<Vec<_>>();
    fixture_paths.sort();

    let mut args = libtest_mimic::Arguments::from_args();
    // On Linux, running e2e fixtures in parallel causes PTY and signal-routing
    // contention (ctrl-c test intermittently fails). macOS and Windows are
    // unaffected, so only force sequential execution on Linux.
    if cfg!(target_os = "linux") && args.test_threads.is_none() {
        args.test_threads = Some(1);
    }

    let tests: Vec<libtest_mimic::Trial> = fixture_paths
        .into_iter()
        .map(|fixture_path| {
            let name = fixture_path.file_name().unwrap().to_str().unwrap().to_owned();
            let tmp_dir_path = tmp_dir_path.clone();
            libtest_mimic::Trial::test(name, move || {
                run_case(&tmp_dir_path, &fixture_path).map_err(Into::into)
            })
        })
        .collect();

    libtest_mimic::run(&args, tests).exit();
}
