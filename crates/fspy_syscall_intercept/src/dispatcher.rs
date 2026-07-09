use crate::{CALLBACK, allowed_gate, arch};

/*
 * Handwritten assembly cannot call the consumer's Rust callback directly: each
 * architecture first saves registers in its own stack layout. `choose_gate` is
 * the shared C-ABI bridge. It converts that private layout into the small,
 * architecture-neutral `SyscallContext`, invokes the callback, and returns one
 * of two code addresses:
 *
 *   true  -> the exact-PC gate that the caller's seccomp filter allows
 *   false -> a different gate that remains visible to seccomp
 *
 * The assembly restores the original syscall registers after this function
 * returns and then calls the selected address. Consequently this function
 * decides only where the syscall executes; it never performs the syscall or
 * changes its arguments itself.
 */

/// Architecture-neutral snapshot of a raw syscall number and its arguments.
#[derive(Clone, Copy, Debug)]
pub struct SyscallContext {
    syscall_number: usize,
    arguments: [usize; 6],
}

impl SyscallContext {
    pub(crate) const fn new(syscall_number: usize, arguments: [usize; 6]) -> Self {
        Self { syscall_number, arguments }
    }

    /// Returns the Linux syscall number.
    #[must_use]
    pub const fn syscall_number(&self) -> usize {
        self.syscall_number
    }

    /// Returns the six raw syscall argument registers.
    #[must_use]
    pub const fn arguments(&self) -> [usize; 6] {
        self.arguments
    }
}

/// Chooses the exact-PC allowlisted gate only when the registered callback has
/// completed its bookkeeping. The assembly entry preserves raw syscall state.
pub extern "C" fn choose_gate(context: &arch::DispatchContext) -> usize {
    let fallback = arch::fallback_gate_address();
    let Some(callback) = CALLBACK.get() else {
        return fallback;
    };
    let context = arch::syscall_context(context);
    // SAFETY: glibc exposes a valid pointer to this thread's errno value.
    let errno = unsafe { libc::__errno_location() };
    // SAFETY: the pointer remains valid for the duration of this call.
    let saved_errno = unsafe { errno.read() };
    let allowed = callback(&context);
    // SAFETY: bookkeeping must not change the raw syscall caller's errno.
    unsafe { errno.write(saved_errno) };
    if allowed { allowed_gate().unwrap_or(fallback) } else { fallback }
}
