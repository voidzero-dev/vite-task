//! Configuration structures for user-defined tasks in `vite.config.*`

use std::sync::Arc;

use rustc_hash::FxHashMap;
use serde::Deserialize;
#[cfg(all(test, not(clippy)))]
use ts_rs::TS;
use vec1::Vec1;
use vt_path::RelativePathBuf;
use vt_str::Str;

mod top_level_cache_fields;

pub(crate) use top_level_cache_fields::TopLevelCacheFieldsCollector;
pub use top_level_cache_fields::{TopLevelCacheFields, TopLevelCacheFieldsError};

/// The base directory for resolving a glob pattern.
#[derive(Debug, Deserialize, PartialEq, Eq, Clone)]
#[cfg_attr(all(test, not(clippy)), derive(TS))]
#[serde(rename_all = "lowercase")]
pub enum InputBase {
    /// Resolve relative to the package directory (where `package.json` is located)
    Package,
    /// Resolve relative to the workspace root
    Workspace,
}

/// Glob pattern with explicit base directory for resolution.
#[derive(Debug, Deserialize, PartialEq, Eq, Clone)]
#[cfg_attr(all(test, not(clippy)), derive(TS))]
#[serde(deny_unknown_fields)]
pub struct GlobWithBase {
    /// The glob pattern (positive or negative starting with `!`)
    pub pattern: Str,
    /// The base directory for resolving the pattern
    pub base: InputBase,
}

/// Automatic file-tracking directive for input fingerprinting or output archiving.
#[derive(Debug, Deserialize, PartialEq, Eq, Clone)]
#[cfg_attr(all(test, not(clippy)), derive(TS))]
#[serde(deny_unknown_fields)]
pub struct AutoTracking {
    /// Enable automatic file tracking for this input or output list.
    pub auto: bool,
}

/// A single input entry in the `input` array.
///
/// Inputs can be:
/// - Glob patterns as strings (resolved relative to the package directory)
/// - Object form with explicit base: `{ "pattern": "...", "base": "workspace" | "package" }`
/// - Automatic tracking directives: `{ "auto": true }`
#[derive(Debug, Deserialize, PartialEq, Eq, Clone)]
// TS derive macro generates code using std types that clippy disallows; skip derive during linting
#[cfg_attr(all(test, not(clippy)), derive(TS))]
#[serde(untagged)]
pub enum UserInputEntry {
    /// Glob pattern (positive or negative starting with `!`), resolved relative to package dir
    Glob(Str),
    /// Glob pattern with explicit base directory
    GlobWithBase(GlobWithBase),
    /// Automatic tracking directive
    Auto(AutoTracking),
}

/// The inputs configuration for cache fingerprinting.
///
/// Default (when field omitted): `[{auto: true}]` - infer from file accesses.
pub type UserInputsConfig = Vec<UserInputEntry>;

/// A supported package.json dependency field for package dependency selection.
#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
// TS derive macro generates code using std types that clippy disallows; skip derive during linting
#[cfg_attr(all(test, not(clippy)), derive(TS), ts(rename = "DependencyType"))]
#[serde(rename_all = "camelCase")]
pub enum UserDependencyType {
    /// Traverse dependencies declared in the package.json `dependencies` field.
    Dependencies,
    /// Traverse dependencies declared in the package.json `devDependencies` field.
    DevDependencies,
    /// Traverse dependencies declared in the package.json `peerDependencies` field.
    PeerDependencies,
}

/// The `from` selector for object-form `dependsOn` entries.
#[derive(Debug, Deserialize, PartialEq, Eq, Clone)]
// TS derive macro generates code using std types that clippy disallows; skip derive during linting
#[cfg_attr(all(test, not(clippy)), derive(TS), ts(rename = "DependsOnFrom"))]
#[serde(untagged)]
pub enum UserDependsOnFrom {
    /// Select one package.json dependency field.
    Single(UserDependencyType),
    /// Select the union of multiple package.json dependency fields.
    Multiple(
        #[cfg_attr(all(test, not(clippy)), ts(as = "Vec<UserDependencyType>"))]
        Vec1<UserDependencyType>,
    ),
}

impl UserDependsOnFrom {
    #[must_use]
    pub fn as_slice(&self) -> &[UserDependencyType] {
        match self {
            Self::Single(dependency_type) => std::slice::from_ref(dependency_type),
            Self::Multiple(dependency_types) => dependency_types,
        }
    }
}

