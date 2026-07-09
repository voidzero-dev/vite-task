use std::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, Ordering},
};

use fspy_shared::ipc::AccessMode;
use fspy_syscall_intercept::{SyscallContext, raw_syscall6};

use crate::client::{Client, ENCODED_PATH_CAPACITY, global_client};

const PATH_CAPACITY: usize = libc::PATH_MAX as usize;

static IN_DISPATCH: AtomicBool = AtomicBool::new(false);
static DISPATCH_SCRATCH: SharedScratch = SharedScratch(UnsafeCell::new(DispatchScratch::new()));

struct DispatchScratch {
    path: [u8; PATH_CAPACITY],
    base: [u8; PATH_CAPACITY],
    resolved: [u8; PATH_CAPACITY],
    encoded: [u8; ENCODED_PATH_CAPACITY],
}

impl DispatchScratch {
    const fn new() -> Self {
        Self {
            path: [0; PATH_CAPACITY],
            base: [0; PATH_CAPACITY],
            resolved: [0; PATH_CAPACITY],
            encoded: [0; ENCODED_PATH_CAPACITY],
        }
    }
}

struct SharedScratch(UnsafeCell<DispatchScratch>);

// SAFETY: mutable access is available only while holding the process-wide
// `IN_DISPATCH` guard. Nested and concurrent dispatch fail before access.
unsafe impl Sync for SharedScratch {}

struct DispatchGuard;

impl DispatchGuard {
    fn enter() -> Option<Self> {
        IN_DISPATCH
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
            .then_some(Self)
    }

    #[expect(
        clippy::unused_self,
        clippy::needless_pass_by_ref_mut,
        reason = "the mutable guard borrow ties exclusive scratch access to the guard lifetime"
    )]
    fn scratch(&mut self) -> &mut DispatchScratch {
        // SAFETY: successful construction changed IN_DISPATCH from false to
        // true, and this mutable borrow cannot outlive that guard.
        unsafe { &mut *DISPATCH_SCRATCH.0.get() }
    }
}

impl Drop for DispatchGuard {
    fn drop(&mut self) {
        IN_DISPATCH.store(false, Ordering::Release);
    }
}

pub(super) fn try_dispatch(context: &SyscallContext) -> bool {
    let Some(mut guard) = DispatchGuard::enter() else {
        return false;
    };
    try_log(context, guard.scratch())
}

fn try_log(context: &SyscallContext, scratch: &mut DispatchScratch) -> bool {
    let Some(client) = global_client() else {
        return false;
    };
    let syscall = context.syscall_number();
    let args = context.arguments();

    #[cfg(target_arch = "x86_64")]
    if is_syscall(syscall, libc::SYS_open) {
        return send_path(client, scratch, libc::AT_FDCWD, args[0], mode_from_flags(args[1]));
    }
    if is_syscall(syscall, libc::SYS_openat) {
        return send_path(client, scratch, as_dirfd(args[0]), args[1], mode_from_flags(args[2]));
    }
    if is_syscall(syscall, libc::SYS_openat2) {
        let Some(flags) = copy_u64(args[2]) else {
            return false;
        };
        let Ok(flags) = usize::try_from(flags) else {
            return false;
        };
        return send_path(client, scratch, as_dirfd(args[0]), args[1], mode_from_flags(flags));
    }

    #[cfg(target_arch = "x86_64")]
    if is_syscall(syscall, libc::SYS_stat)
        || is_syscall(syscall, libc::SYS_lstat)
        || is_syscall(syscall, libc::SYS_access)
    {
        return send_path(client, scratch, libc::AT_FDCWD, args[0], AccessMode::READ);
    }
    if is_syscall(syscall, libc::SYS_newfstatat)
        || is_syscall(syscall, libc::SYS_statx)
        || is_syscall(syscall, libc::SYS_faccessat)
        || is_syscall(syscall, libc::SYS_faccessat2)
    {
        return send_path(client, scratch, as_dirfd(args[0]), args[1], AccessMode::READ);
    }

    #[cfg(target_arch = "x86_64")]
    if is_syscall(syscall, libc::SYS_getdents) {
        return send_fd_path(client, scratch, as_dirfd(args[0]), AccessMode::READ_DIR);
    }
    if is_syscall(syscall, libc::SYS_getdents64) {
        return send_fd_path(client, scratch, as_dirfd(args[0]), AccessMode::READ_DIR);
    }

    if is_syscall(syscall, libc::SYS_execve) {
        return send_path(client, scratch, libc::AT_FDCWD, args[0], AccessMode::READ);
    }
    if is_syscall(syscall, libc::SYS_execveat) {
        return send_path(client, scratch, as_dirfd(args[0]), args[1], AccessMode::READ);
    }

    false
}

