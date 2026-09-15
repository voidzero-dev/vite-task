//! Recreate the startup stack normally supplied by Linux exec.
//!
//! At entry, the stack pointer (SP) points to argc (the argument count), then
//! argv (argument pointers) and NULL, envp (environment pointers) and NULL,
//! then auxiliary-vector (key, value) pairs through `AT_NULL`.
//! The auxiliary vector lets the interpreter and libc
//! discover image addresses and process properties before runtime startup.
//!
//! We copy strings and known auxiliary payloads into the same mapping. Build
//! the pointer table in a temporary vector and payloads downward, then copy
//! the table next to the payloads. Argument and environment pointers are
//! collected first so their strings can be copied in reverse order, preserving
//! Linux's ascending, contiguous string layout. This consumes iterators once.

use core::{mem::size_of, ptr};

use allocator_api2::vec::Vec;
use fspy_nostd::{
    CStr, Thin,
    env::AuxEntry,
    mm::{self, MprotectFlags},
};
use fspy_nostd_alloc::{PageBump, page_bump};
use linux_raw_sys::auxvec as abi;

use crate::{
    image::Image,
    mapping::{Mapping, add},
};

// Includes startup data and room for the target's downward-growing call stack.
// This MVP uses a fixed mapping rather than Linux's growable initial stack.
const CAPACITY: usize = 8 * 1024 * 1024;

struct Builder {
    mapping: Mapping,
    // Offsets from mapping.ptr: first usable byte and payload start.
    start: usize,
    words: Vec<usize, PageBump>,
    strings: usize,
}

impl Builder {
    fn new(page: usize) -> Self {
        let mapping = Mapping::reserve(add(CAPACITY, page));
        // Keep the low page inaccessible so an overflow reaching it faults
        // instead of writing into a neighboring mapping.
        // SAFETY: the first page remains a guard; the remaining pages are
        // exclusively owned and do not have live references.
        unsafe {
            mm::mprotect(
                mapping.ptr.as_ptr().add(page).cast(),
                CAPACITY,
                MprotectFlags::READ | MprotectFlags::WRITE,
            )
        }
        .expect("cannot map target stack");
        Self { strings: mapping.len, mapping, start: page, words: Vec::new_in(page_bump()) }
    }

    fn push(&mut self, value: usize) {
        let end = add(self.table_end(), size_of::<usize>());
        assert!(end <= self.strings, "initial stack exceeds capacity");
        self.words.push(value);
    }

    fn table_end(&self) -> usize {
        add(self.start, size_of_val(self.words.as_slice()))
    }

    fn copy(&mut self, bytes: &[u8]) -> usize {
        let start = self.strings.checked_sub(bytes.len()).expect("initial stack exceeds capacity");
        assert!(start >= self.table_end(), "initial stack exceeds capacity");
        // SAFETY: bytes come from the old process image, not this fresh
        // mapping. The destination is writable and outside the pointer table.
        let target = unsafe { self.mapping.ptr.as_ptr().add(start) };
        // SAFETY: source and destination are valid, disjoint byte ranges.
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len()) };
        self.strings = start;
        target.expose_provenance()
    }

    fn string(&mut self, value: CStr<'_, Thin>) -> usize {
        self.copy(value.count().as_units_with_nul())
    }

    fn args_envs<'a>(
        &mut self,
        argv: impl Iterator<Item = CStr<'a, Thin>>,
        envp: impl Iterator<Item = CStr<'a, Thin>>,
    ) {
        let argc_slot = self.words.len();
        self.push(0);
        let pointers_start = self.words.len();
        let mut argument_count = 0_usize;
        for arg in argv {
            self.push(arg.as_ptr().expose_provenance());
            argument_count = add(argument_count, 1);
        }
        self.words[argc_slot] = argument_count;
        self.push(0);
        for env in envp {
            self.push(env.as_ptr().expose_provenance());
        }
        self.push(0);

        // These slots temporarily point into the original strings. Copy from
        // the last environment entry back to argv[0], because allocation grows
        // downward. The result is contiguous argv strings followed by envp.
        // libuv relies on this layout when updating the process title.
        for index in (pointers_start..self.words.len()).rev() {
            let source = self.words[index];
            if source == 0 {
                continue;
            }
            // SAFETY: nonzero slots came from the CStr values above, whose
            // original storage remains valid for 'a while we copy it.
            let string = unsafe { CStr::from_ptr(ptr::with_exposed_provenance(source)) };
            self.words[index] = self.string(string);
        }
    }

    fn aux(&mut self, key: u32, value: usize) {
        self.push(usize::try_from(key).unwrap());
        self.push(value);
    }

    fn finish(self) -> *mut usize {
        // Final layout, from low to high addresses:
        // guard | free stack space | argc/argv/envp/auxv | padding | payloads
        //                           ^ new SP, aligned to 16 bytes
        // Keeping the table near the top leaves room below SP for calls. Only
        // table bytes move: its pointers already name the final payloads.
        let len = size_of_val(self.words.as_slice());
        let start = self.strings.checked_sub(len).expect("initial stack exceeds capacity") & !15;
        assert!(start >= self.start, "initial stack exceeds capacity");
        // The temporary vector and its owned bump are dropped before handoff.
        // SAFETY: the initialized vector and writable destination are disjoint.
        // The checked destination fits below the strings. The result is
        // aligned to the process-entry ABI and retains the complete mapping.
        unsafe {
            let base = self.mapping.ptr.as_ptr();
            let target = base.add(start);
            ptr::copy_nonoverlapping(self.words.as_ptr().cast::<u8>(), target, len);
            self.mapping.retain();
            target.cast()
        }
    }
}

