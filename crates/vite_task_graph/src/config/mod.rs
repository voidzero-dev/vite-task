mod user;

use std::{collections::HashSet, sync::Arc};

use monostate::MustBe;
pub use user::{UserCacheConfig, UserConfigFile, UserTaskConfig};
use vite_path::{AbsolutePath, AbsolutePathBuf};
use vite_str::Str;

/// Task configuration resolved from `package.json` scripts and/or `vite.config.ts` tasks,
/// without considering external factors like additional args from cli or environment variables.
///
/// It should resolve as much as possible to the final form to save duplicated work when it's further resolved into a spawnable command later.
/// but must be independent of external factors.
///
/// For example, `cwd` is resolved to absolute ones (no external factor can change it),
/// but `command` is not parsed into program and args yet because environment variables in it may need to be expanded.
///
/// `depends_on` is not included here because it's represented in the task graph.
#[derive(Debug)]
pub struct ResolvedUserTaskConfig {
    /// The command to run for this task
    pub command: Str,

    /// The working directory for the task
    pub cwd: Arc<AbsolutePath>,

    /// Cache-related config. None means caching is disabled.
    pub cache_config: Option<CacheConfig>,
}

#[derive(Debug)]
pub struct CacheConfig {
    pub env_config: EnvConfig,
}

#[derive(Debug)]
pub struct EnvConfig {
    /// environment variable names to be fingerprinted and passed to the task, with defaults populated
    pub fingerprinted_envs: HashSet<Str>,
    /// environment variable names to be passed to the task without fingerprinting, with defaults populated
    pub pass_through_envs: Arc<[Str]>,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveTaskError {
    /// Both package.json script and vite.config.* task define commands for the task
    #[error("Both package.json script and vite.config.* task define commands for the task")]
    CommandConflict,

    /// Neither package.json script nor vite.config.* task define a command for the task
    #[error("Neither package.json script nor vite.config.* task define a command for the task")]
    NoCommand,
}

impl ResolvedUserTaskConfig {
    pub fn resolve_package_json_script(
        package_dir: &Arc<AbsolutePath>,
        package_json_script: &str,
    ) -> Self {
        Self::resolve(
            UserTaskConfig::package_json_script_default(),
            package_dir,
            Some(package_json_script),
        )
        .expect("Command conflict/missing for package.json script should never happen")
    }

    /// Resolves from user config, package dir, and package.json script (if any).
    pub fn resolve(
        user_config: UserTaskConfig,
        package_dir: &Arc<AbsolutePath>,
        package_json_script: Option<&str>,
    ) -> Result<Self, ResolveTaskError> {
        let command = match (&user_config.command, package_json_script) {
            (Some(_), Some(_)) => return Err(ResolveTaskError::CommandConflict),
            (None, None) => return Err(ResolveTaskError::NoCommand),
            (Some(cmd), None) => cmd.as_ref(),
            (None, Some(script)) => script,
        };
        let cwd: Arc<AbsolutePath> = if user_config.cwd_relative_to_package.as_str().is_empty() {
            Arc::clone(package_dir)
        } else {
            package_dir.join(user_config.cwd_relative_to_package).into()
        };
        let cache_config = match user_config.cache_config {
            UserCacheConfig::Disabled { cache: MustBe!(false) } => None,
            UserCacheConfig::Enabled { cache: MustBe!(true), envs, mut pass_through_envs } => {
                pass_through_envs.extend(DEFAULT_PASSTHROUGH_ENVS.iter().copied().map(Str::from));
                Some(CacheConfig {
                    env_config: EnvConfig {
                        fingerprinted_envs: envs.into_iter().collect(),
                        pass_through_envs: pass_through_envs.into(),
                    },
                })
            }
        };
        Ok(Self { command: command.into(), cwd, cache_config })
    }
}

// Exact matches for common environment variables
// Referenced from Turborepo's implementation:
// https://github.com/vercel/turborepo/blob/26d309f073ca3ac054109ba0c29c7e230e7caac3/crates/turborepo-lib/src/task_hash.rs#L439
const DEFAULT_PASSTHROUGH_ENVS: &[&str] = &[
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
    // Token patterns
    "*_TOKEN",
];
