//! Load a Linux ELF executable inside an existing process.
//!
//! For a dynamic executable, the kernel maps the executable and its ELF
//! interpreter, writes startup data onto a new stack, and enters the
//! interpreter. The interpreter (such as `ld-linux`) loads shared libraries,
//! fixes addresses through relocations, and enters the executable's startup
//! code. That startup code eventually calls the application's `main`.
//!
//! We implement the mapping, stack, and entry steps after the kernel has
//! already executed our freestanding loader. We leave its mappings and signal
//! handlers in place. Calling another kernel exec here would discard them.
#![cfg(any(target_os = "linux", target_os = "none"))]
#![no_std]

#[cfg(test)]
extern crate std;

use fspy_nostd::{CStr, OwnedFd, Thin, env::AuxEntry};
use linux_raw_sys::auxvec::AT_PAGESZ;

mod image;
mod mapping;
mod stack;

/// Loads an ELF executable into the current process and enters it.
///
/// Takes ownership of a private executable descriptor and closes it after
/// mapping. Loads the executable and its `PT_INTERP`, builds an initial stack,
/// and transfers control to the interpreter entry point.
///
/// `execfn` is the original execution name used for `AT_EXECFN`; it need not
/// match `argv[0]`. Arguments and environment entries are copied to the new
/// stack. `inherited_auxv` borrows entries from the loader's original kernel
/// stack, excluding `AT_NULL`. Its `AT_PAGESZ` supplies the page size used for
/// mapping. Image-specific entries are replaced, reusable entries are
/// preserved, and referenced data is copied where needed. The resulting
/// vector ends with `AT_NULL`.
///
/// Path resolution, shebang handling, and execution-policy checks belong to
/// the caller. This function does not perform kernel exec cleanup: installed
/// signal handlers and their resident code and state must remain usable.
/// Loading failures terminate the process; they cannot return to the program
/// that executed the loader.
///
/// Currently supports native little-endian ELF64 dynamic executables on
/// `AArch64` and x86-64, with an ELF interpreter and a non-executable stack.
/// Both PIE (`ET_DYN`) and fixed-address (`ET_EXEC`) executables are supported.
/// Fixed-address images must not overlap any existing process mappings.
/// The new stack has a fixed 8 MiB capacity and a guard page. Existing loader
/// mappings, the old stack, and kernel process metadata (including `/proc`
/// identity and `brk`) are retained. This does not emulate a complete exec.
/// Processor-specific GNU properties are not applied.
///
/// # Safety
///
/// Call only from a fresh, single-threaded loader process. The executable and
/// interpreter must be trusted to run with the process's current privileges.
/// Their backing files must remain unchanged throughout loading. The loader
/// must not have initialized libc or thread-local runtime state.
/// Auxiliary entries must describe this process; any referenced memory must
/// remain readable and unchanged until copied or retained for the target.
/// The caller must permit execution to abandon its stack without running
/// destructors. Any retained handlers, mappings, or other process state must
/// remain valid while the target runs.
///
/// # Panics
///
/// Panics when `AT_PAGESZ` is missing or invalid, loading detects an unsupported
/// or malformed image, runs out of stack capacity, or fails to map or open a
/// file. The freestanding panic handler must terminate the process. Faults
/// after entering the interpreter or executable are not converted into panics.
pub unsafe fn exec_elf<'a>(
    elf_file: OwnedFd,
    execfn: CStr<'a, Thin>,
    argv: impl Iterator<Item = CStr<'a, Thin>>,
    envp: impl Iterator<Item = CStr<'a, Thin>>,
    inherited_auxv: &[AuxEntry],
) -> ! {
    let page_size = inherited_auxv
        .iter()
        .find(|entry| entry.key == usize::try_from(AT_PAGESZ).unwrap())
        .expect("missing AT_PAGESZ")
        .value;
    assert!(page_size.is_power_of_two(), "invalid AT_PAGESZ");
    let executable = image::File::new(elf_file);
    // PT_INTERP names a second ELF file. Loading it here lets us delegate
    // dynamic linking and libc startup to the program's own interpreter.
    let interpreter = executable.interpreter().expect("executable has no PT_INTERP");
    let interpreter = fspy_nostd::fs::openat(
        fspy_nostd::CWD,
        interpreter,
        fspy_nostd::fs::OFlags::RDONLY | fspy_nostd::fs::OFlags::CLOEXEC,
        fspy_nostd::fs::Mode::empty(),
    )
    .expect("cannot open ELF interpreter");
    let interpreter = image::File::new(interpreter);
    assert!(interpreter.interpreter().is_none(), "nested PT_INTERP is unsupported");
    let executable_image = executable.map(page_size);
    let interpreter_image = interpreter.map(page_size);
    // SAFETY: the caller supplies live auxiliary payloads from this process.
    let stack = unsafe {
        stack::build(
            execfn,
            argv,
            envp,
            inherited_auxv.iter().copied(),
            &executable_image,
            &interpreter_image,
            page_size,
        )
    };
    // The interpreter must run first, even though AT_ENTRY on the new stack
    // points at the executable. The interpreter uses that value to hand off
    // after dynamic linking; it does not call the application's main itself.
    let entry = interpreter_image.entry;
    // File mappings keep their backing storage alive after close. Release
    // only our parsing views and descriptors, then retain the execution maps
    // so Rust destructors cannot unmap code the target is about to use.
    drop(interpreter);
    drop(executable);
    executable_image.retain();
    interpreter_image.retain();
    // SAFETY: the images and stack are initialized and retained. The caller
    // permits abandoning the current stack and entering the trusted image.
    unsafe { enter(entry, stack) }
}

// No Rust code may execute after replacing SP. The dynamic interpreter is
// entered like a program, not called like a function with a return address.
// #[naked] prevents a compiler-generated stack frame. Each stub starts with
// C function arguments in registers, installs the process-entry stack, and
// jumps. A normal call would put an unwanted return address on x86-64's stack.
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
unsafe extern "C" fn enter(_entry: usize, _stack: *mut usize) -> ! {
    // X0 holds entry and X1 holds the new SP. Save entry in scratch register
    // X16 before clearing X0; clear X30 because there is no caller to return to.
    core::arch::naked_asm!("mov x16, x0", "mov sp, x1", "mov x0, xzr", "mov x30, xzr", "br x16");
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn enter(_entry: usize, _stack: *mut usize) -> ! {
    // RDI holds entry and RSI holds the new RSP. RDX is the optional
    // dynamic-linker finalizer (an exit callback); there is none yet.
    // Clear RBP to mark the end of the frame-pointer chain.
    core::arch::naked_asm!("mov rsp, rsi", "xor edx, edx", "xor ebp, ebp", "jmp rdi");
}
