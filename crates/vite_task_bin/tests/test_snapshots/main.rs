use core::panic;
use std::{
    borrow::Cow, collections::HashMap, convert::Infallible, ffi::OsStr, path::Path, sync::Arc,
};

use clap::Parser;
use copy_dir::copy_dir;
use insta::internals::Content;
use petgraph::visit::EdgeRef as _;
use serde::Serialize;
use tokio::runtime::Runtime;
use vite_path::{AbsolutePath, RelativePathBuf, redaction::redact_absolute_paths};
use vite_str::Str;
use vite_task::{CLIArgs, Session};
use vite_task_bin::CustomTaskSubcommand;
use vite_task_graph::config::DEFAULT_PASSTHROUGH_ENVS;
use vite_workspace::find_workspace_root;

fn visit_json(value: &mut serde_json::Value, f: &mut impl FnMut(&mut serde_json::Value)) {
    f(value);
    match value {
        serde_json::Value::Array(arr) => {
            for item in arr {
                visit_json(item, f);
            }
        }
        serde_json::Value::Object(map) => {
            for (_key, val) in map {
                visit_json(val, f);
            }
        }
        _ => {}
    }
}

fn redact_paths(value: &mut serde_json::Value, redactions: &[(&str, &str)]) {
    use cow_utils::CowUtils as _;
    visit_json(value, &mut |v| {
        if let serde_json::Value::String(s) = v {
            for (from, to) in redactions {
                if let Cow::Owned(mut replaced) = s.as_str().cow_replace(from, to) {
                    if cfg!(windows) {
                        // Also replace with backslashes on Windows
                        replaced = replaced.cow_replace("\\", "/").into_owned();
                    }
                    *s = replaced;
                }
            }
        }
    });
}

fn redact_snapshot(value: &impl Serialize, workspace_root: &str) -> serde_json::Value {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let mut json_value = serde_json::to_value(value).unwrap();
    redact_paths(
        &mut json_value,
        &[(workspace_root, "<workspace>"), (manifest_dir.as_str(), "<manifest_dir>")],
    );

    visit_json(&mut json_value, &mut |v| {
        let serde_json::Value::Array(array) = v else {
            return;
        };
        let contains_all_default_pass_through_envs =
            DEFAULT_PASSTHROUGH_ENVS.iter().all(|default_pass_through_envs| {
                array.iter().any(|item| {
                    if let serde_json::Value::String(s) = item {
                        s == *default_pass_through_envs
                    } else {
                        false
                    }
                })
            });
        // Remove default pass-through envs from snapshots to reduce noise
        if contains_all_default_pass_through_envs {
            array.retain(|item| {
                if let serde_json::Value::String(s) = item {
                    !DEFAULT_PASSTHROUGH_ENVS.contains(&s.as_str())
                } else {
                    true
                }
            });
            array.push(serde_json::Value::String("<default pass-through envs>".to_string()));
        }
    });

    json_value
}

#[derive(serde::Deserialize, Debug)]
struct Plan {
    pub name: Str,
    pub args: Vec<Str>,
    #[serde(default)]
    pub cwd: RelativePathBuf,
}

#[derive(serde::Deserialize, Debug)]
struct E2e {
    pub steps: Vec<Str>,
}

#[derive(serde::Deserialize, Default)]
struct SnapshotsFile {
    #[serde(rename = "plan", default)] // toml usually uses singular for arrays
    pub plans: Vec<Plan>,
    #[serde(rename = "e2e", default)] // toml usually uses singular for arrays
    pub e2es: Vec<E2e>,
}

