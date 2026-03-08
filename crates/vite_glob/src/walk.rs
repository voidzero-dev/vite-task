// # Walk design: common-ancestor rerooting
//
// Each `AnchoredGlob` has a `prefix` (invariant absolute directory) and an
// optional `variant` (dynamic glob pattern).  `Glob::partition()` guarantees
// the prefix is a literal path — all glob metacharacters live in the variant.
//
// To walk a positive glob we call `wax::Glob::walk(root)`.  Wax internally
// re-joins the glob's invariant prefix with `root`, descends directly to that
// directory (no extra traversal), and matches entries against the variant.
//
// Negative globs are passed to wax's `.not()`, which filters walk entries by
// matching their path **relative to the original `root`** (not the adjusted
// walk directory).  This is the key insight that makes the design work.
//
// ## The rerooting problem
//
// Positive and negative globs can have different prefixes:
//
//   positive: prefix=/app/src      variant=**/*.rs
//   negative: prefix=/app          variant=**/test/**
//
// If we walk from `/app/src`, wax produces relative paths like `foo.rs`.
// The negative's `.not()` pattern `**/test/**` would be matched against
// `foo.rs` — but the negative was authored relative to `/app`, where the
// full relative path would be `src/foo.rs`.  For patterns starting with
// `**` this happens to work (zero-segment match), but for patterns like
// `*.config.js` it would incorrectly exclude `/app/src/vite.config.js`
// (relative path `vite.config.js` matches `*.config.js`, but the file is
// NOT at the package root where the negative was intended to apply).
//
// ## Solution: walk from the common ancestor
//
// We find the common ancestor of the positive prefix and all related
// negative prefixes, then "reroot" every glob relative to that ancestor:
//
//   common ancestor = /app
//   positive rerooted: "src/**/*.rs"   (bridge "src" + variant "**/*.rs")
//   negative rerooted: "**/test/**"    (bridge "" + variant "**/test/**")
//
// `Glob::new("src/**/*.rs").walk("/app")` still descends directly to
// `/app/src/` (wax extracts the invariant prefix `src/`), so there is no
// efficiency loss.  But `.not()` now sees relative paths like
// `src/foo.rs`, and `*.config.js` correctly fails to match `src/vite.config.js`
// because `*` does not cross path separators.
//
// The bridge is always a literal path (it comes from the difference between
// two invariant prefixes), so escaping its glob metacharacters is sufficient.
//
// ## Relationship cases
//
// Given a positive prefix P and negative prefix N:
//
//   P == N        → bridge is empty, variant used as-is
//   N ancestor P  → positive gets a bridge, negative may not
//   N descendant P → negative gets a bridge, positive may not
//   unrelated     → negative cannot affect this walk, skip it

use std::borrow::Cow;

use rustc_hash::FxHashSet;
use vite_path::{AbsolutePath, AbsolutePathBuf};
use wax::{
    Glob,
    walk::{Entry as _, FileIterator as _},
};

use crate::{AnchoredGlob, Error};

/// Walk the filesystem, returning files matching any of the `positive_globs`
/// while excluding those matching any of the `negative_globs`.
///
/// For each positive glob, computes a common ancestor with all related negative
/// globs and walks from there. This lets wax's `.not()` see full relative paths
/// for both positive and negative pattern matching, with tree pruning.
///
/// # Errors
///
/// Returns an error if a rerooted glob pattern is invalid or if a filesystem
/// walk error occurs.
pub fn walk(
    positive_globs: &[AnchoredGlob],
    negative_globs: &[AnchoredGlob],
) -> Result<FxHashSet<AbsolutePathBuf>, Error> {
    let mut results = FxHashSet::default();
    for pos in positive_globs {
        walk_positive(pos, negative_globs, &mut results)?;
    }
    Ok(results)
}