/// Object form for `dependsOn` entries that select direct workspace package dependencies.
#[derive(Debug, Deserialize, PartialEq, Eq, Clone)]
// TS derive macro generates code using std types that clippy disallows; skip derive during linting
#[cfg_attr(all(test, not(clippy)), derive(TS))]
#[serde(deny_unknown_fields)]
pub struct UserPackageDependency {
    /// Task name to run in dependency packages.
    pub task: Str,

    /// Package.json dependency field or fields to use when selecting direct dependency packages.
    pub from: UserDependsOnFrom,
}

/// A single `dependsOn` entry.
#[derive(Debug, Deserialize, PartialEq, Eq, Clone)]
// TS derive macro generates code using std types that clippy disallows; skip derive during linting
#[cfg_attr(all(test, not(clippy)), derive(TS), ts(rename = "DependsOnEntry"))]
#[serde(untagged)]
pub enum UserDependsOnEntry {
    /// Same-package task or `package#task` specifier.
    Task(Str),
    /// Direct package dependency selection entry.
    Package(UserPackageDependency),
}

/// A single output entry in the `output` array.
///
/// Outputs can be:
/// - Glob patterns as strings (resolved relative to the package directory)
/// - Object form with explicit base: `{ "pattern": "...", "base": "workspace" | "package" }`
/// - Automatic tracking directive: `{ "auto": true }`
#[derive(Debug, Deserialize, PartialEq, Eq, Clone)]
// TS derive macro generates code using std types that clippy disallows; skip derive during linting
#[cfg_attr(all(test, not(clippy)), derive(TS))]
#[serde(untagged)]
pub enum UserOutputEntry {
    /// Glob pattern (positive or negative starting with `!`), resolved relative to package dir
    Glob(Str),
    /// Glob pattern with explicit base directory
    GlobWithBase(GlobWithBase),
    /// Automatic tracking directive
    Auto(AutoTracking),
}

/// The value of a task's `cache` field.
#[derive(Debug, Deserialize, PartialEq, Eq)]
// TS derive macro generates code using std types that clippy disallows; skip derive during linting
#[cfg_attr(all(test, not(clippy)), derive(TS), ts(rename = "TaskCache"))]
#[serde(untagged)]
pub enum UserCacheConfig {
    /// `true` enables caching with default settings; `false` disables caching.
    Bool(bool),
    /// Enables caching with the given settings.
    Config(EnabledCacheConfig),
}

impl Default for UserCacheConfig {
    /// Caching enabled with default settings, used when `cache` is omitted.
    fn default() -> Self {
        Self::Bool(true)
    }
}

impl UserCacheConfig {
    /// Create an enabled cache config with the given `EnabledCacheConfig`.
    #[must_use]
    pub const fn with_config(config: EnabledCacheConfig) -> Self {
        Self::Config(config)
    }

    /// Create a disabled cache config.
    #[must_use]
    pub const fn disabled() -> Self {
        Self::Bool(false)
    }

    /// Returns the cache settings, or `None` if caching is disabled.
    #[must_use]
    pub fn into_enabled(self) -> Option<EnabledCacheConfig> {
        match self {
            Self::Bool(true) => Some(EnabledCacheConfig::default()),
            Self::Bool(false) => None,
            Self::Config(config) => Some(config),
        }
    }
}

/// Cache configuration fields when caching is enabled
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
// TS derive macro generates code using std types that clippy disallows; skip derive during linting
#[cfg_attr(all(test, not(clippy)), derive(TS), ts(optional_fields, rename = "TaskCacheConfig"))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EnabledCacheConfig {
    /// Environment variable names to be fingerprinted and passed to the task.
    pub env: Option<Box<[Str]>>,

    /// Environment variable names to be passed to the task without fingerprinting.
    pub untracked_env: Option<Vec<Str>>,

    /// Files to include in the cache fingerprint.
    ///
    /// - Omitted: automatically tracks which files the task reads
    /// - `[]` (empty): disables file tracking entirely
    /// - Glob patterns (e.g. `"src/**"`) select specific files, relative to the package directory
    /// - `{pattern: "...", base: "workspace" | "package"}` specifies a glob with an explicit base directory
    /// - `{auto: true}` enables automatic file tracking
    /// - Negative patterns (e.g. `"!dist/**"`) exclude matched files
    #[serde(default)]
    #[cfg_attr(all(test, not(clippy)), ts(inline))]
    pub input: Option<UserInputsConfig>,

    /// Output files to archive and restore on cache hit.
    ///
    /// - Omitted: automatically tracks which files the task writes
    /// - `[]` (empty): disables output restoration entirely
    /// - Glob patterns (e.g. `"dist/**"`) select specific output files, relative to the package directory
    /// - `{pattern: "...", base: "workspace" | "package"}` specifies a glob with an explicit base directory
    /// - `{auto: true}` enables automatic output tracking
    /// - Negative patterns (e.g. `"!dist/cache/**"`) exclude matched files
    #[serde(default)]
    #[cfg_attr(all(test, not(clippy)), ts(inline))]
    pub output: Option<Vec<UserOutputEntry>>,

    /// Whether this task can use the remote cache. Defaults to `true`.
    pub remote: Option<bool>,
}

