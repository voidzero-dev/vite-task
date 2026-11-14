use js_semver::{Range, Version};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use vt_str::Str;

#[derive(Copy, Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DependencyType {
    Normal,
    Dev,
    Peer,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct PackageJson {
    #[serde(default)]
    pub name: Str,
    #[serde(default)]
    pub version: Str,
    #[serde(default)]
    pub scripts: FxHashMap<Str, Str>,
    #[serde(default)]
    pub dependencies: FxHashMap<Str, Str>,
    #[serde(default)]
    pub dev_dependencies: FxHashMap<Str, Str>,
    #[serde(default)]
    pub peer_dependencies: FxHashMap<Str, Str>,
}

impl std::fmt::Debug for PackageJson {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if std::env::var("VITE_DEBUG_VERBOSE").is_ok_and(|v| v != "0" && v != "false") {
            write!(
                f,
                "PackageJson {{ name: {:?}, scripts: {:?}, dependencies: {:?}, dev_dependencies: {:?}, peer_dependencies: {:?} }}",
                self.name,
                self.scripts,
                self.dependencies,
                self.dev_dependencies,
                self.peer_dependencies
            )
        } else {
            write!(f, "PackageJson {{ name: {:?}, scripts: {:?} }}", self.name, self.scripts)
        }
    }
}

impl PackageJson {
    /// Returns the dependencies that can resolve to a workspace package, each with the version range that package must
    /// satisfy, `*` for a `workspace:` dependency.
    ///
    /// `link_workspace_packages` also returns dependencies that aren't `workspace:`, for package managers that link a
    /// workspace package whenever its version satisfies the range, as npm, Yarn and Bun do.
    pub fn get_workspace_dependencies(
        &self,
        link_workspace_packages: bool,
    ) -> impl Iterator<Item = (Str, DependencyType, Str)> + use<'_> {
        self.dependencies
            .iter()
            .map(|entry| (entry, DependencyType::Normal))
            .chain(self.dev_dependencies.iter().map(|entry| (entry, DependencyType::Dev)))
            .chain(self.peer_dependencies.iter().map(|entry| (entry, DependencyType::Peer)))
            .filter_map(move |((key, value), dep_type)| {
                let Some(workspace_version) = value.strip_prefix("workspace:") else {
                    return link_workspace_packages.then(|| (key.clone(), dep_type, value.clone()));
                };
                // TODO: support paths: https://github.com/pnpm/pnpm/pull/2972
                Some((
                    if let Some((name, _)) = workspace_version.rsplit_once('@') {
                        name.into()
                    } else {
                        key.clone()
                    },
                    dep_type,
                    "*".into(),
                ))
            })
    }
}

/// `*` and an empty range match any version, prereleases included, as npm does.
///
/// <https://github.com/npm/cli/blob/latest/workspaces/arborist/lib/dep-valid.js>
pub(crate) fn version_satisfies(version: &str, range: &str) -> bool {
    let range = range.trim();
    range.is_empty()
        || range == "*"
        || Version::parse(version)
            .is_ok_and(|version| Range::parse(range).is_ok_and(|range| range.satisfies(&version)))
}