fn run_case(runtime: &Runtime, tmpdir: &AbsolutePath, fixture_path: &Path) {
    let fixture_name = fixture_path.file_name().unwrap().to_str().unwrap();
    if fixture_name.starts_with(".") {
        return; // skip hidden files like .DS_Store
    }

    // Copy the case directory to a temporary directory to avoid discovering workspace outside of the test case.
    let stage_path = tmpdir.join(fixture_name);
    copy_dir(fixture_path, &stage_path).unwrap();

    // let mut settings = insta::Settings::clone_current();
    // let case_stage_path_str = case_stage_path.as_path().to_str().expect("path is valid unicode");
    // settings.add_filter(&regex::escape(case_stage_path_str), "<workspace>");
    // let _guard = settings.bind_to_scope();

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

    // Add bins to PATH so test programs (such as readfile) in fixtures can be found.
    let envs: HashMap<Arc<OsStr>, Arc<OsStr>> = [(
        Arc::<OsStr>::from(OsStr::new("PATH")),
        Arc::<OsStr>::from(
            std::env::current_dir()
                .unwrap()
                .join("test_bins")
                .join("node_modules")
                .join(".bin")
                .into_os_string(),
        ),
    )]
    .into_iter()
    .collect();

    runtime.block_on(async {
        let workspace_root_str = workspace_root.path.as_path().to_str().unwrap();
        let mut owned_callbacks = vite_task_bin::OwnedSessionCallbacks::default();
        let mut session = Session::init_with(
            envs,
            Arc::clone(&workspace_root.path),
            owned_callbacks.as_callbacks(),
        )
        .unwrap();

        let task_graph_json = redact_snapshot(
            &vite_graph_ser::SerializeByKey(
                session.ensure_task_graph_loaded().await.unwrap().task_graph(),
            ),
            workspace_root_str,
        );
        insta::assert_json_snapshot!("task graph", task_graph_json);

        for plan in cases_file.plans {
            let snapshot_name = format!("query - {}", plan.name);

            let cli_args = CLIArgs::<CustomTaskSubcommand, Infallible>::try_parse_from(
                std::iter::once("vite") // dummy program name
                    .chain(plan.args.iter().map(|s| s.as_str())),
            )
            .expect(&format!(
                "Failed to parse CLI args for plan '{}' in '{}'",
                plan.name, fixture_name
            ));

            let task_cli_args = match cli_args {
                CLIArgs::Task(task_cli_args) => task_cli_args,
                CLIArgs::NonTask(never) => match never {},
            };

            let plan_result =
                session.plan(workspace_root.path.join(plan.cwd).into(), task_cli_args).await;

            let plan = match plan_result {
                Ok(plan) => plan,
                Err(err) => {
                    insta::assert_debug_snapshot!(snapshot_name, err);
                    continue;
                }
            };

            let plan_json = redact_snapshot(&plan, workspace_root_str);
            insta::assert_json_snapshot!(snapshot_name, &plan_json);

            //     let cwd: Arc<AbsolutePath> = case_stage_path.join(&cli_query.cwd).into();
            //     let task_query = match cli_task_query.into_task_query(&cwd) {
            //         Ok(ok) => ok,
            //         Err(err) => {
            //             insta::assert_json_snapshot!(snapshot_name, err);
            //             continue;
            //         }
            //     };

            //     let execution_graph = match indexed_task_graph.query_tasks(task_query) {
            //         Ok(ok) => ok,
            //         Err(err) => {
            //             insta::assert_json_snapshot!(snapshot_name, err);
            //             continue;
            //         }
            //     };

            //     let execution_graph_snapshot =
            //         snapshot_execution_graph(&execution_graph, &indexed_task_graph);
            //     insta::assert_json_snapshot!(snapshot_name, execution_graph_snapshot);
        }
    });
}

#[test]
fn test_snapshots() {
    let tokio_runtime = Runtime::new().unwrap();
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp_dir_path = AbsolutePath::new(tmp_dir.path()).unwrap();

    let tests_dir = std::env::current_dir().unwrap().join("tests");

    insta::glob!(tests_dir, "test_snapshots/fixtures/*", |case_path| run_case(
        &tokio_runtime,
        tmp_dir_path,
        case_path
    ));
}
