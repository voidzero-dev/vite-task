//! Conservative startup-only interception for raw Linux syscall instructions.
//!
//! The caller remains responsible for a correctness fallback. A site is
//! intercepted only when it is inside a bounded ELF function symbol, can reach
//! a private trampoline, and satisfies the architecture-specific safety checks.

#![cfg(all(
    target_os = "linux",
    not(target_env = "musl"),
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
mod dispatcher;
mod patcher;

#[cfg(target_arch = "aarch64")]
#[path = "arch/aarch64.rs"]
mod arch;
#[cfg(target_arch = "x86_64")]
#[path = "arch/x86_64.rs"]
mod arch;

use std::{
    ffi::CStr,
    fs::File,
    os::fd::FromRawFd as _,
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

pub use dispatcher::SyscallContext;

/// Decides whether an intercepted syscall may use the allowlisted gate.
pub type Callback = fn(&SyscallContext) -> bool;

static ALLOWED_GATE: AtomicUsize = AtomicUsize::new(0);
static CALLBACK: OnceLock<Callback> = OnceLock::new();

fn allowed_gate() -> Option<usize> {
    let address = ALLOWED_GATE.load(Ordering::Relaxed);
    (address != 0).then_some(address)
}

/// Opens a control file through the exact seccomp-allowlisted syscall gate.
///
/// Returns `Ok(None)` when interception was not initialized on this process.
///
/// # Safety
///
/// The path must identify tracer control state whose access is intentionally
/// omitted from tracing. Using this for workload files makes tracing incomplete.
///
/// # Errors
///
/// Returns the `openat` error or an error converting its result to an owned
/// file descriptor.
pub unsafe fn open_control_file(path: &CStr) -> std::io::Result<Option<File>> {
    let Some(gate) = allowed_gate() else {
        return Ok(None);
    };
    let number = usize::try_from(libc::SYS_openat).map_err(std::io::Error::other)?;
    let dirfd = usize::from_ne_bytes(i64::from(libc::AT_FDCWD).to_ne_bytes());
    let flags = usize::try_from(libc::O_RDONLY | libc::O_CLOEXEC).map_err(std::io::Error::other)?;
    // SAFETY: `path` is a valid C string, and `gate` was installed by `init`.
    let result = unsafe {
        arch::raw_syscall6_at(gate, number, dirfd, path.as_ptr() as usize, flags, 0, 0, 0)
    };
    if result < 0 {
        let errno = i32::try_from(-result).unwrap_or(libc::EIO);
        return Err(std::io::Error::from_raw_os_error(errno));
    }
    let fd = i32::try_from(result).map_err(std::io::Error::other)?;
    // SAFETY: a nonnegative successful openat result is a newly owned fd.
    Ok(Some(unsafe { File::from_raw_fd(fd) }))
}

/// Installs the exact-PC gate and patches supported syscall sites at startup.
///
/// The callback returns `true` only when the syscall may use the allowlisted
/// gate. Returning `false` executes it at the separate fallback gate instead.
pub fn init(post_syscall_pc: u64, callback: Callback) {
    if std::env::var_os("FSPY_DISABLE_ACCELERATION").is_some() {
        return;
    }
    if CALLBACK.get().is_some() {
        return;
    }
    if !has_single_thread() || !arch::runtime_supported() {
        return;
    }
    let Ok(post_syscall_pc) = usize::try_from(post_syscall_pc) else {
        return;
    };
    // SAFETY: the exact-address mapping is private and is not published until
    // it has transitioned from writable to executable.
    let Some(gate) = (unsafe { map_allowed_gate(post_syscall_pc) }) else {
        return;
    };
    if CALLBACK.set(callback).is_err() {
        return;
    }
    ALLOWED_GATE.store(gate, Ordering::Relaxed);
    patcher::patch_initial_elf_mappings();
}

/// Executes a raw syscall at a PC that is intentionally not allowlisted.
///
/// # Safety
///
/// The caller must uphold the pointer and argument requirements of `number`.
#[inline(never)]
#[must_use]
pub unsafe fn raw_syscall6(
    number: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> isize {
    // SAFETY: the caller supplies the syscall-specific argument validity.
    unsafe { arch::raw_syscall6(number, arg0, arg1, arg2, arg3, arg4, arg5) }
}

unsafe fn map_allowed_gate(post_syscall_pc: usize) -> Option<usize> {
    let page_size = usize::try_from(
        // SAFETY: `sysconf` has no pointer preconditions.
        unsafe { libc::sysconf(libc::_SC_PAGESIZE) },
    )
    .ok()
    .filter(|size| size.is_power_of_two())?;
    let instruction = arch::ALLOWED_GATE_INSTRUCTION;
    let instruction_address = post_syscall_pc.checked_sub(instruction.len())?;
    let entry_address = instruction_address.checked_sub(arch::LANDING_PAD.len())?;
    let page_address = post_syscall_pc & !(page_size - 1);
    let page_end = page_address.checked_add(page_size)?;
    if entry_address < page_address
        || post_syscall_pc.checked_add(arch::RETURN_INSTRUCTION.len())? > page_end
    {
        return None;
    }

    // SAFETY: MAP_FIXED_NOREPLACE cannot replace an existing mapping at the PC
    // trusted by seccomp; the requested page is private and anonymous.
    let mapping = unsafe {
        libc::mmap(
            page_address as *mut libc::c_void,
            page_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED_NOREPLACE,
            -1,
            0,
        )
    };
    if mapping == libc::MAP_FAILED || mapping as usize != page_address {
        if mapping != libc::MAP_FAILED {
            // SAFETY: `mapping` was returned by the successful mmap above.
            unsafe { libc::munmap(mapping, page_size) };
        }
        return None;
    }

    // SAFETY: all destinations were bounds-checked against the writable page.
    unsafe {
        std::ptr::copy_nonoverlapping(
            arch::LANDING_PAD.as_ptr(),
            entry_address as *mut u8,
            arch::LANDING_PAD.len(),
        );
        std::ptr::copy_nonoverlapping(
            instruction.as_ptr(),
            instruction_address as *mut u8,
            instruction.len(),
        );
        std::ptr::copy_nonoverlapping(
            arch::RETURN_INSTRUCTION.as_ptr(),
            post_syscall_pc as *mut u8,
            arch::RETURN_INSTRUCTION.len(),
        );
        arch::clear_cache(entry_address, post_syscall_pc + arch::RETURN_INSTRUCTION.len());
    }
    // SAFETY: the mapping is valid and page-aligned; dropping write permission
    // before adding execute permission keeps the generated gate W^X.
    if unsafe { libc::mprotect(mapping, page_size, libc::PROT_READ | libc::PROT_EXEC) } != 0 {
        // SAFETY: `mapping` still names the page created above.
        unsafe { libc::munmap(mapping, page_size) };
        return None;
    }
    Some(entry_address)
}

fn has_single_thread() -> bool {
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return false;
    };
    let mut count = 0;
    for task in tasks {
        if task.is_err() {
            return false;
        }
        count += 1;
        if count > 1 {
            return false;
        }
    }
    count == 1
}

#[cfg(test)]
mod tests {
    #[test]
    fn configured_pc_can_contain_gate_and_return() {
        let post_syscall_pc = 0x0000_0001_0000_0008usize;
        let page_start = post_syscall_pc & !4095;
        assert!(
            post_syscall_pc
                - super::arch::ALLOWED_GATE_INSTRUCTION.len()
                - super::arch::LANDING_PAD.len()
                >= page_start
        );
        assert!(post_syscall_pc + super::arch::RETURN_INSTRUCTION.len() <= page_start + 4096);
    }
}
