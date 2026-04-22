mod shebang;
mod which;

use std::{
    ffi::{CStr, OsStr},
    iter::once,
    mem::replace,
    os::unix::ffi::OsStrExt,
    path::Path,
};

use bstr::{BStr, BString, ByteSlice};
use fspy_shared::ipc::AccessMode;
use nix::unistd::{AccessFlags, access};
use shebang::{ParseShebangOptions, parse_shebang};

use crate::open_exec::open_executable;

#[derive(Debug, Clone)]
pub struct SearchPath<'a> {
    /// Custom search path to use (like execvP), overrides PATH if Some
    pub custom_path: Option<&'a BStr>,
}

/// Configuration for exec resolution behavior
#[derive(Debug, Clone)]
pub struct ExecResolveConfig<'a> {
    /// If Some and the program doesn't contains `/`,
    /// search the program in PATH (like execvp, execvpe, execlp) instead of finding it in current directory
    pub search_path: Option<SearchPath<'a>>,
    /// Options for parsing shebangs (all exec variants handle shebangs)
    pub shebang_options: ParseShebangOptions,
}

impl<'a> ExecResolveConfig<'a> {
    /// Configuration for execve - no PATH search, direct execution
    #[must_use]
    pub fn search_path_disabled() -> Self {
        Self { search_path: None, shebang_options: ParseShebangOptions::default() }
    }

    /// execlp/execvp/execvP/execvpe
    /// `custom_path` allows a customized path to be searched like in execvP (macOS extension)
    #[must_use]
    pub fn search_path_enabled(custom_path: Option<&'a BStr>) -> Self {
        Self {
            search_path: Some(SearchPath { custom_path }),
            shebang_options: ParseShebangOptions::default(),
        }
    }
}

#[derive(Debug)]
pub struct Exec {
    pub program: BString,
    pub args: Vec<BString>,
    /// vec of (name, value). value is None when the entry in environ doesn't contain a `=` character.
    pub envs: Vec<(BString, Option<BString>)>,
}

fn getenv(name: &CStr) -> Option<&'static CStr> {
    // SAFETY: `getenv` is a C standard library function, called with a valid pointer from `CStr::as_ptr`.
    let value = unsafe { nix::libc::getenv(name.as_ptr().cast()) };
    if value.is_null() {
        None
    } else {
        // SAFETY: `value` is non-null (checked above) and points to a null-terminated string owned
        // by the environment, as guaranteed by the C `getenv` contract.
        Some(unsafe { CStr::from_ptr(value) })
    }
}

fn peek_executable(path: &Path, buf: &mut [u8]) -> nix::Result<usize> {
    let fd = open_executable(path)?;
    let mut total_read_size = 0;
    loop {
        let read_size = nix::unistd::read(&fd, &mut buf[total_read_size..])?;
        if read_size == 0 {
            break;
        }
        total_read_size += read_size;
    }
    Ok(total_read_size)
}

impl Exec {
    /// Resolve the program path according to exec family semantics
    ///
    /// This method replicates the behavior of execve/execvp/execvP/execvpe for program resolution,
    /// including PATH searching and shebang handling.
    ///
    /// # Returns
    ///
    /// * `Ok(())` if resolution succeeds and `self` is updated with resolved paths
    /// * `Err(nix::Error)` with appropriate errno, like the exec function would return
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The program is not found in PATH (`ENOENT`)
    /// - The program file cannot be accessed or read (`EACCES`, `EISDIR`, `EIO`)
    /// - Shebang parsing fails due to I/O errors (`EIO`)
    pub fn resolve(
        &mut self,
        mut on_path_access: impl FnMut(AccessMode, &Path),
        config: ExecResolveConfig,
    ) -> nix::Result<()> {
        if let Some(search_path) = config.search_path {
            let path = search_path.custom_path.unwrap_or_else(|| {
                getenv(c"PATH").map_or_else(
                    || {
                        // https://github.com/kraj/musl/blob/1b06420abdf46f7d06ab4067e7c51b8b63731852/src/process/execvp.c#L21
                        b"/usr/local/bin:/bin:/usr/bin".as_bstr()
                    },
                    |path| path.to_bytes().as_bstr(),
                )
            });
            let program = which::which(
                self.program.as_ref(),
                path,
                |path| {
                    on_path_access(AccessMode::READ, Path::new(OsStr::from_bytes(path)));
                    access(OsStr::from_bytes(path), AccessFlags::X_OK)
                },
                |program| Ok(program.to_owned()),
            )?;
            self.program = program;
        }

        self.parse_shebang(on_path_access, config.shebang_options)?;

        Ok(())
    }

