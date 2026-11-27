//! Recreate the startup stack normally supplied by Linux exec.
//!
//! At entry, the stack pointer (SP) points to argc (the argument count), then
//! argv (argument pointers) and NULL, envp (environment pointers) and NULL,
//! then auxiliary-vector (key, value) pairs through `AT_NULL`.
//! The auxiliary vector lets the interpreter and libc
//! discover image addresses and process properties before runtime startup.
//!
//! We copy strings and known auxiliary payloads into the same mapping. Stage
//! the pointer table upward from the bottom of the writable region and payloads
//! downward from the top, then shift the table up next to the payloads.
//! Argument and environment pointers are collected first so their strings can
//! be copied in reverse order, preserving Linux's ascending, contiguous string
//! layout. This consumes iterators once without any heap allocator.

use core::{
    mem::{MaybeUninit, align_of, size_of},
    ptr,
};

use fspy_nostd::{
    CStr, Thin,
    env::AuxEntry,
    mm::{self, MprotectFlags},
};
use linux_raw_sys::{
    auxvec as abi,
    general::{RLIM_INFINITY, RLIMIT_STACK, rlimit64},
};
use syscalls::Sysno;

use crate::{
    image::{Image, PHDR_SIZE},
    mapping::{Mapping, add, align_up, sub},
};

// Clamp target stack capacity derived from `RLIMIT_STACK` to `[8 MiB, 64 MiB]`.
const MIN_CAPACITY: usize = 8 * 1024 * 1024;
const MAX_CAPACITY: usize = 64 * 1024 * 1024;
// Reserve at least 64 KiB below the initial SP above the guard page so a huge
// argv/envp fails loudly during loading instead of immediately SIGSEGVing at
// process entry.
const MIN_STACK_HEADROOM: usize = 64 * 1024;

const RLIM_INFINITY_U64: u64 = (RLIM_INFINITY as i64).cast_unsigned();

/// Computes the target stack capacity from an `RLIMIT_STACK` soft limit,
/// treating `RLIM_INFINITY` as 64 MiB, rounding up to `page`, and clamping to
/// `[8 MiB, 64 MiB]`.
const fn clamp_stack_capacity(rlim_cur: u64, page: usize) -> usize {
    if rlim_cur == RLIM_INFINITY_U64 {
        return MAX_CAPACITY;
    }
    if rlim_cur >= MAX_CAPACITY as u64 {
        return MAX_CAPACITY;
    }
    #[expect(clippy::cast_possible_truncation, reason = "bounded by MAX_CAPACITY above")]
    let raw = rlim_cur as usize;
    if raw <= MIN_CAPACITY {
        return MIN_CAPACITY;
    }
    let aligned = align_up(raw, page);
    if aligned > MAX_CAPACITY { MAX_CAPACITY } else { aligned }
}

/// Queries the current process's `RLIMIT_STACK` soft limit via `prlimit64` and
/// returns the clamped stack capacity, falling back to 8 MiB on error.
fn query_stack_capacity(page: usize) -> usize {
    let mut rlim = MaybeUninit::<rlimit64>::zeroed();
    // SAFETY: `pid = 0` queries the calling process, `new_limit` is NULL, and
    // `rlim` points to writable `rlimit64` storage.
    let res = unsafe {
        syscalls::syscall!(Sysno::prlimit64, 0_usize, RLIMIT_STACK, 0_usize, rlim.as_mut_ptr())
    };
    match res {
        Ok(_) => {
            // SAFETY: a successful `prlimit64` initialized `rlim`.
            let rlim = unsafe { rlim.assume_init() };
            clamp_stack_capacity(rlim.rlim_cur, page)
        }
        Err(_) => MIN_CAPACITY,
    }
}

struct Builder {
    mapping: Mapping,
    // Offsets from mapping.ptr: first usable byte, staged word count, and payload start.
    start: usize,
    word_count: usize,
    strings: usize,
}

impl Builder {
    fn new(page: usize, exec_stack: bool) -> Self {
        let capacity = query_stack_capacity(page);
        Self::with_capacity(page, capacity, exec_stack)
    }

    /// Maps the stack read-write, plus execute when the executable's
    /// `PT_GNU_STACK` asks for it, as Linux does.
    fn with_capacity(page: usize, capacity: usize, exec_stack: bool) -> Self {
        assert!(page.is_multiple_of(align_of::<usize>()), "invalid page size");
        assert!(capacity.is_multiple_of(page), "stack capacity must be page-aligned");
        let mapping = Mapping::reserve(add(capacity, page));
        let mut protection = MprotectFlags::READ | MprotectFlags::WRITE;
        if exec_stack {
            protection |= MprotectFlags::EXEC;
        }
        // Keep the low page inaccessible so an overflow reaching it faults
        // instead of writing into a neighboring mapping.
        // SAFETY: the first page remains a guard; the remaining pages are
        // exclusively owned and do not have live references.
        unsafe { mm::mprotect(mapping.ptr.as_ptr().add(page).cast(), capacity, protection) }
            .expect("cannot map target stack");
        Self { strings: mapping.len, mapping, start: page, word_count: 0 }
    }

