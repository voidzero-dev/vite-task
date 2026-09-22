use std::{collections::BTreeSet, fmt::Display};

use crate::display::TaskDisplay;

/// A non-fatal issue found while loading the task graph.
#[derive(Debug)]
pub enum TaskGraphWarning {
    /// Tasks set cache fields at the top level instead of under `cache`.
    DeprecatedCacheFields {
        /// Names of the deprecated fields in use across all affected tasks.
        fields: BTreeSet<&'static str>,
        /// Tasks that use deprecated fields, sorted for stable output.
        tasks: Vec<TaskDisplay>,
    },
}

impl Display for TaskGraphWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeprecatedCacheFields { fields, tasks } => {
                let (noun, verb, pronoun) = if fields.len() == 1 {
                    ("field", "is", "it")
                } else {
                    ("fields", "are", "them")
                };
                write!(f, "Task {noun} ")?;
                for (index, field) in fields.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "`{field}`")?;
                }
                write!(f, " {verb} deprecated at the top level; move {pronoun} under `cache`")?;
                if let Some(example_field) = fields.first() {
                    write!(f, ", e.g. `cache: {{ {example_field}: [...] }}`")?;
                }
                f.write_str(". Affected tasks: ")?;
                for (index, task) in tasks.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{task}")?;
                }
                Ok(())
            }
        }
    }
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
    fn deprecated_cache_field_singular() {
        let warning = TaskGraphWarning::DeprecatedCacheFields {
            fields: BTreeSet::from(["input"]),
            tasks: vec![task("app", "build")],
        };
        assert_eq!(
            vt_str::format!("{warning}"),
            "Task field `input` is deprecated at the top level; move it under `cache`, \
             e.g. `cache: { input: [...] }`. Affected tasks: app#build"
        );
    }

    #[test]
    fn deprecated_cache_fields_plural() {
        let warning = TaskGraphWarning::DeprecatedCacheFields {
            fields: BTreeSet::from(["untrackedEnv", "output"]),
            tasks: vec![task("app", "build"), task("", "lint")],
        };
        assert_eq!(
            vt_str::format!("{warning}"),
            "Task fields `output`, `untrackedEnv` are deprecated at the top level; move them \
             under `cache`, e.g. `cache: { output: [...] }`. Affected tasks: app#build, lint"
        );
    }
}
