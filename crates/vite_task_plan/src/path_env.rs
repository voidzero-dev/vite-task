use std::{
    collections::HashMap,
    env::{JoinPathsError, join_paths, split_paths},
    ffi::OsStr,
    iter,
    sync::Arc,
};

use vite_path::AbsolutePath;

pub fn prepend_path_env(
    envs: &mut HashMap<Arc<OsStr>, Arc<OsStr>>,
    path_to_prepend: &AbsolutePath,
) -> Result<(), JoinPathsError> {
    // Add node_modules/.bin to PATH
    // On Windows, environment variable names are case-insensitive (e.g., "PATH", "Path", "path" are all the same)
    // However, Rust's HashMap keys are case-sensitive, so we need to find the existing PATH variable
    // regardless of its casing to avoid creating duplicate PATH entries with different casings.
    // For example, if the system has "Path", we should use that instead of creating a new "PATH" entry.
    let env_path = {
        if cfg!(windows)
            && let Some(existing_path) = envs.iter_mut().find_map(|(name, value)| {
                if name.eq_ignore_ascii_case("path") { Some(value) } else { None }
            })
        {
            // Found existing PATH variable (with any casing), use it
            existing_path
        } else {
            // On Unix or no existing PATH on Windows, create/get "PATH" entry
            envs.entry(Arc::from(OsStr::new("PATH")))
                .or_insert_with(|| Arc::<OsStr>::from(OsStr::new("")))
        }
    };

    let existing_paths = split_paths(env_path);
    let paths = iter::once(path_to_prepend.as_path().to_path_buf()).chain(existing_paths.filter(
        // remove duplicates
        |path| path != path_to_prepend.as_path(),
    ));

    let new_path_value = join_paths(paths)?;
    *env_path = new_path_value.into();
    Ok(())
}
