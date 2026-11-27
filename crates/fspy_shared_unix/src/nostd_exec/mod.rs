pub mod open;
pub mod shebang;

use core::{ffi::c_char, ptr::null};

use allocator_api2::{alloc::Allocator, vec::Vec};
use bstr::ByteSlice as _;
use fspy_nostd::{CStr, Error, Result, Thin};

/// An environment entry: `(name, value)`. `value` is `None` when the entry in
/// `environ` does not contain a `=` byte.
pub type Env<A> = (Vec<u8, A>, Option<Vec<u8, A>>);

/// Copies `bytes` into a `Vec` in `allocator`.
pub fn to_vec_in<A: Allocator>(bytes: &[u8], allocator: A) -> Vec<u8, A> {
    let mut vec = Vec::with_capacity_in(bytes.len(), allocator);
    vec.extend_from_slice(bytes);
    vec
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
        mut map_fn: impl FnMut(&[u8]) -> T,
    ) -> Result<Vec<T, A>> {
        if strs.is_null() {
            return Ok(Vec::new_in(allocator));
        }

        let mut count = 0usize;
        let mut cur_str = strs;
        // SAFETY: `cur_str` points into a valid NULL-terminated array of C
        // string pointers (the `argv`/`envp` convention).
        while !(unsafe { *cur_str }).is_null() {
            count += 1;
            // SAFETY: advancing within the bounds of the NULL-terminated array.
            cur_str = unsafe { cur_str.add(1) };
        }

        let mut str_vec = Vec::new_in(allocator);
        str_vec.try_reserve_exact(count).map_err(|_| Error::NOMEM)?;
        for i in 0..count {
            // SAFETY: `i < count`, so `strs.add(i)` is within the pointer array.
            let entry_ptr = unsafe { *strs.add(i) };
            // SAFETY: `entry_ptr` is non-null (verified by the counting loop) and
            // points to a valid NUL-terminated C string.
            let bytes = unsafe { CStr::<Thin>::from_ptr(entry_ptr.cast()) }.count().as_units();
            str_vec.push(map_fn(bytes));
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
    /// Returns [`Error::FAULT`] when `prog` is null. Null `argv` or `envp`
    /// arrays are treated as empty, matching Linux `execve` semantics.
    ///
    /// # Errors
    ///
    /// Returns [`Error::FAULT`] if `prog` is null, or [`Error::NOMEM`] if
    /// storage cannot be allocated.
    ///
    /// # Safety
    ///
    /// When non-null, `prog` must point to a valid NUL-terminated C string, and
    /// `argv` and `envp` must each point to a readable, NULL-terminated array
    /// of valid C string pointers.
    pub unsafe fn to_exec<A: Allocator + Clone>(self, allocator: A) -> Result<Exec<A>> {
        if self.prog.is_null() {
            return Err(Error::FAULT);
        }

        // SAFETY: `self.prog` is non-null and points to a valid NUL-terminated
        // C string, as required by the caller.
        let prog_bytes = unsafe { CStr::<Thin>::from_ptr(self.prog.cast()) }.count().as_units();
        let program = to_vec_in(prog_bytes, allocator.clone());

        // SAFETY: `self.argv` is null or a valid NULL-terminated array of C
        // string pointers.
        let args = unsafe {
            Self::collect_c_str_array(self.argv, allocator.clone(), |arg| {
                to_vec_in(arg, allocator.clone())
            })
        }?;

        // SAFETY: `self.envp` is null or a valid NULL-terminated array of C
        // string pointers.
        let envs = unsafe {
            Self::collect_c_str_array(self.envp, allocator.clone(), |env| {
                env.iter().position(|b| *b == b'=').map_or_else(
                    || (to_vec_in(env, allocator.clone()), None),
                    |eq_pos| {
                        (
                            to_vec_in(&env[..eq_pos], allocator.clone()),
                            Some(to_vec_in(&env[(eq_pos + 1)..], allocator.clone())),
                        )
                    },
                )
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

/// Ensures an environment variable is set to `value`.
///
/// If absent, it is inserted. If present with the same value, no change is
/// made. If present with a different value (or without `=`), returns
/// [`Error::INVAL`].
///
/// # Errors
///
/// Returns [`Error::INVAL`] when `name` already exists with a different value.
pub fn ensure_env<A: Allocator + Clone>(
    envs: &mut Vec<Env<A>, A>,
    name: &[u8],
    value: &[u8],
) -> Result<()> {
    if let Some((_, existing_value)) = envs.iter().find(|(n, _)| n.as_bstr() == name) {
        return match existing_value.as_deref() {
            Some(existing) if existing == value => Ok(()),
            _ => Err(Error::INVAL),
        };
    }
    let allocator = envs.allocator().clone();
    envs.push((to_vec_in(name, allocator.clone()), Some(to_vec_in(value, allocator))));
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
}
