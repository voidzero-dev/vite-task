//! Windows-specific helpers for routing `.cmd` invocations through
//! `PowerShell` so spawning never goes through `cmd.exe`.
//!
//! Running a `.cmd` from any shell makes `cmd.exe` prompt "Terminate batch
//! job (Y/N)?" on Ctrl+C, which leaves the terminal corrupt. Routing
//! through `PowerShell` against the sibling `.ps1` shim sidesteps the prompt
//! and lets Ctrl+C propagate cleanly.
//!
//! This crate carries only the platform-shared primitives (the
//! `PowerShell` host lookup, the fixed argument prefix, and the
//! sibling-`.ps1` discovery). Higher-level wrappers in
//! `vite_task_plan::ps1_shim` (cwd-relative arg rewrite, scoped to
//! `node_modules/.bin`) and `vite_command::ps1_shim` (absolute-path
//! arg rewrite, applied to any `.cmd`) compose these primitives with
//! their own scope rules and return-type conventions.
//!
//! See <https://github.com/voidzero-dev/vite-plus/issues/1176> and
//! <https://github.com/voidzero-dev/vite-plus/issues/1489>.

use std::sync::Arc;

use vite_path::{AbsolutePath, AbsolutePathBuf};

/// Fixed arguments prepended before the `.ps1` path. `-NoProfile`/`-NoLogo`
/// skip user profile loading; `-ExecutionPolicy Bypass` allows running the
/// unsigned shims that npm/pnpm/yarn install.
pub const POWERSHELL_PREFIX: &[&str] =
    &["-NoProfile", "-NoLogo", "-ExecutionPolicy", "Bypass", "-File"];

/// Cached location of the `PowerShell` host. Prefers cross-platform
/// `pwsh.exe` when present, falling back to the Windows built-in
/// `powershell.exe`. Returns `None` on non-Windows or when neither host
/// is on `PATH`.
///
/// Cached as `Arc<AbsolutePath>` so callers that want shared ownership
/// (e.g. `vite_task_plan`'s plan-time rewrite) can do `Arc::clone(host)`
/// without copying the path.
#[cfg(windows)]
#[must_use]
pub fn powershell_host() -> Option<&'static Arc<AbsolutePath>> {
    use std::sync::LazyLock;

    static POWERSHELL_HOST: LazyLock<Option<Arc<AbsolutePath>>> = LazyLock::new(|| {
        let resolved = which::which("pwsh.exe").or_else(|_| which::which("powershell.exe")).ok()?;
        AbsolutePathBuf::new(resolved).map(Arc::<AbsolutePath>::from)
    });
    POWERSHELL_HOST.as_ref()
}

#[cfg(not(windows))]
#[must_use]
pub const fn powershell_host() -> Option<&'static Arc<AbsolutePath>> {
    None
}

/// Given a resolved `.cmd` path, return its sibling `.ps1` if one exists
/// on disk. The extension match is case-insensitive (matches `.cmd`,
/// `.CMD`, `.Cmd`).
///
/// Returns `None` when the path is not a `.cmd` or no `.ps1` sibling
/// exists. Callers that need additional scope checks (e.g. "must live
/// inside the workspace's `node_modules/.bin`") should layer those on
/// top of this primitive.
#[must_use]
pub fn find_ps1_sibling(resolved: &AbsolutePath) -> Option<AbsolutePathBuf> {
    let ext = resolved.as_path().extension().and_then(|e| e.to_str())?;
    if !ext.eq_ignore_ascii_case("cmd") {
        return None;
    }

    let ps1 = resolved.with_extension("ps1");
    if !ps1.as_path().is_file() {
        return None;
    }

    Some(ps1)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[expect(clippy::disallowed_types, reason = "tempdir bridges std PathBuf into AbsolutePath")]
    fn abs(buf: std::path::PathBuf) -> AbsolutePathBuf {
        AbsolutePathBuf::new(buf).unwrap()
    }

    #[test]
    fn find_ps1_sibling_returns_path_when_both_present() {
        let dir = tempdir().unwrap();
        let root = abs(dir.path().canonicalize().unwrap());
        fs::write(root.as_path().join("npm.cmd"), "").unwrap();
        fs::write(root.as_path().join("npm.ps1"), "").unwrap();

        let resolved = abs(root.as_path().join("npm.cmd"));
        let sibling = find_ps1_sibling(&resolved).expect("should find sibling");
        assert_eq!(sibling.as_path(), root.as_path().join("npm.ps1"));
    }

    #[test]
    fn find_ps1_sibling_is_case_insensitive_on_extension() {
        let dir = tempdir().unwrap();
        let root = abs(dir.path().canonicalize().unwrap());
        fs::write(root.as_path().join("pnpm.CMD"), "").unwrap();
        fs::write(root.as_path().join("pnpm.ps1"), "").unwrap();

        let resolved = abs(root.as_path().join("pnpm.CMD"));
        assert!(find_ps1_sibling(&resolved).is_some());
    }

    #[test]
    fn find_ps1_sibling_returns_none_when_sibling_missing() {
        let dir = tempdir().unwrap();
        let root = abs(dir.path().canonicalize().unwrap());
        fs::write(root.as_path().join("npm.cmd"), "").unwrap();

        let resolved = abs(root.as_path().join("npm.cmd"));
        assert!(find_ps1_sibling(&resolved).is_none());
    }

    #[test]
    fn find_ps1_sibling_returns_none_for_non_cmd() {
        let dir = tempdir().unwrap();
        let root = abs(dir.path().canonicalize().unwrap());
        fs::write(root.as_path().join("bun.exe"), "").unwrap();
        fs::write(root.as_path().join("bun.ps1"), "").unwrap();

        let resolved = abs(root.as_path().join("bun.exe"));
        assert!(find_ps1_sibling(&resolved).is_none());
    }

    #[test]
    fn find_ps1_sibling_returns_none_for_no_extension() {
        let dir = tempdir().unwrap();
        let root = abs(dir.path().canonicalize().unwrap());
        fs::write(root.as_path().join("node"), "").unwrap();
        fs::write(root.as_path().join("node.ps1"), "").unwrap();

        let resolved = abs(root.as_path().join("node"));
        assert!(find_ps1_sibling(&resolved).is_none());
    }
}