pub unsafe fn build<'a>(
    execfn: CStr<'a, Thin>,
    argv: impl Iterator<Item = CStr<'a, Thin>>,
    envp: impl Iterator<Item = CStr<'a, Thin>>,
    inherited: impl Iterator<Item = AuxEntry>,
    image: &Image,
    interpreter: &Image,
    page: usize,
) -> *mut usize {
    let mut stack = Builder::new(page);
    stack.args_envs(argv, envp);
    for mut entry in inherited {
        let Ok(key) = u32::try_from(entry.key) else {
            stack.push(entry.key);
            stack.push(entry.value);
            continue;
        };
        match key {
            // Replace loader-image addresses below. AT_EXECFD would refer to
            // an old execution descriptor; omit it. Emit our own end marker.
            abi::AT_NULL
            | abi::AT_PHDR
            | abi::AT_PHENT
            | abi::AT_PHNUM
            | abi::AT_ENTRY
            | abi::AT_BASE
            | abi::AT_EXECFN
            | abi::AT_EXECFD
            | abi::AT_FLAGS => continue,
            abi::AT_RANDOM => {
                // The value is a pointer, not the random number itself. Copy
                // the kernel-provided bytes used by libc for stack canaries.
                // SAFETY: the exec_elf caller supplies the kernel's readable
                // 16-byte random payload, kept immutable while copying.
                let bytes = unsafe {
                    core::slice::from_raw_parts(ptr::with_exposed_provenance(entry.value), 16)
                };
                entry.value = stack.copy(bytes);
            }
            abi::AT_PLATFORM | abi::AT_BASE_PLATFORM => {
                // SAFETY: the caller supplies a live NUL-terminated auxiliary string.
                let string = unsafe { CStr::from_ptr(ptr::with_exposed_provenance(entry.value)) };
                entry.value = stack.string(string);
            }
            // Scalars such as page size and hardware capabilities still apply.
            // Unknown pointers remain usable only while their old mappings
            // survive. This includes the vDSO, kernel-provided userspace code.
            _ => {} // Old mappings, including the vDSO, remain resident.
        }
        stack.aux(key, entry.value);
    }
    // Describe the main executable even though we enter the interpreter first:
    // PHDR/PHENT/PHNUM locate and size its header table; ENTRY is its startup
    // address. BASE identifies the interpreter's load bias for its own linking.
    stack.aux(abi::AT_PHDR, image.phdr.expect("program headers are not mapped by PT_LOAD"));
    stack.aux(abi::AT_PHENT, 56);
    stack.aux(abi::AT_PHNUM, image.phnum);
    stack.aux(abi::AT_ENTRY, image.entry);
    stack.aux(abi::AT_BASE, interpreter.bias);
    stack.aux(abi::AT_FLAGS, 0);
    let execfn = stack.string(execfn);
    // Execution name and argv[0] are independent: callers can choose a custom
    // argv[0] without changing which pathname AT_EXECFN reports.
    stack.aux(abi::AT_EXECFN, execfn);
    stack.aux(abi::AT_NULL, 0);
    stack.finish()
}

#[cfg(test)]
mod tests {
    use fspy_nostd::env::InitialStack;

    use super::*;

    #[test]
    fn args_and_envs_are_contiguous_in_forward_order() {
        let args = [c"program", c"", c"\xff", c"longer-argument"];
        let envs = [c"EMPTY=", c"BYTES=\xff", c"A=B=C"];
        let page = fspy_nostd::param::page_size();
        for argc in 0..=args.len() {
            for envc in 0..=envs.len() {
                let mut builder = Builder::new(page);
                let thin = |value: &'static core::ffi::CStr| {
                    CStr::from_units_with_nul(value.to_bytes_with_nul()).unwrap().as_thin()
                };
                builder.args_envs(
                    args[..argc].iter().copied().map(thin),
                    envs[..envc].iter().copied().map(thin),
                );
                builder.aux(abi::AT_NULL, 0);
                let ptr = builder.mapping.ptr;
                let len = builder.mapping.len;
                let sp = builder.finish();
                // Reclaim the mapping retained by finish after all views expire.
                let _mapping = Mapping { ptr, len };
                assert!(sp.addr().is_multiple_of(16));
                // SAFETY: the builder initialized a complete initial stack;
                // its mapping stays readable and unchanged throughout this scope.
                let stack = unsafe { InitialStack::from_ptr(sp) };
                assert_eq!(stack.args().len(), argc);
                assert_eq!(stack.envs().len(), envc);
                let mut previous_end = None;
                for (actual, expected) in stack
                    .args()
                    .iter()
                    .chain(stack.envs())
                    .zip(args[..argc].iter().chain(&envs[..envc]))
                {
                    let actual = actual.count();
                    assert_eq!(actual.as_units_with_nul(), expected.to_bytes_with_nul());
                    let start = actual.as_ptr().addr();
                    if let Some(end) = previous_end {
                        assert_eq!(start, end, "argv and envp strings must be contiguous");
                    }
                    previous_end = Some(add(start, actual.as_units_with_nul().len()));
                }
            }
        }
    }
}
