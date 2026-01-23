//! Configuration structures for user-defined tasks in `vite.config.*`

use std::{collections::HashMap, sync::Arc};

use monostate::MustBe;
use serde::Deserialize;
#[cfg(feature = "ts-types")]
use ts_rs::TS;
use vite_path::RelativePathBuf;
use vite_str::Str;

/// Cache-related fields of a task defined by user in `vite.config.*`
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts-types", derive(TS))]
#[serde(untagged, deny_unknown_fields, rename_all = "camelCase")]
pub enum UserCacheConfig {
    /// Cache is enabled
    Enabled {
        /// The `cache` field must be true or omitted
        #[serde(default)]
        #[cfg_attr(feature = "ts-types", ts(type = "true"))]
        cache: MustBe!(true),

        #[serde(flatten)]
        enabled_cache_config: EnabledCacheConfig,
    },
    /// Cache is disabled
    Disabled {
        /// The `cache` field must be false
        #[cfg_attr(feature = "ts-types", ts(type = "false"))]
        cache: MustBe!(false),
    },
}

/// Cache configuration fields when caching is enabled
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts-types", derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct EnabledCacheConfig {
    /// Environment variable names to be fingerprinted and passed to the task.
    #[serde(default)] // default to empty if omitted
    pub envs: Box<[Str]>,

    /// Environment variable names to be passed to the task without fingerprinting.
    #[serde(default)] // default to empty if omitted
    pub pass_through_envs: Vec<Str>,
}

/// Options for user-defined tasks in `vite.config.*`, excluding the command.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts-types", derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct UserTaskOptions {
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

impl Default for UserTaskOptions {
    /// The default user task options for package.json scripts.
    fn default() -> Self {
        Self {
            // Runs in the package root
            cwd_relative_to_package: RelativePathBuf::default(),
            // No dependencies
            depends_on: Arc::new([]),
            // Caching enabled with no fingerprinted envs
            cache_config: UserCacheConfig::Enabled {
                cache: MustBe!(true),
                enabled_cache_config: EnabledCacheConfig {
                    envs: Box::new([]),
                    pass_through_envs: Vec::new(),
                },
            },
        }
    }
}

/// Full user-defined task configuration in `vite.config.*`, including the command and options.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts-types", derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct UserTaskConfig {
    /// If None, the script from `package.json` with the same name will be used
    pub command: Option<Box<str>>,

    /// Fields other than the command
    #[serde(flatten)]
    pub options: UserTaskOptions,
}

/// User configuration file structure for `vite.config.*`
#[derive(Debug, Deserialize)]
pub struct UserConfigFile {
    pub tasks: UserConfigTasks,
}

/// Type of the `tasks` field in `vite.config.*`
#[derive(Debug, Default, Deserialize)]
#[serde(transparent)]
#[cfg_attr(feature = "ts-types", derive(TS))]
pub struct UserConfigTasks(pub HashMap<Str, UserTaskConfig>);

#[cfg(feature = "ts-types")]
impl UserConfigTasks {
    /// Returns the TypeScript type definitions for user task configuration.
    pub fn typescript_definition() -> String {
        use ts_rs::TypeVisitor;

        struct DeclCollector(Vec<String>);

        impl TypeVisitor for DeclCollector {
            fn visit<T: TS + 'static + ?Sized>(&mut self) {
                // Only collect declarations from types that are exportable
                // (i.e., have an output path - built-in types like HashMap don't)
                if T::output_path().is_some() {
                    self.0.push(T::decl());
                }
            }
        }

        let mut collector = DeclCollector(Vec::new());
        Self::visit_dependencies(&mut collector);
        collector.0.push(Self::decl());
        collector.0.join("\n\n")
    }
}

#[cfg(all(test, feature = "ts-types"))]
mod ts_tests {
    use super::*;

    #[test]
    fn test_typescript_generation() {
        let ts = UserConfigTasks::typescript_definition();
        eprintln!("Generated TypeScript:\n{ts}");
        // Check for key type definitions
        assert!(ts.contains("type UserTaskConfig"), "Missing UserTaskConfig in:\n{ts}");
        assert!(ts.contains("type UserConfigTasks"), "Missing UserConfigTasks in:\n{ts}");
        // Check for key fields (flattened types are inlined)
        assert!(ts.contains("cache: true"), "Missing cache: true in:\n{ts}");
        assert!(ts.contains("cache: false"), "Missing cache: false in:\n{ts}");
        assert!(ts.contains("cwd:"), "Missing cwd field in:\n{ts}");
        assert!(ts.contains("dependsOn:"), "Missing dependsOn field in:\n{ts}");
    }
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
                // A empty task config (`{}`) should be equivalent to not specifying any config at all (just package.json script)
                options: UserTaskOptions::default(),
            }
        );
    }

    #[test]
    fn test_cwd_rename() {
        let user_config_json = json!({
            "cwd": "src"
        });
        let user_config: UserTaskConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(user_config.options.cwd_relative_to_package.as_str(), "src");
    }

    #[test]
    fn test_cache_disabled() {
        let user_config_json = json!({
            "cache": false
        });
        let user_config: UserTaskConfig = serde_json::from_value(user_config_json).unwrap();
        assert_eq!(
            user_config.options.cache_config,
            UserCacheConfig::Disabled { cache: MustBe!(false) }
        );
    }

    #[test]
    fn test_cache_explicitly_enabled() {
        let user_config_json = json!({
            "cache": true,
            "envs": ["NODE_ENV"],
            "passThroughEnvs": ["FOO"],
        });
        assert_eq!(
            serde_json::from_value::<UserCacheConfig>(user_config_json).unwrap(),
            UserCacheConfig::Enabled {
                cache: MustBe!(true),
                enabled_cache_config: EnabledCacheConfig {
                    envs: ["NODE_ENV".into()].into_iter().collect(),
                    pass_through_envs: ["FOO".into()].into_iter().collect(),
                }
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
