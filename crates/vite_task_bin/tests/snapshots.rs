use core::panic;
use std::{path::Path, sync::Arc};

use clap::Parser;
use copy_dir::copy_dir;
use insta::internals::Content;
use petgraph::visit::EdgeRef as _;
use tokio::runtime::Runtime;
use vite_path::{AbsolutePath, RelativePathBuf, redaction::redact_absolute_paths};
use vite_str::Str;
use vite_task::Session;
use vite_task_graph::{
    IndexedTaskGraph, TaskDependencyType, TaskNodeIndex,
    loader::JsonUserConfigLoader,
    query::{TaskExecutionGraph, cli::CLITaskQuery},
};
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

#[derive(serde::Serialize, PartialEq, PartialOrd, Eq, Ord)]
struct TaskIdSnapshot {
    package_dir: Arc<AbsolutePath>,
    task_name: Str,
}
impl TaskIdSnapshot {
    fn new(task_index: TaskNodeIndex, indexed_task_graph: &IndexedTaskGraph) -> Self {
        let task_display = &indexed_task_graph.task_graph()[task_index].task_display;
        Self {
            task_name: task_display.task_name.clone(),
            package_dir: Arc::clone(&task_display.package_path),
        }
    }
}

/// Create a stable json representation of the task graph for snapshot testing.
///
/// All paths are relative to `base_dir`.
fn snapshot_task_graph(indexed_task_graph: &IndexedTaskGraph) -> impl serde::Serialize {
    #[derive(serde::Serialize)]
    struct TaskNodeSnapshot {
        id: TaskIdSnapshot,
        command: Str,
        cwd: Arc<AbsolutePath>,
        depends_on: Vec<(TaskIdSnapshot, TaskDependencyType)>,
    }

    let task_graph = indexed_task_graph.task_graph();
    let mut node_snapshots = Vec::<TaskNodeSnapshot>::with_capacity(task_graph.node_count());
    for task_index in task_graph.node_indices() {
        let task_node = &task_graph[task_index];
        let mut depends_on: Vec<(TaskIdSnapshot, TaskDependencyType)> = task_graph
            .edges_directed(task_index, petgraph::Direction::Outgoing)
            .map(|edge| (TaskIdSnapshot::new(edge.target(), indexed_task_graph), *edge.weight()))
            .collect();
        depends_on.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        node_snapshots.push(TaskNodeSnapshot {
            id: TaskIdSnapshot::new(task_index, indexed_task_graph),
            command: task_node.resolved_config.command.clone(),
            cwd: Arc::clone(&task_node.resolved_config.resolved_options.cwd),
            depends_on,
        });
    }
    node_snapshots.sort_unstable_by(|a, b| a.id.cmp(&b.id));

    node_snapshots
}

#[derive(serde::Deserialize, Debug)]
struct Plan {
    pub name: Str,
    pub args: Vec<Str>,
    pub cwd: RelativePathBuf,
}

#[derive(serde::Deserialize, Debug)]
struct E2e {
    pub steps: Vec<Str>,
}

#[derive(serde::Deserialize, Default)]
struct PlansFile {
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
    let cases_file: PlansFile = match std::fs::read(&cases_toml_path) {
        Ok(content) => toml::from_slice(&content).unwrap(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(err) => panic!("Failed to read cases.toml for fixture {}: {}", fixture_name, err),
    };

    runtime.block_on(async {
        let _redaction_guard = redact_absolute_paths(&workspace_root.path);

        let mut owned_callbacks = vite_task_bin::OwnedSessionCallbacks::default();
        let mut session = Session::init_with(
            Default::default(),
            Arc::clone(&workspace_root.path),
            owned_callbacks.as_callbacks(),
        )
        .unwrap();

        insta::assert_ron_snapshot!(
            "task graph",
            vite_graph_ser::SerializeByKey(
                session.ensure_task_graph_loaded().await.unwrap().task_graph()
            )
        );

        for plan in cases_file.plans {
            let snapshot_name = format!("query - {}", plan.name);

            let cli_task_query = CLITaskQuery::try_parse_from(
                std::iter::once("vite") // dummy program name
                    .chain(plan.args.iter().map(|s| s.as_str())),
            )
            .expect(&format!(
                "Failed to parse CLI args for plan '{}' in '{}'",
                plan.name, fixture_name
            ));

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

    insta::glob!("fixtures/*", |case_path| run_case(&tokio_runtime, tmp_dir_path, case_path));
}