    const fn table_len(&self) -> usize {
        self.word_count.checked_mul(size_of::<usize>()).expect("initial stack exceeds capacity")
    }

    const fn table_end(&self) -> usize {
        add(self.start, self.table_len())
    }

    #[expect(clippy::cast_ptr_alignment, reason = "mapping base and start offset are page-aligned")]
    const unsafe fn words_ptr(&self) -> *mut usize {
        // SAFETY: `self.start` is a page boundary inside `self.mapping`,
        // which is strictly aligned for `usize`.
        unsafe { self.mapping.ptr.as_ptr().add(self.start).cast::<usize>() }
    }

    const fn words_mut(&mut self) -> &mut [usize] {
        // SAFETY: `self.words_ptr()` is `usize`-aligned, and the staged words
        // were exclusively written by `push`, disjoint from `[self.strings, mapping.len)`.
        unsafe { core::slice::from_raw_parts_mut(self.words_ptr(), self.word_count) }
    }

    fn push(&mut self, value: usize) {
        let end = add(self.table_end(), size_of::<usize>());
        assert!(end <= self.strings, "initial stack exceeds capacity");
        // SAFETY: `self.word_count` lies inside the writable staged region
        // below `self.strings` and is unaliased.
        unsafe { self.words_ptr().add(self.word_count).write(value) };
        self.word_count = add(self.word_count, 1);
    }

    fn copy(&mut self, bytes: &[u8]) -> usize {
        self.copy_aligned(bytes, 1)
    }

