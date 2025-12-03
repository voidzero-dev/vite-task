use std::collections::{HashMap, HashSet};

use monostate::MustBe;
use serde::Deserialize;
use vite_path::RelativePathBuf;
use vite_str::Str;

/// Cache-related fields of a task defined by user in `vite.config.*`
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged, deny_unknown_fields, rename_all = "camelCase")]
pub enum UserCacheConfig {
    /// Cache is enabled
    Enabled {
        /// The `cache` field must be true or omitted
        #[serde(default)]
        cache: MustBe!(true),

        // Fields only relevant when cache is enabled
        /// Environment variable names to be fingerprinted and passed to the task.
        #[serde(default)] // default to empty if omitted
        envs: HashSet<Str>,

        /// Environment variable names to be passed to the task without fingerprinting.
        #[serde(default)] // default to empty if omitted
        pass_through_envs: HashSet<Str>,
    },
    /// Cache is disabled
    Disabled {
        /// The `cache` field must be false
        cache: MustBe!(false),
    },
}

/// Task configuration defined by user in `vite.config.*`
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_defaults() {
        let user_config_json = json!({});
        let user_config: UserTaskConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(
            user_config,
            UserTaskConfig {
                command: None,
                cwd: "".try_into().unwrap(),
                depends_on: HashSet::new(),
                cache_config: UserCacheConfig::Enabled {
                    cache: MustBe!(true),
                    envs: HashSet::new(),
                    pass_through_envs: HashSet::new(),
                },
            }
        );
    }

    #[test]
    fn test_cache_disabled() {
        let user_config_json = json!({
            "cache": false
        });
        let user_config: UserTaskConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(user_config.cache_config, UserCacheConfig::Disabled { cache: MustBe!(false) });
    }

    #[test]
    fn test_cache_explictly_enabled() {
        let user_config_json = json!({
            "cache": true,
            "envs": ["NODE_ENV"],
        });
        assert_eq!(
            serde_json::from_value::<UserCacheConfig>(user_config_json).unwrap(),
            UserCacheConfig::Enabled {
                cache: MustBe!(true),
                envs: ["NODE_ENV".into()].into_iter().collect(),
                pass_through_envs: HashSet::new(),
            },
        );
    }

    #[test]
    fn test_cache_disabled_but_with_fields() {
        let user_config_json = json!({
            "cache": false,
            "envs": ["NODE_ENV"],
        });
        assert!(serde_json::from_value::<UserCacheConfig>(user_config_json).is_err());
    }
}