/// Options for user-defined tasks in `vite.config.*`, excluding the command.
#[derive(Debug, Deserialize, PartialEq, Eq)]
// TS derive macro generates code using std types that clippy disallows; skip derive during linting
#[cfg_attr(all(test, not(clippy)), derive(TS), ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct UserTaskOptions {
    /// The working directory for the task, relative to the package root (not workspace root).
    #[serde(rename = "cwd")]
    pub cwd_relative_to_package: Option<RelativePathBuf>,

    /// Tasks that must run before this task.
    ///
    /// - A string runs one named task, such as `"build"` in the same package or
    ///   `"package-name#build"` in another package.
    /// - An object runs a task in direct workspace dependency packages selected
    ///   from package.json fields. For example,
    ///   `{ "task": "build", "from": "dependencies" }` runs `build` in each
    ///   direct workspace dependency that defines a `build` task.
    pub depends_on: Option<Arc<[UserDependsOnEntry]>>,

    /// Whether and how to cache the task.
    ///
    /// - Omitted or `true`: caching enabled with default settings (same as `{}`)
    /// - `false`: caching disabled
    /// - Object: caching enabled with the given settings
    #[serde(rename = "cache")]
    pub cache_config: Option<UserCacheConfig>,
}

impl Default for UserTaskOptions {
    /// The default user task options for package.json scripts.
    fn default() -> Self {
        Self {
            // Runs in the package root
            cwd_relative_to_package: None,
            // No dependencies
            depends_on: None,
            // Caching enabled with default settings
            cache_config: None,
        }
    }
}
/// The command to run for a task: a single string or a sequence of strings.
#[derive(Debug, Deserialize, PartialEq, Eq)]
// TS derive macro generates code using std types that clippy disallows; skip derive during linting
#[cfg_attr(all(test, not(clippy)), derive(TS))]
#[serde(untagged)]
pub enum Command {
    /// A single command string.
    Single(Str),
    /// A sequence of command strings, run in order.
    Array(Arc<[Str]>),
}

/// Full user-defined task configuration in `vite.config.*`, including the command and options.
#[derive(Debug, Deserialize, PartialEq)]
// TS derive macro generates code using std types that clippy disallows; skip derive during linting
#[cfg_attr(all(test, not(clippy)), derive(TS), ts(optional_fields, rename = "Task"))]
// `deny_unknown_fields` works with the flattened fields only because neither
// has flattened fields of its own.
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UserTaskConfig {
    /// Command to run, or an array of commands to run in order.
    pub command: Command,

    /// Fields other than the command.
    #[serde(flatten)]
    pub options: UserTaskOptions,

    /// Cache settings written at the top level, reported when loading the task graph.
    #[serde(flatten)]
    #[cfg_attr(all(test, not(clippy)), ts(skip))]
    pub top_level_cache_fields: TopLevelCacheFields,
}

/// User-defined task configuration or command-only shorthand in `vite.config.*`.
#[derive(Debug, Deserialize, PartialEq)]
// TS derive macro generates code using std types that clippy disallows; skip derive during linting
#[cfg_attr(all(test, not(clippy)), derive(TS), ts(rename = "TaskDefinition"))]
#[serde(untagged)]
pub enum UserTaskDefinition {
    /// Full task object form.
    Object(UserTaskConfig),
    /// Command-only shorthand form using default task options.
    CommandShorthand(Command),
}

