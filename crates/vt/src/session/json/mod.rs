//! Conventions shared by the JSON documents `vp run` prints: task IDs, paths, the format
//! version, and the JSON Schema that `$schema` points to.

mod schema;

use std::{
    borrow::Cow,
    io::{self, Write},
};

use cow_utils::CowUtils as _;
pub use schema::schema_url;
#[cfg(test)]
pub use schema::{check_schema, materialize};
use serde::{Serialize, Serializer};
use vt_path::AbsolutePath;
use vt_str::Str;

/// The format version, `1`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Version;

impl Serialize for Version {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(1)
    }
}

#[cfg(test)]
impl schemars::JsonSchema for Version {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("Version")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "integer", "const": 1 })
    }
}

/// The ID of the task named `name` in the package at `package` (`"."` for the root).
#[must_use]
pub fn task_id(package: &str, name: &str) -> Str {
    let name = encode_id_part(name);
    if package == "." {
        Str::from(name.as_ref())
    } else {
        vt_str::format!("{}#{name}", encode_id_part(package))
    }
}

fn encode_id_part(part: &str) -> Cow<'_, str> {
    match part.cow_replace('%', "%25") {
        Cow::Borrowed(part) => part.cow_replace('#', "%23"),
        Cow::Owned(part) => Cow::Owned(part.cow_replace('#', "%23").into_owned()),
    }
}

/// `path` relative to the workspace root, with `/` separators and `"."` for the root
/// itself, or absolute if it's outside the workspace.
pub fn path_key(path: &AbsolutePath, workspace_root: &AbsolutePath) -> Str {
    match path.strip_prefix(workspace_root) {
        Ok(Some(relative)) if relative.as_str().is_empty() => Str::from("."),
        Ok(Some(relative)) => Str::from(relative.as_str()),
        Ok(None) | Err(_) => Str::from(path.as_path().to_string_lossy().as_ref()),
    }
}

/// Write `document` to `writer` as one JSON document and a newline: indented
/// when `pretty` is set, as in a terminal, and on one line otherwise. It's
/// serialized fully first, so an error leaves `writer` empty.
pub fn write_document(
    writer: &mut impl Write,
    document: &impl Serialize,
    pretty: bool,
) -> io::Result<()> {
    let json =
        if pretty { serde_json::to_vec_pretty(document) } else { serde_json::to_vec(document) };
    let mut json = json.map_err(io::Error::other)?;
    json.push(b'\n');
    writer.write_all(&json)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_ids() {
        assert_eq!(task_id(".", "ci"), "ci");
        assert_eq!(task_id("packages/ui", "build"), "packages/ui#build");
        // Encoding `#` keeps task `c` in the package at `a#b` apart from task `b#c` in `a`.
        assert_eq!(task_id("a#b", "c"), "a%23b#c");
        assert_eq!(task_id("a", "b#c"), "a#b%23c");
        // Encoding `%` keeps root tasks `a#b` and `a%23b` apart.
        assert_eq!(task_id(".", "a#b"), "a%23b");
        assert_eq!(task_id(".", "a%23b"), "a%2523b");
    }
}
