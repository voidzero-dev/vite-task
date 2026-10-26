mod shebang;
mod which;

use std::{
    ffi::{CStr, OsStr},
    iter::once,
    mem::replace,
    os::unix::ffi::OsStrExt,
    path::Path,
};

use allocator_api2::{alloc::Allocator, vec::Vec};
use bstr::{BStr, ByteSlice};
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

/// An environment entry: (name, value). value is None when the entry in
/// environ doesn't contain a `=` character.
pub type Env<A> = (Vec<u8, A>, Option<Vec<u8, A>>);

/// An exec call's program, arguments and environment.
///
/// The strings live in `A`, so the preload can keep them in the
/// interception's bump rather than libc's heap: exec may run in the child of
/// `fork()`, where malloc's lock may be held by a thread that no longer
/// exists.
/// Copies `bytes` into a `Vec` in `allocator`.
pub fn to_vec_in<A: Allocator>(bytes: &[u8], allocator: A) -> Vec<u8, A> {
    let mut vec = Vec::with_capacity_in(bytes.len(), allocator);
    vec.extend_from_slice(bytes);
    vec
}

#[derive(Debug)]
pub struct Exec<A: Allocator> {
    pub program: Vec<u8, A>,
    pub args: Vec<Vec<u8, A>, A>,
    pub envs: Vec<Env<A>, A>,
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

impl<A: Allocator + Clone> Exec<A> {
    /// The allocator the strings live in.
    pub fn allocator(&self) -> &A {
        self.args.allocator()
    }

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
        mut on_path_read: impl FnMut(&Path),
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
                self.program.as_bstr(),
                path,
                |path| {
                    on_path_read(Path::new(OsStr::from_bytes(path)));
                    access(OsStr::from_bytes(path), AccessFlags::X_OK)
                },
                |program| Ok(to_vec_in(program, self.allocator().clone())),
            )?;
            self.program = program;
        }

        self.parse_shebang(on_path_read, config.shebang_options)?;

        Ok(())
    }

    fn parse_shebang(
        &mut self,
        mut on_path_read: impl FnMut(&Path),
        options: ParseShebangOptions,
    ) -> nix::Result<()> {
        if let Some(shebang) = parse_shebang(
            |path, buf| {
                on_path_read(path);
                peek_executable(path, buf)
            },
            Path::new(OsStr::from_bytes(&self.program)),
            options,
            self.allocator().clone(),
        )? {
            self.args[0].clone_from(&shebang.interpreter);
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
pub fn ensure_env<A: Allocator + Clone>(
    envs: &mut Vec<Env<A>, A>,
    name: impl AsRef<BStr>,
    value: impl AsRef<BStr>,
) -> nix::Result<()> {
    let name = name.as_ref();
    let value = value.as_ref();
    let existing_value =
        envs.iter().find_map(|(n, v)| if n.as_bstr() == name { v.as_ref() } else { None });
    if let Some(existing_value) = existing_value {
        return if existing_value.as_bstr() == value { Ok(()) } else { Err(nix::Error::EINVAL) };
    }
    let allocator = envs.allocator().clone();
    envs.push((to_vec_in(name, allocator.clone()), Some(to_vec_in(value, allocator))));
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
pub fn append_path_env<A: Allocator + Clone>(
    envs: &mut Vec<Env<A>, A>,
    name: impl AsRef<BStr>,
    value: impl AsRef<BStr>,
) {
    let name = name.as_ref();
    let value = value.as_ref();
    let allocator = envs.allocator().clone();
    if let Some(entry) = envs.iter_mut().find(|(n, _)| n.as_bstr() == name) {
        let existing: &[u8] = entry.1.as_deref().unwrap_or_default();
        let value_bytes: &[u8] = value.as_ref();
        let already_last = existing == value_bytes
            || (existing.len() > value_bytes.len()
                && existing.ends_with(value_bytes)
                && existing[existing.len() - value_bytes.len() - 1] == b':');
        if already_last {
            return;
        }
        let mut new_value =
            Vec::with_capacity_in(existing.len() + 1 + value_bytes.len(), allocator);
        if !existing.is_empty() {
            new_value.extend_from_slice(existing);
            new_value.push(b':');
        }
        new_value.extend_from_slice(value_bytes);
        entry.1 = Some(new_value);
    } else {
        envs.push((to_vec_in(name, allocator.clone()), Some(to_vec_in(value, allocator))));
    }
}

#[cfg(test)]
mod tests {
    use allocator_api2::{alloc::Global, vec::Vec};

    use super::{append_path_env, to_vec_in};

    type Envs = Vec<(Vec<u8>, Option<Vec<u8>>)>;

    fn env(envs: &Envs, name: &[u8]) -> Option<std::vec::Vec<u8>> {
        envs.iter().find(|(n, _)| n == name).and_then(|(_, v)| v.as_ref().map(|v| v.to_vec()))
    }

    /// (case, envs before the call, appended `LD_PRELOAD` value, expected value)
    type AppendCase<'a> = (&'a str, &'a [(&'a str, Option<&'a str>)], &'a str, &'a [u8]);

    #[test]
    fn appends_value_to_the_env_list() {
        let cases: &[AppendCase<'_>] = &[
            ("inserts when absent", &[], "/a.so", b"/a.so"),
            ("noop when equal", &[("LD_PRELOAD", Some("/a.so"))], "/a.so", b"/a.so"),
            (
                "noop when value is last entry",
                &[("LD_PRELOAD", Some("/user.so:/a.so"))],
                "/a.so",
                b"/user.so:/a.so",
            ),
            (
                "appends with colon when present and different",
                &[("LD_PRELOAD", Some("/user.so"))],
                "/a.so",
                b"/user.so:/a.so",
            ),
            (
                "sets without leading colon when existing is empty",
                &[("LD_PRELOAD", Some(""))],
                "/a.so",
                b"/a.so",
            ),
            // `lib/a.so` ends with `/a.so` as bytes, but the preceding byte is
            // `b` not `:`, so it must NOT be treated as already-present.
            (
                "does not false match prefix without preceding colon",
                &[("LD_PRELOAD", Some("/lib/a.so"))],
                "a.so",
                b"/lib/a.so:a.so",
            ),
            // An env var present in the list but with `None` value (name without
            // `=`) should be rewritten to `Some(value)`.
            ("inserts when present with none value", &[("LD_PRELOAD", None)], "/a.so", b"/a.so"),
        ];
        for &(case, existing, value, expected) in cases {
            let mut envs: Envs = existing
                .iter()
                .map(|&(name, value)| {
                    (
                        to_vec_in(name.as_bytes(), Global),
                        value.map(|v| to_vec_in(v.as_bytes(), Global)),
                    )
                })
                .collect();
            append_path_env(&mut envs, "LD_PRELOAD", value);
            assert_eq!(env(&envs, b"LD_PRELOAD"), Some(expected.to_vec()), "{case}");
        }
    }

    #[test]
    fn idempotent_on_repeat() {
        let mut envs = Envs::new();
        append_path_env(&mut envs, "LD_PRELOAD", "/a.so");
        append_path_env(&mut envs, "LD_PRELOAD", "/a.so");
        append_path_env(&mut envs, "LD_PRELOAD", "/a.so");
        assert_eq!(env(&envs, b"LD_PRELOAD"), Some(b"/a.so".to_vec()));
    }
}
