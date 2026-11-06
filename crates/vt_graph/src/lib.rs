pub mod config;
pub mod display;
pub mod lazy_graph;
pub mod loader;
pub mod query;
mod specifier;
mod task_loader;

use std::{convert::Infallible, rc::Rc, sync::Arc};

use config::{GlobResolver, ResolvedGlobalCacheConfig, ResolvedTaskConfig};
use serde::Serialize;
pub use specifier::TaskSpecifier;
use vt_path::AbsolutePath;
use vt_str::Str;
use vt_workspace::{PackageNodeIndex, WorkspaceRoot, package_graph::IndexedPackageGraph};

use crate::{
    display::TaskDisplay, lazy_graph::LazyGraph, loader::UserConfigLoader, task_loader::TaskLoader,
};

/// Uniquely identifies a task, by its name and the package where it's defined.
#[derive(Debug, PartialEq, Eq, Hash, Clone, PartialOrd, Ord)]
pub struct TaskId {
    /// The index of the package where the task is defined.
    pub package_index: PackageNodeIndex,

    /// The name of the script or the entry in `vite.config.*`.
    pub task_name: Str,
}

/// Whether a task originates from the `tasks` map or from a package.json script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum TaskSource {
    /// Defined in the `tasks` map in the workspace config.
    TaskConfig,
    /// Pure package.json script (not in the tasks map).
    PackageJsonScript,
}

/// A node in the task graph, representing a task with its resolved configuration.
#[derive(Debug, Serialize)]
pub struct TaskNode {
    /// The key the task graph stores this task under.
    #[serde(skip)]
    pub id: TaskId,

    /// Printing the task in a human-readable way.
    pub task_display: TaskDisplay,

    /// The resolved configuration of this task.
    ///
    /// This contains information affecting how the task is spawn,
    /// whereas `task_id` is for looking up the task.
    ///
    /// However, it does not contain external factors like additional args from cli and env vars.
    pub resolved_config: ResolvedTaskConfig,

    /// Whether this task comes from the tasks map or a package.json script.
    pub source: TaskSource,
}

#[derive(Debug, thiserror::Error)]
pub enum TaskGraphLoadError {
    #[error("Failed to load package graph")]
    PackageGraphLoadError(#[from] vt_workspace::Error),

    #[error("Failed to load task config file for package at {}", .package_path.as_path().display())]
    ConfigLoadError {
        package_path: Arc<AbsolutePath>,
        #[source]
        error: anyhow::Error,
    },

    #[error(
        "Task {task_display} conflicts with a package.json script of the same name. \
         Remove the script from package.json or rename the task"
    )]
    ScriptConflict { task_display: TaskDisplay },

    #[error("Failed to resolve task config for task {task_display}")]
    ResolveConfigError {
        task_display: TaskDisplay,
        #[source]
        error: crate::config::ResolveTaskConfigError,
    },

    #[error("Failed to lookup dependency '{specifier}' for task {task_display}")]
    DependencySpecifierLookupError {
        specifier: Str,
        task_display: TaskDisplay,
        #[source]
        error: Box<SpecifierLookupError>,
    },

    #[error("`cache` can only be set in the workspace root config, but found in {package_path}")]
    CacheInNonRootPackage { package_path: Arc<AbsolutePath> },

    #[error(
        "`enablePrePostScripts` can only be set in the workspace root config, but found in {package_path}"
    )]
    PrePostScriptsInNonRootPackage { package_path: Arc<AbsolutePath> },

    #[error(transparent)]
    TopLevelCacheFields(config::user::TopLevelCacheFieldsError),
}

/// Error when looking up a task by its specifier.
///
/// It's generic over `UnknownPackageError`, which is the error type when looking up a task without a package name and without a package origin.
///
/// - When the specifier is from `dependOn` of a known task, `UnknownPackageError` is `Infallible` because the origin package is always known.
/// - When the specifier is from a CLI command, `UnknownPackageError` can be a real error type in case cwd is not in any package.
#[derive(Debug, thiserror::Error, Serialize)]
pub enum SpecifierLookupError<PackageUnknownError = Infallible> {
    #[error("Package '{package_name}' is ambiguous among multiple packages: {package_paths:?}")]
    AmbiguousPackageName { package_name: Str, package_paths: Box<[Arc<AbsolutePath>]> },

    #[error("Package '{package_name}' not found")]
    PackageNameNotFound { package_name: Str },

    #[error("Task '{task_name}' not found in package {package_name}")]
    TaskNameNotFound {
        package_name: Str,
        task_name: Str,
        #[serde(skip)]
        package_index: PackageNodeIndex,
    },

    #[error(
        "Nowhere to look for task '{task_name}' because the package is unknown: {unspecifier_package_error}"
    )]
    PackageUnknown { unspecifier_package_error: PackageUnknownError, task_name: Str },
}

/// Full task graph of a workspace.
///
/// The package list loads up front. Tasks load per package the first time
/// they're read, but readers can't tell: every read goes through the methods
/// below, which load what they need.
///
/// External factors (e.g. additional args from cli, current working directory,
/// environmental variables) are not stored here.
#[derive(Debug)]
pub struct IndexedTaskGraph<'l> {
    graph: LazyGraph<TaskLoader<'l>>,

    /// Global cache configuration resolved from the workspace root config.
    resolved_global_cache: ResolvedGlobalCacheConfig,

    /// Whether pre/post script hooks are enabled (from `enablePrePostScripts` in workspace root config).
    pre_post_scripts_enabled: bool,
}

