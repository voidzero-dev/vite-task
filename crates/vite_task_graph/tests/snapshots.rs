use std::path::Path;

use copy_dir::copy_dir;
use tokio::runtime::Runtime;
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_str::Str;
use vite_task_graph::{IndexedTaskGraph, TaskDependencyType, TaskId, loader::JsonUserConfigLoader};
use vite_workspace::find_workspace_root;

/// Create a stable json representation of the task graph for snapshot testing.
///
/// All paths are relative to `base_dir`.
fn snapshot_task_graph(
    indexed_task_graph: &IndexedTaskGraph,
    base_dir: &AbsolutePath,
) -> impl serde::Serialize {
    #[derive(serde::Serialize, PartialEq, PartialOrd, Eq, Ord)]
    struct TaskIdSnapshot {
        package_dir: RelativePathBuf,
        task_name: Str,
    }
    impl TaskIdSnapshot {
        fn from_task_id(
            task_id: &TaskId,
            base_dir: &AbsolutePath,
            indexed_task_graph: &IndexedTaskGraph,
        ) -> Self {
            Self {
                task_name: task_id.task_name.clone(),
                package_dir: indexed_task_graph
                    .get_package_path(task_id.package_index)
                    .strip_prefix(base_dir)
                    .unwrap()
                    .unwrap(),
            }
        }
    }

    #[derive(serde::Serialize)]
    struct TaskNodeSnapshot {
        id: TaskIdSnapshot,
        command: Str,
        cwd: RelativePathBuf,
        depends_on: Vec<(TaskIdSnapshot, TaskDependencyType)>,
    }

    let task_graph = indexed_task_graph.task_graph();
    let mut node_snapshots = Vec::<TaskNodeSnapshot>::with_capacity(task_graph.node_count());
    for task_index in task_graph.node_indices() {
        let task_node = &task_graph[task_index];
        let mut depends_on: Vec<(TaskIdSnapshot, TaskDependencyType)> = task_graph
            .edges_directed(task_index, petgraph::Direction::Outgoing)
            .map(|edge| {
                use petgraph::visit::EdgeRef as _;
                let target_node = &task_graph[edge.target()];
                (
                    TaskIdSnapshot::from_task_id(
                        &target_node.task_id,
                        base_dir,
                        indexed_task_graph,
                    ),
                    *edge.weight(),
                )
            })
            .collect();
        depends_on.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        node_snapshots.push(TaskNodeSnapshot {
            id: TaskIdSnapshot::from_task_id(&task_node.task_id, base_dir, indexed_task_graph),
            command: task_node.resolved_config.command.clone(),
            cwd: task_node.resolved_config.cwd.strip_prefix(base_dir).unwrap().unwrap(),
            depends_on,
        });
    }
    node_snapshots.sort_unstable_by(|a, b| a.id.cmp(&b.id));

    node_snapshots
}

fn run_case(runtime: &Runtime, tmpdir: &AbsolutePath, case_path: &Path) {
    let case_name = case_path.file_name().unwrap().to_str().unwrap();
    if case_name.starts_with(".") {
        return; // skip hidden files like .DS_Store
    }

    // Copy the case directory to a temporary directory to avoid discovering workspace outside of the test case.
    let case_stage_path = tmpdir.join(case_name);
    copy_dir(case_path, &case_stage_path).unwrap();

    let workspace_root = find_workspace_root(&case_stage_path).unwrap();

    assert_eq!(
        &case_stage_path, workspace_root.path,
        "folder '{}' should be a workspace root",
        case_name
    );

    runtime.block_on(async {
        let indexed_task_graph = vite_task_graph::IndexedTaskGraph::load(
            workspace_root,
            JsonUserConfigLoader::default(),
        )
        .await
        .expect(&format!("Failed to load task graph for case {case_name}"));
        let task_graph_snapshot = snapshot_task_graph(&indexed_task_graph, &case_stage_path);
        insta::assert_json_snapshot!("task graph", task_graph_snapshot);
    });
}

#[test]
fn test_snapshots() {
    let tokio_runtime = Runtime::new().unwrap();
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp_dir_path = AbsolutePath::new(tmp_dir.path()).unwrap();
    insta::glob!("fixtures/*", |case_path| run_case(&tokio_runtime, tmp_dir_path, case_path));
}