fn walk_positive(
    pos: &AnchoredGlob,
    negatives: &[AnchoredGlob],
    results: &mut FxHashSet<AbsolutePathBuf>,
) -> Result<(), Error> {
    let pos_prefix = pos.prefix();

    let Some(pos_variant) = pos.variant() else {
        // Exact path — include if file exists and no negative matches it
        if pos_prefix.as_path().is_file() && !negatives.iter().any(|neg| neg.is_match(pos_prefix)) {
            results.insert(pos_prefix.to_absolute_path_buf());
        }
        return Ok(());
    };

    // Only negatives whose prefix is an ancestor or descendant of pos_prefix
    // can affect this walk. Unrelated negatives (disjoint subtrees) are skipped.
    //
    // The walk root is the common ancestor of pos_prefix and every related
    // negative prefix. When all negatives share the same prefix as the
    // positive (the common case), the walk root stays at pos_prefix — no
    // unnecessary traversal.
    let walk_root = negatives
        .iter()
        .filter(|neg| {
            pos_prefix.as_path().starts_with(neg.prefix().as_path())
                || neg.prefix().as_path().starts_with(pos_prefix.as_path())
        })
        .fold(pos_prefix.to_absolute_path_buf(), |acc, neg| common_ancestor(&acc, neg.prefix()));

    // Reroot the positive glob: prepend the bridge (walk_root → pos_prefix)
    // to the variant so wax walks from walk_root but descends into pos_prefix.
    let pos_bridge =
        path_bridge(&walk_root, pos_prefix).expect("walk root is an ancestor of pos prefix");
    let pos_pattern = rerooted_pattern(&pos_bridge, pos_variant);
    let pos_glob = Glob::new(&pos_pattern)?.into_owned();

    // Reroot each negative glob the same way: prepend its bridge
    // (walk_root → neg_prefix) to the variant. Negatives with unrelated
    // prefixes fail path_bridge and are skipped.
    let mut neg_globs = Vec::new();
    for neg in negatives {
        let Some(bridge) = path_bridge(&walk_root, neg.prefix()) else {
            continue;
        };
        match neg.variant() {
            Some(variant) => {
                let pattern = rerooted_pattern(&bridge, variant);
                neg_globs.push(Glob::new(&pattern)?.into_owned());
            }
            None if !bridge.is_empty() => {
                neg_globs.push(Glob::new(&escape_glob(&bridge))?.into_owned());
            }
            None => {} // variant-less negative at the walk root itself — cannot exclude files
        }
    }

    let walk = pos_glob.walk(walk_root.into_path_buf());
    if neg_globs.is_empty() {
        collect_entries(walk, results)?;
    } else {
        collect_entries(walk.not(wax::any(neg_globs)?)?, results)?;
    }

    Ok(())
}

fn collect_entries(
    walk: impl wax::walk::FileIterator,
    results: &mut FxHashSet<AbsolutePathBuf>,
) -> Result<(), Error> {
    for entry in walk {
        let entry = entry?;
        if !entry.file_type().is_dir() {
            let abs = AbsolutePathBuf::new(entry.into_path())
                .expect("walk entry under absolute root is absolute");
            results.insert(abs);
        }
    }
    Ok(())
}

/// Compute the "bridge" — the relative path from `ancestor` to `path` — as a
/// `/`-separated string. Returns `None` if `path` is not under `ancestor`
/// (i.e. the prefixes are unrelated and no rerooting is possible).
#[expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "bridge computation requires std String and str::replace for wax glob patterns"
)]
fn path_bridge(ancestor: &AbsolutePath, path: &AbsolutePath) -> Option<String> {
    let remainder = path.as_path().strip_prefix(ancestor.as_path()).ok()?;
    Some(remainder.to_string_lossy().replace('\\', "/"))
}

/// Build a rerooted glob pattern by joining an escaped bridge path with a
/// variant glob. When the bridge is empty (prefix == walk root), the variant
/// is returned unchanged.
#[expect(clippy::disallowed_types, reason = "building glob pattern string for wax requires String")]
fn rerooted_pattern(bridge: &str, variant: &Glob<'_>) -> String {
    if bridge.is_empty() {
        variant.to_string()
    } else {
        [&*escape_glob(bridge), "/", &variant.to_string()].concat()
    }
}

/// Compute the longest common ancestor of two absolute paths.
#[expect(
    clippy::disallowed_types,
    reason = "collecting std::path::Components requires std::path::PathBuf"
)]
fn common_ancestor(a: &AbsolutePath, b: &AbsolutePath) -> AbsolutePathBuf {
    let common: std::path::PathBuf = a
        .as_path()
        .components()
        .zip(b.as_path().components())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a)
        .collect();
    AbsolutePathBuf::new(common).expect("common ancestor of absolute paths is absolute")
}

