pub mod open;
pub mod shebang;

use core::{ffi::c_char, ptr::null};

use allocator_api2::{alloc::Allocator, vec::Vec};
use bstr::ByteSlice as _;
use fspy_nostd::{CStr, Error, Result, Thin};

/// An environment entry: `(name, value)`. `value` is `None` when the entry in
/// `environ` does not contain a `=` byte.
pub type Env<A> = (Vec<u8, A>, Option<Vec<u8, A>>);

/// Minimum valid user-space address (page 0 is never mapped, matching
/// `fspy_elf_loader`'s `usermem::user_cstr`).
const MIN_USER_ADDR: usize = 4096;

/// Copies `bytes` into a `Vec` in `allocator`.
pub fn to_vec_in<A: Allocator>(bytes: &[u8], allocator: A) -> Vec<u8, A> {
    let mut vec = Vec::with_capacity_in(bytes.len(), allocator);
    vec.extend_from_slice(bytes);
    vec
}

fn try_to_vec_in<A: Allocator>(bytes: &[u8], allocator: A) -> Result<Vec<u8, A>> {
    let mut vec = Vec::new_in(allocator);
    vec.try_reserve_exact(bytes.len()).map_err(|_| Error::NOMEM)?;
    vec.extend_from_slice(bytes);
    Ok(vec)
}

/// An exec call's program, arguments, and environment.
///
/// The strings live in `A`, so preloads and signal handlers can keep them in a
/// per-call bump allocator rather than libc's heap: exec may run in the child
/// of `fork()` or inside a `SIGSYS` handler, where malloc is unsafe.
#[derive(Debug)]
pub struct Exec<A: Allocator> {
    pub program: Vec<u8, A>,
    pub args: Vec<Vec<u8, A>, A>,
    pub envs: Vec<Env<A>, A>,
}

impl<A: Allocator + Clone> Exec<A> {
    /// Returns the allocator the strings live in.
    #[must_use]
    pub fn allocator(&self) -> &A {
        self.args.allocator()
    }
}

/// Raw C pointers for an `execve`-family invocation.
#[derive(Clone, Copy)]
pub struct RawExec {
    pub prog: *const c_char,
    pub argv: *const *const c_char,
    pub envp: *const *const c_char,
}

impl RawExec {
    unsafe fn collect_c_str_array<T, A: Allocator>(
        strs: *const *const c_char,
        allocator: A,
        mut map_fn: impl FnMut(&[u8]) -> Result<T>,
    ) -> Result<Vec<T, A>> {
        if strs.is_null() {
            return Ok(Vec::new_in(allocator));
        }
        if strs.addr() < MIN_USER_ADDR {
            return Err(Error::FAULT);
        }

        let mut count = 0usize;
        let mut cur_str = strs;
        loop {
            // SAFETY: `cur_str` points into a valid NULL-terminated array of C
            // string pointers (the `argv`/`envp` convention).
            let entry_ptr = unsafe { *cur_str };
            if entry_ptr.is_null() {
                break;
            }
            if entry_ptr.addr() < MIN_USER_ADDR {
                return Err(Error::FAULT);
            }
            count += 1;
            // SAFETY: advancing within the bounds of the NULL-terminated array.
            cur_str = unsafe { cur_str.add(1) };
        }

        let mut str_vec = Vec::new_in(allocator);
        str_vec.try_reserve_exact(count).map_err(|_| Error::NOMEM)?;
        for i in 0..count {
            // SAFETY: `i < count`, so `strs.add(i)` is within the pointer array.
            let entry_ptr = unsafe { *strs.add(i) };
            // Re-verify against concurrent mutation in multithreaded callers
            // between the counting pass and this copy pass.
            if entry_ptr.is_null() {
                break;
            }
            if entry_ptr.addr() < MIN_USER_ADDR {
                return Err(Error::FAULT);
            }
            // SAFETY: `entry_ptr` is non-null and `>= MIN_USER_ADDR` and points
            // to a valid NUL-terminated C string.
            let bytes = unsafe { CStr::<Thin>::from_ptr(entry_ptr.cast()) }.count().as_units();
            str_vec.push(map_fn(bytes)?);
        }
        Ok(str_vec)
    }

