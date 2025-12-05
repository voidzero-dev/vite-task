mod user;

use std::collections::HashSet;

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
    pub cwd: AbsolutePathBuf,

    /// Cache-related config. None means caching is disabled.
    pub cache_config: Option<CacheConfig>,
}

#[derive(Debug)]
pub struct CacheConfig {
    /// environment variable names to be fingerprinted and passed to the task, with defaults populated
    pub envs: HashSet<Str>,
    /// environment variable names to be passed to the task without fingerprinting, with defaults populated
    pub pass_through_envs: HashSet<Str>,
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
        package_dir: &AbsolutePath,
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
        package_dir: &AbsolutePath,
        package_json_script: Option<&str>,
    ) -> Result<Self, ResolveTaskError> {
        let command = match (&user_config.command, package_json_script) {
            (Some(_), Some(_)) => return Err(ResolveTaskError::CommandConflict),
            (None, None) => return Err(ResolveTaskError::NoCommand),
            (Some(cmd), None) => cmd.as_ref(),
            (None, Some(script)) => script,
        };
        let cwd = package_dir.join(user_config.cwd_relative_to_package);
        let cache_config = match user_config.cache_config {
            UserCacheConfig::Disabled { cache: MustBe!(false) } => None,
            UserCacheConfig::Enabled { cache: MustBe!(true), envs, pass_through_envs } => {
                Some(CacheConfig {
                    envs: envs.into_iter().collect(),
                    pass_through_envs: pass_through_envs.into_iter().collect(),
                })
            }
        };
        Ok(Self { command: command.into(), cwd, cache_config })
    }
}
