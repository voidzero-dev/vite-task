//! pnpm's `node_modules/.modules.yaml`, which records how `node_modules` was
//! installed, including timestamps, absolute paths, and pnpm's version.
//!
//! Only whether it exists is fingerprinted. The installation it describes is
//! tracked through the files in `node_modules` the task reads.

use vt_path::AbsolutePath;

/// Whether `path` is pnpm's modules manifest. Matches whole path components,
/// with either separator on Windows.
pub fn matches(path: &AbsolutePath) -> bool {
    path.ends_with("node_modules/.modules.yaml")
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
