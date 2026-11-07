//! The JSON Schema that `$schema` in the task graph document points to.

use std::io;

use vt_materialized_artifact::{Artifact, artifact};
use vt_str::Str;

use crate::session::json;

/// The task graph's JSON Schema, `schemas/task-graph.json`.
const SCHEMA: Artifact = artifact!("task-graph", "VT_TASK_GRAPH_SCHEMA");

/// The `$schema` URL of the task graph document. Its JSON Schema is copied into the user's own
/// folder in the system temp dir, at a path named by a hash of its content, unless an earlier
/// run already did.
pub fn schema_url() -> io::Result<Str> {
    json::schema_url(SCHEMA)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use schemars::{Schema, generate::SchemaSettings, transform::RecursiveTransform};
    use vt_path::AbsolutePath;
    use vt_snapshot_test::Snapshots;

    use super::*;
    use crate::session::task_graph_json::TaskGraph;

    /// Regenerates `schemas/task-graph.json` from the document types. Run with
    /// `UPDATE_SNAPSHOTS=1` after changing them.
    #[test]
    fn schema_matches_the_document_types() {
        let schema = SchemaSettings::draft07()
            .for_serialize()
            .with_transform(RecursiveTransform(unwrap_description))
            .into_generator()
            .into_root_schema_for::<TaskGraph>();
        let mut schema = serde_json::to_string_pretty(&schema).unwrap();
        schema.push('\n');

        // Read at runtime: CI builds the Windows tests on Linux, so `env!` would give a Linux path.
        let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").unwrap();
        let manifest_dir = AbsolutePath::new(&manifest_dir).unwrap();
        let snapshots = Snapshots::new(manifest_dir.join("schemas").into_path_buf());
        snapshots.check_snapshot("task-graph.json", &schema).unwrap();
    }

    /// Joins the lines of each paragraph in a description, which come from wrapped doc
    /// comments.
    #[expect(clippy::disallowed_types, reason = "serde_json stores strings as `String`")]
    fn unwrap_description(schema: &mut Schema) {
        if let Some(serde_json::Value::String(description)) = schema.get_mut("description") {
            let paragraphs: Vec<String> = description
                .split("\n\n")
                .map(|paragraph| paragraph.lines().map(str::trim).collect::<Vec<_>>().join(" "))
                .collect();
            *description = paragraphs.join("\n\n");
        }
    }

    #[test]
    fn materializes_the_committed_schema() {
        let path = json::materialize(SCHEMA).unwrap();
        let file_name = path.as_path().file_name().unwrap().to_str().unwrap();
        let hash =
            file_name.strip_prefix("task-graph_").and_then(|rest| rest.strip_suffix(".json"));
        assert!(hash.is_some_and(|hash| hash.len() == 32), "{file_name}");
        assert_eq!(fs::read(&path).unwrap(), include_bytes!("../../../schemas/task-graph.json"));
        assert_eq!(json::materialize(SCHEMA).unwrap(), path);
    }
}
