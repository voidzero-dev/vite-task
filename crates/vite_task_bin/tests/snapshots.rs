use core::panic;
use std::{path::Path, sync::Arc};

use clap::Parser;
use copy_dir::copy_dir;
use petgraph::visit::EdgeRef as _;
use tokio::runtime::Runtime;
use vite_path::{AbsolutePath, RelativePathBuf, redaction::redact_absolute_paths};
use vite_str::Str;
use vite_task_graph::{
    IndexedTaskGraph, TaskDependencyType, TaskNodeIndex,
    loader::JsonUserConfigLoader,
    query::{TaskExecutionGraph, cli::CLITaskQuery},
};
use vite_workspace::find_workspace_root;

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

/// Create a stable json representation of the task graph for snapshot testing.
///
/// All paths are relative to `base_dir`.
fn snapshot_execution_graph(
    execution_graph: &TaskExecutionGraph,
    indexed_task_graph: &IndexedTaskGraph,
) -> impl serde::Serialize {
    #[derive(serde::Serialize, PartialEq)]
    struct ExecutionNodeSnapshot {
        task: TaskIdSnapshot,
        deps: Vec<TaskIdSnapshot>,
    }

    let mut execution_node_snapshots = Vec::<ExecutionNodeSnapshot>::new();
    for task_index in execution_graph.nodes() {
        let mut deps = execution_graph
            .neighbors(task_index)
            .map(|dep_index| TaskIdSnapshot::new(dep_index, indexed_task_graph))
            .collect::<Vec<_>>();
        deps.sort_unstable();

        execution_node_snapshots.push(ExecutionNodeSnapshot {
            task: TaskIdSnapshot::new(task_index, indexed_task_graph),
            deps,
        });
    }
    execution_node_snapshots.sort_unstable_by(|a, b| a.task.cmp(&b.task));
    execution_node_snapshots
}

#[derive(serde::Deserialize)]
struct CLIQuery {
    pub name: Str,
    pub args: Vec<Str>,
    pub cwd: RelativePathBuf,
}

#[derive(serde::Deserialize, Default)]
struct CLIQueriesFile {
    #[serde(rename = "query")] // toml usually uses singular for arrays
    pub queries: Vec<CLIQuery>,
}

fn run_case(runtime: &Runtime, tmpdir: &AbsolutePath, case_path: &Path) {
    let case_name = case_path.file_name().unwrap().to_str().unwrap();
    if case_name.starts_with(".") {
        return; // skip hidden files like .DS_Store
    }

    // Copy the case directory to a temporary directory to avoid discovering workspace outside of the test case.
    let case_stage_path = tmpdir.join(case_name);
    copy_dir(case_path, &case_stage_path).unwrap();

    let (workspace_root, _cwd) = find_workspace_root(&case_stage_path).unwrap();

    assert_eq!(
        &case_stage_path, &*workspace_root.path,
        "folder '{}' should be a workspace root",
        case_name
    );

    let cli_queries_toml_path = case_path.join("cli-queries.toml");
    let cli_queries_file: CLIQueriesFile = match std::fs::read(&cli_queries_toml_path) {
        Ok(content) => toml::from_slice(&content).unwrap(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(err) => panic!("Failed to read cli-queries.toml for case {}: {}", case_name, err),
    };

    runtime.block_on(async {
        let _redaction_guard = redact_absolute_paths(&workspace_root.path);

        let indexed_task_graph = vite_task_graph::IndexedTaskGraph::load(
            &workspace_root,
            &JsonUserConfigLoader::default(),
        )
        .await
        .expect(&format!("Failed to load task graph for case {case_name}"));

        let task_graph_snapshot = snapshot_task_graph(&indexed_task_graph);
        insta::assert_json_snapshot!("task graph", task_graph_snapshot);

        for cli_query in cli_queries_file.queries {
            let snapshot_name = format!("query - {}", cli_query.name);

            let cli_task_query = CLITaskQuery::try_parse_from(
                std::iter::once("vite-run") // dummy program name
                    .chain(cli_query.args.iter().map(|s| s.as_str())),
            )
            .expect(&format!(
                "Failed to parse CLI args for query '{}' in case '{}'",
                cli_query.name, case_name
            ));

            let cwd: Arc<AbsolutePath> = case_stage_path.join(&cli_query.cwd).into();
            let task_query = match cli_task_query.into_task_query(&cwd) {
                Ok(ok) => ok,
                Err(err) => {
                    insta::assert_json_snapshot!(snapshot_name, err);
                    continue;
                }
            };

            let execution_graph = match indexed_task_graph.query_tasks(task_query) {
                Ok(ok) => ok,
                Err(err) => {
                    insta::assert_json_snapshot!(snapshot_name, err);
                    continue;
                }
            };

            let execution_graph_snapshot =
                snapshot_execution_graph(&execution_graph, &indexed_task_graph);
            insta::assert_json_snapshot!(snapshot_name, execution_graph_snapshot);
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