    fn parse_shebang(
        &mut self,
        mut on_path_access: impl FnMut(AccessMode, &Path),
        options: ParseShebangOptions,
    ) -> nix::Result<()> {
        if let Some(shebang) = parse_shebang(
            |path, buf| {
                on_path_access(AccessMode::READ, path);
                peek_executable(path, buf)
            },
            Path::new(OsStr::from_bytes(&self.program)),
            options,
        )? {
            self.args[0] = shebang.interpreter.clone();
            let old_program = replace(&mut self.program, shebang.interpreter);
            self.args.splice(1..1, shebang.arguments.into_iter().chain(once(old_program)));
        }
        Ok(())
    }
}

/// Ensures an environment variable is set to the specified value
///
/// If the variable doesn't exist, it is added. If it exists with the same value,
/// no change is made. If it exists with a different value, an error is returned.
///
/// # Errors
///
/// Returns `Err(nix::Error::EINVAL)` if the environment variable already exists with a different value.
pub fn ensure_env(
    envs: &mut Vec<(BString, Option<BString>)>,
    name: impl AsRef<BStr>,
    value: impl AsRef<BStr>,
) -> nix::Result<()> {
    let name = name.as_ref();
    let value = value.as_ref();
    let existing_value = envs.iter().find_map(|(n, v)| if n == name { v.as_ref() } else { None });
    if let Some(existing_value) = existing_value {
        return if existing_value == value { Ok(()) } else { Err(nix::Error::EINVAL) };
    }
    envs.push((name.to_owned(), Some(value.to_owned())));
    Ok(())
}

/// Ensures `value` is the trailing colon-separated entry of env var `name`.
///
/// Used for `LD_PRELOAD` / `DYLD_INSERT_LIBRARIES`, which the dynamic loader
/// treats as colon-separated lists. Appending (rather than overwriting)
/// preserves any user-provided preload, and appending to the *end* keeps
/// fspy's shim as the last interposer so a user preload that short-circuits
/// a call (returning without forwarding to libc) stays invisible to fspy —
/// mirroring what the OS actually did.
///
/// - Absent: inserts `(name, value)`.
/// - Present with `value` already as the last colon-separated entry: no
///   change (idempotent across nested execs within the preloaded shim).
/// - Present otherwise: rewrites to `{existing}:{value}`. If `existing` is
///   empty, sets to `value` alone to avoid a leading `:` (which glibc's
///   `ld.so` interprets as the current directory).
pub fn append_path_env(
    envs: &mut Vec<(BString, Option<BString>)>,
    name: impl AsRef<BStr>,
    value: impl AsRef<BStr>,
) {
    let name = name.as_ref();
    let value = value.as_ref();
    if let Some(entry) = envs.iter_mut().find(|(n, _)| n == name) {
        let existing: &[u8] = entry.1.as_deref().map_or(&[][..], |v| v.as_ref());
        let value_bytes: &[u8] = value.as_ref();
        let already_last = existing == value_bytes
            || (existing.len() > value_bytes.len()
                && existing.ends_with(value_bytes)
                && existing[existing.len() - value_bytes.len() - 1] == b':');
        if already_last {
            return;
        }
        let mut new_value = Vec::with_capacity(existing.len() + 1 + value_bytes.len());
        if !existing.is_empty() {
            new_value.extend_from_slice(existing);
            new_value.push(b':');
        }
        new_value.extend_from_slice(value_bytes);
        entry.1 = Some(BString::from(new_value));
    } else {
        envs.push((name.to_owned(), Some(value.to_owned())));
    }
}

