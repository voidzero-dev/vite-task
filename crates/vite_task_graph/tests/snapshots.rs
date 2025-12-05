use std::{
    env::{current_dir, var_os},
    path::{Path, PathBuf},
};

use copy_dir::copy_dir;
use tokio::runtime::Runtime;
use vite_path::AbsolutePath;
use vite_task_graph::loader::JsonUserConfigLoader;
use vite_workspace::{discover_package_graph, find_workspace_root};

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
        let task_graph =
            vite_task_graph::TaskGraph::load(workspace_root, JsonUserConfigLoader::default())
                .await
                .expect(&format!("Failed to load task graph for case {case_name}"));
        let task_graph_snaphost = task_graph.snapshot(&case_stage_path);
        insta::assert_json_snapshot!("task graph", task_graph_snaphost);
    });
}

#[test]
fn test_snapshots() {
    let tokio_runtime = Runtime::new().unwrap();
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp_dir_path = AbsolutePath::new(tmp_dir.path()).unwrap();
    insta::glob!("fixtures/*", |case_path| run_case(&tokio_runtime, tmp_dir_path, case_path));
}
