//! Configuration structures for user-defined tasks in `vite.config.*`

use std::{collections::HashMap, sync::Arc};

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
        envs: Box<[Str]>,

        /// Environment variable names to be passed to the task without fingerprinting.
        #[serde(default)] // default to empty if omitted
        pass_through_envs: Box<[Str]>,
    },
    /// Cache is disabled
    Disabled {
        /// The `cache` field must be false
        cache: MustBe!(false),
    },
}

/// Task configuration defined by user in `vite.config.*`
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserTaskConfig {
    /// If None, the script from `package.json` with the same name will be used
    pub command: Option<Box<str>>,

    /// The working directory for the task, relative to the package root (not workspace root).
    #[serde(default)] // default to empty if omitted
    #[serde(rename = "cwd")]
    pub cwd_relative_to_package: RelativePathBuf,

    /// Explicit dependencies of this task.
    #[serde(default)] // default to empty if omitted
    pub depends_on: Arc<[Str]>,

    /// Cache-related fields
    #[serde(flatten)]
    pub cache_config: UserCacheConfig,
}

impl UserTaskConfig {
    /// The default user task config for package.json scripts.
    pub fn package_json_script_default() -> Self {
        Self {
            command: None,
            cwd_relative_to_package: RelativePathBuf::default(),
            depends_on: Arc::new([]),
            cache_config: UserCacheConfig::Enabled {
                cache: MustBe!(true),
                envs: Box::new([]),
                pass_through_envs: Box::new([]),
            },
        }
    }
}

/// User configuration file structure for `vite.config.*`
#[derive(Debug, Deserialize)]
pub struct UserConfigFile {
    pub tasks: HashMap<Str, UserTaskConfig>,
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
                cwd_relative_to_package: "".try_into().unwrap(),
                depends_on: Default::default(),
                cache_config: UserCacheConfig::Enabled {
                    cache: MustBe!(true),
                    envs: Default::default(),
                    pass_through_envs: Default::default(),
                },
            }
        );
    }

    #[test]
    fn test_cwd_rename() {
        let user_config_json = json!({
            "cwd": "src"
        });
        let user_config: UserTaskConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(user_config.cwd_relative_to_package.as_str(), "src");
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
                pass_through_envs: Default::default(),
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

    #[test]
    fn test_deny_unknown_field() {
        let user_config_json = json!({
            "foo": 42,
        });
        assert!(serde_json::from_value::<UserCacheConfig>(user_config_json).is_err());
    }
}
