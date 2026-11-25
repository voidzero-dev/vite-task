//! Load a Linux ELF executable inside an existing process.
//!
//! For a dynamic executable, the kernel maps the executable and its ELF
//! interpreter, writes startup data onto a new stack, and enters the
//! interpreter. The interpreter (such as `ld-linux`) loads shared libraries,
//! fixes addresses through relocations, and enters the executable's startup
//! code. That startup code eventually calls the application's `main`.
//!
//! For a static executable there is no `PT_INTERP`. The kernel maps only the
//! executable and enters its `e_entry`. `AT_BASE` is 0. Static-PIE images are
//! `ET_DYN` without an interpreter; their CRT applies `RELATIVE` relocations.
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
/// mapping. Loads the executable and, when present, its `PT_INTERP`. Builds an
/// initial stack and transfers control to the interpreter, or to the
/// executable when there is no interpreter.
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
/// Currently supports native little-endian ELF64 executables on `AArch64` and
/// x86-64, with a non-executable stack. Both PIE (`ET_DYN`) and fixed-address
/// (`ET_EXEC`) images are supported, with or without `PT_INTERP`.
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
    // Seed fspy_nostd's page-size cache from AT_PAGESZ so temporary stack
    // allocations via PageBump do not trigger a redundant 2 MiB probe mapping.
    fspy_nostd::param::seed_page_size(page_size);
    let executable = image::File::new(elf_file);
    // PT_INTERP names a second ELF file. Loading it here lets us delegate
    // dynamic linking and libc startup to the program's own interpreter.
    // Static and static-PIE images omit it; the executable is the startup image.
    let interpreter = executable.interpreter().map(|path| {
        let fd = fspy_nostd::fs::openat(
            fspy_nostd::CWD,
            path,
            fspy_nostd::fs::OFlags::RDONLY | fspy_nostd::fs::OFlags::CLOEXEC,
            fspy_nostd::fs::Mode::empty(),
        )
        .expect("cannot open ELF interpreter");
        let file = image::File::new(fd);
        assert!(file.interpreter().is_none(), "nested PT_INTERP is unsupported");
        file
    });
    let executable_image = executable.map(page_size);
    let interpreter_image = interpreter.as_ref().map(|file| file.map(page_size));
    // SAFETY: the caller supplies live auxiliary payloads from this process.
    let stack = unsafe {
        stack::build(
            execfn,
            argv,
            envp,
            inherited_auxv.iter().copied(),
            &executable_image,
            interpreter_image.as_ref(),
            page_size,
        )
    };
    // With PT_INTERP, the interpreter runs first even though AT_ENTRY points
    // at the executable. It uses that value to hand off after linking.
    // Without PT_INTERP, AT_BASE is 0 and the executable's e_entry starts.
    let entry = interpreter_image.as_ref().map_or(executable_image.entry, |image| image.entry);
    // File mappings keep their backing storage alive after close. Release
    // only our parsing views and descriptors, then retain the execution maps
    // so Rust destructors cannot unmap code the target is about to use.
    drop(interpreter);
    drop(executable);
    executable_image.retain();
    if let Some(image) = interpreter_image {
        image.retain();
    }
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
    // X0 holds entry and X1 holds the new SP. Linux start_thread zeros GPRs;
    // clear the ones this stub dirties. X16 must keep the entry for BR.
    core::arch::naked_asm!(
        "mov x16, x0",
        "mov sp, x1",
        "mov x0, xzr",
        "mov x1, xzr",
        "mov x29, xzr",
        "mov x30, xzr",
        "br x16",
    );
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn enter(_entry: usize, _stack: *mut usize) -> ! {
    // RDI holds entry and RSI holds the new RSP. RDX is the optional
    // dynamic-linker finalizer (an exit callback); there is none yet.
    // Clear RBP to mark the end of the frame-pointer chain. RSI held SP.
    // RDI must keep the entry for JMP.
    core::arch::naked_asm!(
        "mov rsp, rsi",
        "xor esi, esi",
        "xor edx, edx",
        "xor ebp, ebp",
        "jmp rdi"
    );
}
