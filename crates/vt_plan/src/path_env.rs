use std::{
    env::{JoinPathsError, join_paths, split_paths},
    ffi::OsStr,
    iter,
    sync::Arc,
};

use rustc_hash::FxHashMap;
use vt_casefold::EnvName;
use vt_path::AbsolutePath;

/// Get the PATH environment variable from the given envs map, matching the
/// name by the platform's rules.
#[must_use]
#[expect(clippy::implicit_hasher, reason = "function is specific to FxHashMap")]
pub fn get_path_env(envs: &FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>>) -> Option<&Arc<OsStr>> {
    envs.get(EnvName::from_ref(OsStr::new("PATH")))
}

/// Prepend a path to the PATH environment variable.
///
/// An existing PATH keeps its spelling (Windows commonly uses `Path`).
///
/// # Errors
/// Returns an error if the paths cannot be joined.
#[expect(clippy::implicit_hasher, reason = "function is specific to FxHashMap")]
pub fn prepend_path_env(
    envs: &mut FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>>,
    path_to_prepend: &AbsolutePath,
) -> Result<(), JoinPathsError> {
    let env_path = envs
        .entry(EnvName::new(Arc::from(OsStr::new("PATH"))))
        .or_insert_with(|| Arc::<OsStr>::from(OsStr::new("")));

    let existing_paths = split_paths(env_path);
    let paths = iter::once(path_to_prepend.as_path().to_path_buf()).chain(existing_paths.filter(
        // remove duplicates
        |path| path != path_to_prepend.as_path(),
    ));

    let new_path_value = join_paths(paths)?;
    *env_path = new_path_value.into();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_envs(pairs: Vec<(&str, &str)>) -> FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>> {
        pairs
            .into_iter()
            .map(|(k, v)| (EnvName::new(Arc::from(OsStr::new(k))), Arc::from(OsStr::new(v))))
            .collect()
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_path_case_insensitive_mixed_case() {
        let mut envs = create_test_envs(vec![("Path", "C:\\existing\\path")]);
        let path_to_prepend =
            AbsolutePath::new("C:\\workspace\\packages\\app\\node_modules\\.bin").unwrap();

        prepend_path_env(&mut envs, path_to_prepend).unwrap();

        // Verify the existing "Path" entry was updated in place: its spelling is
        // preserved and no separate "PATH" entry was created.
        let names: Vec<&OsStr> = envs.keys().map(|name| &**name.inner()).collect();
        assert_eq!(names, [OsStr::new("Path")]);

        // Verify the PATH value has node_modules/.bin prepended
        let path_value = get_path_env(&envs).unwrap();
        assert!(path_value.to_str().unwrap().contains("node_modules\\.bin"));
        assert!(path_value.to_str().unwrap().contains("C:\\existing\\path"));
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_path_case_insensitive_uppercase() {
        let mut envs = create_test_envs(vec![("PATH", "C:\\existing\\path")]);
        let path_to_prepend =
            AbsolutePath::new("C:\\workspace\\packages\\app\\node_modules\\.bin").unwrap();

        prepend_path_env(&mut envs, path_to_prepend).unwrap();

        // Verify the PATH value has node_modules/.bin prepended
        let path_value = envs.get(EnvName::from_ref(OsStr::new("PATH"))).unwrap();
        assert!(path_value.to_str().unwrap().contains("node_modules\\.bin"));
        assert!(path_value.to_str().unwrap().contains("C:\\existing\\path"));
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_path_created_when_missing() {
        let mut envs = create_test_envs(vec![]);
        let path_to_prepend =
            AbsolutePath::new("C:\\workspace\\packages\\app\\node_modules\\.bin").unwrap();

        prepend_path_env(&mut envs, path_to_prepend).unwrap();

        // Verify PATH was created with only node_modules/.bin
        let path_value = envs.get(EnvName::from_ref(OsStr::new("PATH"))).unwrap();
        assert!(path_value.to_str().unwrap().contains("node_modules\\.bin"));
    }

    #[test]
    #[cfg(unix)]
    fn test_unix_path_case_sensitive() {
        let mut envs = create_test_envs(vec![("PATH", "/existing/path")]);
        let path_to_prepend =
            AbsolutePath::new("/workspace/packages/app/node_modules/.bin").unwrap();

        prepend_path_env(&mut envs, path_to_prepend).unwrap();

        // Verify "PATH" exists and the complete value has node_modules/.bin prepended
        let path_value = envs.get(EnvName::from_ref(OsStr::new("PATH"))).unwrap();
        let path_str = path_value.to_str().unwrap();
        assert!(path_str.contains("node_modules/.bin"));
        assert!(path_str.contains("/existing/path"));

        // Verify that on Unix, the code uses exact "PATH" match (case-sensitive)
        assert!(!envs.contains_key(EnvName::from_ref(OsStr::new("Path"))));
        assert!(!envs.contains_key(EnvName::from_ref(OsStr::new("path"))));
    }

    #[test]
    #[cfg(unix)]
    fn test_prepend_paths_removes_duplicates() {
        let mut envs = create_test_envs(vec![("PATH", "/workspace/node_modules/.bin:/other/path")]);
        let path_to_prepend = AbsolutePath::new("/workspace/node_modules/.bin").unwrap();

        prepend_path_env(&mut envs, path_to_prepend).unwrap();

        let path_value = envs.get(EnvName::from_ref(OsStr::new("PATH"))).unwrap();
        let path_str = path_value.to_str().unwrap();

        // Should only have one occurrence of node_modules/.bin (duplicates removed)
        let node_modules_count = path_str.matches("/workspace/node_modules/.bin").count();
        assert_eq!(node_modules_count, 1, "Duplicate paths should be removed");
    }
}