/// Escape wax glob metacharacters in a literal path string. The bridge is
/// always a literal path (derived from invariant prefixes), but it may
/// contain characters that wax interprets as glob syntax.
fn escape_glob(s: &str) -> Cow<'_, str> {
    const GLOB_CHARS: &[char] = &['?', '*', '$', ':', '<', '>', '(', ')', '[', ']', '{', '}', ','];
    if !s.contains(GLOB_CHARS) {
        return Cow::Borrowed(s);
    }
    let mut escaped = s.to_owned();
    escaped.clear();
    escaped.reserve(s.len() + 4);
    for c in s.chars() {
        if GLOB_CHARS.contains(&c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    Cow::Owned(escaped)
}

#[cfg(test)]
mod tests {
    use path_clean::PathClean as _;

    use super::*;

    fn setup_files(files: &[&str]) -> tempfile::TempDir {
        let tmp = tempfile::TempDir::with_prefix("globtest").unwrap();
        for file in files {
            let file = file.trim_start_matches('/');
            let path = tmp.path().join(file);
            let parent = path.parent().unwrap();
            std::fs::create_dir_all(parent).unwrap();
            std::fs::File::create(path).unwrap();
        }
        tmp
    }

    #[expect(
        clippy::disallowed_types,
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        reason = "test helper uses std types and format! for path manipulation"
    )]
    fn run_walk(
        tmp: &tempfile::TempDir,
        base_path: &str,
        include: &[&str],
        exclude: &[&str],
    ) -> Vec<String> {
        let base_path = base_path.trim_start_matches('/');
        let abs_base =
            AbsolutePathBuf::new(tmp.path().join(base_path)).expect("tmp path is absolute");

        let positives: Vec<AnchoredGlob> = include
            .iter()
            .map(|p| AnchoredGlob::new(p, &abs_base))
            .collect::<Result<_, _>>()
            .unwrap();
        let negatives: Vec<AnchoredGlob> = exclude
            .iter()
            .map(|p| AnchoredGlob::new(p, &abs_base))
            .collect::<Result<_, _>>()
            .unwrap();

        let results = walk(&positives, &negatives).unwrap();
        let clean_root = AbsolutePathBuf::new(tmp.path().clean()).expect("tmp path is absolute");

        let mut out: Vec<String> = results
            .iter()
            .filter_map(|p| {
                let remainder = p.as_path().strip_prefix(clean_root.as_path()).ok()?;
                Some(format!("/{}", remainder.to_string_lossy().replace('\\', "/")))
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn hello_world() {
        let files = &["/test.txt"];
        let tmp = setup_files(files);
        let result = run_walk(&tmp, "/", &["*.txt"], &[]);
        assert_eq!(result, vec!["/test.txt"]);
    }

    #[test]
    fn bullet_files() {
        let files = &["/test.txt", "/subdir/test.txt", "/other/test.txt"];
        let tmp = setup_files(files);
        let result = run_walk(&tmp, "/", &["subdir/test.txt", "test.txt"], &[]);
        assert_eq!(result, vec!["/subdir/test.txt", "/test.txt"]);
    }

    #[test]
    fn finding_workspace_package_json() {
        let files = &[
            "/external/file.txt",
            "/repos/some-app/apps/docs/package.json",
            "/repos/some-app/apps/web/package.json",
            "/repos/some-app/bower_components/readline/package.json",
            "/repos/some-app/examples/package.json",
            "/repos/some-app/node_modules/gulp/bower_components/readline/package.json",
            "/repos/some-app/node_modules/react/package.json",
            "/repos/some-app/package.json",
            "/repos/some-app/packages/colors/package.json",
            "/repos/some-app/packages/faker/package.json",
            "/repos/some-app/packages/left-pad/package.json",
            "/repos/some-app/test/mocks/kitchen-sink/package.json",
            "/repos/some-app/tests/mocks/kitchen-sink/package.json",
        ];
        let tmp = setup_files(files);
        let result = run_walk(
            &tmp,
            "/repos/some-app/",
            &["packages/*/package.json", "apps/*/package.json"],
            &["**/node_modules/**", "**/bower_components/**", "**/test/**", "**/tests/**"],
        );
        assert_eq!(
            result,
            vec![
                "/repos/some-app/apps/docs/package.json",
                "/repos/some-app/apps/web/package.json",
                "/repos/some-app/packages/colors/package.json",
                "/repos/some-app/packages/faker/package.json",
                "/repos/some-app/packages/left-pad/package.json",
            ]
        );
    }

    #[test]
    fn excludes_unexpected_package_json() {
        let files = &[
            "/external/file.txt",
            "/repos/some-app/apps/docs/package.json",
            "/repos/some-app/apps/web/package.json",
            "/repos/some-app/bower_components/readline/package.json",
            "/repos/some-app/examples/package.json",
            "/repos/some-app/node_modules/gulp/bower_components/readline/package.json",
            "/repos/some-app/node_modules/react/package.json",
            "/repos/some-app/package.json",
            "/repos/some-app/packages/colors/package.json",
            "/repos/some-app/packages/faker/package.json",
            "/repos/some-app/packages/left-pad/package.json",
            "/repos/some-app/test/mocks/spanish-inquisition/package.json",
            "/repos/some-app/tests/mocks/spanish-inquisition/package.json",
        ];
        let tmp = setup_files(files);
        let result = run_walk(
            &tmp,
            "/repos/some-app/",
            &["**/package.json"],
            &["**/node_modules/**", "**/bower_components/**", "**/test/**", "**/tests/**"],
        );
        assert_eq!(
            result,
            vec![
                "/repos/some-app/apps/docs/package.json",
                "/repos/some-app/apps/web/package.json",
                "/repos/some-app/examples/package.json",
                "/repos/some-app/package.json",
                "/repos/some-app/packages/colors/package.json",
                "/repos/some-app/packages/faker/package.json",
                "/repos/some-app/packages/left-pad/package.json",
            ]
        );
    }

    #[test]
    fn nested_packages() {
        let files = &[
            "/repos/some-app/packages/xzibit/package.json",
            "/repos/some-app/packages/xzibit/node_modules/street-legal/package.json",
            "/repos/some-app/packages/xzibit/node_modules/paint-colors/package.json",
            "/repos/some-app/packages/xzibit/packages/yo-dawg/package.json",
            "/repos/some-app/packages/xzibit/packages/yo-dawg/node_modules/meme/package.json",
            "/repos/some-app/packages/colors/package.json",
            "/repos/some-app/packages/faker/package.json",
            "/repos/some-app/packages/left-pad/package.json",
        ];
        let tmp = setup_files(files);
        let result = run_walk(
            &tmp,
            "/repos/some-app/",
            &["packages/**/package.json"],
            &["**/node_modules/**", "**/bower_components/**"],
        );
        assert_eq!(
            result,
            vec![
                "/repos/some-app/packages/colors/package.json",
                "/repos/some-app/packages/faker/package.json",
                "/repos/some-app/packages/left-pad/package.json",
                "/repos/some-app/packages/xzibit/package.json",
                "/repos/some-app/packages/xzibit/packages/yo-dawg/package.json",
            ]
        );
    }

    #[test]
    fn passing_doublestar_captures_children() {
        let files = &[
            "/repos/some-app/dist/index.html",
            "/repos/some-app/dist/js/index.js",
            "/repos/some-app/dist/js/lib.js",
            "/repos/some-app/dist/js/node_modules/browserify.js",
        ];
        let tmp = setup_files(files);
        let result = run_walk(&tmp, "/repos/some-app/", &["dist/**"], &[]);
        assert_eq!(
            result,
            vec![
                "/repos/some-app/dist/index.html",
                "/repos/some-app/dist/js/index.js",
                "/repos/some-app/dist/js/lib.js",
                "/repos/some-app/dist/js/node_modules/browserify.js",
            ]
        );
    }

    #[test]
    fn exclude_everything_include_everything() {
        let files = &["/repos/some-app/dist/index.html", "/repos/some-app/dist/js/index.js"];
        let tmp = setup_files(files);
        let result = run_walk(&tmp, "/repos/some-app/", &["**"], &["**"]);
        assert_eq!(result, Vec::<&str>::new());
    }

    #[test]
    fn exclude_directory_prevents_children() {
        let files = &[
            "/repos/some-app/dist/index.html",
            "/repos/some-app/dist/js/index.js",
            "/repos/some-app/dist/js/lib.js",
            "/repos/some-app/dist/js/node_modules/browserify.js",
        ];
        let tmp = setup_files(files);
        let result = run_walk(&tmp, "/repos/some-app/", &["dist/**"], &["dist/js/**"]);
        assert_eq!(result, vec!["/repos/some-app/dist/index.html"]);
    }

    #[test]
    fn include_with_dotdot_traversal() {
        let files = &[
            "/repos/some-app/dist/index.html",
            "/repos/some-app/dist/js/index.js",
            "/repos/some-app/dist/js/lib.js",
            "/repos/some-app/dist/js/node_modules/browserify.js",
        ];
        let tmp = setup_files(files);
        let result = run_walk(&tmp, "/repos/some-app/", &["dist/js/../**"], &[]);
        assert_eq!(
            result,
            vec![
                "/repos/some-app/dist/index.html",
                "/repos/some-app/dist/js/index.js",
                "/repos/some-app/dist/js/lib.js",
                "/repos/some-app/dist/js/node_modules/browserify.js",
            ]
        );
    }

    #[test]
    fn include_with_dot_self_references() {
        let files = &["/repos/some-app/dist/index.html", "/repos/some-app/dist/js/index.js"];
        let tmp = setup_files(files);
        let result = run_walk(&tmp, "/repos/some-app/", &["dist/./././**"], &[]);
        assert_eq!(
            result,
            vec!["/repos/some-app/dist/index.html", "/repos/some-app/dist/js/index.js",]
        );
    }

    #[test]
    fn exclude_single_file() {
        let files = &["/repos/some-app/included.txt", "/repos/some-app/excluded.txt"];
        let tmp = setup_files(files);
        let result = run_walk(&tmp, "/repos/some-app", &["*.txt"], &["excluded.txt"]);
        assert_eq!(result, vec!["/repos/some-app/included.txt"]);
    }

    #[test]
    fn exclude_nested_single_file() {
        let files = &[
            "/repos/some-app/one/included.txt",
            "/repos/some-app/one/two/included.txt",
            "/repos/some-app/one/two/three/included.txt",
            "/repos/some-app/one/excluded.txt",
            "/repos/some-app/one/two/excluded.txt",
            "/repos/some-app/one/two/three/excluded.txt",
        ];
        let tmp = setup_files(files);
        let result = run_walk(&tmp, "/repos/some-app", &["**"], &["**/excluded.txt"]);
        assert_eq!(
            result,
            vec![
                "/repos/some-app/one/included.txt",
                "/repos/some-app/one/two/included.txt",
                "/repos/some-app/one/two/three/included.txt",
            ]
        );
    }

    #[test]
    fn directory_traversal_above_base() {
        let files = &["root-file", "child/some-file"];
        let tmp = setup_files(files);
        let abs_child =
            AbsolutePathBuf::new(tmp.path().join("child")).expect("tmp path is absolute");

        let positives = vec![AnchoredGlob::new("../*-file", &abs_child).unwrap()];
        let results = walk(&positives, &[]).unwrap();

        let clean_root = AbsolutePathBuf::new(tmp.path().clean()).expect("tmp path is absolute");
        let names: Vec<_> = results
            .iter()
            .filter_map(|p| {
                let remainder = p.as_path().strip_prefix(clean_root.as_path()).ok()?;
                Some(remainder.to_string_lossy().into_owned())
            })
            .collect();
        assert_eq!(names, vec!["root-file"]);
    }

    #[test]
    fn redundant_includes_do_not_duplicate() {
        let files = &[
            "/repos/some-app/dist/index.html",
            "/repos/some-app/dist/js/index.js",
            "/repos/some-app/dist/js/lib.js",
            "/repos/some-app/dist/js/node_modules/browserify.js",
        ];
        let tmp = setup_files(files);
        let result = run_walk(&tmp, "/repos/some-app/", &["**/*", "dist/**"], &[]);
        assert_eq!(
            result,
            vec![
                "/repos/some-app/dist/index.html",
                "/repos/some-app/dist/js/index.js",
                "/repos/some-app/dist/js/lib.js",
                "/repos/some-app/dist/js/node_modules/browserify.js",
            ]
        );
    }

    #[test]
    fn no_trailing_slash_base_path() {
        let files = &["/repos/some-app/dist/index.html", "/repos/some-app/dist/js/index.js"];
        let tmp = setup_files(files);
        let result = run_walk(&tmp, "/repos/some-app", &["dist/**"], &[]);
        assert_eq!(
            result,
            vec!["/repos/some-app/dist/index.html", "/repos/some-app/dist/js/index.js",]
        );
    }

    #[test]
    fn exclude_with_leading_star() {
        let files = &[
            "/repos/some-app/foo/bar",
            "/repos/some-app/some-foo/bar",
            "/repos/some-app/included",
        ];
        let tmp = setup_files(files);
        let result = run_walk(&tmp, "/repos/some-app", &["**"], &["*foo/**"]);
        assert_eq!(result, vec!["/repos/some-app/included"]);
    }

    #[test]
    fn exclude_with_trailing_star() {
        let files = &[
            "/repos/some-app/foo/bar",
            "/repos/some-app/foo-file",
            "/repos/some-app/foo-dir/bar",
            "/repos/some-app/included",
        ];
        let tmp = setup_files(files);
        // wax's ** matches zero or more components, so foo*/** also matches foo-file
        let result = run_walk(&tmp, "/repos/some-app", &["**"], &["foo*/**"]);
        assert_eq!(result, vec!["/repos/some-app/included"]);
    }

    #[test]
    fn output_globbing() {
        let files = &[
            "/repos/some-app/src/index.js",
            "/repos/some-app/public/src/css/index.css",
            "/repos/some-app/.turbo/turbo-build.log",
            "/repos/some-app/.turbo/somebody-touched-this-file-into-existence.txt",
            "/repos/some-app/.next/log.txt",
            "/repos/some-app/.next/cache/db6a76a62043520e7aaadd0bb2104e78.txt",
            "/repos/some-app/dist/index.html",
            "/repos/some-app/dist/js/index.js",
            "/repos/some-app/dist/js/lib.js",
            "/repos/some-app/dist/js/node_modules/browserify.js",
            "/repos/some-app/public/dist/css/index.css",
            "/repos/some-app/public/dist/images/rick_astley.jpg",
        ];
        let tmp = setup_files(files);
        let result = run_walk(
            &tmp,
            "/repos/some-app/",
            &[".turbo/turbo-build.log", "dist/**", ".next/**", "public/dist/**"],
            &[],
        );
        assert_eq!(
            result,
            vec![
                "/repos/some-app/.next/cache/db6a76a62043520e7aaadd0bb2104e78.txt",
                "/repos/some-app/.next/log.txt",
                "/repos/some-app/.turbo/turbo-build.log",
                "/repos/some-app/dist/index.html",
                "/repos/some-app/dist/js/index.js",
                "/repos/some-app/dist/js/lib.js",
                "/repos/some-app/dist/js/node_modules/browserify.js",
                "/repos/some-app/public/dist/css/index.css",
                "/repos/some-app/public/dist/images/rick_astley.jpg",
            ]
        );
    }

    #[test]
    fn includes_do_not_override_excludes() {
        let files = &[
            "/repos/some-app/packages/colors/package.json",
            "/repos/some-app/packages/faker/package.json",
            "/repos/some-app/packages/left-pad/package.json",
            "/repos/some-app/packages/xzibit/package.json",
            "/repos/some-app/packages/xzibit/packages/yo-dawg/package.json",
            "/repos/some-app/packages/xzibit/node_modules/street-legal/package.json",
            "/repos/some-app/tests/mocks/spanish-inquisition/package.json",
        ];
        let tmp = setup_files(files);
        let result = run_walk(
            &tmp,
            "/repos/some-app/",
            &["packages/**/package.json", "tests/mocks/*/package.json"],
            &["**/node_modules/**", "**/bower_components/**", "**/test/**", "**/tests/**"],
        );
        assert_eq!(
            result,
            vec![
                "/repos/some-app/packages/colors/package.json",
                "/repos/some-app/packages/faker/package.json",
                "/repos/some-app/packages/left-pad/package.json",
                "/repos/some-app/packages/xzibit/package.json",
                "/repos/some-app/packages/xzibit/packages/yo-dawg/package.json",
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn base_path_with_symlink_preserves_prefix() {
        let files = &["real/file.txt", "real/sub/other.txt"];
        let tmp = setup_files(files);
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(tmp.path().join("real"), &link).unwrap();
        let abs_link = AbsolutePathBuf::new(link).expect("tmp path is absolute");

        let positives = vec![AnchoredGlob::new("**/*.txt", &abs_link).unwrap()];
        let results = walk(&positives, &[]).unwrap();

        for path in &results {
            assert!(
                path.as_path().starts_with(abs_link.as_path()),
                "expected path {path:?} to start with {abs_link:?}",
            );
        }
        assert_eq!(results.len(), 2);
    }
}
