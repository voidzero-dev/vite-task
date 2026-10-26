use std::{ffi::CStr, ptr::null};

use allocator_api2::{alloc::Allocator, vec::Vec};
use fspy_shared_unix::exec::{Exec, to_vec_in};

#[derive(Clone, Copy)]
pub struct RawExec {
    pub prog: *const libc::c_char,
    pub argv: *const *const libc::c_char,
    pub envp: *const *const libc::c_char,
}

impl RawExec {
    unsafe fn collect_c_str_array<T, A: Allocator>(
        strs: *const *const libc::c_char,
        allocator: A,
        mut map_fn: impl FnMut(&[u8]) -> T,
    ) -> Vec<T, A> {
        let mut count = 0usize;
        let mut cur_str = strs;
        // SAFETY: cur_str points into a valid null-terminated array of C
        // string pointers (argv/envp convention).
        while !(unsafe { *cur_str }).is_null() {
            count += 1;
            // SAFETY: advancing within the bounds of the null-terminated
            // pointer array.
            cur_str = unsafe { cur_str.add(1) };
        }

        let mut str_vec = Vec::with_capacity_in(count, allocator);
        for i in 0..count {
            // SAFETY: i < count, so strs.add(i) is within the bounds of the
            // pointer array.
            let cur_str = unsafe { strs.add(i) };
            // SAFETY: *cur_str is a non-null pointer to a valid
            // null-terminated C string, verified by the counting loop above.
            str_vec.push(map_fn(unsafe { CStr::from_ptr(*cur_str) }.to_bytes()));
        }
        str_vec
    }

    /// NUL-terminates `s` and leaks it into its allocator.
    fn into_c_str<A: Allocator>(mut s: Vec<u8, A>) -> *const libc::c_char {
        s.push(0);
        s.leak().as_ptr().cast()
    }

    /// Builds a NULL-terminated array of C strings, leaked into the strings'
    /// allocator.
    fn into_c_str_array<A: Allocator + Clone>(
        strs: Vec<Vec<u8, A>, A>,
    ) -> *const *const libc::c_char {
        let mut ptrs = Vec::with_capacity_in(strs.len() + 1, strs.allocator().clone());
        ptrs.extend(strs.into_iter().map(Self::into_c_str));
        ptrs.push(null());
        ptrs.leak().as_ptr()
    }

    /// Copies the raw exec arguments into `allocator`.
    ///
    /// # Safety
    ///
    /// `prog` must point to a valid C string. `argv` and `envp` must each
    /// point to a readable, null-terminated array of valid C string pointers.
    pub unsafe fn to_exec<A: Allocator + Clone>(self, allocator: A) -> Exec<A> {
        // SAFETY: self.prog is a non-null pointer to a valid null-terminated C
        // string, as guaranteed by the libc exec calling convention.
        let program = to_vec_in(unsafe { CStr::from_ptr(self.prog) }.to_bytes(), allocator.clone());

        // SAFETY: self.argv is a valid null-terminated array of C string
        // pointers, as guaranteed by the libc exec calling convention.
        let args = unsafe {
            Self::collect_c_str_array(self.argv, allocator.clone(), |arg| {
                to_vec_in(arg, allocator.clone())
            })
        };

        // SAFETY: self.envp is a valid null-terminated array of C string
        // pointers, as guaranteed by the libc exec calling convention.
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
        };

        Exec { program, args, envs }
    }

    /// Builds the C strings and pointer arrays for `cmd` in its allocator.
    ///
    /// They are leaked there rather than freed, so the pointers stay valid
    /// until the allocator itself goes away. In the preload that is the
    /// interception's per-call bump: exec runs in the child of `fork()` in
    /// multithreaded programs, where libc malloc's lock may be held by a
    /// thread that no longer exists, and the bump hands everything back
    /// when the call ends.
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
