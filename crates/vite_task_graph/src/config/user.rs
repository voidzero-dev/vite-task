//! Configuration structures for user-defined tasks in `vite.config.*`

use std::{collections::HashMap, sync::Arc};

use monostate::MustBe;
use serde::Deserialize;
#[cfg(test)]
use ts_rs::TS;
use vite_path::RelativePathBuf;
use vite_str::Str;

/// Cache-related fields of a task defined by user in `vite.config.*`
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[cfg_attr(test, derive(TS), ts(optional_fields))]
#[serde(untagged, deny_unknown_fields, rename_all = "camelCase")]
pub enum UserCacheConfig {
    /// Cache is enabled
    Enabled {
        /// Whether to cache the task
        #[cfg_attr(test, ts(type = "true", optional))]
        cache: Option<MustBe!(true)>,

        #[serde(flatten)]
        enabled_cache_config: EnabledCacheConfig,
    },
    /// Cache is disabled
    Disabled {
        /// Whether to cache the task
        #[cfg_attr(test, ts(type = "false"))]
        cache: MustBe!(false),
    },
}

/// Cache configuration fields when caching is enabled
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[cfg_attr(test, derive(TS), ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct EnabledCacheConfig {
    /// Environment variable names to be fingerprinted and passed to the task.
    pub envs: Option<Box<[Str]>>,

    /// Environment variable names to be passed to the task without fingerprinting.
    pub pass_through_envs: Option<Vec<Str>>,
}

/// Options for user-defined tasks in `vite.config.*`, excluding the command.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[cfg_attr(test, derive(TS), ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct UserTaskOptions {
    /// The working directory for the task, relative to the package root (not workspace root).
    #[serde(rename = "cwd")]
    pub cwd_relative_to_package: Option<RelativePathBuf>,

    /// Dependencies of this task. Use `package-name#task-name` to refer to tasks in other packages.
    pub depends_on: Option<Arc<[Str]>>,

    /// Cache-related fields
    #[serde(flatten)]
    pub cache_config: UserCacheConfig,
}

impl Default for UserTaskOptions {
    /// The default user task options for package.json scripts.
    fn default() -> Self {
        Self {
            // Runs in the package root
            cwd_relative_to_package: None,
            // No dependencies
            depends_on: None,
            // Caching enabled with no fingerprinted envs
            cache_config: UserCacheConfig::Enabled {
                cache: None,
                enabled_cache_config: EnabledCacheConfig { envs: None, pass_through_envs: None },
            },
        }
    }
}

/// Full user-defined task configuration in `vite.config.*`, including the command and options.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[cfg_attr(test, derive(TS), ts(optional_fields, rename = "Task"))]
#[serde(rename_all = "camelCase")]
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
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(test, derive(TS))]
#[serde(transparent)]
#[cfg_attr(test, ts(rename = "Tasks"))]
pub struct UserConfigTasks(pub HashMap<Str, UserTaskConfig>);

impl UserConfigTasks {
    /// TypeScript type definitions for user task configuration.
    pub const TS_TYPE: &str = include_str!("../../task-config.ts");

    /// Generates TypeScript type definitions for user task configuration.
    #[cfg(test)]
    pub fn generate_ts_definition() -> String {
        use std::{
            io::Write,
            process::{Command, Stdio},
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

        // Export all types
        let mut types: String = collector
            .0
            .iter()
            .map(|decl| vite_str::format!("export {decl}"))
            .collect::<Vec<_>>()
            .join("\n\n");

        // Export the main type
        types.push_str("\n\nexport ");
        types.push_str(&Self::decl());

        // Format using oxfmt from packages/tools
        let workspace_root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
        let tools_path = workspace_root.join("packages/tools/node_modules/.bin");

        let oxfmt_path = which::which_in("oxfmt", Some(&tools_path), &tools_path)
            .expect("oxfmt not found in packages/tools");

        let mut child = Command::new(oxfmt_path)
            .arg("--stdin-filepath=task-config.ts")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to spawn oxfmt");

        child
            .stdin
            .take()
            .unwrap()
            .write_all(types.as_bytes())
            .expect("failed to write to oxfmt stdin");

        let output = child.wait_with_output().expect("failed to read oxfmt output");
        assert!(output.status.success(), "oxfmt failed");

        String::from_utf8(output.stdout).expect("oxfmt output is not valid UTF-8")
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
        let ts = UserConfigTasks::generate_ts_definition().replace("\r", "");

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
        assert_eq!(user_config.options.cwd_relative_to_package.as_ref().unwrap().as_str(), "src");
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
                cache: Some(MustBe!(true)),
                enabled_cache_config: EnabledCacheConfig {
                    envs: Some(["NODE_ENV".into()].into_iter().collect()),
                    pass_through_envs: Some(["FOO".into()].into_iter().collect()),
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
