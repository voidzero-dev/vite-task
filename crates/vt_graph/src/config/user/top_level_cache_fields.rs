//! Detection of cache settings written at the top level of a task instead of under `cache`.
//!
//! These fields are no longer supported. They are parsed only so that loading the task graph
//! can explain how to migrate, instead of failing with a generic parse error.

use std::{collections::BTreeSet, fmt::Display};

use serde::{Deserialize, Deserializer, de::IgnoredAny};

use crate::display::TaskDisplay;

/// Which cache settings a task sets at the top level instead of under `cache`.
#[derive(Debug, Default, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag independently records whether one config field is present"
)]
pub struct TopLevelCacheFields {
    #[serde(default, deserialize_with = "deserialize_present")]
    env: bool,
    #[serde(default, deserialize_with = "deserialize_present")]
    untracked_env: bool,
    #[serde(default, deserialize_with = "deserialize_present")]
    input: bool,
    #[serde(default, deserialize_with = "deserialize_present")]
    output: bool,
}

impl TopLevelCacheFields {
    /// Returns the names of the fields that are set, as written in the config.
    pub fn names(self) -> impl Iterator<Item = &'static str> {
        let Self { env, untracked_env, input, output } = self;
        [("env", env), ("untrackedEnv", untracked_env), ("input", input), ("output", output)]
            .into_iter()
            .filter_map(|(name, is_set)| is_set.then_some(name))
    }
}

/// Marks a field as present regardless of its value.
fn deserialize_present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    IgnoredAny::deserialize(deserializer).map(|IgnoredAny| true)
}

/// Collects tasks that set top-level cache fields while loading the task graph.
#[derive(Debug, Default)]
pub struct TopLevelCacheFieldsCollector {
    fields: BTreeSet<&'static str>,
    tasks: Vec<TaskDisplay>,
}

impl TopLevelCacheFieldsCollector {
    /// Records `fields` for the task built by `task`, if any field is set.
    pub fn record(&mut self, fields: TopLevelCacheFields, task: impl FnOnce() -> TaskDisplay) {
        let mut names = fields.names().peekable();
        if names.peek().is_some() {
            self.fields.extend(names);
            self.tasks.push(task());
        }
    }

    /// Returns an error listing every recorded task, if any.
    pub fn finish(self) -> Result<(), TopLevelCacheFieldsError> {
        let Self { fields, mut tasks } = self;
        if tasks.is_empty() {
            return Ok(());
        }
        tasks.sort_unstable_by(|a, b| {
            (&a.package_name, &a.task_name, &a.package_path).cmp(&(
                &b.package_name,
                &b.task_name,
                &b.package_path,
            ))
        });
        Err(TopLevelCacheFieldsError { fields, tasks })
    }
}

/// Tasks set cache settings at the top level instead of under `cache`.
#[derive(Debug)]
pub struct TopLevelCacheFieldsError {
    /// Names of the top-level cache fields set across all affected tasks.
    pub fields: BTreeSet<&'static str>,
    /// Tasks that set top-level cache fields, sorted for stable output.
    pub tasks: Vec<TaskDisplay>,
}

impl Display for TopLevelCacheFieldsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cache settings ")?;
        write_list(f, self.fields.iter().map(|field| vt_str::format!("`{field}`")))?;
        f.write_str(" must be set under `cache` in ")?;
        f.write_str(if self.tasks.len() == 1 { "task " } else { "tasks " })?;
        write_list(f, self.tasks.iter())?;
        if let Some(example_field) = self.fields.first() {
            write!(f, ", e.g. `cache: {{ {example_field}: [...] }}`")?;
        }
        f.write_str(
            ". Run `vp migrate` to update your config automatically. \
             See https://viteplus.dev/config/run for details.",
        )
    }
}

impl std::error::Error for TopLevelCacheFieldsError {}

