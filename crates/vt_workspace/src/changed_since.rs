//! Changed files for the `[<since>]` filter selector.
//!
//! Follows pnpm's `get_changed_projects`
//! (<https://github.com/pnpm/pnpm/blob/main/pnpm/crates/workspace-projects-filter/src/get_changed_projects.rs>):
//! `git` is run as a subprocess, and the working tree is compared with the
//! merge base of `<since>` and `HEAD`. Changes to tracked files count whether
//! they are committed or not; untracked files don't.
//!
//! When `pnpm-workspace.yaml` changed, its catalogs at the merge base are
//! compared with the ones on disk, so that the packages that take a changed
//! entry through `catalog:` count as changed too.

use std::{fs, io, process::Command};

use rustc_hash::{FxHashMap, FxHashSet};
use serde::Deserialize;
use vt_path::AbsolutePath;
use vt_str::Str;

use crate::{PackageJson, strip_bom};

/// The file name of the pnpm workspace manifest, which holds the catalogs.
pub const PNPM_WORKSPACE_YAML: &str = "pnpm-workspace.yaml";

/// Errors from finding what changed since a git revision.
#[derive(Debug, thiserror::Error)]
pub enum ChangedSinceError {
    #[error("Failed to run `git`")]
    Spawn(#[source] io::Error),

    #[error("{} is not in a git repository: {stderr}", .workspace_root.as_path().display())]
    NotARepository { workspace_root: Box<AbsolutePath>, stderr: Str },

    #[error("'{since}' is not a git revision")]
    InvalidRevision { since: Str },

    #[error("Unknown git revision '{since}'")]
    UnknownRevision { since: Str },

    #[error(
        "'{since}' and HEAD have no common ancestor{}",
        if *.shallow {
            "; the repository is a shallow clone, so fetch more history, e.g. `git fetch --unshallow`"
        } else {
            ""
        }
    )]
    NoMergeBase { since: Str, shallow: bool },

    #[error("`git {args}` failed: {stderr}")]
    Git { args: Str, stderr: Str },

    #[error("Failed to read {PNPM_WORKSPACE_YAML}")]
    ReadWorkspaceYaml(#[source] io::Error),

    #[error("Failed to parse {PNPM_WORKSPACE_YAML} at {revision}")]
    ParseWorkspaceYaml {
        revision: Str,
        #[source]
        error: serde_norway::Error,
    },
}

/// What changed in the workspace since a git revision.
#[derive(Debug)]
pub struct WorkspaceChanges {
    /// Changed files, relative to the workspace root, with `/` separators.
    pub files: Vec<Str>,

    /// Changed catalog entries: dependency names by catalog name.
    pub catalogs: ChangedCatalogs,
}

/// Changed catalog entries: dependency names by catalog name (`default` for `catalog:`).
pub type ChangedCatalogs = FxHashMap<Str, FxHashSet<Str>>;

/// Find what changed in the workspace at `workspace_root` since `since`.
///
/// # Errors
///
/// Fails, instead of reporting no changes, when `git` can't run, the
/// workspace is not in a git repository, `since` is unknown, or `since` and
/// `HEAD` have no merge base (as in a shallow clone).
pub fn workspace_changes(
    workspace_root: &AbsolutePath,
    since: &str,
) -> Result<WorkspaceChanges, ChangedSinceError> {
    let git = Git { cwd: workspace_root };

    let output = git.run(&["rev-parse", "--git-dir"])?;
    if !output.status.success() {
        return Err(ChangedSinceError::NotARepository {
            workspace_root: workspace_root.into(),
            stderr: stderr_text(&output),
        });
    }

    // Revisions never start with `-`; rejecting them keeps `since` from being
    // read as an option, without `--end-of-options` (git 2.24+).
    if since.starts_with('-') {
        return Err(ChangedSinceError::InvalidRevision { since: since.into() });
    }
    let commit = vt_str::format!("{since}^{{commit}}");
    let output = git.run(&["rev-parse", "--verify", "--quiet", &commit])?;
    if !output.status.success() {
        return Err(ChangedSinceError::UnknownRevision { since: since.into() });
    }
    let since_commit = stdout_line(&output);

    let output = git.run(&["merge-base", &since_commit, "HEAD"])?;
    let base = match output.status.code() {
        Some(0) => stdout_line(&output),
        // `git merge-base` exits with 1, printing nothing, when there is no merge base.
        Some(1) if output.stderr.is_empty() => {
            let shallow =
                stdout_line(&git.run(&["rev-parse", "--is-shallow-repository"])?) == "true";
            return Err(ChangedSinceError::NoMergeBase { since: since.into(), shallow });
        }
        _ => return Err(git_error(&["merge-base", &since_commit, "HEAD"], &output)),
    };

    // `--relative` lists paths from the workspace root, leaving out files outside it.
    // `--no-renames` lists both sides of a move, so both owners count as changed.
    let diff_args = ["diff", "--name-only", "-z", "--no-renames", "--relative", &base, "--"];
    let output = git.run(&diff_args)?;
    if !output.status.success() {
        return Err(git_error(&diff_args, &output));
    }
    let files: Vec<Str> =
        lossy(&output.stdout).split('\0').filter(|file| !file.is_empty()).map(Str::from).collect();

    let catalogs = if files.iter().any(|file| file == PNPM_WORKSPACE_YAML) {
        changed_catalogs_since(&git, workspace_root, &base)?
    } else {
        ChangedCatalogs::default()
    };

    Ok(WorkspaceChanges { files, catalogs })
}

