#![expect(clippy::disallowed_types, reason = "vite_path needs to use std path types internally")]

pub mod absolute;
pub mod relative;

use std::{
    ffi::OsStr,
    io,
    path::{Path, StripPrefixError},
};

#[cfg(feature = "absolute-redaction")]
pub use absolute::redaction;
pub use absolute::{AbsolutePath, AbsolutePathBuf};
pub use relative::{RelativePath, RelativePathBuf};

/// Returns the current working directory as an absolute path.
///
/// # Errors
///
/// Returns an error if the current directory cannot be determined, which can occur if:
/// - The current directory has been removed
/// - The current directory is not accessible
///
/// # Panics
///
/// Panics if `std::env::current_dir()` returns a non-absolute path, which should never happen in practice.
pub fn current_dir() -> io::Result<AbsolutePathBuf> {
    #[expect(
        clippy::disallowed_methods,
        reason = "std current_dir needed to get the current working directory as an absolute path"
    )]
    let cwd = std::env::current_dir()?;
    // `std::env::current_dir` should always return a absolute path but its documentation doesn't guarantee that.
    // Do a runtime check just in case.
    Ok(AbsolutePathBuf::new(cwd).unwrap())
}

/// Strips `base` from `path`, after normalizing Windows path namespace prefixes.
///
/// On Windows, the `\\?\`, `\\.\`, and `\??\` prefixes are ignored before
/// matching. On other platforms this is equivalent to [`Path::strip_prefix`].
///
/// This is purely lexical and does not access the filesystem.
///
/// # Errors
///
/// Returns an error if `base` is not a path prefix of `path` after applying the
/// platform-specific prefix normalization above.
pub fn strip_path_prefix<'a>(path: &'a OsStr, base: &OsStr) -> Result<&'a Path, StripPrefixError> {
    let path = strip_windows_path_prefix(path);
    let base = strip_windows_path_prefix(base);
    Path::new(path).strip_prefix(base)
}

/// Strip the `\\?\`, `\\.\`, `\??\` prefix from a Windows path, if present.
/// Does nothing on non-Windows platforms.
///
/// `\\?\` and `\\.\` are used to enable long paths and access to device paths.
/// `\??\` is used in Nt* calls.
/// The resulting path is not necessarily valid or points to the same location,
/// but it is enough for lexical path-prefix comparisons.
#[cfg_attr(
    not(windows),
    expect(
        clippy::missing_const_for_fn,
        reason = "uses non-const for loop and strip_prefix on Windows"
    )
)]
fn strip_windows_path_prefix(p: &OsStr) -> &OsStr {
    #[cfg(windows)]
    {
        use os_str_bytes::OsStrBytesExt as _;

        for prefix in [r"\\?\", r"\\.\", r"\??\"] {
            if let Some(stripped) = p.strip_prefix(prefix) {
                return stripped;
            }
        }
        p
    }
    #[cfg(not(windows))]
    {
        p
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[test]
    fn strip_path_prefix_strips_base() {
        let path =
            OsStr::new(if cfg!(windows) { r"C:\repo\pkg\file.txt" } else { "/repo/pkg/file.txt" });
        let base = OsStr::new(if cfg!(windows) { r"C:\repo" } else { "/repo" });

        let stripped = strip_path_prefix(path, base).unwrap();

        assert_eq!(
            stripped,
            Path::new(if cfg!(windows) { r"pkg\file.txt" } else { "pkg/file.txt" })
        );
    }

    #[test]
    fn strip_path_prefix_reports_mismatch() {
        let path =
            OsStr::new(if cfg!(windows) { r"C:\repo\pkg\file.txt" } else { "/repo/pkg/file.txt" });
        let base = OsStr::new(if cfg!(windows) { r"C:\other" } else { "/other" });

        assert!(strip_path_prefix(path, base).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn strip_path_prefix_ignores_windows_namespace_prefixes() {
        let path = OsStr::new(r"\??\C:\repo\pkg\file.txt");
        let base = OsStr::new(r"\\?\C:\repo");

        let stripped = strip_path_prefix(path, base).unwrap();

        assert_eq!(stripped, Path::new(r"pkg\file.txt"));
    }
}
