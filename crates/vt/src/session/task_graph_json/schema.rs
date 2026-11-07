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

    use super::*;
    use crate::session::task_graph_json::TaskGraph;

    /// Regenerates `schemas/task-graph.json` from the document types. Run with
    /// `UPDATE_SNAPSHOTS=1` after changing them.
    #[test]
    fn schema_matches_the_document_types() {
        json::check_schema::<TaskGraph>("task-graph.json");
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
