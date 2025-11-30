use vite_path::AbsolutePathBuf;

/// Lazy-loading the package graph and task graph in the workspace.
pub struct Workspace {
    root_path: AbsolutePathBuf,
}
