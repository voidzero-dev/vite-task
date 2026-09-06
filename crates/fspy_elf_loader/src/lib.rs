#![no_std]

use fspy_nostd::{CStr, OwnedFd, Thin};

/// Loads an ELF executable into the current process and enters it.
///
/// Takes ownership of a private executable descriptor and closes it after
/// mapping. Loads the executable and its `PT_INTERP`, builds an initial stack,
/// and transfers control to the interpreter or executable entry point.
///
/// `execfn` is the original execution name used for `AT_EXECFN`; it need not
/// match `argv[0]`. Arguments and environment entries are copied to the new
/// stack. `inherited_auxv` comes from the loader's kernel exec: image-specific
/// entries are replaced, reusable entries are preserved, and referenced data
/// is copied where needed. The resulting vector ends with `AT_NULL`.
///
/// Path resolution, shebang handling, and execution-policy checks belong to
/// the caller. This function does not perform kernel exec cleanup: installed
/// signal handlers and their resident code and state must remain usable.
/// Loading failures terminate the process; they cannot return to the program
/// that executed the loader.
///
/// # Safety
///
/// Call only from a fresh, single-threaded loader process. The executable and
/// interpreter must be trusted to run with the process's current privileges.
/// Auxiliary entries must describe this process; any referenced memory must
/// remain readable and unchanged until copied or retained for the target.
/// The caller must permit execution to abandon its stack without running
/// destructors. Any retained handlers, mappings, or other process state must
/// remain valid while the target runs.
///
/// # Panics
///
/// Not implemented yet; currently always panics.
pub unsafe fn exec_elf<'a>(
    elf_file: OwnedFd,
    execfn: CStr<'a, Thin>,
    argv: impl Iterator<Item = CStr<'a, Thin>>,
    envp: impl Iterator<Item = CStr<'a, Thin>>,
    inherited_auxv: impl Iterator<Item = (usize, usize)>,
) -> ! {
    let _ = (elf_file, execfn, argv, envp, inherited_auxv);
    unimplemented!()
}
