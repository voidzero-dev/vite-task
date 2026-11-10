//! Loads the tasks of one package, and resolves the `dependsOn` of one task.

use std::{cell::OnceCell, rc::Rc, sync::Arc};

use petgraph::visit::EdgeRef as _;
use rustc_hash::FxHashMap;
use vt_path::AbsolutePath;
use vt_str::Str;
use vt_workspace::{DependencyType, PackageNodeIndex, package_graph::IndexedPackageGraph};

use crate::{
    SpecifierLookupError, TaskGraphLoadError, TaskId, TaskNode, TaskSource, TaskSpecifier,
    config::{
        GlobResolver, ResolvedGlobalCacheConfig, ResolvedTaskConfig, UserTaskConfig,
        UserTaskDefinition,
        user::{
            TopLevelCacheFields, TopLevelCacheFieldsCollector, UserDependencyType,
            UserDependsOnEntry, UserPackageDependency, UserTaskOptions,
        },
    },
    display::TaskDisplay,
    lazy_graph::{Keyed, Loader, Lookup},
    loader::UserConfigLoader,
};

impl Keyed for TaskNode {
    type Key = TaskId;

    fn key(&self) -> &TaskId {
        &self.id
    }
}

/// Settings that only the workspace root's config can set.
#[derive(Debug, Clone)]
pub struct RootSettings {
    pub global_cache: ResolvedGlobalCacheConfig,
    pub pre_post_scripts_enabled: bool,
}

impl Default for RootSettings {
    fn default() -> Self {
        Self {
            global_cache: ResolvedGlobalCacheConfig::resolve_from(None),
            pre_post_scripts_enabled: true,
        }
    }
}

/// Loads tasks from `vite.config.*` and `package.json` scripts, one package at a time.
#[derive(Debug)]
pub struct TaskLoader<'l> {
    pub package_graph: IndexedPackageGraph,
    workspace_path: Arc<AbsolutePath>,
    config_loader: &'l dyn UserConfigLoader,
    glob_resolver: Rc<GlobResolver>,
    /// Set when the workspace root package loads.
    root_settings: OnceCell<RootSettings>,
}

impl<'l> TaskLoader<'l> {
    pub fn new(
        package_graph: IndexedPackageGraph,
        workspace_path: Arc<AbsolutePath>,
        config_loader: &'l dyn UserConfigLoader,
        glob_resolver: Rc<GlobResolver>,
    ) -> Self {
        Self {
            package_graph,
            workspace_path,
            config_loader,
            glob_resolver,
            root_settings: OnceCell::new(),
        }
    }

    /// The root settings, or the defaults if the root package hasn't loaded or doesn't exist.
    pub fn root_settings(&self) -> RootSettings {
        self.root_settings.get().cloned().unwrap_or_default()
    }

    /// The package a task specifier from `dependsOn` refers to.
    fn specifier_package(
        &self,
        specifier: &TaskSpecifier,
        origin: PackageNodeIndex,
    ) -> Result<PackageNodeIndex, SpecifierLookupError> {
        let Some(package_name) = &specifier.package_name else {
            // No '#', so the specifier only names a task in the origin package.
            return Ok(origin);
        };
        let Some(package_indices) = self.package_graph.get_package_indices_by_name(package_name)
        else {
            return Err(SpecifierLookupError::PackageNameNotFound {
                package_name: package_name.clone(),
            });
        };
        if package_indices.len() > 1 {
            return Err(SpecifierLookupError::AmbiguousPackageName {
                package_name: package_name.clone(),
                package_paths: package_indices
                    .iter()
                    .map(|package_index| {
                        Arc::clone(
                            &self.package_graph.package_graph()[*package_index].absolute_path,
                        )
                    })
                    .collect(),
            });
        }
        Ok(*package_indices.first())
    }
}

