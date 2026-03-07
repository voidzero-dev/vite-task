pub mod user;

use std::{collections::BTreeSet, sync::Arc};

use bincode::{Decode, Encode};
use monostate::MustBe;
use rustc_hash::FxHashSet;
use serde::Serialize;
pub use user::{
    EnabledCacheConfig, ResolvedGlobalCacheConfig, UserCacheConfig, UserGlobalCacheConfig,
    UserInputEntry, UserInputsConfig, UserRunConfig, UserTaskConfig,
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

                let input_config =
                    ResolvedInputConfig::from_user_config(enabled_cache_config.inputs.as_ref());

                Some(CacheConfig {
                    env_config: EnvConfig {
                        fingerprinted_envs: enabled_cache_config
                            .envs
                            .map(|e| e.into_vec().into_iter().collect())
                            .unwrap_or_default(),
                        pass_through_envs,
                    },
                    input_config,
                })
            }
        };
        Self { cwd, cache_config }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheConfig {
    pub env_config: EnvConfig,
    pub input_config: ResolvedInputConfig,
}

/// Resolved input configuration for cache fingerprinting.
///
/// This is the normalized form after parsing user config.
/// - `includes_auto`: Whether automatic file tracking is enabled
/// - `positive_globs`: Glob patterns for files to include (without `!` prefix)
/// - `negative_globs`: Glob patterns for files to exclude (without `!` prefix)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Encode, Decode)]
pub struct ResolvedInputConfig {
    /// Whether automatic file tracking is enabled
    pub includes_auto: bool,

    /// Positive glob patterns (files to include).
    /// Sorted for deterministic cache keys.
    pub positive_globs: BTreeSet<Str>,

    /// Negative glob patterns (files to exclude, without the `!` prefix).
    /// Sorted for deterministic cache keys.
    pub negative_globs: BTreeSet<Str>,
}

impl ResolvedInputConfig {
    /// Default configuration: auto-inference enabled, no explicit patterns
    #[must_use]
    pub const fn default_auto() -> Self {
        Self {
            includes_auto: true,
            positive_globs: BTreeSet::new(),
            negative_globs: BTreeSet::new(),
        }
    }

    /// Resolve from user configuration.
    ///
    /// - `None`: defaults to auto-inference (`[{auto: true}]`)
    /// - `Some([])`: no inputs, inference disabled
    /// - `Some([...])`: explicit patterns
    #[must_use]
    pub fn from_user_config(user_inputs: Option<&UserInputsConfig>) -> Self {
        let Some(entries) = user_inputs else {
            // None means default to auto-inference
            return Self::default_auto();
        };

        let mut includes_auto = false;
        let mut positive_globs = BTreeSet::new();
        let mut negative_globs = BTreeSet::new();

        for entry in entries {
            match entry {
                UserInputEntry::Auto { auto: true } => includes_auto = true,
                UserInputEntry::Auto { auto: false } => {} // Ignore {auto: false}
                UserInputEntry::Glob(pattern) => {
                    if let Some(negated) = pattern.strip_prefix('!') {
                        negative_globs.insert(Str::from(negated));
                    } else {
                        positive_globs.insert(pattern.clone());
                    }
                }
            }
        }

        Self { includes_auto, positive_globs, negative_globs }
    }
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
    /// Resolve from package.json script only (no config entry for this task).
    ///
    /// Always resolves with caching enabled (default settings).
    /// The global cache config is applied at plan time, not here.
    #[must_use]
    pub fn resolve_package_json_script(
        package_dir: &Arc<AbsolutePath>,
        package_json_script: &str,
    ) -> Self {
        Self {
            command: package_json_script.into(),
            resolved_options: ResolvedTaskOptions::resolve(UserTaskOptions::default(), package_dir),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolved_input_config_default_auto() {
        let config = ResolvedInputConfig::default_auto();
        assert!(config.includes_auto);
        assert!(config.positive_globs.is_empty());
        assert!(config.negative_globs.is_empty());
    }

    #[test]
    fn test_resolved_input_config_from_none() {
        // None means default to auto-inference
        let config = ResolvedInputConfig::from_user_config(None);
        assert!(config.includes_auto);
        assert!(config.positive_globs.is_empty());
        assert!(config.negative_globs.is_empty());
    }

    #[test]
    fn test_resolved_input_config_empty_array() {
        // Empty array means no inputs, inference disabled
        let user_inputs = vec![];
        let config = ResolvedInputConfig::from_user_config(Some(&user_inputs));
        assert!(!config.includes_auto);
        assert!(config.positive_globs.is_empty());
        assert!(config.negative_globs.is_empty());
    }

    #[test]
    fn test_resolved_input_config_auto_only() {
        let user_inputs = vec![UserInputEntry::Auto { auto: true }];
        let config = ResolvedInputConfig::from_user_config(Some(&user_inputs));
        assert!(config.includes_auto);
        assert!(config.positive_globs.is_empty());
        assert!(config.negative_globs.is_empty());
    }

    #[test]
    fn test_resolved_input_config_auto_false_ignored() {
        let user_inputs = vec![UserInputEntry::Auto { auto: false }];
        let config = ResolvedInputConfig::from_user_config(Some(&user_inputs));
        assert!(!config.includes_auto);
        assert!(config.positive_globs.is_empty());
        assert!(config.negative_globs.is_empty());
    }

    #[test]
    fn test_resolved_input_config_globs_only() {
        // Globs without auto means inference disabled
        let user_inputs = vec![
            UserInputEntry::Glob("src/**/*.ts".into()),
            UserInputEntry::Glob("package.json".into()),
        ];
        let config = ResolvedInputConfig::from_user_config(Some(&user_inputs));
        assert!(!config.includes_auto);
        assert_eq!(config.positive_globs.len(), 2);
        assert!(config.positive_globs.contains("src/**/*.ts"));
        assert!(config.positive_globs.contains("package.json"));
        assert!(config.negative_globs.is_empty());
    }

    #[test]
    fn test_resolved_input_config_negative_globs() {
        let user_inputs = vec![
            UserInputEntry::Glob("src/**".into()),
            UserInputEntry::Glob("!src/**/*.test.ts".into()),
        ];
        let config = ResolvedInputConfig::from_user_config(Some(&user_inputs));
        assert!(!config.includes_auto);
        assert_eq!(config.positive_globs.len(), 1);
        assert!(config.positive_globs.contains("src/**"));
        assert_eq!(config.negative_globs.len(), 1);
        assert!(config.negative_globs.contains("src/**/*.test.ts")); // Without ! prefix
    }

    #[test]
    fn test_resolved_input_config_mixed() {
        let user_inputs = vec![
            UserInputEntry::Glob("package.json".into()),
            UserInputEntry::Auto { auto: true },
            UserInputEntry::Glob("!node_modules/**".into()),
        ];
        let config = ResolvedInputConfig::from_user_config(Some(&user_inputs));
        assert!(config.includes_auto);
        assert_eq!(config.positive_globs.len(), 1);
        assert!(config.positive_globs.contains("package.json"));
        assert_eq!(config.negative_globs.len(), 1);
        assert!(config.negative_globs.contains("node_modules/**"));
    }

    #[test]
    fn test_resolved_input_config_globs_with_auto() {
        // Globs with auto keeps inference enabled
        let user_inputs =
            vec![UserInputEntry::Glob("src/**/*.ts".into()), UserInputEntry::Auto { auto: true }];
        let config = ResolvedInputConfig::from_user_config(Some(&user_inputs));
        assert!(config.includes_auto);
    }
}
