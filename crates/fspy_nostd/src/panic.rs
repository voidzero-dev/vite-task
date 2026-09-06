use core::{
    fmt::Write as _,
    panic::PanicInfo,
    sync::atomic::{AtomicBool, Ordering},
};

/// Installs a Linux panic handler in a freestanding executable.
///
/// Invoke once as `fspy_nostd::panic_handler!()`. Do not combine it with `std`
/// or another panic handler. [`crate::main`] already installs this handler.
/// The executable must keep stderr open and unreplaced until termination.
///
/// Prints the panic to descriptor 2, then calls `exit_group(101)` without
/// unwinding or running destructors. Output errors are ignored. Recursive or
/// concurrent panics skip printing and terminate immediately.
///
/// This uses no libc, allocation, or locks. Formatting the panic message may
/// invoke user code, and writing may block or raise `SIGPIPE`. The process's
/// syscall policy must allow the writes and process termination.
#[macro_export]
macro_rules! panic_handler {
    () => {
        const _: () = {
            #[panic_handler]
            fn panic(info: &::core::panic::PanicInfo<'_>) -> ! {
                $crate::__panic_handler(info)
            }
        };
    };
}

pub fn handle(info: &PanicInfo<'_>) -> ! {
    static PANICKING: AtomicBool = AtomicBool::new(false);
    // This only elects the first printer; there is no shared payload to order.
    if !PANICKING.swap(true, Ordering::Relaxed) {
        // SAFETY: the executable keeps stderr open until termination, as
        // required by panic_handler!'s contract.
        let mut stderr = unsafe { crate::stdio::stderr() };
        let _ = writeln!(stderr, "{info}");
    }
    loop {
        // SAFETY: terminates the process without accessing userspace memory.
        let _ = unsafe { syscalls::syscall!(syscalls::Sysno::exit_group, 101) };
    }
}