/// The catalog entries that differ between `pnpm-workspace.yaml` at `base`
/// and on disk.
fn changed_catalogs_since(
    git: &Git<'_>,
    workspace_root: &AbsolutePath,
    base: &str,
) -> Result<ChangedCatalogs, ChangedSinceError> {
    // `<rev>:./<path>` names the file relative to the current directory, the workspace root.
    let object = vt_str::format!("{base}:./{PNPM_WORKSPACE_YAML}");
    let old_content = if git.run(&["cat-file", "-e", &object])?.status.success() {
        let output = git.run(&["show", &object])?;
        if !output.status.success() {
            return Err(git_error(&["show", &object], &output));
        }
        Some(output.stdout)
    } else {
        // The file didn't exist at the merge base.
        None
    };
    let new_content = match fs::read(workspace_root.join(PNPM_WORKSPACE_YAML)) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(ChangedSinceError::ReadWorkspaceYaml(error)),
    };

    let old_catalogs = parse_catalogs(old_content.as_deref(), base)?;
    let new_catalogs = parse_catalogs(new_content.as_deref(), "the working tree")?;
    Ok(diff_catalogs(&old_catalogs, &new_catalogs))
}

/// Catalog entries by catalog name, then dependency name.
type Catalogs = FxHashMap<Str, FxHashMap<Str, serde_norway::Value>>;

/// The catalog fields of `pnpm-workspace.yaml`.
///
/// <https://pnpm.io/catalogs>
#[derive(Debug, Default, Deserialize)]
struct WorkspaceCatalogs {
    /// The default catalog, referenced as `catalog:` or `catalog:default`.
    #[serde(default)]
    catalog: Option<FxHashMap<Str, serde_norway::Value>>,

    /// Named catalogs, referenced as `catalog:<name>`.
    #[serde(default)]
    catalogs: Option<FxHashMap<Str, Option<FxHashMap<Str, serde_norway::Value>>>>,
}

const DEFAULT_CATALOG: &str = "default";

fn parse_catalogs(content: Option<&[u8]>, revision: &str) -> Result<Catalogs, ChangedSinceError> {
    let Some(content) = content else {
        return Ok(Catalogs::default());
    };
    let manifest: Option<WorkspaceCatalogs> =
        serde_norway::from_slice(strip_bom(content)).map_err(|error| {
            ChangedSinceError::ParseWorkspaceYaml { revision: revision.into(), error }
        })?;
    let WorkspaceCatalogs { catalog, catalogs } = manifest.unwrap_or_default();

    let mut result: Catalogs = catalogs
        .unwrap_or_default()
        .into_iter()
        .map(|(name, entries)| (name, entries.unwrap_or_default()))
        .collect();
    // pnpm rejects a manifest that sets both `catalog` and `catalogs.default`;
    // here `catalog` wins.
    if let Some(catalog) = catalog {
        result.entry(DEFAULT_CATALOG.into()).or_default().extend(catalog);
    }
    Ok(result)
}