    /// NUL-terminates `s` and leaks it into its allocator.
    fn into_c_str<A: Allocator>(mut s: Vec<u8, A>) -> *const c_char {
        s.push(0);
        s.leak().as_ptr().cast()
    }

    /// Builds a NULL-terminated array of C strings, leaked into the strings'
    /// allocator.
    fn into_c_str_array<A: Allocator + Clone>(strs: Vec<Vec<u8, A>, A>) -> *const *const c_char {
        let mut ptrs = Vec::with_capacity_in(strs.len() + 1, strs.allocator().clone());
        ptrs.extend(strs.into_iter().map(Self::into_c_str));
        ptrs.push(null());
        ptrs.leak().as_ptr()
    }

    /// Copies the raw exec arguments into `allocator`.
    ///
    /// Returns [`Error::FAULT`] when `prog` is in page 0 (`< 4096`, including
    /// null) or when a non-null `argv`/`envp` array or entry lies in page 0.
    /// Null `argv` or `envp` arrays are treated as empty, matching Linux
    /// `execve` semantics.
    ///
    /// # Errors
    ///
    /// Returns [`Error::FAULT`] for invalid first-page pointers, or
    /// [`Error::NOMEM`] if storage cannot be allocated.
    ///
    /// # Safety
    ///
    /// When non-null and outside page 0, `prog` must point to a valid
    /// NUL-terminated C string, and `argv` and `envp` must each point to a
    /// readable, NULL-terminated array of valid C string pointers.
    pub unsafe fn to_exec<A: Allocator + Clone>(self, allocator: A) -> Result<Exec<A>> {
        if self.prog.addr() < MIN_USER_ADDR {
            return Err(Error::FAULT);
        }

        // SAFETY: `self.prog` is outside page 0 and points to a valid
        // NUL-terminated C string, as required by the caller.
        let prog_bytes = unsafe { CStr::<Thin>::from_ptr(self.prog.cast()) }.count().as_units();
        let program = try_to_vec_in(prog_bytes, allocator.clone())?;

        // SAFETY: `self.argv` is null or a valid NULL-terminated array of C
        // string pointers.
        let args = unsafe {
            Self::collect_c_str_array(self.argv, allocator.clone(), |arg| {
                try_to_vec_in(arg, allocator.clone())
            })
        }?;

        // SAFETY: `self.envp` is null or a valid NULL-terminated array of C
        // string pointers.
        let envs = unsafe {
            Self::collect_c_str_array(self.envp, allocator.clone(), |env| {
                if let Some(eq_pos) = env.iter().position(|b| *b == b'=') {
                    Ok((
                        try_to_vec_in(&env[..eq_pos], allocator.clone())?,
                        Some(try_to_vec_in(&env[(eq_pos + 1)..], allocator.clone())?),
                    ))
                } else {
                    Ok((try_to_vec_in(env, allocator.clone())?, None))
                }
            })
        }?;

        Ok(Exec { program, args, envs })
    }

    /// Builds the C strings and pointer arrays for `cmd` in its allocator.
    ///
    /// They are leaked there rather than freed, so the pointers stay valid
    /// until the allocator itself goes away (typically a per-call bump).
    #[must_use]
    pub fn from_exec<A: Allocator + Clone>(cmd: Exec<A>) -> Self {
        let mut envs = Vec::with_capacity_in(cmd.envs.len(), cmd.allocator().clone());
        for (name, value) in cmd.envs {
            let mut env = name;
            if let Some(value) = value {
                env.push(b'=');
                env.extend_from_slice(&value);
            }
            envs.push(env);
        }

        Self {
            prog: Self::into_c_str(cmd.program),
            argv: Self::into_c_str_array(cmd.args),
            envp: Self::into_c_str_array(envs),
        }
    }
}