fn is_syscall(actual: usize, expected: libc::c_long) -> bool {
    usize::try_from(expected) == Ok(actual)
}

fn as_dirfd(value: usize) -> libc::c_int {
    let low = u32::try_from(value & u32::MAX as usize).unwrap_or_default();
    libc::c_int::from_ne_bytes(low.to_ne_bytes())
}

fn syscall_number(number: libc::c_long) -> Option<usize> {
    usize::try_from(number).ok()
}

fn register_from_c_int(value: libc::c_int) -> usize {
    usize::from_ne_bytes(i64::from(value).to_ne_bytes())
}

fn mode_from_flags(flags: usize) -> AccessMode {
    let mut mode = match flags & libc::O_ACCMODE as usize {
        value if value == libc::O_RDWR as usize => AccessMode::READ | AccessMode::WRITE,
        value if value == libc::O_WRONLY as usize => AccessMode::WRITE,
        _ => AccessMode::READ,
    };
    if flags & (libc::O_CREAT | libc::O_TRUNC) as usize != 0
        || flags & libc::O_TMPFILE as usize == libc::O_TMPFILE as usize
    {
        mode.insert(AccessMode::WRITE);
    }
    mode
}

fn send_path(
    client: &Client,
    scratch: &mut DispatchScratch,
    dirfd: libc::c_int,
    address: usize,
    mode: AccessMode,
) -> bool {
    let Some(length) =
        resolve_path(dirfd, address, &mut scratch.path, &mut scratch.base, &mut scratch.resolved)
    else {
        return false;
    };
    client.try_send_bytes(mode, &scratch.resolved[..length], &mut scratch.encoded)
}

fn send_fd_path(
    client: &Client,
    scratch: &mut DispatchScratch,
    fd: libc::c_int,
    mode: AccessMode,
) -> bool {
    let Some(length) = resolve_fd_path(fd, &mut scratch.resolved) else {
        return false;
    };
    client.try_send_bytes(mode, &scratch.resolved[..length], &mut scratch.encoded)
}

fn resolve_path(
    dirfd: libc::c_int,
    address: usize,
    path: &mut [u8; PATH_CAPACITY],
    base: &mut [u8; PATH_CAPACITY],
    resolved: &mut [u8; PATH_CAPACITY],
) -> Option<usize> {
    let path_length = copy_c_string(address, path)?;
    if path[..path_length].first() == Some(&b'/') {
        resolved[..path_length].copy_from_slice(&path[..path_length]);
        return Some(path_length);
    }

    let base_length =
        if dirfd == libc::AT_FDCWD { getcwd(base)? } else { resolve_fd_path(dirfd, base)? };
    join_path(&base[..base_length], &path[..path_length], resolved)
}

fn getcwd(output: &mut [u8; PATH_CAPACITY]) -> Option<usize> {
    let syscall = syscall_number(libc::SYS_getcwd)?;
    // SAFETY: `output` is writable for the supplied length. getcwd is not in
    // the monitored set, so this non-allowlisted raw syscall cannot recurse.
    let result =
        unsafe { raw_syscall6(syscall, output.as_mut_ptr() as usize, output.len(), 0, 0, 0, 0) };
    let length = usize::try_from(result).ok()?;
    if length == 0 || length > output.len() || output[length - 1] != 0 {
        return None;
    }
    Some(length - 1)
}

fn resolve_fd_path(fd: libc::c_int, output: &mut [u8; PATH_CAPACITY]) -> Option<usize> {
    let mut proc_path = [0; 64];
    let prefix = b"/proc/self/fd/";
    proc_path[..prefix.len()].copy_from_slice(prefix);
    let proc_path_end = proc_path.len() - 1;
    let digits = write_i32_decimal(fd, &mut proc_path[prefix.len()..proc_path_end])?;
    proc_path[prefix.len() + digits] = 0;
    let syscall = syscall_number(libc::SYS_readlinkat)?;

    // SAFETY: both buffers are valid for the supplied lengths. readlinkat is
    // deliberately executed at the fallback PC and is not monitored.
    let result = unsafe {
        raw_syscall6(
            syscall,
            register_from_c_int(libc::AT_FDCWD),
            proc_path.as_ptr() as usize,
            output.as_mut_ptr() as usize,
            output.len(),
            0,
            0,
        )
    };
    let length = usize::try_from(result).ok()?;
    (length < output.len()).then_some(length)
}

