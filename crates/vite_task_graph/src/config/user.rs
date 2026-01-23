//! Configuration structures for user-defined tasks in `vite.config.*`

use std::{collections::HashMap, sync::Arc};

use monostate::MustBe;
use serde::Deserialize;
use ts_rs::TS;
use vite_path::RelativePathBuf;
use vite_str::Str;

/// Cache-related fields of a task defined by user in `vite.config.*`
#[derive(Debug, Deserialize, PartialEq, Eq, TS)]
#[serde(untagged, deny_unknown_fields, rename_all = "camelCase")]
pub enum UserCacheConfig {
    /// Cache is enabled
    Enabled {
        /// Whether to cache the task
        #[serde(default)]
        #[ts(type = "true")]
        cache: MustBe!(true),

        #[serde(flatten)]
        enabled_cache_config: EnabledCacheConfig,
    },
    /// Cache is disabled
    Disabled {
        /// Whether to cache the task
        #[ts(type = "false")]
        cache: MustBe!(false),
    },
}

/// Cache configuration fields when caching is enabled
#[derive(Debug, Deserialize, PartialEq, Eq, TS)]
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
#[derive(Debug, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
pub struct UserTaskOptions {
    /// The working directory for the task, relative to the package root (not workspace root).
    #[serde(default)] // default to empty if omitted
    #[serde(rename = "cwd")]
    pub cwd_relative_to_package: RelativePathBuf,

    /// Dependencies of this task. Use `package-name#task-name` to refer to tasks in other packages.
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
#[derive(Debug, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "Task")]
pub struct UserTaskConfig {
    /// The command to run for the task.
    ///
    /// If omitted, the script from `package.json` with the same name will be used
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
#[derive(Debug, Default, Deserialize, TS)]
#[serde(transparent)]
#[ts(rename = "Tasks")]
pub struct UserConfigTasks(pub HashMap<Str, UserTaskConfig>);

impl UserConfigTasks {
    /// TypeScript type definitions for user task configuration.
    pub const TS_TYPE: &str = include_str!("../../task-config.ts");

    /// Generates TypeScript type definitions for user task configuration.
    #[cfg(test)]
    pub fn generate_ts_definition() -> String {
        use dprint_plugin_typescript::{
            FormatTextOptions, configuration::ConfigurationBuilder, format_text,
        };
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
        let mut types = collector.0.join("\n\n");

        // Export the main type
        types.push_str("\n\nexport ");
        types.push_str(&Self::decl());

        // Format
        let fmt_cfg = ConfigurationBuilder::new().build();
        let options = FormatTextOptions {
            config: &fmt_cfg,
            path: std::path::Path::new("task-config.ts"),
            text: types.clone(),
            extension: None,
            external_formatter: None,
        };
        format_text(options).unwrap().unwrap()
    }
}

#[cfg(test)]
mod ts_tests {
    use std::{env, path::PathBuf};

    use super::UserConfigTasks;

    #[test]
    fn typescript_generation() {
        let file_path =
            PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("task-config.ts");
        let ts = UserConfigTasks::generate_ts_definition();

        if env::var("VT_UPDATE_TS_TYPES").unwrap_or_default() == "1" {
            std::fs::write(&file_path, ts).unwrap();
        } else {
            let existing_ts = std::fs::read_to_string(&file_path).unwrap_or_default();
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
