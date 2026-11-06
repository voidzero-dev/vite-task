//! The document `vp run --json` prints: every package and task in the workspace.

mod document;
mod schema;

use std::{collections::BTreeMap, sync::Arc};

pub use document::TaskGraph;
use document::{Command, Package, Task, Version, task_id};
use petgraph::visit::{EdgeRef as _, IntoNodeReferences as _};
pub use schema::schema_url;
use vt_graph::{IndexedTaskGraph, TaskGraphLoadError};
use vt_path::AbsolutePath;
use vt_str::Str;
use vt_workspace::{DependencyType, PackageNodeIndex};

/// Builds the document from the task graph, loading every package's config, as listing
/// the tasks does.
pub async fn build(
    graph: &IndexedTaskGraph<'_>,
    workspace_root: &AbsolutePath,
    schema: Str,
) -> Result<TaskGraph, TaskGraphLoadError> {
    let package_graph = graph.package_graph().package_graph();
    let package_key = |package_index: PackageNodeIndex| {
        let path = package_graph[package_index].path.as_str();
        Str::from(if path.is_empty() { "." } else { path })
    };

    let mut packages = BTreeMap::new();
    for (package_index, package_info) in package_graph.node_references() {
        let name = &package_info.package_json.name;
        let mut package =
            Package { name: (!name.is_empty()).then(|| name.clone()), ..Package::default() };
        for edge in package_graph.edges(package_index) {
            let dependencies = match edge.weight() {
                DependencyType::Normal => &mut package.dependencies,
                DependencyType::Dev => &mut package.dev_dependencies,
                DependencyType::Peer => &mut package.peer_dependencies,
            };
            dependencies.push(package_key(edge.target()));
        }
        for dependencies in [
            &mut package.dependencies,
            &mut package.dev_dependencies,
            &mut package.peer_dependencies,
        ] {
            dependencies.sort_unstable();
        }
        packages.insert(package_key(package_index), package);
    }

    let mut tasks = BTreeMap::new();
    for task in graph.tasks().await? {
        let package = package_key(task.id.package_index);
        let mut depends_on: Vec<Str> = graph
            .depends_on(&task.id)
            .await?
            .map(|dependency| {
                task_id(&package_key(dependency.id.package_index), &dependency.id.task_name)
            })
            .collect();
        depends_on.sort_unstable();
        let cwd = task.resolved_config.resolved_options.cwd.clean();
        let cwd = (cwd.as_absolute_path() != &*package_graph[task.id.package_index].absolute_path)
            .then(|| path_key(&cwd, workspace_root));
        tasks.insert(
            task_id(&package, &task.id.task_name),
            Task {
                package,
                name: task.id.task_name.clone(),
                cwd,
                command: Command::new(&task.resolved_config.commands),
                depends_on,
            },
        );
    }

    Ok(TaskGraph {
        schema,
        version: Version,
        // `clean` drops a trailing separator, which a workspace root found from such a cwd keeps.
        workspace_root: Arc::from(workspace_root.clean().as_absolute_path()),
        packages,
        tasks,
    })
}

/// `path` relative to the workspace root, with `/` separators and `"."` for the root
/// itself, or absolute if it's outside the workspace.
fn path_key(path: &AbsolutePath, workspace_root: &AbsolutePath) -> Str {
    match path.strip_prefix(workspace_root) {
        Ok(Some(relative)) if relative.as_str().is_empty() => Str::from("."),
        Ok(Some(relative)) => Str::from(relative.as_str()),
        Ok(None) | Err(_) => Str::from(path.as_path().to_string_lossy().as_ref()),
    }
}