fn write_list(
    f: &mut std::fmt::Formatter<'_>,
    items: impl Iterator<Item = impl Display>,
) -> std::fmt::Result {
    for (index, item) in items.enumerate() {
        if index > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{item}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use vt_path::AbsolutePathBuf;

    use super::*;
    use crate::config::user::UserTaskConfig;

    fn task(package_name: &str, task_name: &str) -> TaskDisplay {
        let path = if cfg!(windows) { "C:\\workspace" } else { "/workspace" };
        TaskDisplay {
            package_name: package_name.into(),
            task_name: task_name.into(),
            package_path: Arc::from(AbsolutePathBuf::new(path.into()).unwrap()),
        }
    }

    fn parse(user_config_json: serde_json::Value) -> TopLevelCacheFields {
        serde_json::from_value::<UserTaskConfig>(user_config_json).unwrap().top_level_cache_fields
    }

    #[test]
    fn parse_each_field_with_any_cache_value() {
        let fields = [
            ("env", json!(["NODE_ENV"])),
            ("untrackedEnv", json!(["FOO"])),
            ("input", json!(["src/**"])),
            // Any value is recorded, even invalid ones, so the migration error still applies.
            ("output", json!(null)),
        ];
        let cache_values = [None, Some(json!(true)), Some(json!(false)), Some(json!({}))];
        for (field, value) in fields {
            for cache in &cache_values {
                let mut user_config_json = json!({ "command": "echo test" });
                user_config_json[field] = value.clone();
                if let Some(cache) = cache {
                    user_config_json["cache"] = cache.clone();
                }
                assert_eq!(
                    parse(user_config_json.clone()).names().collect::<Vec<_>>(),
                    [field],
                    "{user_config_json}"
                );
            }
        }
    }

    #[test]
    fn parse_all_fields() {
        let fields = parse(json!({
            "command": "echo test",
            "output": [],
            "env": [],
            "untrackedEnv": [],
            "input": [],
        }));
        assert_eq!(fields.names().collect::<Vec<_>>(), ["env", "untrackedEnv", "input", "output"]);
    }

    #[test]
    fn collector_without_fields_succeeds() {
        let mut collector = TopLevelCacheFieldsCollector::default();
        collector
            .record(parse(json!({ "command": "echo test", "cache": { "input": [] } })), || {
                unreachable!("task display is only built for tasks with top-level cache fields")
            });
        assert!(collector.finish().is_ok());
    }

    #[test]
    fn collector_reports_sorted_tasks_and_all_fields() {
        let mut collector = TopLevelCacheFieldsCollector::default();
        collector
            .record(parse(json!({ "command": "echo test", "output": [] })), || task("lib", "test"));
        collector.record(parse(json!({ "command": "echo lint" })), || task("app", "lint"));
        collector
            .record(parse(json!({ "command": "echo build", "env": [] })), || task("app", "build"));
        let error = collector.finish().unwrap_err();
        assert_eq!(error.fields, BTreeSet::from(["env", "output"]));
        assert_eq!(
            error.tasks.iter().map(|task| vt_str::format!("{task}")).collect::<Vec<_>>(),
            ["app#build", "lib#test"]
        );
    }

    #[test]
    fn error_message_single_field_single_task() {
        let error = TopLevelCacheFieldsError {
            fields: BTreeSet::from(["input"]),
            tasks: vec![task("app", "build")],
        };
        assert_eq!(
            vt_str::format!("{error}"),
            "Cache settings `input` must be set under `cache` in task app#build, \
             e.g. `cache: { input: [...] }`. Run `vp migrate` to update your config \
             automatically. See https://viteplus.dev/config/run for details."
        );
    }

    #[test]
    fn error_message_multiple_fields_multiple_tasks() {
        let error = TopLevelCacheFieldsError {
            fields: BTreeSet::from(["untrackedEnv", "env"]),
            tasks: vec![task("app", "build"), task("", "lint")],
        };
        assert_eq!(
            vt_str::format!("{error}"),
            "Cache settings `env`, `untrackedEnv` must be set under `cache` in tasks \
             app#build, lint, e.g. `cache: { env: [...] }`. Run `vp migrate` to update \
             your config automatically. See https://viteplus.dev/config/run for details."
        );
    }
}
