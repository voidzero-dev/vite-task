use std::collections::{HashMap, HashSet};

use monostate::MustBe;
use serde::Deserialize;
use vite_path::RelativePathBuf;
use vite_str::Str;

/// Cache-related fields of a task defined by user in `vite.config.*`
#[derive(Debug, Deserialize)]
#[serde(untagged, rename_all = "camelCase")]
pub enum UserCacheConfig {
    /// Cache is enabled
    Enabled {
        /// The `cache` field must be true or omitted
        #[serde(default)]
        cache: MustBe!(true),

        // Fields only relevant when cache is enabled
        /// Environment variable names to be fingerprinted and passed to the task.
        envs: HashSet<Str>,

        /// Environment variable names to be passed to the task without fingerprinting.
        pass_through_envs: HashSet<Str>,
    },
    /// Cache is disabled
    Disabled {
        /// The `cache` field must be false
        cache: MustBe!(false),
    },
}

/// Task configuration defined by user in `vite.config.*`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserTaskConfig {
    /// If None, the script from `package.json` with the same name will be used
    command: Option<Box<str>>,

    /// The working directory for the task, relative to the package root (not workspace root).
    #[serde(default)] // default to empty if omitted
    cwd: RelativePathBuf,

    /// Explicit dependencies of this task.
    #[serde(default)] // default to empty if omitted
    depends_on: HashSet<Str>,

    /// Cache-related fields
    #[serde(flatten)]
    cache_config: UserCacheConfig,
}

/// User configuration file structure for `vite.config.*`
pub struct UserConfigFile {
    tasks: HashMap<Str, UserTaskConfig>,
}