fn join_path(base: &[u8], path: &[u8], output: &mut [u8; PATH_CAPACITY]) -> Option<usize> {
    let separator = usize::from(!base.ends_with(b"/") && !path.is_empty());
    let length = base.len().checked_add(separator)?.checked_add(path.len())?;
    if length > output.len() {
        return None;
    }
    output[..base.len()].copy_from_slice(base);
    if separator != 0 {
        output[base.len()] = b'/';
    }
    output[base.len() + separator..length].copy_from_slice(path);
    Some(length)
}

fn copy_c_string(address: usize, output: &mut [u8; PATH_CAPACITY]) -> Option<usize> {
    if address == 0 {
        return None;
    }
    let length = copy_from_process(address, output)?;
    output[..length].iter().position(|byte| *byte == 0)
}

fn copy_u64(address: usize) -> Option<u64> {
    let mut bytes = [0; size_of::<u64>()];
    (copy_from_process(address, &mut bytes)? == bytes.len()).then(|| u64::from_ne_bytes(bytes))
}

fn copy_from_process(address: usize, output: &mut [u8]) -> Option<usize> {
    if address == 0 || output.is_empty() {
        return None;
    }
    let local = libc::iovec { iov_base: output.as_mut_ptr().cast(), iov_len: output.len() };
    let remote = libc::iovec { iov_base: address as *mut libc::c_void, iov_len: output.len() };
    // SAFETY: the local iovec is valid. process_vm_readv validates the remote
    // address in the kernel, avoiding a preload-side fault on an EFAULT input.
    let getpid = syscall_number(libc::SYS_getpid)?;
    // SAFETY: getpid has no pointer arguments or caller-side preconditions.
    let pid = unsafe { raw_syscall6(getpid, 0, 0, 0, 0, 0, 0) };
    let Ok(pid) = usize::try_from(pid) else {
        return None;
    };
    let process_vm_readv = syscall_number(libc::SYS_process_vm_readv)?;
    // SAFETY: both iovec descriptors live through the raw syscall.
    let result = unsafe {
        raw_syscall6(
            process_vm_readv,
            pid,
            std::ptr::from_ref(&local) as usize,
            1,
            std::ptr::from_ref(&remote) as usize,
            1,
            0,
        )
    };
    usize::try_from(result).ok()
}

fn write_i32_decimal(value: libc::c_int, output: &mut [u8]) -> Option<usize> {
    let negative = value < 0;
    let mut magnitude = value.unsigned_abs();
    let mut reversed = [0; 10];
    let mut digits = 0;
    loop {
        reversed[digits] = b'0' + u8::try_from(magnitude % 10).ok()?;
        digits += 1;
        magnitude /= 10;
        if magnitude == 0 {
            break;
        }
    }
    let length = digits + usize::from(negative);
    if length > output.len() {
        return None;
    }
    if negative {
        output[0] = b'-';
    }
    for index in 0..digits {
        output[length - index - 1] = reversed[index];
    }
    Some(length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_paths_without_allocating() {
        let mut output = [0; PATH_CAPACITY];
        let length = join_path(b"/workspace", b"src/main.rs", &mut output).unwrap();
        assert_eq!(&output[..length], b"/workspace/src/main.rs");

        let length = join_path(b"/", b"tmp", &mut output).unwrap();
        assert_eq!(&output[..length], b"/tmp");
    }

    #[test]
    fn formats_all_fd_values() {
        let mut output = [0; 16];
        let length = write_i32_decimal(libc::AT_FDCWD, &mut output).unwrap();
        assert_eq!(&output[..length], b"-100");
        let length = write_i32_decimal(libc::c_int::MIN, &mut output).unwrap();
        assert_eq!(&output[..length], b"-2147483648");
    }

    #[test]
    fn maps_open_modes() {
        assert_eq!(mode_from_flags(libc::O_RDONLY as usize), AccessMode::READ);
        assert_eq!(mode_from_flags(libc::O_WRONLY as usize), AccessMode::WRITE);
        assert_eq!(mode_from_flags(libc::O_RDWR as usize), AccessMode::READ | AccessMode::WRITE);
        assert_eq!(
            mode_from_flags((libc::O_RDONLY | libc::O_CREAT) as usize),
            AccessMode::READ | AccessMode::WRITE
        );
        assert_eq!(
            mode_from_flags((libc::O_RDONLY | libc::O_TRUNC) as usize),
            AccessMode::READ | AccessMode::WRITE
        );
    }

    #[test]
    fn nested_dispatch_cannot_access_shared_scratch() {
        let mut guard = DispatchGuard::enter().unwrap();
        guard.scratch().path[0] = 42;
        assert!(DispatchGuard::enter().is_none());
        assert_eq!(guard.scratch().path[0], 42);
        drop(guard);
        assert!(DispatchGuard::enter().is_some());
    }
}
