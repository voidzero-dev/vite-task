pub mod user;

use std::sync::Arc;

use monostate::MustBe;
use rustc_hash::FxHashSet;
use serde::Serialize;
pub use user::{
    EnabledCacheConfig, ResolvedGlobalCacheConfig, UserCacheConfig, UserGlobalCacheConfig,
    UserRunConfig, UserTaskConfig,
};
use vite_path::AbsolutePath;
use vite_str::Str;

use crate::config::user::UserTaskOptions;

/// Task configuration resolved from `package.json` scripts and/or `vite.config.ts` tasks,
/// without considering external factors like additional args from cli or environment variables.
///
/// It should resolve as much as possible to the final form to save duplicated work when it's further resolved into a spawnable command later.
/// but must be independent of external factors.
///
/// For example, `cwd` is resolved to absolute ones (no external factor can change it),
/// but `command` is not parsed into program and args yet because environment variables in it may need to be expanded.
///
/// `depends_on` is not included here because it's represented by the edges of the task graph.
#[derive(Debug, Serialize)]
pub struct ResolvedTaskConfig {
    /// The command to run for this task, as a raw string.
    ///
    /// The command may contain environment variables that need to be expanded later.
    pub command: Str,

    pub resolved_options: ResolvedTaskOptions,
}

#[derive(Debug, Serialize)]
pub struct ResolvedTaskOptions {
    /// The working directory for the task
    pub cwd: Arc<AbsolutePath>,
    /// Cache-related config. None means caching is disabled.
    pub cache_config: Option<CacheConfig>,
}

impl ResolvedTaskOptions {
    /// Resolves from user-defined options and the directory path where the options are defined.
    /// For user-defined tasks or scripts in package.json, `dir` is the package directory
    /// For synthetic tasks, `dir` is the cwd of the original command (e.g. the cwd of `vp lint`).
    pub fn resolve(user_options: UserTaskOptions, dir: &Arc<AbsolutePath>) -> Self {
        let cwd: Arc<AbsolutePath> = match user_options.cwd_relative_to_package {
            Some(ref cwd) if !cwd.as_str().is_empty() => dir.join(cwd).into(),
            _ => Arc::clone(dir),
        };
        let cache_config = match user_options.cache_config {
            UserCacheConfig::Disabled { cache: MustBe!(false) } => None,
            UserCacheConfig::Enabled { cache: _, enabled_cache_config } => {
                let mut pass_through_envs: FxHashSet<Str> = enabled_cache_config
                    .pass_through_envs
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                pass_through_envs.extend(DEFAULT_PASSTHROUGH_ENVS.iter().copied().map(Str::from));
                Some(CacheConfig {
                    env_config: EnvConfig {
                        fingerprinted_envs: enabled_cache_config
                            .envs
                            .map(|e| e.into_vec().into_iter().collect())
                            .unwrap_or_default(),
                        pass_through_envs,
                    },
                })
            }
        };
        Self { cwd, cache_config }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheConfig {
    pub env_config: EnvConfig,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvConfig {
    /// environment variable names to be fingerprinted and passed to the task, with defaults populated
    pub fingerprinted_envs: FxHashSet<Str>,
    /// environment variable names to be passed to the task without fingerprinting, with defaults populated
    pub pass_through_envs: FxHashSet<Str>,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveTaskConfigError {
    /// Both package.json script and vite.config.* task define commands for the task
    #[error("Both package.json script and vite.config.* task define commands for the task")]
    CommandConflict,

    /// Neither package.json script nor vite.config.* task define a command for the task
    #[error("Neither package.json script nor vite.config.* task define a command for the task")]
    NoCommand,
}

impl ResolvedTaskConfig {
    /// Resolve from package.json script only (no vite-task.json config for this task)
    ///
    /// The `cache_scripts` parameter determines whether caching is enabled for the script.
    /// When `true`, caching is enabled with default settings.
    /// When `false`, caching is disabled.
    #[must_use]
    pub fn resolve_package_json_script(
        package_dir: &Arc<AbsolutePath>,
        package_json_script: &str,
        cache_scripts: bool,
    ) -> Self {
        let cache_config = if cache_scripts {
            UserCacheConfig::Enabled {
                cache: None,
                enabled_cache_config: EnabledCacheConfig { envs: None, pass_through_envs: None },
            }
        } else {
            UserCacheConfig::Disabled { cache: MustBe!(false) }
        };
        let options = UserTaskOptions { cache_config, ..Default::default() };
        Self {
            command: package_json_script.into(),
            resolved_options: ResolvedTaskOptions::resolve(options, package_dir),
        }
    }

    /// Resolves from user config, package dir, and package.json script (if any).
    ///
    /// # Errors
    ///
    /// Returns [`ResolveTaskConfigError::CommandConflict`] if both the user config and
    /// package.json define a command, or [`ResolveTaskConfigError::NoCommand`] if neither does.
    pub fn resolve(
        user_config: UserTaskConfig,
        package_dir: &Arc<AbsolutePath>,
        package_json_script: Option<&str>,
    ) -> Result<Self, ResolveTaskConfigError> {
        let command = match (&user_config.command, package_json_script) {
            (Some(_), Some(_)) => return Err(ResolveTaskConfigError::CommandConflict),
            (None, None) => return Err(ResolveTaskConfigError::NoCommand),
            (Some(cmd), None) => cmd.as_ref(),
            (None, Some(script)) => script,
        };
        Ok(Self {
            command: command.into(),
            resolved_options: ResolvedTaskOptions::resolve(user_config.options, package_dir),
        })
    }
}

// Exact matches for common environment variables
// Referenced from Turborepo's implementation:
// https://github.com/vercel/turborepo/blob/26d309f073ca3ac054109ba0c29c7e230e7caac3/crates/turborepo-lib/src/task_hash.rs#L439
#[doc(hidden)] // exported for redacting snapshots in tests
pub const DEFAULT_PASSTHROUGH_ENVS: &[&str] = &[
    // System and shell
    "HOME",
    "USER",
    "TZ",
    "LANG",
    "SHELL",
    "PWD",
    "PATH",
    // CI/CD environments
    "CI",
    // Node.js specific
    "NODE_OPTIONS",
    "COREPACK_HOME",
    "NPM_CONFIG_STORE_DIR",
    "PNPM_HOME",
    // Library paths
    "LD_LIBRARY_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH",
    "LIBPATH",
    // Terminal/display
    "COLORTERM",
    "TERM",
    "TERM_PROGRAM",
    "DISPLAY",
    "FORCE_COLOR",
    "NO_COLOR",
    // Temporary directories
    "TMP",
    "TEMP",
    // Vercel specific
    "VERCEL",
    "VERCEL_*",
    "NEXT_*",
    "USE_OUTPUT_FOR_EDGE_FUNCTIONS",
    "NOW_BUILDER",
    // Windows specific
    "APPDATA",
    "PROGRAMDATA",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    // IDE specific (exact matches)
    "ELECTRON_RUN_AS_NODE",
    "JB_INTERPRETER",
    "_JETBRAINS_TEST_RUNNER_RUN_SCOPE_TYPE",
    "JB_IDE_*",
    // VSCode specific
    "VSCODE_*",
    // Docker specific
    "DOCKER_*",
    "BUILDKIT_*",
    "COMPOSE_*",
    // Playwright specific
    "PLAYWRIGHT_*",
    // Token patterns
    "*_TOKEN",
];
