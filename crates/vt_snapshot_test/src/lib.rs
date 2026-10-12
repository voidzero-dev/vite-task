#![expect(
    clippy::disallowed_types,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    reason = "standalone test utility crate; std types, methods, and format! are appropriate"
)]

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

/// Scoped snapshot configuration. Created per-fixture, drives all assertions within.
pub struct Snapshots {
    snapshot_dir: PathBuf,
    update: bool,
}

impl Snapshots {
    pub fn new(snapshot_dir: impl Into<PathBuf>) -> Self {
        let update = std::env::var("UPDATE_SNAPSHOTS").is_ok_and(|v| v == "1");
        Self { snapshot_dir: snapshot_dir.into(), update }
    }

    /// Serialize `value` as pretty JSON, prepend `// {comment}`, and compare
    /// against `{snapshot_dir}/{name}.jsonc`.
    pub fn check_json_snapshot(
        &self,
        name: &str,
        comment: &str,
        value: &impl Serialize,
    ) -> Result<(), String> {
        let json = serde_json::to_string_pretty(value).expect("failed to serialize snapshot value");
        self.check_snapshot(&format!("{name}.jsonc"), &format!("// {comment}\n{json}\n"))
    }

    /// Compare `actual` text against `{snapshot_dir}/{name}`.
    /// `name` is used as the filename directly (caller includes any extension).
    pub fn check_snapshot(&self, name: &str, actual: &str) -> Result<(), String> {
        let snap_path = self.snapshot_dir.join(name);

        if self.update {
            fs::create_dir_all(&self.snapshot_dir).expect("failed to create snapshot directory");
            fs::write(&snap_path, actual).expect("failed to write snapshot");
            let _ = fs::remove_file(new_path(&snap_path));
            return Ok(());
        }

        let expected = match fs::read_to_string(&snap_path) {
            Ok(content) => content.replace("\r\n", "\n"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(&self.snapshot_dir)
                    .expect("failed to create snapshot directory");
                fs::write(new_path(&snap_path), actual).expect("failed to write new snapshot");
                return Err(format_new_snapshot(name, &snap_path, actual));
            }
            Err(e) => {
                return Err(format!("failed to read snapshot {}: {e}", snap_path.display()));
            }
        };

        if expected == actual {
            return Ok(());
        }

        fs::write(new_path(&snap_path), actual).expect("failed to write new snapshot");
        Err(format_diff(name, &snap_path, &expected, actual))
    }
}

/// Append `.new` to the full filename (e.g. `foo.jsonc` → `foo.jsonc.new`).
fn new_path(snap_path: &Path) -> PathBuf {
    let mut s = snap_path.as_os_str().to_owned();
    s.push(".new");
    PathBuf::from(s)
}

fn format_diff(name: &str, snap_path: &Path, expected: &str, actual: &str) -> String {
    let diff = similar::TextDiff::from_lines(expected, actual);
    let mut out = String::new();
    let _ = writeln!(out, "Snapshot: {name}");
    let _ = writeln!(out, "Stored new snapshot at: {}", new_path(snap_path).display());
    let _ = writeln!(out);
    let _ = write!(out, "{}", diff.unified_diff().context_radius(3).header("expected", "actual"));
    let _ = writeln!(out);
    let _ = writeln!(out, "To update, re-run with UPDATE_SNAPSHOTS=1");
    out
}

fn format_new_snapshot(name: &str, snap_path: &Path, actual: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Snapshot: {name}");
    let _ = writeln!(out, "Stored new snapshot at: {}", new_path(snap_path).display());
    let _ = writeln!(out);
    let _ = writeln!(out, "{actual}");
    let _ = writeln!(out);
    let _ = writeln!(out, "To update, re-run with UPDATE_SNAPSHOTS=1");
    out
}
