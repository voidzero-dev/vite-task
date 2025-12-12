use std::{
    collections::{BTreeMap, HashMap},
    env::{self, join_paths, split_paths},
    ffi::{OsStr, OsString},
    path::{self, PathBuf},
    sync::Arc,
};

use sha2::{Digest as _, Sha256};
use supports_color::{Stream, on};
use vite_glob::GlobPatternSet;
use vite_path::{AbsolutePath, AbsolutePathBuf};
use vite_str::Str;
use vite_task_graph::config::EnvConfig;

/// Resolved environment variables for a task to be fingerprinted.
///
/// Contents of this struct are only for fingerprinting and cache key computation (some of envs may be hashed for security).
/// The actual environment variables to be passed to the execution are in `LeafExecutionItem.all_envs`.
#[derive(Debug)]
pub struct ResolvedEnvs {
    /// Environment variables that should be fingerprinted for this execution.
    ///
    /// Use `BTreeMap` to ensure stable order.
    pub fingerprinted_envs: Arc<BTreeMap<Str, Arc<str>>>,

    /// Environment variable names that should be passed through without values being fingerprinted.
    ///
    /// Names are still included in the fingerprint so that changes to these names can invalidate the cache.
    pub pass_through_envs: Arc<[Str]>,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveEnvError {
    #[error("Failed to resolve envs with invalid glob patterns")]
    GlobError {
        #[source]
        #[from]
        glob_error: vite_glob::Error,
    },

    #[error("Env value is not valid unicode: {key} = {value:?}")]
    EnvValueIsNotValidUnicode { key: Str, value: Arc<OsStr> },

    #[error("Failed to join paths for PATH env")]
    JoinPathsError {
        #[source]
        #[from]
        join_paths_error: env::JoinPathsError,
    },
}

fn prepend_paths(
    envs: &mut HashMap<Arc<OsStr>, Arc<OsStr>>,
    new_paths: &[impl AsRef<AbsolutePath>],
) -> Result<(), env::JoinPathsError> {
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
    let paths = new_paths
        .iter()
        .map(|path| path.as_ref().to_absolute_path_buf().into_path_buf()) // Prepend new paths
        .chain(existing_paths.filter(
            // and remove duplicates
            |path| new_paths.iter().all(|new_path| path != new_path.as_ref().as_path()),
        ));

    *env_path = join_paths(paths)?.into();
    Ok(())
}

impl ResolvedEnvs {
    /// Resolves from all available envs and env config.
    ///
    /// Before the call, `all_envs` is expected to contain all available envs.
    /// After the call, it will be modified to contain only envs to be passed to the execution (fingerprinted + pass_through).
    ///
    /// node_modules/.bin under package and workspace root will be added to PATH env.
    ///
    /// `package_path` can be `None` if the task is not associated with any package (e.g. synthetic tasks).
    pub fn resolve(
        all_envs: &mut Arc<HashMap<Arc<OsStr>, Arc<OsStr>>>,
        env_config: &EnvConfig,
        package_path: Option<&AbsolutePath>,
        workspace_root: &AbsolutePath,
    ) -> Result<Self, ResolveEnvError> {
        // Collect all envs matching fingerpinted or pass-through envs in env_config
        *all_envs = Arc::new({
            let mut new_all_envs = resolve_envs_with_patterns(
                all_envs.iter(),
                &env_config
                    .pass_through_envs
                    .iter()
                    .map(std::convert::AsRef::as_ref)
                    .chain(env_config.fingerprinted_envs.iter().map(std::convert::AsRef::as_ref))
                    .collect::<Vec<&str>>(),
            )?;

            // Automatically add FORCE_COLOR environment variable if not already set
            // This enables color output in subprocesses when color is supported
            // TODO: will remove this temporarily until we have a better solution
            if !new_all_envs.contains_key(OsStr::new("FORCE_COLOR"))
                && !new_all_envs.contains_key(OsStr::new("NO_COLOR"))
                && let Some(support) = on(Stream::Stdout)
            {
                let force_color_value = if support.has_16m {
                    "3" // True color (16 million colors)
                } else if support.has_256 {
                    "2" // 256 colors
                } else if support.has_basic {
                    "1" // Basic ANSI colors
                } else {
                    "0" // No color support
                };
                new_all_envs.insert(
                    OsStr::new("FORCE_COLOR").into(),
                    Arc::<OsStr>::from(OsStr::new(force_color_value)),
                );
            }

            // Prepend package/node_modules/.bin and workspace/node_modules/.bin to PATH
            prepend_paths(&mut new_all_envs, &{
                let mut node_modules_bin_paths: Vec<AbsolutePathBuf> = vec![];
                if let Some(package_path) = package_path
                    && package_path != workspace_root
                {
                    node_modules_bin_paths.push(package_path.join("node_modules").join(".bin"));
                }
                node_modules_bin_paths.push(workspace_root.join("node_modules").join(".bin"));
                node_modules_bin_paths
            })?;
            new_all_envs
        });

        // Resolve fingerprinted envs
        let mut fingerprinted_envs = BTreeMap::<Str, Arc<str>>::new();
        if !env_config.fingerprinted_envs.is_empty() {
            let fingerprinted_env_patterns = GlobPatternSet::new(
                env_config.fingerprinted_envs.iter().filter(|s| !s.starts_with('!')),
            )?;
            let sensitive_patterns = GlobPatternSet::new(SENSITIVE_PATTERNS)?;
            for (name, value) in all_envs.iter() {
                let Some(name) = name.to_str() else {
                    continue;
                };
                if !fingerprinted_env_patterns.is_match(name) {
                    continue;
                }
                let Some(value) = value.to_str() else {
                    return Err(ResolveEnvError::EnvValueIsNotValidUnicode {
                        key: name.into(),
                        value: Arc::clone(value),
                    });
                };
                // Hash sensitive env values
                let value: Arc<str> = if sensitive_patterns.is_match(name) {
                    let mut hasher = Sha256::new();
                    hasher.update(value.as_bytes());
                    format!("sha256:{:x}", hasher.finalize()).into()
                } else {
                    value.into()
                };
                fingerprinted_envs.insert(name.into(), value);
            }
        }

        Ok(Self {
            fingerprinted_envs: Arc::new(fingerprinted_envs),
            // Save pass_through_envs names as-is, so any changes to it will invalidate the cache
            pass_through_envs: Arc::clone(&env_config.pass_through_envs),
        })
    }
}

fn resolve_envs_with_patterns<'a>(
    env_vars: impl Iterator<Item = (&'a Arc<OsStr>, &'a Arc<OsStr>)>,
    patterns: &[&str],
) -> Result<HashMap<Arc<OsStr>, Arc<OsStr>>, vite_glob::Error> {
    let patterns = GlobPatternSet::new(patterns.iter().filter(|pattern| {
        if pattern.starts_with('!') {
            // FIXME: use better way to print warning log
            // Or parse and validate TaskConfig in command parsing phase
            tracing::warn!(
                "env pattern starts with '!' is not supported, will be ignored: {}",
                pattern
            );
            false
        } else {
            true
        }
    }))?;
    let envs: HashMap<Arc<OsStr>, Arc<OsStr>> = env_vars
        .filter_map(|(name, value)| {
            let Some(name_str) = name.as_ref().to_str() else {
                return None;
            };

            if patterns.is_match(name_str) {
                Some((Arc::clone(&name), Arc::clone(&value)))
            } else {
                None
            }
        })
        .collect();
    Ok(envs)
}

const SENSITIVE_PATTERNS: &[&str] = &[
    "*_KEY",
    "*_SECRET",
    "*_TOKEN",
    "*_PASSWORD",
    "*_PASS",
    "*_PWD",
    "*_CREDENTIAL*",
    "*_API_KEY",
    "*_PRIVATE_*",
    "AWS_*",
    "GITHUB_*",
    "NPM_*TOKEN",
    "DATABASE_URL",
    "MONGODB_URI",
    "REDIS_URL",
    "*_CERT*",
    // Exact matches for known sensitive names
    "PASSWORD",
    "SECRET",
    "TOKEN",
];