    fn copy_aligned(&mut self, bytes: &[u8], align: usize) -> usize {
        assert!(align.is_power_of_two(), "invalid stack payload alignment");
        let start = self.strings.checked_sub(bytes.len()).expect("initial stack exceeds capacity")
            & !(align - 1);
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
        let argc_slot = self.word_count;
        self.push(0);
        let pointers_start = self.word_count;
        let mut argument_count = 0_usize;
        for arg in argv {
            self.push(arg.as_ptr().expose_provenance());
            argument_count = add(argument_count, 1);
        }
        self.words_mut()[argc_slot] = argument_count;
        self.push(0);
        for env in envp {
            self.push(env.as_ptr().expose_provenance());
        }
        self.push(0);

        // These slots temporarily point into the original strings. Copy from
        // the last environment entry back to argv[0], because allocation grows
        // downward. The result is contiguous argv strings followed by envp.
        // libuv relies on this layout when updating the process title.
        for index in (pointers_start..self.word_count).rev() {
            let source = self.words_mut()[index];
            if source == 0 {
                continue;
            }
            // SAFETY: nonzero slots came from the CStr values above, whose
            // original storage remains valid for 'a while we copy it.
            let string = unsafe { CStr::from_ptr(ptr::with_exposed_provenance(source)) };
            self.words_mut()[index] = self.string(string);
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
        let len = self.table_len();
        let start = self.strings.checked_sub(len).expect("initial stack exceeds capacity") & !15;
        assert!(start >= add(self.start, MIN_STACK_HEADROOM), "initial stack exceeds capacity");
        // SAFETY: both staged source and final destination lie inside the
        // writable region of `self.mapping` below `self.strings`; `ptr::copy`
        // handles any overlap. Zero the vacated staging prefix below SP.
        unsafe {
            let base = self.mapping.ptr.as_ptr();
            let source = base.add(self.start);
            let target = base.add(start);
            ptr::copy(source, target, len);
            ptr::write_bytes(source, 0, len.min(sub(start, self.start)));
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
    interpreter: Option<&Image>,
    page: usize,
) -> *mut usize {
    let mut stack = Builder::new(page, image.exec_stack);
    stack.args_envs(argv, envp);
    for mut entry in inherited {
        let Ok(key) = u32::try_from(entry.key) else {
            stack.push(entry.key);
            stack.push(entry.value);
            continue;
        };
        match key {
            abi::AT_NULL => break,
            // Replace loader-image addresses below. AT_EXECFD would refer to
            // an old execution descriptor; omit it. Emit our own end marker.
            abi::AT_PHDR
            | abi::AT_PHENT
            | abi::AT_PHNUM
            | abi::AT_ENTRY
            | abi::AT_BASE
            | abi::AT_EXECFN
            | abi::AT_EXECFD
            | abi::AT_FLAGS => continue,
            abi::AT_RANDOM => {
                assert!(entry.value != 0, "null AT_RANDOM");
                // The value is a pointer, not the random number itself. Copy
                // the kernel-provided bytes used by libc for stack canaries.
                // SAFETY: the exec_elf caller supplies the kernel's readable
                // 16-byte random payload, kept immutable while copying.
                let bytes = unsafe {
                    core::slice::from_raw_parts(ptr::with_exposed_provenance(entry.value), 16)
                };
                entry.value = stack.copy_aligned(bytes, 16);
            }
            abi::AT_PLATFORM | abi::AT_BASE_PLATFORM => {
                assert!(entry.value != 0, "null auxiliary string");
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
    // PHDR/PHENT/PHNUM locate and size the executable's header table; ENTRY
    // is its startup address. With PT_INTERP we enter the interpreter first
    // and BASE is that image's load bias. Without one, BASE is 0 as on Linux.
    stack.aux(abi::AT_PHDR, image.phdr);
    stack.aux(abi::AT_PHENT, PHDR_SIZE);
    stack.aux(abi::AT_PHNUM, image.phnum);
    stack.aux(abi::AT_ENTRY, image.entry);
    stack.aux(abi::AT_BASE, interpreter.map_or(0, |image| image.bias));
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
    fn clamp_stack_capacity_bounds_and_aligns_rlimit_values() {
        let page = 4096;
        assert_eq!(clamp_stack_capacity(0, page), MIN_CAPACITY);
        assert_eq!(clamp_stack_capacity(1024 * 1024, page), MIN_CAPACITY);
        assert_eq!(clamp_stack_capacity(MIN_CAPACITY as u64, page), MIN_CAPACITY);
        assert_eq!(clamp_stack_capacity(MIN_CAPACITY as u64 + 1, page), MIN_CAPACITY + page);
        assert_eq!(clamp_stack_capacity(16 * 1024 * 1024, page), 16 * 1024 * 1024);
        assert_eq!(clamp_stack_capacity(MAX_CAPACITY as u64, page), MAX_CAPACITY);
        assert_eq!(clamp_stack_capacity(128 * 1024 * 1024, page), MAX_CAPACITY);
        assert_eq!(clamp_stack_capacity(RLIM_INFINITY_U64, page), MAX_CAPACITY);
    }

    #[test]
    fn maps_the_stack_executable_only_when_asked() {
        let page = fspy_nostd::param::page_size();
        for (exec_stack, expected) in [(false, "rw-p"), (true, "rwxp")] {
            let builder = Builder::with_capacity(page, MIN_CAPACITY, exec_stack);
            let usable = builder.mapping.ptr.as_ptr().addr() + page;
            let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
            let protection = maps
                .lines()
                .find_map(|line| {
                    let mut fields = line.split_whitespace();
                    let (start, end) = fields.next()?.split_once('-')?;
                    let range = usize::from_str_radix(start, 16).ok()?
                        ..usize::from_str_radix(end, 16).ok()?;
                    range.contains(&usable).then(|| fields.next().unwrap())
                })
                .unwrap();
            assert_eq!(protection, expected);
        }
    }

    #[test]
    fn args_and_envs_are_contiguous_in_forward_order() {
        let args = [c"program", c"", c"\xff", c"longer-argument"];
        let envs = [c"EMPTY=", c"BYTES=\xff", c"A=B=C"];
        let page = fspy_nostd::param::page_size();
        for argc in 0..=args.len() {
            for envc in 0..=envs.len() {
                let mut builder = Builder::with_capacity(page, MIN_CAPACITY, false);
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

    #[test]
    fn stops_at_at_null_and_aligns_at_random() {
        let page = fspy_nostd::param::page_size();
        let random = [0x5a_u8; 16];
        let image = Image::test_stub(page);
        let thin = |value: &'static core::ffi::CStr| {
            CStr::from_units_with_nul(value.to_bytes_with_nul()).unwrap().as_thin()
        };
        let inherited = [
            AuxEntry { key: usize::try_from(abi::AT_PAGESZ).unwrap(), value: page },
            AuxEntry {
                key: usize::try_from(abi::AT_RANDOM).unwrap(),
                value: random.as_ptr().expose_provenance(),
            },
            AuxEntry { key: usize::try_from(abi::AT_NULL).unwrap(), value: 0 },
            // Trailing entry past AT_NULL must not be emitted.
            AuxEntry { key: 0x1234, value: 0xdead },
        ];
        // SAFETY: synthetic image/auxv pointers stay valid throughout build.
        let sp = unsafe {
            build(
                thin(c"prog"),
                [thin(c"a")].into_iter(),
                core::iter::empty(),
                inherited.into_iter(),
                &image,
                None,
                page,
            )
        };
        // SAFETY: build returned a complete initial stack.
        let stack = unsafe { InitialStack::from_ptr(sp) };
        let aux_random = stack
            .auxv()
            .iter()
            .find(|entry| entry.key == usize::try_from(abi::AT_RANDOM).unwrap())
            .unwrap()
            .value;
        assert_eq!(aux_random % 16, 0, "AT_RANDOM should be 16-byte aligned");
        assert!(
            !stack.auxv().iter().any(|entry| entry.key == 0x1234),
            "entries after AT_NULL must not be emitted"
        );
    }

    #[test]
    #[should_panic(expected = "initial stack exceeds capacity")]
    fn rejects_stack_without_call_headroom() {
        let page = fspy_nostd::param::page_size();
        let mut builder = Builder::with_capacity(page, MIN_CAPACITY, false);
        let huge = std::vec![b'x'; MIN_CAPACITY - MIN_STACK_HEADROOM / 2];
        let _ = builder.copy(&huge);
        let _ = builder.finish();
    }
}