/// The entries added, removed or changed between `old` and `new`.
fn diff_catalogs(old: &Catalogs, new: &Catalogs) -> ChangedCatalogs {
    let mut changed = ChangedCatalogs::default();
    let names: FxHashSet<&Str> = old.keys().chain(new.keys()).collect();
    for name in names {
        let old_entries = old.get(name);
        let new_entries = new.get(name);
        let dependencies: FxHashSet<&Str> =
            old_entries.into_iter().chain(new_entries).flat_map(FxHashMap::keys).collect();
        for dependency in dependencies {
            if old_entries.and_then(|entries| entries.get(dependency))
                != new_entries.and_then(|entries| entries.get(dependency))
            {
                changed.entry(name.clone()).or_default().insert(dependency.clone());
            }
        }
    }
    changed
}

/// Whether `package_json` takes any of the `changed` catalog entries.
pub fn uses_changed_catalog(package_json: &PackageJson, changed: &ChangedCatalogs) -> bool {
    [
        &package_json.dependencies,
        &package_json.dev_dependencies,
        &package_json.peer_dependencies,
        &package_json.optional_dependencies,
    ]
    .into_iter()
    .flatten()
    .any(|(name, specifier)| {
        catalog_reference(name, specifier).is_some_and(|(catalog, dependency)| {
            changed.get(catalog).is_some_and(|entries| entries.contains(dependency))
        })
    })
}

/// The catalog name and catalog entry a dependency specifier refers to.
///
/// `catalog:` and `catalog:default` refer to the default catalog. An alias
/// such as `npm:lodash@catalog:` refers to the aliased package's entry, as in
/// pnpm's `parse_catalog_dep`.
fn catalog_reference<'a>(name: &'a str, specifier: &'a str) -> Option<(&'a str, &'a str)> {
    if let Some(catalog) = parse_catalog_protocol(specifier) {
        return Some((catalog, name));
    }
    let aliased = specifier.strip_prefix("npm:")?;
    let (aliased_name, aliased_specifier) = aliased.rsplit_once('@')?;
    if aliased_name.is_empty() {
        return None;
    }
    Some((parse_catalog_protocol(aliased_specifier)?, aliased_name))
}

fn parse_catalog_protocol(specifier: &str) -> Option<&str> {
    let catalog = specifier.strip_prefix("catalog:")?.trim();
    Some(if catalog.is_empty() { DEFAULT_CATALOG } else { catalog })
}

/// Runs `git` in the workspace root.
struct Git<'a> {
    cwd: &'a AbsolutePath,
}

impl Git<'_> {
    fn run(&self, args: &[&str]) -> Result<std::process::Output, ChangedSinceError> {
        Command::new("git")
            // Keep a user's `diff.relative` setting from changing which paths are listed.
            .args(["-c", "diff.relative=false"])
            .args(args)
            .current_dir(self.cwd.as_path())
            .output()
            .map_err(ChangedSinceError::Spawn)
    }
}

/// `bytes` as UTF-8, with invalid sequences replaced by `U+FFFD`.
fn lossy(bytes: &[u8]) -> Str {
    let mut text = Str::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        text.push_str(chunk.valid());
        if !chunk.invalid().is_empty() {
            text.push(char::REPLACEMENT_CHARACTER);
        }
    }
    text
}

fn stdout_line(output: &std::process::Output) -> Str {
    lossy(&output.stdout).trim().into()
}

fn stderr_text(output: &std::process::Output) -> Str {
    lossy(&output.stderr).trim().into()
}

