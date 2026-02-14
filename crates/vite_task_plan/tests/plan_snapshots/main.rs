mod redact;

use std::{ffi::OsStr, sync::Arc};

use clap::Parser;
use copy_dir::copy_dir;
use cow_utils::CowUtils as _;
use redact::redact_snapshot;
use rustc_hash::FxHashMap;
use tokio::runtime::Runtime;
use vite_path::{AbsolutePath, AbsolutePathBuf, RelativePathBuf};
use vite_str::Str;
use vite_task::{Command, Session};
use vite_task_plan::ExecutionPlan;
use vite_workspace::find_workspace_root;

/// Local parser wrapper for `BuiltInCommand`
#[derive(Parser)]
#[command(name = "vp")]
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

#[expect(clippy::disallowed_types, reason = "Path required by insta::glob! callback signature")]
fn run_case(
    runtime: &Runtime,
    tmpdir: &AbsolutePath,
    fixture_path: &std::path::Path,
    filter: Option<&str>,
) {
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
    #[expect(clippy::print_stdout, reason = "test progress output for plan snapshot test runner")]
    {
        println!("{fixture_name}");
    }
    // Configure insta to write snapshots to fixture directory
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(fixture_path.join("snapshots"));
    settings.set_prepend_module_to_snapshot(false);
    settings.remove_snapshot_suffix();

    settings.bind(|| run_case_inner(runtime, tmpdir, fixture_path, fixture_name));
}

#[expect(
    clippy::disallowed_types,
    reason = "Path required by insta::glob! callback; String required by std::fs::read and toml::from_slice"
)]
#[expect(clippy::too_many_lines, reason = "test setup and assertion logic in a single function")]
fn run_case_inner(
    runtime: &Runtime,
    tmpdir: &AbsolutePath,
    fixture_path: &std::path::Path,
    fixture_name: &str,
) {
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
        reason = "Path required for CARGO_MANIFEST_DIR path manipulation to locate packages/tools"
    )]
    let test_bin_path = {
        let repo_root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
        Arc::<OsStr>::from(
            repo_root
                .join("packages")
                .join("tools")
                .join("node_modules")
                .join(".bin")
                .into_os_string(),
        )
    };

    // Add packages/tools to PATH so test programs (such as print-file) in fixtures can be found.
    let plan_envs: FxHashMap<Arc<OsStr>, Arc<OsStr>> = [
        (Arc::<OsStr>::from(OsStr::new("PATH")), Arc::clone(&test_bin_path)),
        (Arc::<OsStr>::from(OsStr::new("NO_COLOR")), Arc::<OsStr>::from(OsStr::new("1"))),
    ]
    .into_iter()
    .collect();

    runtime.block_on(async {
        let workspace_root_str = workspace_root.path.as_path().to_str().unwrap();
        let mut owned_callbacks = vite_task_bin::OwnedSessionCallbacks::default();
        let mut session = Session::init_with(
            plan_envs,
            Arc::clone(&workspace_root.path),
            owned_callbacks.as_callbacks(),
        )
        .unwrap();

        let task_graph_result = session.ensure_task_graph_loaded().await;
        let task_graph = match task_graph_result {
            Ok(task_graph) => task_graph,
            Err(err) => {
                let err_formatted = vite_str::format!("{err:#}");
                let err_str = err_formatted.as_str().cow_replace(workspace_root_str, "<workspace>");
                let err_str =
                    if cfg!(windows) { err_str.as_ref().cow_replace('\\', "/") } else { err_str };
                #[expect(
                    clippy::disallowed_macros,
                    reason = "insta::assert_snapshot! internally uses std::format!"
                )]
                {
                    insta::assert_snapshot!("task graph load error", err_str.as_ref());
                }
                return;
            }
        };
        let task_graph_json = redact_snapshot(
            &vite_graph_ser::SerializeByKey(task_graph.task_graph()),
            workspace_root_str,
        );
        insta::assert_json_snapshot!("task graph", task_graph_json);

        for plan in cases_file.plan_cases {
            let snapshot_name = vite_str::format!("query - {}", plan.name);

            let cli = match Cli::try_parse_from(
                std::iter::once("vp") // dummy program name
                    .chain(plan.args.iter().map(vite_str::Str::as_str)),
            ) {
                Ok(ok) => ok,
                Err(err) => {
                    #[expect(
                        clippy::disallowed_macros,
                        reason = "insta::assert_snapshot! internally uses std::format!"
                    )]
                    {
                        insta::assert_snapshot!(snapshot_name.as_str(), err);
                    }
                    continue;
                }
            };
            let Cli::Command(command) = cli;
            let Command::Run(run_command) = command else {
                panic!("only `run` commands supported in plan tests")
            };

            let plan_result = session
                .plan_from_cli_run(workspace_root.path.join(plan.cwd).into(), run_command)
                .await;

            let plan = match plan_result {
                Ok(graph) => ExecutionPlan::from_execution_graph(graph),
                Err(err) => {
                    // Format the full error chain using anyhow's `{:#}` formatter
                    // and redact workspace paths for snapshot stability.
                    let anyhow_err: anyhow::Error = err.into();
                    let err_formatted = vite_str::format!("{anyhow_err:#}");
                    let err_str =
                        err_formatted.as_str().cow_replace(workspace_root_str, "<workspace>");
                    let err_str = if cfg!(windows) {
                        err_str.as_ref().cow_replace('\\', "/")
                    } else {
                        err_str
                    };
                    #[expect(
                        clippy::disallowed_macros,
                        reason = "insta::assert_snapshot! internally uses std::format!"
                    )]
                    {
                        insta::assert_snapshot!(snapshot_name.as_str(), err_str.as_ref());
                    }
                    continue;
                }
            };

            let plan_json = redact_snapshot(&plan, workspace_root_str);
            insta::assert_json_snapshot!(snapshot_name.as_str(), &plan_json);
        }
    });
}

#[expect(clippy::disallowed_types, reason = "Path required by insta::glob! macro callback")]
#[expect(
    clippy::disallowed_methods,
    reason = "current_dir needed because insta::glob! requires std PathBuf"
)]
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
