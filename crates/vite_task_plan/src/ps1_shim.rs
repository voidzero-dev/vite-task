//! Windows-specific: rewrite `.cmd` shims in `node_modules/.bin` so the plan
//! records a `powershell.exe -File <sibling.ps1>` invocation in place of the
//! `.cmd` hop.
//!
//! Running a `.cmd` shim from any shell causes `cmd.exe` to prompt "Terminate
//! batch job (Y/N)?" on Ctrl+C, which leaves the terminal corrupt. Rewriting
//! to the `.ps1` sibling, invoked via `powershell.exe -File`, sidesteps that
//! prompt. Doing the rewrite at plan time (rather than at spawn time) means
//! the command shown in the task graph and cache fingerprint is the command
//! that actually runs.
//!
//! The `.ps1` path recorded in args is **relative to the task's cwd**, with
//! `/` as the separator. That keeps `SpawnFingerprint.args` portable across
//! machines (no absolute paths leak into cache keys) and PowerShell resolves
//! `-File <relative>` against its own working directory, which is the task's
//! cwd, so the spawn lands on the correct file.
//!
//! The rewrite is limited to `node_modules/.bin/` triplets **inside the
//! workspace** and produced by npm/pnpm/yarn (via cmd-shim, which only emits
//! `.cmd` — not `.bat`). Any `.cmd` file outside the workspace — e.g. a
//! globally installed tool's shim somewhere on the user's system PATH — is
//! left alone even if it happens to live under some other `node_modules/.bin`.
//!
//! Cross-platform primitives (`POWERSHELL_PREFIX`, `powershell_host`,
//! `find_ps1_sibling`) live in the `vite_powershell` crate so
//! `vite_command::ps1_shim` can share them.
//!
//! See <https://github.com/voidzero-dev/vite-plus/issues/1176>.

use std::sync::Arc;

#[cfg(any(windows, test))]
use cow_utils::CowUtils as _;
use vite_path::AbsolutePath;
use vite_str::Str;

/// Rewrite a `node_modules/.bin/*.cmd` invocation to go through PowerShell.
/// See the module docstring for the full contract; the short form: returns
/// `(powershell.exe, [-NoProfile, …, -File, <cwd-relative .ps1>, ...args])`
/// when the rewrite applies, otherwise `(resolved, args)` unchanged.
#[cfg(windows)]
#[must_use]
pub fn rewrite_cmd_shim_with_args(
    resolved: Arc<AbsolutePath>,
    args: Arc<[Str]>,
    cwd: &AbsolutePath,
    workspace_root: &AbsolutePath,
) -> (Arc<AbsolutePath>, Arc<[Str]>) {
    if let Some(host) = vite_powershell::powershell_host()
        && let Some(rewritten) = rewrite_with_host(&resolved, &args, cwd, workspace_root, host)
    {
        return rewritten;
    }
    (resolved, args)
}

#[cfg(not(windows))]
#[must_use]
pub const fn rewrite_cmd_shim_with_args(
    resolved: Arc<AbsolutePath>,
    args: Arc<[Str]>,
    _cwd: &AbsolutePath,
    _workspace_root: &AbsolutePath,
) -> (Arc<AbsolutePath>, Arc<[Str]>) {
    (resolved, args)
}

/// Pure rewrite logic, factored out so tests can exercise it on any platform
/// without depending on a real `powershell.exe` being on PATH.
#[cfg(any(windows, test))]
fn rewrite_with_host(
    resolved: &Arc<AbsolutePath>,
    args: &Arc<[Str]>,
    cwd: &AbsolutePath,
    workspace_root: &AbsolutePath,
    host: &Arc<AbsolutePath>,
) -> Option<(Arc<AbsolutePath>, Arc<[Str]>)> {
    if !is_in_workspace_node_modules_bin(resolved, workspace_root) {
        return None;
    }
    let ps1 = vite_powershell::find_ps1_sibling(resolved)?;
    let ps1_rel = pathdiff::diff_paths(ps1.as_path(), cwd.as_path())?;
    let ps1_rel_str = ps1_rel.to_str()?.cow_replace('\\', "/");

    tracing::debug!(
        "rewriting .cmd shim to powershell: {} -> {} -File {}",
        resolved.as_path().display(),
        host.as_path().display(),
        ps1_rel_str,
    );

    let new_args: Arc<[Str]> = vite_powershell::POWERSHELL_PREFIX
        .iter()
        .copied()
        .map(Str::from)
        .chain(std::iter::once(Str::from(ps1_rel_str.as_ref())))
        .chain(args.iter().cloned())
        .collect();

    Some((Arc::clone(host), new_args))
}