fn git_error(args: &[&str], output: &std::process::Output) -> ChangedSinceError {
    ChangedSinceError::Git { args: args.join(" ").into(), stderr: stderr_text(output) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalogs(yaml: &str) -> Catalogs {
        parse_catalogs(Some(yaml.as_bytes()), "test").unwrap()
    }

    fn changed(old: &str, new: &str) -> Vec<(Str, Vec<Str>)> {
        let mut changed: Vec<(Str, Vec<Str>)> = diff_catalogs(&catalogs(old), &catalogs(new))
            .into_iter()
            .map(|(name, entries)| {
                let mut entries: Vec<Str> = entries.into_iter().collect();
                entries.sort_unstable();
                (name, entries)
            })
            .collect();
        changed.sort_unstable();
        changed
    }

    #[test]
    fn diff_reports_changed_added_and_removed_entries() {
        let old = "catalog:\n  react: ^18.0.0\n  lodash: ^4.0.0\n  zod: ^3.0.0\n";
        let new = "catalog:\n  react: ^19.0.0\n  zod: ^3.0.0\n  vue: ^3.0.0\n";
        assert_eq!(
            changed(old, new),
            vec![(Str::from("default"), vec!["lodash".into(), "react".into(), "vue".into()])]
        );
    }

    #[test]
    fn diff_covers_named_catalogs() {
        let old = "catalogs:\n  legacy:\n    react: ^17.0.0\n  next:\n    react: ^19.0.0\n";
        let new = "catalogs:\n  legacy:\n    react: ^17.0.2\n  next:\n    react: ^19.0.0\n";
        assert_eq!(changed(old, new), vec![(Str::from("legacy"), vec!["react".into()])]);
    }

    #[test]
    fn catalog_and_catalogs_default_are_the_same_catalog() {
        let old = "catalog:\n  react: ^18.0.0\n";
        let new = "catalogs:\n  default:\n    react: ^18.0.0\n";
        assert!(changed(old, new).is_empty());
    }

    #[test]
    fn unrelated_manifest_changes_change_no_catalog() {
        let old = "packages:\n  - packages/*\ncatalog:\n  react: ^18.0.0\n";
        let new = "packages:\n  - packages/*\n  - apps/*\ncatalog:\n  react: ^18.0.0\n";
        assert!(changed(old, new).is_empty());
    }

    #[test]
    fn missing_or_empty_manifest_has_no_catalogs() {
        assert!(parse_catalogs(None, "test").unwrap().is_empty());
        assert!(catalogs("").is_empty());
        assert!(catalogs("catalog:\ncatalogs:\n  empty:\n").values().all(FxHashMap::is_empty));
    }

    #[test]
    fn invalid_manifest_is_an_error() {
        let error = parse_catalogs(Some(b"catalog: [unclosed"), "abc123").unwrap_err();
        assert!(
            matches!(&error, ChangedSinceError::ParseWorkspaceYaml { revision, .. } if revision == "abc123"),
            "{error:?}"
        );
    }

    #[test]
    fn catalog_references() {
        assert_eq!(catalog_reference("react", "catalog:"), Some(("default", "react")));
        assert_eq!(catalog_reference("react", "catalog:default"), Some(("default", "react")));
        assert_eq!(catalog_reference("react", "catalog:legacy"), Some(("legacy", "react")));
        assert_eq!(catalog_reference("ui", "npm:@acme/ui@catalog:"), Some(("default", "@acme/ui")));
        assert_eq!(catalog_reference("react", "^18.0.0"), None);
        assert_eq!(catalog_reference("lib", "workspace:*"), None);
        assert_eq!(catalog_reference("lodash", "npm:lodash@^4.0.0"), None);
    }

    #[test]
    fn package_uses_changed_catalog() {
        let package_json: PackageJson = serde_json::from_str(
            r#"{
                "name": "app",
                "dependencies": { "react": "catalog:" },
                "devDependencies": { "typescript": "catalog:tools" },
                "optionalDependencies": { "fsevents": "catalog:" }
            }"#,
        )
        .unwrap();
        let changed_entries = |catalog: &str, dependency: &str| -> ChangedCatalogs {
            std::iter::once((Str::from(catalog), std::iter::once(Str::from(dependency)).collect()))
                .collect()
        };

        assert!(uses_changed_catalog(&package_json, &changed_entries("default", "react")));
        assert!(uses_changed_catalog(&package_json, &changed_entries("tools", "typescript")));
        assert!(uses_changed_catalog(&package_json, &changed_entries("default", "fsevents")));
        // Same dependency, other catalog.
        assert!(!uses_changed_catalog(&package_json, &changed_entries("default", "typescript")));
        assert!(!uses_changed_catalog(&package_json, &changed_entries("default", "vue")));
        assert!(!uses_changed_catalog(&package_json, &ChangedCatalogs::default()));
    }
}
