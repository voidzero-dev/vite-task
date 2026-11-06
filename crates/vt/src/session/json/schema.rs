//! The JSON Schema that `$schema` in a document points to.

use std::io;

use vt_materialized_artifact::{Artifact, user_temp_dir};
use vt_path::{AbsolutePath, AbsolutePathBuf};
use vt_str::Str;

/// The `$schema` URL of a document whose JSON Schema is `artifact`. The schema is copied into
/// the user's own folder in the system temp dir, at a path named by a hash of its content,
/// unless an earlier run already did.
pub fn schema_url(artifact: Artifact) -> io::Result<Str> {
    Ok(file_url(&materialize(artifact)?))
}

/// Copies the JSON Schema `artifact` into the user's own folder in the system temp dir,
/// unless an earlier run already did, and returns its path.
pub fn materialize(artifact: Artifact) -> io::Result<AbsolutePathBuf> {
    let dir = user_temp_dir("vp-schemas")?;
    let path = artifact.materialize().suffix(".json").at(&dir)?;
    AbsolutePathBuf::new(path)
        .ok_or_else(|| io::Error::other("the system temp directory isn't an absolute path"))
}

/// `file://`, then the path with `/` separators, not percent-encoded, which both
/// VS Code and `JetBrains` IDEs read as a `$schema` location.
#[cfg(not(windows))]
fn file_url(path: &AbsolutePath) -> Str {
    vt_str::format!("file://{}", path.as_path().to_string_lossy())
}

/// `file:///C:/…` for a path on a drive, and `file://host/share/…` for a UNC path.
#[cfg(windows)]
fn file_url(path: &AbsolutePath) -> Str {
    use cow_utils::CowUtils as _;

    let path = path.as_path().to_string_lossy();
    let path = path.cow_replace('\\', "/");
    if let Some(unc) = path.strip_prefix("//?/UNC/") {
        return vt_str::format!("file://{unc}");
    }
    let path = path.strip_prefix("//?/").unwrap_or(&path);
    if let Some(unc) = path.strip_prefix("//") {
        return vt_str::format!("file://{unc}");
    }
    vt_str::format!("file:///{path}")
}

/// Generates the draft-07 JSON Schema of `T`, and checks that the committed
/// `schemas/{file_name}` matches it. Run with `UPDATE_SNAPSHOTS=1` to regenerate it.
#[cfg(test)]
pub fn check_schema<T: schemars::JsonSchema>(file_name: &str) {
    use schemars::{generate::SchemaSettings, transform::RecursiveTransform};
    use vt_snapshot_test::Snapshots;

    let schema = SchemaSettings::draft07()
        .for_serialize()
        .with_transform(RecursiveTransform(unwrap_description))
        .into_generator()
        .into_root_schema_for::<T>();
    let mut schema = serde_json::to_string_pretty(&schema).unwrap();
    schema.push('\n');

    // Read at runtime: CI builds the Windows tests on Linux, so `env!` would give a Linux path.
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").unwrap();
    let manifest_dir = AbsolutePath::new(&manifest_dir).unwrap();
    let snapshots = Snapshots::new(manifest_dir.join("schemas").into_path_buf());
    snapshots.check_snapshot(file_name, &schema).unwrap();
}

/// Joins the lines of each paragraph in a description, which come from wrapped doc
/// comments.
#[cfg(test)]
#[expect(clippy::disallowed_types, reason = "serde_json stores strings as `String`")]
fn unwrap_description(schema: &mut schemars::Schema) {
    if let Some(serde_json::Value::String(description)) = schema.get_mut("description") {
        let paragraphs: Vec<String> = description
            .split("\n\n")
            .map(|paragraph| paragraph.lines().map(str::trim).collect::<Vec<_>>().join(" "))
            .collect();
        *description = paragraphs.join("\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn file_urls() {
        let path = AbsolutePath::new("/var/my tmp/vp-schemas-501/task-graph_0123.json").unwrap();
        assert_eq!(file_url(path), "file:///var/my tmp/vp-schemas-501/task-graph_0123.json");
    }

    #[cfg(windows)]
    #[test]
    fn file_urls() {
        let url = |path: &str| file_url(AbsolutePath::new(path).unwrap());
        assert_eq!(
            url(r"C:\Users\me\App Data\Temp\vp-schemas\task-graph_0123.json"),
            "file:///C:/Users/me/App Data/Temp/vp-schemas/task-graph_0123.json"
        );
        assert_eq!(
            url(r"\\?\C:\Temp\task-graph_0123.json"),
            "file:///C:/Temp/task-graph_0123.json"
        );
        assert_eq!(
            url(r"\\host\share\task-graph_0123.json"),
            "file://host/share/task-graph_0123.json"
        );
        assert_eq!(
            url(r"\\?\UNC\host\share\task-graph_0123.json"),
            "file://host/share/task-graph_0123.json"
        );
    }
}