/// Ensures an environment variable is set to `value`, overwriting and
/// deduplicating any existing entries with the same `name`.
///
/// # Errors
///
/// Returns [`Error::NOMEM`] if storage cannot be allocated.
pub fn ensure_env<A: Allocator + Clone>(
    envs: &mut Vec<Env<A>, A>,
    name: &[u8],
    value: &[u8],
) -> Result<()> {
    let allocator = envs.allocator().clone();
    let mut updated = false;
    let mut i = 0;
    while i < envs.len() {
        if envs[i].0.as_bstr() == name {
            if updated {
                envs.remove(i);
                continue;
            }
            if envs[i].1.as_deref() != Some(value) {
                envs[i].1 = Some(try_to_vec_in(value, allocator.clone())?);
            }
            updated = true;
        }
        i += 1;
    }
    if !updated {
        envs.try_reserve(1).map_err(|_| Error::NOMEM)?;
        envs.push((
            try_to_vec_in(name, allocator.clone())?,
            Some(try_to_vec_in(value, allocator)?),
        ));
    }
    Ok(())
}

/// Ensures `value` is the trailing colon-separated entry of env var `name`.
#[cfg_attr(
    not(any(target_os = "macos", test)),
    expect(dead_code, reason = "used by macOS DYLD_INSERT_LIBRARIES injection")
)]
pub fn append_path_env<A: Allocator + Clone>(envs: &mut Vec<Env<A>, A>, name: &[u8], value: &[u8]) {
    let allocator = envs.allocator().clone();
    if let Some(entry) = envs.iter_mut().find(|(n, _)| n.as_bstr() == name) {
        let existing: &[u8] = entry.1.as_deref().unwrap_or_default();
        let already_last = existing == value
            || (existing.len() > value.len()
                && existing.ends_with(value)
                && existing[existing.len() - value.len() - 1] == b':');
        if already_last {
            return;
        }
        let mut new_value = Vec::with_capacity_in(existing.len() + 1 + value.len(), allocator);
        if !existing.is_empty() {
            new_value.extend_from_slice(existing);
            new_value.push(b':');
        }
        new_value.extend_from_slice(value);
        entry.1 = Some(new_value);
    } else {
        envs.push((to_vec_in(name, allocator.clone()), Some(to_vec_in(value, allocator))));
    }
}

