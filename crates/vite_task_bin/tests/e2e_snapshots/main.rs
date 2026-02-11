mod redact;

use std::{
    env::{self, join_paths, split_paths},
    ffi::OsStr,
    sync::{Arc, mpsc},
    time::Duration,
};

use copy_dir::copy_dir;
use pty_terminal::{
    ExitStatus,
    geo::ScreenSize,
    terminal::{CommandBuilder, Terminal},
};
use redact::redact_e2e_output;
use vite_path::{AbsolutePath, AbsolutePathBuf, RelativePathBuf};
use vite_str::Str;
use vite_workspace::find_workspace_root;

/// Timeout for each step in e2e tests
const STEP_TIMEOUT: Duration = Duration::from_secs(10);

/// Screen size for the PTY terminal. Large enough to avoid line wrapping.
const SCREEN_SIZE: ScreenSize = ScreenSize { rows: 500, cols: 500 };

/// Get the shell executable for running e2e test steps.
/// On Unix, uses /bin/sh.
/// On Windows, uses BASH env var or falls back to Git Bash.
#[expect(
    clippy::disallowed_types,
    reason = "PathBuf required for CommandBuilder and std::path operations on shell executable"
)]
fn get_shell_exe() -> std::path::PathBuf {
    if cfg!(windows) {
        std::env::var_os("BASH").map_or_else(
            || {
                let git_bash = std::path::PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
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
            },
            std::path::PathBuf::from,
        )
    } else {
        std::path::PathBuf::from("/bin/sh")
    }
}

#[derive(serde::Deserialize, Debug)]
#[serde(transparent)]
struct Step(Str);

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

#[expect(clippy::disallowed_types, reason = "Path required by insta::glob! callback signature")]
fn run_case(tmpdir: &AbsolutePath, fixture_path: &std::path::Path, filter: Option<&str>) {
    let fixture_name = fixture_path.file_name().unwrap().to_str().unwrap();
    if fixture_name.starts_with('.') {
        return; // skip hidden files like .DS_Store
    }

    // Skip if filter doesn't match
    if let Some(f) = filter
        && !fixture_name.contains(f)
    {
        return;
    }
    #[expect(clippy::print_stdout, reason = "test progress output for e2e test runner")]
    {
        println!("{fixture_name}");
    }
    // Configure insta to write snapshots to fixture directory
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(fixture_path.join("snapshots"));
    settings.set_prepend_module_to_snapshot(false);
    settings.remove_snapshot_suffix();

    settings.bind(|| run_case_inner(tmpdir, fixture_path, fixture_name));
}

enum TerminationState {
    Exited(ExitStatus),
    TimedOut,
}

#[expect(
    clippy::too_many_lines,
    reason = "e2e test runner with process management necessarily has many lines"
)]
#[expect(
    clippy::disallowed_types,
    reason = "Path required by insta::glob! callback; String required by from_utf8_lossy and string accumulation"
)]
fn run_case_inner(tmpdir: &AbsolutePath, fixture_path: &std::path::Path, fixture_name: &str) {
    // Copy the case directory to a temporary directory to avoid discovering workspace outside of the test case.
    let stage_path = tmpdir.join(fixture_name);
    copy_dir(fixture_path, &stage_path).unwrap();

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

    // Navigate from CARGO_MANIFEST_DIR to packages/tools at the repo root
    #[expect(
        clippy::disallowed_types,
        reason = "Path required for CARGO_MANIFEST_DIR path manipulation via env! macro"
    )]
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
            // Include vp binary path to PATH so that e2e tests can run "vp ..." commands.
            {
                let vp_path = AbsolutePath::new(env!("CARGO_BIN_EXE_vp")).unwrap();
                let vp_dir = vp_path.parent().unwrap();
                vp_dir.as_path().as_os_str().into()
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

        let e2e_stage_path = tmpdir.join(vite_str::format!("{fixture_name}_e2e_stage_{e2e_count}"));
        e2e_count += 1;
        assert!(copy_dir(fixture_path, &e2e_stage_path).unwrap().is_empty());

        let e2e_stage_path_str = e2e_stage_path.as_path().to_str().unwrap();

        let mut e2e_outputs = String::new();
        for step in &e2e.steps {
            let mut cmd = CommandBuilder::new(&shell_exe);
            cmd.arg("-c");
            cmd.arg(step.0.as_str());
            cmd.env_clear();
            cmd.env("PATH", &e2e_env_path);
            cmd.env("NO_COLOR", "1");
            cmd.env("TERM", "dumb");
            cmd.cwd(e2e_stage_path.join(&e2e.cwd).as_path());

            // On Windows, inherit PATHEXT for executable lookup
            if cfg!(windows)
                && let Ok(pathext) = std::env::var("PATHEXT")
            {
                cmd.env("PATHEXT", pathext);
            }

            let mut terminal = Terminal::spawn(SCREEN_SIZE, cmd).unwrap();

            // Read to end on a separate thread with timeout via channel
            let mut killer = terminal.clone_killer();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let status = terminal.read_to_end();
                let screen = terminal.screen_contents();
                let _ = tx.send((status, screen));
            });

            let (termination_state, screen) = match rx.recv_timeout(STEP_TIMEOUT) {
                Ok((status, screen)) => (TerminationState::Exited(status.unwrap()), screen),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = killer.kill();
                    let (_, screen) = rx.recv().unwrap();
                    (TerminationState::TimedOut, screen)
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
                TerminationState::Exited(status) => {
                    let exit_code = status.exit_code();
                    if exit_code != 0 {
                        e2e_outputs.push_str(vite_str::format!("[{exit_code}]").as_str());
                    }
                }
            }

            e2e_outputs.push_str("> ");
            e2e_outputs.push_str(step.0.as_str());
            e2e_outputs.push('\n');

            e2e_outputs.push_str(&redact_e2e_output(screen, e2e_stage_path_str));
            e2e_outputs.push('\n');

            // Skip remaining steps if timed out
            if matches!(termination_state, TerminationState::TimedOut) {
                break;
            }
        }
        #[expect(
            clippy::disallowed_macros,
            reason = "insta::assert_snapshot! internally uses std::format!"
        )]
        {
            insta::assert_snapshot!(e2e.name.as_str(), e2e_outputs);
        }
    }
}

#[expect(clippy::disallowed_types, reason = "Path required by insta::glob! macro callback")]
#[expect(
    clippy::disallowed_methods,
    reason = "current_dir needed because insta::glob! requires std PathBuf"
)]
fn main() {
    let filter = std::env::args().nth(1);

    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp_dir_path = AbsolutePathBuf::new(tmp_dir.path().canonicalize().unwrap()).unwrap();

    let tests_dir = std::env::current_dir().unwrap().join("tests");

    insta::glob!(tests_dir, "e2e_snapshots/fixtures/*", |case_path| {
        run_case(&tmp_dir_path, case_path, filter.as_deref());
    });
}