#[async_trait::async_trait(?Send)]
impl Loader for TaskLoader<'_> {
    /// The task's `dependsOn` entries.
    type EdgeSpec = Option<Arc<[UserDependsOnEntry]>>;
    type Error = TaskGraphLoadError;
    type Key = TaskId;
    type Node = TaskNode;
    type Partition = PackageNodeIndex;

    fn partition_of(key: &TaskId) -> PackageNodeIndex {
        key.package_index
    }

    async fn load(
        &self,
        package_index: PackageNodeIndex,
    ) -> Result<Vec<(TaskNode, Self::EdgeSpec)>, TaskGraphLoadError> {
        let package = &self.package_graph.package_graph()[package_index];
        let package_dir: Arc<AbsolutePath> = self.workspace_path.join(&package.path).into();
        let is_workspace_root = package.path.as_str().is_empty();
        let task_display = |task_name: Str| TaskDisplay {
            package_name: package.package_json.name.clone(),
            task_name,
            package_path: Arc::clone(&package_dir),
        };

        let user_config = self
            .config_loader
            .load_user_config_file(&package_dir)
            .await
            .map_err(|error| TaskGraphLoadError::ConfigLoadError {
                error,
                package_path: Arc::clone(&package_dir),
            })?
            .unwrap_or_default();

        if is_workspace_root {
            self.root_settings.get_or_init(|| RootSettings {
                global_cache: ResolvedGlobalCacheConfig::resolve_from(user_config.cache.as_ref()),
                pre_post_scripts_enabled: user_config.enable_pre_post_scripts.unwrap_or(true),
            });
        } else {
            if user_config.cache.is_some() {
                return Err(TaskGraphLoadError::CacheInNonRootPackage {
                    package_path: Arc::clone(&package_dir),
                });
            }
            if user_config.enable_pre_post_scripts.is_some() {
                return Err(TaskGraphLoadError::PrePostScriptsInNonRootPackage {
                    package_path: Arc::clone(&package_dir),
                });
            }
        }

        // Collect package.json scripts into a mutable map for draining lookup.
        let mut package_json_scripts: FxHashMap<&str, &str> = package
            .package_json
            .scripts
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();

        let mut top_level_cache_fields = TopLevelCacheFieldsCollector::default();
        let mut tasks = Vec::new();

        for (task_name, task_user_config) in user_config.tasks.unwrap_or_default() {
            // Error if a package.json script with the same name exists
            if package_json_scripts.remove(task_name.as_str()).is_some() {
                return Err(TaskGraphLoadError::ScriptConflict {
                    task_display: task_display(task_name),
                });
            }

            let task_user_config = match task_user_config {
                UserTaskDefinition::Object(config) => config,
                UserTaskDefinition::CommandShorthand(command) => UserTaskConfig {
                    command,
                    options: UserTaskOptions::default(),
                    top_level_cache_fields: TopLevelCacheFields::default(),
                },
            };
            let depends_on = task_user_config.options.depends_on.clone();

            top_level_cache_fields.record(task_user_config.top_level_cache_fields, || {
                task_display(task_name.clone())
            });

            let resolved_config = ResolvedTaskConfig::resolve(
                task_user_config,
                &package_dir,
                &self.workspace_path,
                &self.glob_resolver,
            )
            .map_err(|error| TaskGraphLoadError::ResolveConfigError {
                error,
                task_display: task_display(task_name.clone()),
            })?;

            let task_node = TaskNode {
                id: TaskId { package_index, task_name: task_name.clone() },
                task_display: task_display(task_name),
                resolved_config,
                source: TaskSource::TaskConfig,
            };
            tasks.push((task_node, depends_on));
        }

        // For remaining package.json scripts not in the tasks map, create tasks with default config
        for (script_name, package_json_script) in package_json_scripts {
            let resolved_config = ResolvedTaskConfig::resolve_package_json_script(
                &package_dir,
                package_json_script,
                &self.workspace_path,
                &self.glob_resolver,
            )
            .map_err(|error| TaskGraphLoadError::ResolveConfigError {
                error,
                task_display: task_display(script_name.into()),
            })?;
            let task_node = TaskNode {
                id: TaskId { package_index, task_name: script_name.into() },
                task_display: task_display(script_name.into()),
                resolved_config,
                source: TaskSource::PackageJsonScript,
            };
            tasks.push((task_node, None));
        }

        top_level_cache_fields.finish().map_err(TaskGraphLoadError::TopLevelCacheFields)?;

        Ok(tasks)
    }

    /// Resolves `dependsOn`. String-form entries name one task; object-form
    /// entries such as `{ "task": "build", "from": "dependencies" }` select that
    /// task in each direct package dependency that defines it.
    async fn resolve_edges(
        &self,
        from: &TaskNode,
        depends_on: &Self::EdgeSpec,
        graph: Lookup<'_, Self>,
    ) -> Result<Vec<TaskId>, TaskGraphLoadError> {
        let mut targets = Vec::new();
        for entry in depends_on.iter().flat_map(|entries| entries.iter()) {
            match entry {
                UserDependsOnEntry::Task(specifier) => {
                    let lookup_error = |error| TaskGraphLoadError::DependencySpecifierLookupError {
                        error: Box::new(error),
                        specifier: specifier.clone(),
                        task_display: from.task_display.clone(),
                    };
                    let specifier = TaskSpecifier::parse_raw(specifier);
                    let package_index = self
                        .specifier_package(&specifier, from.id.package_index)
                        .map_err(lookup_error)?;
                    let target = TaskId { package_index, task_name: specifier.task_name };
                    if graph.get(&target).await?.is_none() {
                        return Err(lookup_error(SpecifierLookupError::TaskNameNotFound {
                            package_name: self.package_graph.package_graph()[package_index]
                                .package_json
                                .name
                                .clone(),
                            task_name: target.task_name,
                            package_index,
                        }));
                    }
                    targets.push(target);
                }
                UserDependsOnEntry::Package(entry) => {
                    let entry = PackageDependencyEntry::from_user_config(entry);
                    let dependency_packages = self
                        .package_graph
                        .package_graph()
                        .edges(from.id.package_index)
                        // Only use package.json dependency fields requested by `from`.
                        .filter(|edge| entry.dependency_types.contains(edge.weight()))
                        .map(|edge| edge.target());
                    for package_index in dependency_packages {
                        let target = TaskId { package_index, task_name: entry.task_name.clone() };
                        // Missing tasks are intentionally ignored: selecting
                        // `{ task: "build", from: "dependencies" }` means "run build in
                        // direct dependency packages that define build", not "require every
                        // selected package to define build".
                        if graph.get(&target).await?.is_some() {
                            targets.push(target);
                        }
                    }
                }
            }
        }
        Ok(targets)
    }
}

/// A `dependsOn` entry that selects direct package dependencies from a source task.
struct PackageDependencyEntry {
    task_name: Str,
    dependency_types: Box<[DependencyType]>,
}

impl PackageDependencyEntry {
    fn from_user_config(entry: &UserPackageDependency) -> Self {
        Self {
            task_name: entry.task.clone(),
            dependency_types: entry
                .from
                .as_slice()
                .iter()
                .copied()
                .map(DependencyType::from)
                .collect(),
        }
    }
}

impl From<UserDependencyType> for DependencyType {
    fn from(value: UserDependencyType) -> Self {
        match value {
            UserDependencyType::Dependencies => Self::Normal,
            UserDependencyType::DevDependencies => Self::Dev,
            UserDependencyType::PeerDependencies => Self::Peer,
        }
    }
}