#[cfg(test)]
mod tests {
    use bstr::BString;

    use super::append_path_env;

    fn env(envs: &[(BString, Option<BString>)], name: &[u8]) -> Option<Vec<u8>> {
        envs.iter()
            .find(|(n, _)| AsRef::<[u8]>::as_ref(n) == name)
            .and_then(|(_, v)| v.as_ref().map(|v| AsRef::<[u8]>::as_ref(v).to_vec()))
    }

    #[test]
    fn inserts_when_absent() {
        let mut envs: Vec<(BString, Option<BString>)> = vec![];
        append_path_env(&mut envs, "LD_PRELOAD", "/a.so");
        assert_eq!(env(&envs, b"LD_PRELOAD"), Some(b"/a.so".to_vec()));
    }

    #[test]
    fn noop_when_equal() {
        let mut envs = vec![(BString::from("LD_PRELOAD"), Some(BString::from("/a.so")))];
        append_path_env(&mut envs, "LD_PRELOAD", "/a.so");
        assert_eq!(env(&envs, b"LD_PRELOAD"), Some(b"/a.so".to_vec()));
    }

    #[test]
    fn noop_when_value_is_last_entry() {
        let mut envs = vec![(BString::from("LD_PRELOAD"), Some(BString::from("/user.so:/a.so")))];
        append_path_env(&mut envs, "LD_PRELOAD", "/a.so");
        assert_eq!(env(&envs, b"LD_PRELOAD"), Some(b"/user.so:/a.so".to_vec()));
    }

    #[test]
    fn appends_with_colon_when_present_and_different() {
        let mut envs = vec![(BString::from("LD_PRELOAD"), Some(BString::from("/user.so")))];
        append_path_env(&mut envs, "LD_PRELOAD", "/a.so");
        assert_eq!(env(&envs, b"LD_PRELOAD"), Some(b"/user.so:/a.so".to_vec()));
    }

    #[test]
    fn sets_without_leading_colon_when_existing_is_empty() {
        let mut envs = vec![(BString::from("LD_PRELOAD"), Some(BString::from("")))];
        append_path_env(&mut envs, "LD_PRELOAD", "/a.so");
        assert_eq!(env(&envs, b"LD_PRELOAD"), Some(b"/a.so".to_vec()));
    }

    #[test]
    fn idempotent_on_repeat() {
        let mut envs: Vec<(BString, Option<BString>)> = vec![];
        append_path_env(&mut envs, "LD_PRELOAD", "/a.so");
        append_path_env(&mut envs, "LD_PRELOAD", "/a.so");
        append_path_env(&mut envs, "LD_PRELOAD", "/a.so");
        assert_eq!(env(&envs, b"LD_PRELOAD"), Some(b"/a.so".to_vec()));
    }

    #[test]
    fn does_not_false_match_prefix_without_preceding_colon() {
        // `lib/a.so` ends with `/a.so` as bytes, but the preceding byte is
        // `b` not `:`, so it must NOT be treated as already-present.
        let mut envs = vec![(BString::from("LD_PRELOAD"), Some(BString::from("/lib/a.so")))];
        append_path_env(&mut envs, "LD_PRELOAD", "a.so");
        assert_eq!(env(&envs, b"LD_PRELOAD"), Some(b"/lib/a.so:a.so".to_vec()));
    }

    #[test]
    fn inserts_when_present_with_none_value() {
        // An env var present in the list but with `None` value (name without
        // `=`) should be rewritten to `Some(value)`.
        let mut envs = vec![(BString::from("LD_PRELOAD"), None)];
        append_path_env(&mut envs, "LD_PRELOAD", "/a.so");
        assert_eq!(env(&envs, b"LD_PRELOAD"), Some(b"/a.so".to_vec()));
    }
}
