mod redact;

use std::{collections::HashMap, ffi::OsStr, path::Path, sync::Arc};

use clap::Parser;
use copy_dir::copy_dir;
use redact::redact_snapshot;
use tokio::runtime::Runtime;
use vite_path::{AbsolutePath, AbsolutePathBuf, RelativePathBuf};
use vite_str::Str;
use vite_task::{Command, Session};
use vite_workspace::find_workspace_root;

/// Local parser wrapper for BuiltInCommand
#[derive(Parser)]
#[command(name = "vite")]
enum Cli {
    #[clap(flatten)]
    Command(Command),
}

#[derive(serde::Deserialize, Debug)]
struct Plan {
    pub name: Str,
    pub args: Vec<Str>,
    #[serde(default)]
    pub cwd: RelativePathBuf,
}

#[derive(serde::Deserialize, Default)]
struct SnapshotsFile {
    #[serde(rename = "plan", default)] // toml usually uses singular for arrays
    pub plan_cases: Vec<Plan>,
}

fn run_case(runtime: &Runtime, tmpdir: &AbsolutePath, fixture_path: &Path, filter: Option<&str>) {
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

    settings.bind(|| run_case_inner(runtime, tmpdir, fixture_path, fixture_name));
}

fn run_case_inner(
    runtime: &Runtime,
    tmpdir: &AbsolutePath,
    fixture_path: &Path,
    fixture_name: &str,
) {
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

    // Add packages/tools to PATH so test programs (such as print-file) in fixtures can be found.
    let plan_envs: HashMap<Arc<OsStr>, Arc<OsStr>> = [
        (Arc::<OsStr>::from(OsStr::new("PATH")), Arc::clone(&test_bin_path)),
        (Arc::<OsStr>::from(OsStr::new("NO_COLOR")), Arc::<OsStr>::from(OsStr::new("1"))),
    ]
    .into_iter()
    .collect();

    runtime.block_on(async {
        let workspace_root_str = workspace_root.path.as_path().to_str().unwrap();
        let mut owned_callbacks = vite_task_bin::OwnedSessionCallbacks::default();
        let mut session = Session::init_with(
            plan_envs.into(),
            Arc::clone(&workspace_root.path),
            owned_callbacks.as_callbacks(),
        )
        .unwrap();

        let task_graph_result = session.ensure_task_graph_loaded().await;
        let task_graph = match task_graph_result {
            Ok(task_graph) => task_graph,
            Err(err) => {
                let mut err_str = format!("{err:#}").replace(workspace_root_str, "<workspace>");
                if cfg!(windows) {
                    err_str = err_str.replace('\\', "/");
                }
                insta::assert_snapshot!("task graph load error", err_str);
                return;
            }
        };
        let task_graph_json = redact_snapshot(
            &vite_graph_ser::SerializeByKey(task_graph.task_graph()),
            workspace_root_str,
        );
        insta::assert_json_snapshot!("task graph", task_graph_json);

        for plan in cases_file.plan_cases {
            let snapshot_name = format!("query - {}", plan.name);

            let cli = match Cli::try_parse_from(
                std::iter::once("vp") // dummy program name
                    .chain(plan.args.iter().map(|s| s.as_str())),
            ) {
                Ok(ok) => ok,
                Err(err) => {
                    insta::assert_snapshot!(snapshot_name, err);
                    continue;
                }
            };
            let Cli::Command(command) = cli;
            let run_command = match command {
                Command::Run(run_command) => run_command,
                _ => panic!("only `run` commands supported in plan tests"),
            };

            let plan_result =
                session.plan_from_cli(workspace_root.path.join(plan.cwd).into(), run_command).await;

            let plan = match plan_result {
                Ok(plan) => plan,
                Err(err) => {
                    insta::assert_debug_snapshot!(snapshot_name, err);
                    continue;
                }
            };

            let plan_json = redact_snapshot(&plan, workspace_root_str);
            insta::assert_json_snapshot!(snapshot_name, &plan_json);
        }
    });
}

fn main() {
    let filter = std::env::args().nth(1);

    let tokio_runtime = Runtime::new().unwrap();
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp_dir_path = AbsolutePathBuf::new(tmp_dir.path().canonicalize().unwrap()).unwrap();

    let tests_dir = std::env::current_dir().unwrap().join("tests");

    insta::glob!(tests_dir, "plan_snapshots/fixtures/*", |case_path| run_case(
        &tokio_runtime,
        &tmp_dir_path,
        case_path,
        filter.as_deref()
    ));
}