impl<'l> IndexedTaskGraph<'l> {
    /// Load the package list and the root config of a discovered workspace.
    ///
    /// The other packages' configs load when their tasks are first read, and
    /// their errors are returned from those reads.
    ///
    /// # Errors
    ///
    /// Returns [`TaskGraphLoadError`] if the package graph fails to load, or the
    /// root config can't be read or has a task that can't be resolved.
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn load(
        workspace_root: &WorkspaceRoot,
        config_loader: &'l dyn UserConfigLoader,
        glob_resolver: Rc<GlobResolver>,
    ) -> Result<Self, TaskGraphLoadError> {
        let package_graph =
            IndexedPackageGraph::index(vt_workspace::load_package_graph(workspace_root)?);
        let package_indices: Vec<PackageNodeIndex> =
            package_graph.package_graph().node_indices().collect();
        let root_package = package_indices.iter().copied().find(|&package_index| {
            package_graph.package_graph()[package_index].path.as_str().is_empty()
        });

        let graph = LazyGraph::new(
            TaskLoader::new(
                package_graph,
                Arc::clone(&workspace_root.path),
                config_loader,
                glob_resolver,
            ),
            package_indices,
        );

        // The root config holds the workspace-wide settings, so it loads with the package list.
        if let Some(root_package) = root_package {
            let _root_tasks = graph.nodes_in(root_package).await?;
        }
        let root_settings = graph.loader().root_settings();

        Ok(Self {
            graph,
            resolved_global_cache: root_settings.global_cache,
            pre_post_scripts_enabled: root_settings.pre_post_scripts_enabled,
        })
    }

    /// The task with `id`, or `None` if its package has no such task.
    ///
    /// # Errors
    ///
    /// Returns [`TaskGraphLoadError`] if the package's tasks fail to load.
    pub async fn task(&self, id: &TaskId) -> Result<Option<&TaskNode>, TaskGraphLoadError> {
        self.graph.get(id).await
    }

    /// Every task in the workspace, package by package.
    ///
    /// # Errors
    ///
    /// Returns [`TaskGraphLoadError`] if any package's tasks fail to load.
    pub async fn tasks(&self) -> Result<Vec<&TaskNode>, TaskGraphLoadError> {
        let mut tasks = Vec::new();
        for package_index in self.package_indices() {
            tasks.extend(self.graph.nodes_in(package_index).await?);
        }
        Ok(tasks)
    }

    /// The tasks in the `dependsOn` of the task with `id`. An entry such as
    /// `{ task: "build", from: "dependencies" }` is replaced by the tasks it selects.
    ///
    /// # Errors
    ///
    /// Returns [`TaskGraphLoadError`] if a package's tasks fail to load, or an
    /// entry names a task that doesn't exist.
    pub async fn depends_on(
        &self,
        id: &TaskId,
    ) -> Result<impl Iterator<Item = &TaskNode>, TaskGraphLoadError> {
        self.graph.neighbors(id).await
    }

    #[must_use]
    pub fn get_package_name(&self, package_index: PackageNodeIndex) -> &str {
        self.package_graph().package_graph()[package_index].package_json.name.as_str()
    }

    #[must_use]
    pub fn get_package_path(&self, package_index: PackageNodeIndex) -> &Arc<AbsolutePath> {
        &self.package_graph().package_graph()[package_index].absolute_path
    }

    /// Get the package path for a given current working directory by traversing up the directory
    /// tree to find the nearest package.
    #[must_use]
    pub fn get_package_path_from_cwd(&self, cwd: &AbsolutePath) -> Option<&Arc<AbsolutePath>> {
        let index = self.package_graph().get_package_index_from_cwd(cwd)?;
        Some(self.get_package_path(index))
    }

    #[must_use]
    pub const fn global_cache_config(&self) -> &ResolvedGlobalCacheConfig {
        &self.resolved_global_cache
    }

    /// Whether pre/post script hooks are enabled workspace-wide.
    #[must_use]
    pub const fn pre_post_scripts_enabled(&self) -> bool {
        self.pre_post_scripts_enabled
    }

    /// Returns the pre/post hook for a `PackageJsonScript` task.
    ///
    /// Given a task named `X` and `prefix = "pre"`, looks up `preX` in the same package.
    /// Given a task named `X` and `prefix = "post"`, looks up `postX` in the same package.
    ///
    /// Returns `None` if:
    /// - The task is not a `PackageJsonScript`
    /// - No `{prefix}{name}` script exists in the same package
    /// - The hook is not itself a `PackageJsonScript`
    ///
    /// # Errors
    ///
    /// Returns [`TaskGraphLoadError`] if the package's tasks fail to load.
    pub async fn get_script_hook(
        &self,
        task: &TaskNode,
        prefix: &str,
    ) -> Result<Option<&TaskNode>, TaskGraphLoadError> {
        if task.source != TaskSource::PackageJsonScript {
            return Ok(None);
        }
        let hook_id = TaskId {
            package_index: task.id.package_index,
            task_name: vt_str::format!("{prefix}{}", task.id.task_name),
        };
        let hook = self.task(&hook_id).await?;
        Ok(hook.filter(|hook| hook.source == TaskSource::PackageJsonScript))
    }

    /// The workspace packages, including those without tasks. Reading them loads no config.
    #[must_use]
    pub const fn package_graph(&self) -> &IndexedPackageGraph {
        &self.graph.loader().package_graph
    }

    fn package_indices(&self) -> impl Iterator<Item = PackageNodeIndex> + '_ {
        self.package_graph().package_graph().node_indices()
    }
}