/// Root-level cache configuration.
///
/// Controls caching behavior for the entire workspace.
///
/// - `true` is equivalent to `{ scripts: true, tasks: true }` — enables caching for both
///   package.json scripts and task entries.
/// - `false` is equivalent to `{ scripts: false, tasks: false }` — disables all caching.
/// - When omitted, defaults to `{ scripts: false, tasks: true }`.
///
/// This option can only be set in the workspace root's config file.
/// Setting it in a package's config will result in an error.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
// TS derive macro generates code using std types that clippy disallows; skip derive during linting
#[cfg_attr(all(test, not(clippy)), derive(TS), ts(optional_fields))]
#[serde(untagged, deny_unknown_fields)]
pub enum UserGlobalCacheConfig {
    Bool(bool),
    /// Detailed cache configuration with separate control for scripts and tasks.
    Detailed {
        /// Enable caching for package.json scripts not defined in the `tasks` map.
        ///
        /// When `false`, package.json scripts will not be cached.
        /// When `true`, package.json scripts will be cached with default settings.
        ///
        /// Default: `false`
        scripts: Option<bool>,

        /// Global cache kill switch for task entries.
        ///
        /// When `false`, overrides all tasks to disable caching, even tasks with `cache: true`
        /// or a `cache` object.
        /// When `true`, respects each task's individual `cache` setting
        /// (each task's `cache` defaults to `true` if omitted).
        ///
        /// Default: `true`
        tasks: Option<bool>,
    },
}

/// Resolved global cache configuration with concrete boolean values.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedGlobalCacheConfig {
    pub scripts: bool,
    pub tasks: bool,
}

impl ResolvedGlobalCacheConfig {
    /// Resolve from an optional user config, using defaults when `None`.
    ///
    /// Default: `{ scripts: false, tasks: true }`
    #[must_use]
    pub fn resolve_from(config: Option<&UserGlobalCacheConfig>) -> Self {
        match config {
            None => Self { scripts: false, tasks: true },
            Some(UserGlobalCacheConfig::Bool(true)) => Self { scripts: true, tasks: true },
            Some(UserGlobalCacheConfig::Bool(false)) => Self { scripts: false, tasks: false },
            Some(UserGlobalCacheConfig::Detailed { scripts, tasks }) => {
                Self { scripts: scripts.unwrap_or(false), tasks: tasks.unwrap_or(true) }
            }
        }
    }
}

/// Remote cache endpoint shared by tasks in a workspace.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(all(test, not(clippy)), derive(TS), ts(rename = "RemoteCacheConfig"))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UserRemoteCacheConfig {
    /// HTTP or HTTPS namespace endpoint. Overridden by `VP_REMOTE_CACHE_URL`.
    pub url: Str,
}

/// User configuration structure for `run` field in `vite.config.*`
#[derive(Debug, Default, Deserialize)]
// TS derive macro generates code using std types that clippy disallows; skip derive during linting
#[cfg_attr(all(test, not(clippy)), derive(TS), ts(optional_fields, rename = "RunConfig"))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UserRunConfig {
    /// Root-level cache configuration.
    ///
    /// This option can only be set in the workspace root's config file.
    /// Setting it in a package's config will result in an error.
    pub cache: Option<UserGlobalCacheConfig>,

    /// Remote cache endpoint. Only allowed in the workspace root config.
    pub remote_cache: Option<UserRemoteCacheConfig>,

    /// Task definitions: full task objects, command strings, or command string arrays.
    pub tasks: Option<FxHashMap<Str, UserTaskDefinition>>,

    /// Whether to automatically run `preX`/`postX` package.json scripts as
    /// lifecycle hooks when script `X` is executed.
    ///
    /// When `true` (the default), running script `test` will automatically
    /// run `pretest` before and `posttest` after, if they exist.
    ///
    /// This option can only be set in the workspace root's config file.
    /// Setting it in a package's config will result in an error.
    pub enable_pre_post_scripts: Option<bool>,
}

impl UserRunConfig {
    /// TypeScript type definitions for user run configuration.
    pub const TS_TYPE: &str = include_str!("../../run-config.ts");

    /// Generates TypeScript type definitions for user task configuration.
    #[cfg(all(test, not(clippy)))]
    #[must_use]
    // test code: uses std types for convenience
    #[expect(clippy::disallowed_types, reason = "test code uses std types for convenience")]
    pub fn generate_ts_definition() -> String {
        use std::{any::TypeId, collections::HashSet};

        use ts_rs::TypeVisitor;

        struct DeclCollector {
            decls: Vec<String>,
            visited: HashSet<TypeId>,
        }

        impl TypeVisitor for DeclCollector {
            fn visit<T: TS + 'static + ?Sized>(&mut self) {
                if !self.visited.insert(TypeId::of::<T>()) {
                    return;
                }
                // Only collect declarations from types that are exportable
                // (i.e., have an output path - built-in types like HashMap don't)
                if T::output_path().is_some() {
                    self.decls.push(T::decl(&ts_rs::Config::default()));
                }
                // Recursively visit dependencies of T
                T::visit_dependencies(self);
            }
        }

