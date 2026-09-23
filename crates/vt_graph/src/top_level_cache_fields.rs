use std::{collections::BTreeSet, fmt::Display};

use crate::display::TaskDisplay;

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

    use vt_path::AbsolutePathBuf;

    use super::*;

    fn task(package_name: &str, task_name: &str) -> TaskDisplay {
        let path = if cfg!(windows) { "C:\\workspace" } else { "/workspace" };
        TaskDisplay {
            package_name: package_name.into(),
            task_name: task_name.into(),
            package_path: Arc::from(AbsolutePathBuf::new(path.into()).unwrap()),
        }
    }

    #[test]
    fn single_field_single_task() {
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
    fn multiple_fields_multiple_tasks() {
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