/// True when `resolved` is a `<workspace>/.../node_modules/.bin/<file>` path
/// inside the workspace. Used to keep the rewrite hands-off for globally
/// installed shims (e.g. `%AppData%\npm\node_modules\.bin`).
#[cfg(any(windows, test))]
fn is_in_workspace_node_modules_bin(
    resolved: &AbsolutePath,
    workspace_root: &AbsolutePath,
) -> bool {
    let path = resolved.as_path();
    if !path.starts_with(workspace_root.as_path()) {
        return false;
    }
    let mut parents = path.components().rev();
    parents.next(); // shim filename
    let Some(bin) = parents.next() else { return false };
    if !bin.as_os_str().eq_ignore_ascii_case(".bin") {
        return false;
    }
    let Some(node_modules) = parents.next() else { return false };
    node_modules.as_os_str().eq_ignore_ascii_case("node_modules")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;
    use vite_path::AbsolutePathBuf;

    use super::{AbsolutePath, Arc, Str, rewrite_with_host};

    #[expect(clippy::disallowed_types, reason = "tempdir bridges std PathBuf into AbsolutePath")]
    fn abs(buf: std::path::PathBuf) -> Arc<AbsolutePath> {
        Arc::<AbsolutePath>::from(AbsolutePathBuf::new(buf).unwrap())
    }

    #[expect(clippy::disallowed_types, reason = "tempdir hands out std Path for the test root")]
    fn bin_dir(root: &std::path::Path) -> std::path::PathBuf {
        let bin = root.join("node_modules").join(".bin");
        fs::create_dir_all(&bin).unwrap();
        bin
    }

    fn host_arc(root: &AbsolutePath) -> Arc<AbsolutePath> {
        Arc::<AbsolutePath>::from(
            AbsolutePathBuf::new(root.as_path().join("powershell.exe")).unwrap(),
        )
    }

    #[test]
    fn rewrites_cmd_to_cwd_relative_ps1_at_workspace_root() {
        let dir = tempdir().unwrap();
        let workspace = abs(dir.path().canonicalize().unwrap());
        let bin = bin_dir(workspace.as_path());
        fs::write(bin.join("vite.CMD"), "").unwrap();
        fs::write(bin.join("vite.ps1"), "").unwrap();

        let host = host_arc(&workspace);
        let resolved = abs(bin.join("vite.CMD"));
        let args: Arc<[Str]> = Arc::from(vec![Str::from("--port"), Str::from("3000")]);

        let (program, rewritten_args) =
            rewrite_with_host(&resolved, &args, &workspace, &workspace, &host)
                .expect("should rewrite");

        assert_eq!(program.as_path(), host.as_path());
        let as_strs: Vec<&str> = rewritten_args.iter().map(Str::as_str).collect();
        assert_eq!(
            as_strs,
            vec![
                "-NoProfile",
                "-NoLogo",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                "node_modules/.bin/vite.ps1",
                "--port",
                "3000",
            ]
        );
    }

    #[test]
    fn rewrites_cmd_to_cwd_relative_ps1_in_hoisted_monorepo_subpackage() {
        // Task cwd is `<ws>/packages/foo`; shim lives at hoisted
        // `<ws>/node_modules/.bin/vite.ps1`. The recorded argument should
        // traverse up to the workspace and back down into node_modules/.bin.
        let dir = tempdir().unwrap();
        let workspace = abs(dir.path().canonicalize().unwrap());
        let bin = bin_dir(workspace.as_path());
        fs::write(bin.join("vite.cmd"), "").unwrap();
        fs::write(bin.join("vite.ps1"), "").unwrap();

        let sub_pkg_path = workspace.as_path().join("packages").join("foo");
        fs::create_dir_all(&sub_pkg_path).unwrap();
        let sub_pkg = abs(sub_pkg_path);

        let host = host_arc(&workspace);
        let resolved = abs(bin.join("vite.cmd"));
        let args: Arc<[Str]> = Arc::from(vec![]);

        let (_program, rewritten_args) =
            rewrite_with_host(&resolved, &args, &sub_pkg, &workspace, &host)
                .expect("should rewrite");

        assert_eq!(
            rewritten_args.get(5).map(Str::as_str),
            Some("../../node_modules/.bin/vite.ps1")
        );
    }

    #[test]
    fn returns_none_when_no_ps1_sibling() {
        let dir = tempdir().unwrap();
        let workspace = abs(dir.path().canonicalize().unwrap());
        let bin = bin_dir(workspace.as_path());
        fs::write(bin.join("vite.cmd"), "").unwrap();

        let host = host_arc(&workspace);
        let resolved = abs(bin.join("vite.cmd"));
        let args: Arc<[Str]> = Arc::from(vec![Str::from("build")]);

        assert!(rewrite_with_host(&resolved, &args, &workspace, &workspace, &host).is_none());
    }

    #[test]
    fn returns_none_for_cmd_outside_node_modules_bin() {
        let dir = tempdir().unwrap();
        let workspace = abs(dir.path().canonicalize().unwrap());
        fs::write(workspace.as_path().join("where.cmd"), "").unwrap();
        fs::write(workspace.as_path().join("where.ps1"), "").unwrap();

        let host = host_arc(&workspace);
        let resolved = abs(workspace.as_path().join("where.cmd"));
        let args: Arc<[Str]> = Arc::from(vec![]);

        assert!(rewrite_with_host(&resolved, &args, &workspace, &workspace, &host).is_none());
    }

    #[test]
    fn returns_none_for_non_shim_extensions() {
        let dir = tempdir().unwrap();
        let workspace = abs(dir.path().canonicalize().unwrap());
        let bin = bin_dir(workspace.as_path());
        fs::write(bin.join("node.exe"), "").unwrap();
        fs::write(bin.join("node.ps1"), "").unwrap();

        let host = host_arc(&workspace);
        let resolved = abs(bin.join("node.exe"));
        let args: Arc<[Str]> = Arc::from(vec![Str::from("--version")]);

        assert!(rewrite_with_host(&resolved, &args, &workspace, &workspace, &host).is_none());
    }

    #[test]
    fn returns_none_for_cmd_outside_workspace() {
        // Globally installed shim (e.g. `%AppData%\npm\node_modules\.bin\foo.cmd`)
        // that matches every structural check — `.cmd` extension, under
        // `node_modules/.bin`, sibling `.ps1` present — but lives outside the
        // project. The rewrite must stay hands-off so unrelated user tooling
        // isn't silently retargeted.
        let dir = tempdir().unwrap();
        let root = abs(dir.path().canonicalize().unwrap());
        let workspace_path = root.as_path().join("workspace");
        fs::create_dir_all(&workspace_path).unwrap();
        let workspace = abs(workspace_path);

        let global_bin = root.as_path().join("global").join("node_modules").join(".bin");
        fs::create_dir_all(&global_bin).unwrap();
        fs::write(global_bin.join("vite.cmd"), "").unwrap();
        fs::write(global_bin.join("vite.ps1"), "").unwrap();

        let host = host_arc(&workspace);
        let resolved = abs(global_bin.join("vite.cmd"));
        let args: Arc<[Str]> = Arc::from(vec![]);

        assert!(rewrite_with_host(&resolved, &args, &workspace, &workspace, &host).is_none());
    }
}
