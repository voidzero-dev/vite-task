/// Defines the entry point for a freestanding Linux executable.
///
/// Invoke once as `fspy_nostd::main!(run)`, where `run` has signature
/// `fn(fspy_nostd::env::InitialStack<'_>) -> !`. The macro parses the original
/// Linux stack before calling this safe Rust function. The callback must
/// accept any stack lifetime; it cannot require `InitialStack<'static>`.
///
/// ```compile_fail,E0308
/// #![no_std]
/// #![no_main]
///
/// fn run(_: fspy_nostd::env::InitialStack<'static>) -> ! {
///     loop {}
/// }
/// fspy_nostd::main!(run);
/// ```
///
/// Supports `AArch64` and x86-64. The executable must use `#![no_main]` and
/// must not link `std`, another entry point, or another panic handler.
/// This installs [`crate::panic_handler`], which prints panics and terminates
/// with status 101. Stderr must remain open and unreplaced until termination.
/// This does not initialize libc, TLS, or Rust's standard runtime.
#[macro_export]
macro_rules! main {
    ($run:path $(,)?) => {
        $crate::panic_handler!();

        const _: () = {
            unsafe extern "C" fn __fspy_entry(sp: *const usize) -> ! {
                let run: for<'a> fn($crate::env::InitialStack<'a>) -> ! = $run;
                // SAFETY: `_start` passes the untouched Linux entry stack.
                // The callback borrows it immutably; unsafe code that replaces
                // its storage must preserve the validity of these borrows.
                let stack = unsafe { $crate::env::InitialStack::from_ptr(sp) };
                run(stack)
            }

            #[cfg(target_arch = "aarch64")]
            #[unsafe(naked)]
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _start() -> ! {
                // Capture SP before any stack frame is created. BL keeps its
                // alignment and puts the return address in the link register.
                ::core::arch::naked_asm!("mov x0, sp", "bl {entry}", "brk #0", entry = sym __fspy_entry)
            }

            #[cfg(target_arch = "x86_64")]
            #[unsafe(naked)]
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _start() -> ! {
                // Linux enters with RSP aligned to 16 bytes. CALL pushes the
                // return address, giving the bridge normal C entry alignment.
                ::core::arch::naked_asm!("mov rdi, rsp", "call {entry}", "ud2", entry = sym __fspy_entry)
            }

            #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
            compile_error!("fspy_nostd::main! supports only AArch64 and x86-64");
        };
    };
}
