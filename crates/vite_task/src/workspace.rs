use std::sync::{Arc, LazyLock, OnceLock};

use vite_path::AbsolutePath;
use vite_workspace::{WorkspaceFile, find_workspace_root, get_package_graph};

/// Type alias for a LazyLock that uses a Boxed FnOnce to initialize the value.
type LazyLockWithBoxFn<T> = LazyLock<T, Box<dyn FnOnce() -> T>>;

/// Represents a lazily loaded workspace.
/// No IO is performed in initialization.
/// The workspace discovery and task graph are lazily loaded when the respective methods are called.
pub struct Workspace {
    discovered: LazyLockWithBoxFn<Result<DiscoveredWorkspace, Arc<vite_workspace::Error>>>,
}

struct DiscoveredWorkspace {
    root_path: Arc<AbsolutePath>,
    task_graph: LazyLockWithBoxFn<Result<LoadedTaskGraph, Arc<crate::error::Error>>>,
}

impl DiscoveredWorkspace {
    fn discover(cwd: &AbsolutePath) -> Result<Self, vite_workspace::Error> {
        let workspace_root = find_workspace_root(cwd)?;
        let root_path = workspace_root.path.to_absolute_path_buf().into();
        Ok(DiscoveredWorkspace {
            root_path: Arc::clone(&root_path),
            task_graph: LazyLock::new(Box::new(move || {
                Ok(LoadedTaskGraph::load(&root_path, workspace_root.workspace_file)?)
            })),
        })
    }
}

struct LoadedTaskGraph {}
impl LoadedTaskGraph {
    fn load(
        workspace_root: &AbsolutePath,
        workspace_file: WorkspaceFile,
    ) -> Result<Self, crate::error::Error> {
        Ok(LoadedTaskGraph {})
    }
}

impl Workspace {
    fn new(cwd: Arc<AbsolutePath>) -> Self {
        Self {
            discovered: LazyLock::new(Box::new(move || {
                DiscoveredWorkspace::discover(&cwd).map_err(Arc::new)
            })),
        }
    }

    fn discover_once(&self) -> anyhow::Result<&DiscoveredWorkspace> {
        let discovered =
            self.discovered.as_ref().map_err(|err| anyhow::Error::from(Arc::clone(&err)));
        Ok(discovered?)
    }

    /// Get the root path of the workspace.
    /// This will trigger workspace discovery if not already done.
    pub fn get_root(&self) -> anyhow::Result<&Arc<AbsolutePath>> {
        Ok(&self.discover_once()?.root_path)
    }

    fn load_task_graph_once(&self) -> anyhow::Result<&LoadedTaskGraph> {
        let discovered = self.discover_once()?;
        let task_graph =
            discovered.task_graph.as_ref().map_err(|err| anyhow::Error::from(Arc::clone(&err)));
        Ok(task_graph?)
    }

    pub fn get_task_graph(&self) -> anyhow::Result<&()> {
        let discovered_workspace = self.discover_once()?;
        let package_graph = get_package_graph(&discovered_workspace.root_path)?;
        todo!()
    }
}
