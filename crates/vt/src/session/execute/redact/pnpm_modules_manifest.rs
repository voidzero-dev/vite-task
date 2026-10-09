//! pnpm's `node_modules/.modules.yaml`, which records how `node_modules` was
//! installed.

use std::io;

use serde_json::Value;
use vt_path::AbsolutePath;

use super::{UnexpectedFormat, write_canonical_json};

/// Fields that vary with time or with the machine rather than with what is
/// installed:
///
/// - `prunedAt`: when pnpm last pruned the virtual store.
/// - `storeDir`: absolute path of pnpm's store.
/// - `virtualStoreDir`: absolute on Windows, and inside pnpm's store with the
///   global virtual store.
/// - `linkedSkills`: absolute for skill entries outside the workspace.
const UNSTABLE_FIELDS: [&str; 4] = ["prunedAt", "storeDir", "virtualStoreDir", "linkedSkills"];

/// Whether `path` is pnpm's modules manifest. Matches whole path components,
/// with either separator on Windows.
pub fn matches(path: &AbsolutePath) -> bool {
    path.ends_with("node_modules/.modules.yaml")
}

/// Writes the manifest without [`UNSTABLE_FIELDS`] to `out`. Fails when
/// `content` is neither a JSON nor a YAML object.
pub fn redact<W: io::Write>(content: &[u8], out: &mut W) -> Result<(), UnexpectedFormat> {
    let mut manifest = parse_json_or_yaml(content)?;
    let fields = manifest.as_object_mut().ok_or(UnexpectedFormat)?;
    for field in UNSTABLE_FIELDS {
        fields.remove(field);
    }
    write_canonical_json(manifest, out)
}

/// pnpm 10.29 and later write JSON; earlier versions write YAML. JSON is tried
/// first because it parses faster, and because the YAML parser rejects keys
/// longer than 1024 characters, which long dependency paths exceed.
fn parse_json_or_yaml(content: &[u8]) -> Result<Value, UnexpectedFormat> {
    serde_json::from_slice(content)
        .or_else(|_| serde_norway::from_slice(content))
        .map_err(|_| UnexpectedFormat)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn manifest() -> Value {
        json!({
            "hoistedDependencies": {
                "debug@4.4.0": { "debug": "private" },
                "ms@2.1.3": { "ms": "private" },
            },
            "packageManager": "pnpm@11.24.0",
            "prunedAt": "Thu, 08 Oct 2026 08:18:18 GMT",
            "storeDir": "/Users/alice/Library/pnpm/store/v11",
            "virtualStoreDir": ".pnpm",
            "linkedSkills": [".claude/skills/a"],
        })
    }

    fn redact_manifest(content: &[u8]) -> Result<Vec<u8>, UnexpectedFormat> {
        let mut out = Vec::new();
        redact(content, &mut out)?;
        Ok(out)
    }

    fn redact_manifest_value(manifest: &Value) -> Vec<u8> {
        redact_manifest(&serde_json::to_vec_pretty(manifest).unwrap()).unwrap()
    }

    #[test]
    fn matches_only_inside_node_modules() {
        let cwd = vt_path::current_dir().unwrap();
        assert!(matches(&cwd.join("node_modules/.modules.yaml")));
        assert!(matches(&cwd.join("packages/a/node_modules/.modules.yaml")));
        assert!(!matches(&cwd.join(".modules.yaml")));
        assert!(!matches(&cwd.join("node_modules/.modules.yaml.bak")));
        #[cfg(windows)]
        assert!(matches(&cwd.join(r"packages\a\node_modules\.modules.yaml")));
    }

    #[test]
    fn unstable_fields_do_not_change_redacted_manifest() {
        let mut changed = manifest();
        changed["prunedAt"] = json!("Fri, 09 Oct 2026 03:13:58 GMT");
        changed["storeDir"] = json!("/tmp/store/v11");
        changed["virtualStoreDir"] = json!("../../tmp/store/v11/links");
        changed["linkedSkills"] = json!(["/opt/skills/a"]);
        assert_eq!(redact_manifest_value(&manifest()), redact_manifest_value(&changed));
    }

    #[test]
    fn other_fields_change_redacted_manifest() {
        let mut changed = manifest();
        changed["packageManager"] = json!("pnpm@11.25.0");
        assert_ne!(redact_manifest_value(&manifest()), redact_manifest_value(&changed));
    }

    #[test]
    fn key_order_does_not_change_redacted_manifest() {
        let reordered = json!({
            "linkedSkills": [".claude/skills/a"],
            "virtualStoreDir": ".pnpm",
            "storeDir": "/Users/alice/Library/pnpm/store/v11",
            "prunedAt": "Thu, 08 Oct 2026 08:18:18 GMT",
            "packageManager": "pnpm@11.24.0",
            "hoistedDependencies": {
                "ms@2.1.3": { "ms": "private" },
                "debug@4.4.0": { "debug": "private" },
            },
        });
        assert_ne!(
            serde_json::to_vec(&manifest()).unwrap(),
            serde_json::to_vec(&reordered).unwrap()
        );
        assert_eq!(redact_manifest_value(&manifest()), redact_manifest_value(&reordered));
    }

    #[test]
    fn yaml_manifest_redacts_like_json_manifest() {
        let yaml = b"hoistedDependencies:
  debug@4.4.0:
    debug: private
  ms@2.1.3:
    ms: private
linkedSkills:
  - /opt/skills/a
packageManager: pnpm@11.24.0
prunedAt: Fri, 09 Oct 2026 03:13:58 GMT
storeDir: /tmp/store/v11
virtualStoreDir: ../../tmp/store/v11/links
";
        assert_eq!(redact_manifest(yaml).unwrap(), redact_manifest_value(&manifest()));
    }

    #[test]
    fn json_manifest_with_long_keys_is_redacted() {
        let mut manifest = manifest();
        manifest["hoistedDependencies"]["a".repeat(2000)] = json!({ "a": "private" });
        let mut changed = manifest.clone();
        changed["prunedAt"] = json!("Fri, 09 Oct 2026 03:13:58 GMT");
        assert_eq!(redact_manifest_value(&manifest), redact_manifest_value(&changed));
    }

    #[test]
    fn unparsable_manifest_is_not_redacted() {
        assert_eq!(redact_manifest(b"{ \"prunedAt\": "), Err(UnexpectedFormat));
    }
}