/// Removes all colon-separated occurrences of `value` from env var `name`,
/// removing the variable itself when no entries remain.
#[cfg_attr(
    not(any(target_os = "macos", test)),
    expect(dead_code, reason = "used by macOS DYLD_INSERT_LIBRARIES cleanup")
)]
pub fn remove_path_env<A: Allocator + Clone>(envs: &mut Vec<Env<A>, A>, name: &[u8], value: &[u8]) {
    let allocator = envs.allocator().clone();
    let mut i = 0;
    while i < envs.len() {
        if envs[i].0.as_bstr() != name {
            i += 1;
            continue;
        }
        let Some(existing) = envs[i].1.as_deref() else {
            i += 1;
            continue;
        };
        if !existing.split(|&b| b == b':').any(|part| part == value) {
            i += 1;
            continue;
        }
        let mut new_value = Vec::with_capacity_in(existing.len(), allocator.clone());
        for part in existing.split(|&b| b == b':').filter(|&part| part != value && !part.is_empty())
        {
            if !new_value.is_empty() {
                new_value.push(b':');
            }
            new_value.extend_from_slice(part);
        }
        if new_value.is_empty() {
            envs.remove(i);
        } else {
            envs[i].1 = Some(new_value);
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use core::ptr;

    use allocator_api2::alloc::Global;

    use super::*;

    #[test]
    fn raw_exec_rejects_null_prog_and_accepts_null_argv_envp() {
        let raw_null = RawExec { prog: ptr::null(), argv: ptr::null(), envp: ptr::null() };
        // SAFETY: `to_exec` explicitly checks for null `prog`, `argv`, and `envp`.
        assert_eq!(unsafe { raw_null.to_exec(Global) }.unwrap_err(), Error::FAULT);

        let raw_empty =
            RawExec { prog: c"/bin/true".as_ptr(), argv: ptr::null(), envp: ptr::null() };
        // SAFETY: `prog` is a valid static C string and `argv`/`envp` are null.
        let exec = unsafe { raw_empty.to_exec(Global) }.unwrap();
        assert_eq!(exec.program.as_slice(), b"/bin/true");
        assert!(exec.args.is_empty());
        assert!(exec.envs.is_empty());
    }

    #[test]
    fn raw_exec_round_trips_program_args_and_envs() {
        let bump = fspy_nostd_alloc::pooled_bump();
        let mut args = Vec::new_in(&bump);
        args.push(to_vec_in(b"prog", &bump));
        args.push(to_vec_in(b"--flag", &bump));
        let mut envs = Vec::new_in(&bump);
        envs.push((to_vec_in(b"K", &bump), Some(to_vec_in(b"V=1", &bump))));
        envs.push((to_vec_in(b"BARE", &bump), None));
        let exec = Exec { program: to_vec_in(b"/usr/bin/prog", &bump), args, envs };

        let raw = RawExec::from_exec(exec);
        // SAFETY: `raw` points into `bump`, which stays alive for this call.
        let back = unsafe { raw.to_exec(Global) }.unwrap();
        assert_eq!(back.program.as_slice(), b"/usr/bin/prog");
        assert_eq!(back.args.len(), 2);
        assert_eq!(back.args[0].as_slice(), b"prog");
        assert_eq!(back.args[1].as_slice(), b"--flag");
        assert_eq!(back.envs.len(), 2);
        assert_eq!(back.envs[0].0.as_slice(), b"K");
        assert_eq!(back.envs[0].1.as_deref(), Some(b"V=1".as_slice()));
        assert_eq!(back.envs[1].0.as_slice(), b"BARE");
        assert_eq!(back.envs[1].1, None);
    }

    #[test]
    fn append_path_env_preserves_order_and_is_idempotent() {
        let mut envs: Vec<Env<Global>, Global> = Vec::new_in(Global);
        append_path_env(&mut envs, b"DYLD_INSERT_LIBRARIES", b"/a.dylib");
        assert_eq!(envs[0].1.as_deref(), Some(b"/a.dylib".as_slice()));

        append_path_env(&mut envs, b"DYLD_INSERT_LIBRARIES", b"/a.dylib");
        assert_eq!(envs[0].1.as_deref(), Some(b"/a.dylib".as_slice()));

        envs[0].1 = Some(to_vec_in(b"/user.dylib", Global));
        append_path_env(&mut envs, b"DYLD_INSERT_LIBRARIES", b"/a.dylib");
        assert_eq!(envs[0].1.as_deref(), Some(b"/user.dylib:/a.dylib".as_slice()));

        append_path_env(&mut envs, b"DYLD_INSERT_LIBRARIES", b"/a.dylib");
        assert_eq!(envs[0].1.as_deref(), Some(b"/user.dylib:/a.dylib".as_slice()));
    }

    #[test]
    fn remove_path_env_strips_only_target_entry() {
        let mut envs: Vec<Env<Global>, Global> = Vec::new_in(Global);
        append_path_env(&mut envs, b"DYLD_INSERT_LIBRARIES", b"/ours.dylib");
        remove_path_env(&mut envs, b"DYLD_INSERT_LIBRARIES", b"/ours.dylib");
        assert!(envs.is_empty());

        envs.push((
            to_vec_in(b"DYLD_INSERT_LIBRARIES", Global),
            Some(to_vec_in(b"/user1.dylib:/ours.dylib:/user2.dylib", Global)),
        ));
        remove_path_env(&mut envs, b"DYLD_INSERT_LIBRARIES", b"/ours.dylib");
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].1.as_deref(), Some(b"/user1.dylib:/user2.dylib".as_slice()));

        // Untouched when the target entry is absent.
        remove_path_env(&mut envs, b"DYLD_INSERT_LIBRARIES", b"/ours.dylib");
        assert_eq!(envs[0].1.as_deref(), Some(b"/user1.dylib:/user2.dylib".as_slice()));
    }

    #[test]
    fn raw_exec_rejects_first_page_pointers() {
        let low = ptr::with_exposed_provenance::<c_char>(0x10);
        let low_arr = ptr::with_exposed_provenance::<*const c_char>(0x10);

        let bad_prog = RawExec { prog: low, argv: ptr::null(), envp: ptr::null() };
        // SAFETY: `to_exec` rejects pointers in page 0 without dereferencing them.
        assert_eq!(unsafe { bad_prog.to_exec(Global) }.unwrap_err(), Error::FAULT);

        let bad_argv = RawExec { prog: c"/bin/true".as_ptr(), argv: low_arr, envp: ptr::null() };
        // SAFETY: `to_exec` rejects non-null `argv` in page 0 without dereferencing.
        assert_eq!(unsafe { bad_argv.to_exec(Global) }.unwrap_err(), Error::FAULT);

        let bad_envp = RawExec { prog: c"/bin/true".as_ptr(), argv: ptr::null(), envp: low_arr };
        // SAFETY: `to_exec` rejects non-null `envp` in page 0 without dereferencing.
        assert_eq!(unsafe { bad_envp.to_exec(Global) }.unwrap_err(), Error::FAULT);

        let bad_elem_arr = [low, ptr::null()];
        let bad_elem =
            RawExec { prog: c"/bin/true".as_ptr(), argv: bad_elem_arr.as_ptr(), envp: ptr::null() };
        // SAFETY: `bad_elem_arr` is a readable NULL-terminated array; `to_exec`
        // rejects the first-page element pointer without dereferencing it.
        assert_eq!(unsafe { bad_elem.to_exec(Global) }.unwrap_err(), Error::FAULT);
    }

    #[test]
    fn ensure_env_inserts_overwrites_and_deduplicates() {
        let mut envs: Vec<Env<Global>, Global> = Vec::new_in(Global);
        ensure_env(&mut envs, b"__FSPY_BOOTSTRAP", b"v1").unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].0.as_slice(), b"__FSPY_BOOTSTRAP");
        assert_eq!(envs[0].1.as_deref(), Some(b"v1".as_slice()));

        // Idempotent when value matches.
        ensure_env(&mut envs, b"__FSPY_BOOTSTRAP", b"v1").unwrap();
        assert_eq!(envs.len(), 1);

        // Overwrites a different value and deduplicates multiple occurrences
        // (including bare entries without `=`), preserving unrelated variables.
        envs.push((to_vec_in(b"KEEP", Global), Some(to_vec_in(b"ok", Global))));
        envs.push((to_vec_in(b"__FSPY_BOOTSTRAP", Global), None));
        envs.push((to_vec_in(b"__FSPY_BOOTSTRAP", Global), Some(to_vec_in(b"stale", Global))));
        ensure_env(&mut envs, b"__FSPY_BOOTSTRAP", b"v2").unwrap();
        assert_eq!(envs.len(), 2);
        assert_eq!(envs[0].0.as_slice(), b"__FSPY_BOOTSTRAP");
        assert_eq!(envs[0].1.as_deref(), Some(b"v2".as_slice()));
        assert_eq!(envs[1].0.as_slice(), b"KEEP");
        assert_eq!(envs[1].1.as_deref(), Some(b"ok".as_slice()));
    }
}