        let mut collector = DeclCollector { decls: Vec::new(), visited: HashSet::new() };
        Self::visit_dependencies(&mut collector);

        // Sort declarations for deterministic output order
        collector.decls.sort();

        // Header
        let mut types: String =
            "// This file is auto-generated by `cargo test`. Do not edit manually.\n\n".into();

        // Export all types
        let dep_types: String = collector
            .decls
            .iter()
            .map(|decl| vt_str::format!("export {decl}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        types.push_str(&dep_types);

        // Export the main type
        types.push_str("\n\nexport ");
        types.push_str(&Self::decl(&ts_rs::Config::default()));

        types.lines().map(str::trim_end).collect::<Vec<_>>().join("\n") + "\n"
    }
}

#[cfg(all(test, not(clippy)))]
mod ts_tests {
    // test code: uses std types for convenience
    #[expect(clippy::disallowed_types, reason = "test code uses std types for convenience")]
    use std::{env, path::PathBuf};

    use super::UserRunConfig;

    #[test]
    // test code: uses std types for convenience
    #[expect(
        clippy::disallowed_methods,
        clippy::disallowed_types,
        reason = "test code uses std types for convenience"
    )]
    fn typescript_generation() {
        let file_path =
            PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("run-config.ts");
        let ts = UserRunConfig::generate_ts_definition().replace('\r', "");

        if env::var("VT_UPDATE_TS_TYPES").unwrap_or_default() == "1" {
            std::fs::write(&file_path, ts).unwrap();
        } else {
            let existing_ts =
                std::fs::read_to_string(&file_path).unwrap_or_default().replace('\r', "");
            pretty_assertions::assert_eq!(
                ts,
                existing_ts,
                "Generated TypeScript types do not match the existing ones. If you made changes to the types, please set VT_UPDATE_TS_TYPES=1 and run the tests again to update the TypeScript definitions."
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_command_required() {
        let user_config_json = json!({});
        assert!(
            serde_json::from_value::<UserTaskConfig>(user_config_json).is_err(),
            "task config without command should fail to deserialize"
        );
    }

    #[test]
    fn test_command_with_defaults() {
        let user_config_json = json!({
            "command": "echo hello"
        });
        let user_config: UserTaskConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(
            user_config,
            UserTaskConfig {
                command: Command::Single("echo hello".into()),
                options: UserTaskOptions::default(),
                top_level_cache_fields: TopLevelCacheFields::default(),
            }
        );
    }

    #[test]
    fn test_command_array() {
        let user_config_json = json!({
            "command": ["echo one", "echo two", "echo three"]
        });
        let user_config: UserTaskConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(
            user_config.command,
            Command::Array(Arc::from(["echo one".into(), "echo two".into(), "echo three".into()]))
        );
        assert_eq!(user_config.options, UserTaskOptions::default());
    }

    #[test]
    fn test_task_string_shorthand() {
        let user_config_json = json!({
            "tasks": {
                "build": "echo build"
            }
        });
        let mut user_config: UserRunConfig = serde_json::from_value(user_config_json).unwrap();
        let task = user_config.tasks.as_mut().unwrap().remove("build").unwrap();
        assert_eq!(
            task,
            UserTaskDefinition::CommandShorthand(Command::Single("echo build".into()))
        );
    }

    #[test]
    fn test_task_array_shorthand() {
        let user_config_json = json!({
            "tasks": {
                "build": ["echo one", "echo two", "echo three"]
            }
        });
        let mut user_config: UserRunConfig = serde_json::from_value(user_config_json).unwrap();
        let task = user_config.tasks.as_mut().unwrap().remove("build").unwrap();
        assert_eq!(
            task,
            UserTaskDefinition::CommandShorthand(Command::Array(Arc::from([
                "echo one".into(),
                "echo two".into(),
                "echo three".into()
            ])))
        );
    }

    #[test]
    fn test_command_array_with_options() {
        let user_config_json = json!({
            "command": ["echo one", "echo two"],
            "cwd": "src",
            "dependsOn": ["build"],
            "cache": false
        });
        let user_config: UserTaskConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(
            user_config.command,
            Command::Array(Arc::from(["echo one".into(), "echo two".into()]))
        );
        let options = user_config.options;
        assert_eq!(options.cwd_relative_to_package.as_ref().unwrap().as_str(), "src");
        assert_eq!(
            options.depends_on.as_ref().unwrap().as_ref(),
            [UserDependsOnEntry::Task(Str::from("build"))]
        );
        assert_eq!(options.cache_config, Some(UserCacheConfig::disabled()));
    }

    #[test]
    fn test_depends_on_package_dependency_single_from() {
        let user_config_json = json!({
            "command": "echo test",
            "dependsOn": [{ "task": "build", "from": "dependencies" }]
        });
        let user_config: UserTaskConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(
            user_config.options.depends_on.as_ref().unwrap().as_ref(),
            [UserDependsOnEntry::Package(UserPackageDependency {
                task: "build".into(),
                from: UserDependsOnFrom::Single(UserDependencyType::Dependencies),
            })]
        );
    }

    #[test]
    fn test_depends_on_package_dependency_array_from() {
        let user_config_json = json!({
            "command": "echo test",
            "dependsOn": [{
                "task": "build",
                "from": ["dependencies", "devDependencies", "peerDependencies"]
            }]
        });
        let user_config: UserTaskConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(
            user_config.options.depends_on.as_ref().unwrap().as_ref(),
            [UserDependsOnEntry::Package(UserPackageDependency {
                task: "build".into(),
                from: UserDependsOnFrom::Multiple(
                    Vec1::try_from_vec(vec![
                        UserDependencyType::Dependencies,
                        UserDependencyType::DevDependencies,
                        UserDependencyType::PeerDependencies,
                    ])
                    .unwrap()
                ),
            })]
        );
    }

    #[test]
    fn test_depends_on_package_dependency_empty_from_error() {
        let user_config_json = json!({
            "command": "echo test",
            "dependsOn": [{ "task": "build", "from": [] }]
        });
        assert!(serde_json::from_value::<UserTaskConfig>(user_config_json).is_err());
    }

    #[test]
    fn test_depends_on_package_dependency_unknown_from_error() {
        let user_config_json = json!({
            "command": "echo test",
            "dependsOn": [{ "task": "build", "from": "optionalDependencies" }]
        });
        assert!(serde_json::from_value::<UserTaskConfig>(user_config_json).is_err());
    }

    #[test]
    fn test_task_invalid_shorthand_error() {
        let user_config_json = json!({
            "tasks": {
                "build": 123
            }
        });
        assert!(serde_json::from_value::<UserRunConfig>(user_config_json).is_err());
    }

    #[test]
    fn test_command_array_invalid_item_error() {
        let user_config_json = json!({
            "command": ["echo one", 123]
        });
        assert!(serde_json::from_value::<UserTaskConfig>(user_config_json).is_err());
    }

    #[test]
    fn test_cwd_rename() {
        let user_config_json = json!({
            "command": "echo test",
            "cwd": "src"
        });
        let user_config: UserTaskConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(user_config.options.cwd_relative_to_package.as_ref().unwrap().as_str(), "src");
    }

    fn task_cache_config(user_config_json: serde_json::Value) -> Option<EnabledCacheConfig> {
        let user_config: UserTaskConfig = serde_json::from_value(user_config_json).unwrap();
        user_config.options.cache_config.unwrap_or_default().into_enabled()
    }

    #[test]
    fn test_cache_disabled() {
        let user_config_json = json!({
            "command": "echo test",
            "cache": false
        });
        let user_config: UserTaskConfig = serde_json::from_value(user_config_json.clone()).unwrap();
        assert_eq!(user_config.options.cache_config, Some(UserCacheConfig::disabled()));
        assert_eq!(task_cache_config(user_config_json), None);
    }

    #[test]
    fn test_cache_default_settings_forms() {
        for user_config_json in [
            json!({ "command": "echo test" }),
            json!({ "command": "echo test", "cache": true }),
            json!({ "command": "echo test", "cache": {} }),
        ] {
            assert_eq!(
                task_cache_config(user_config_json.clone()),
                Some(EnabledCacheConfig::default()),
                "{user_config_json}"
            );
        }
    }

    #[test]
    fn test_cache_object() {
        let user_config_json = json!({
            "command": "echo test",
            "cwd": "src",
            "cache": {
                "env": ["NODE_ENV"],
                "untrackedEnv": ["FOO"],
                "input": ["src/**"],
                "output": ["dist/**"],
                "remote": false,
            },
        });
        let user_config: UserTaskConfig = serde_json::from_value(user_config_json.clone()).unwrap();
        assert_eq!(user_config.options.cwd_relative_to_package.as_ref().unwrap().as_str(), "src");
        assert_eq!(
            task_cache_config(user_config_json),
            Some(EnabledCacheConfig {
                env: Some(std::iter::once("NODE_ENV".into()).collect()),
                untracked_env: Some(std::iter::once("FOO".into()).collect()),
                input: Some(vec![UserInputEntry::Glob("src/**".into())]),
                output: Some(vec![UserOutputEntry::Glob("dist/**".into())]),
                remote: Some(false),
            })
        );
    }

    #[test]
    fn test_cache_object_unknown_field_error() {
        let user_config_json = json!({
            "command": "echo test",
            "cache": { "foo": 42 },
        });
        assert!(serde_json::from_value::<UserTaskConfig>(user_config_json).is_err());
    }

    #[test]
    fn test_top_level_remote_cache_error() {
        let user_config_json = json!({
            "command": "echo test",
            "remoteCache": false,
        });
        assert!(serde_json::from_value::<UserTaskConfig>(user_config_json).is_err());
    }

    #[test]
    fn test_input_empty_array() {
        let user_config_json = json!({
            "input": []
        });
        let config: EnabledCacheConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(config.input, Some(vec![]));
    }

    #[test]
    fn test_input_auto_true() {
        let user_config_json = json!({
            "input": [{ "auto": true }]
        });
        let config: EnabledCacheConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(config.input, Some(vec![UserInputEntry::Auto(AutoTracking { auto: true })]));
    }

    #[test]
    fn test_input_auto_false() {
        let user_config_json = json!({
            "input": [{ "auto": false }]
        });
        let config: EnabledCacheConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(config.input, Some(vec![UserInputEntry::Auto(AutoTracking { auto: false })]));
    }

    #[test]
    fn test_input_globs() {
        let user_config_json = json!({
            "input": ["src/**/*.ts", "package.json"]
        });
        let config: EnabledCacheConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(
            config.input,
            Some(vec![
                UserInputEntry::Glob("src/**/*.ts".into()),
                UserInputEntry::Glob("package.json".into()),
            ])
        );
    }

    #[test]
    fn test_input_negative_globs() {
        let user_config_json = json!({
            "input": ["src/**", "!src/**/*.test.ts"]
        });
        let config: EnabledCacheConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(
            config.input,
            Some(vec![
                UserInputEntry::Glob("src/**".into()),
                UserInputEntry::Glob("!src/**/*.test.ts".into()),
            ])
        );
    }

    #[test]
    fn test_input_mixed() {
        let user_config_json = json!({
            "input": ["package.json", { "auto": true }, "!node_modules/**"]
        });
        let config: EnabledCacheConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(
            config.input,
            Some(vec![
                UserInputEntry::Glob("package.json".into()),
                UserInputEntry::Auto(AutoTracking { auto: true }),
                UserInputEntry::Glob("!node_modules/**".into()),
            ])
        );
    }

    #[test]
    fn test_input_glob_with_base_workspace() {
        let user_config_json = json!({
            "input": [{ "pattern": "configs/tsconfig.json", "base": "workspace" }]
        });
        let config: EnabledCacheConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(
            config.input,
            Some(vec![UserInputEntry::GlobWithBase(GlobWithBase {
                pattern: "configs/tsconfig.json".into(),
                base: InputBase::Workspace,
            })])
        );
    }

    #[test]
    fn test_input_glob_with_base_package() {
        let user_config_json = json!({
            "input": [{ "pattern": "src/**", "base": "package" }]
        });
        let config: EnabledCacheConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(
            config.input,
            Some(vec![UserInputEntry::GlobWithBase(GlobWithBase {
                pattern: "src/**".into(),
                base: InputBase::Package,
            })])
        );
    }

    #[test]
    fn test_input_negative_glob_with_base() {
        let user_config_json = json!({
            "input": [{ "pattern": "!dist/**", "base": "workspace" }]
        });
        let config: EnabledCacheConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(
            config.input,
            Some(vec![UserInputEntry::GlobWithBase(GlobWithBase {
                pattern: "!dist/**".into(),
                base: InputBase::Workspace,
            })])
        );
    }

    #[test]
    fn test_input_glob_with_base_missing_base_error() {
        // { "pattern": "src/**" } without "base" should fail (base is required)
        let user_config_json = json!({
            "input": [{ "pattern": "src/**" }]
        });
        let result = serde_json::from_value::<EnabledCacheConfig>(user_config_json);
        assert!(result.is_err(), "missing 'base' field should produce an error");
    }

    #[test]
    fn test_input_glob_with_base_invalid_base_error() {
        let user_config_json = json!({
            "input": [{ "pattern": "src/**", "base": "invalid" }]
        });
        let result = serde_json::from_value::<EnabledCacheConfig>(user_config_json);
        assert!(result.is_err(), "invalid 'base' value should produce an error");
    }

    #[test]
    fn test_input_mixed_auto_and_glob_with_base_error() {
        // An object with both "auto" and "pattern"/"base" should fail due to deny_unknown_fields
        let user_config_json = json!({
            "input": [{ "auto": true, "pattern": "src/**", "base": "workspace" }]
        });
        let result = serde_json::from_value::<EnabledCacheConfig>(user_config_json);
        assert!(result.is_err(), "mixing auto and pattern/base fields should produce an error");
    }

    #[test]
    fn test_input_mixed_with_glob_base() {
        let user_config_json = json!({
            "input": [
                "package.json",
                { "pattern": "configs/**", "base": "workspace" },
                { "auto": true },
                "!node_modules/**"
            ]
        });
        let config: EnabledCacheConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(
            config.input,
            Some(vec![
                UserInputEntry::Glob("package.json".into()),
                UserInputEntry::GlobWithBase(GlobWithBase {
                    pattern: "configs/**".into(),
                    base: InputBase::Workspace,
                }),
                UserInputEntry::Auto(AutoTracking { auto: true }),
                UserInputEntry::Glob("!node_modules/**".into()),
            ])
        );
    }

    #[test]
    fn test_global_cache_bool_true() {
        let config: UserGlobalCacheConfig = serde_json::from_value(json!(true)).unwrap();
        assert_eq!(config, UserGlobalCacheConfig::Bool(true));
        let resolved = ResolvedGlobalCacheConfig::resolve_from(Some(&config));
        assert!(resolved.scripts);
        assert!(resolved.tasks);
    }

    #[test]
    fn test_global_cache_bool_false() {
        let config: UserGlobalCacheConfig = serde_json::from_value(json!(false)).unwrap();
        assert_eq!(config, UserGlobalCacheConfig::Bool(false));
        let resolved = ResolvedGlobalCacheConfig::resolve_from(Some(&config));
        assert!(!resolved.scripts);
        assert!(!resolved.tasks);
    }

    #[test]
    fn test_global_cache_detailed_scripts_only() {
        let config: UserGlobalCacheConfig =
            serde_json::from_value(json!({ "scripts": true })).unwrap();
        let resolved = ResolvedGlobalCacheConfig::resolve_from(Some(&config));
        assert!(resolved.scripts);
        assert!(resolved.tasks); // defaults to true
    }

    #[test]
    fn test_global_cache_detailed_tasks_false() {
        let config: UserGlobalCacheConfig =
            serde_json::from_value(json!({ "tasks": false })).unwrap();
        let resolved = ResolvedGlobalCacheConfig::resolve_from(Some(&config));
        assert!(!resolved.scripts); // defaults to false
        assert!(!resolved.tasks);
    }

    #[test]
    fn test_global_cache_detailed_both() {
        let config: UserGlobalCacheConfig =
            serde_json::from_value(json!({ "scripts": true, "tasks": false })).unwrap();
        let resolved = ResolvedGlobalCacheConfig::resolve_from(Some(&config));
        assert!(resolved.scripts);
        assert!(!resolved.tasks);
    }

    #[test]
    fn test_global_cache_none_defaults() {
        let resolved = ResolvedGlobalCacheConfig::resolve_from(None);
        assert!(!resolved.scripts); // defaults to false
        assert!(resolved.tasks); // defaults to true
    }

    #[test]
    fn test_global_cache_detailed_unknown_field() {
        assert!(
            serde_json::from_value::<UserGlobalCacheConfig>(json!({ "unknown": true })).is_err()
        );
    }

    #[test]
    fn test_run_config_unknown_top_level_field() {
        assert!(serde_json::from_value::<UserRunConfig>(json!({ "unknown": true })).is_err());
    }

    #[test]
    fn test_task_config_unknown_field() {
        assert!(
            serde_json::from_value::<UserTaskConfig>(json!({ "command": "echo", "unknown": true }))
                .is_err()
        );
    }
}
