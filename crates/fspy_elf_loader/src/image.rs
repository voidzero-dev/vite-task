//! Map the regions described by an ELF file's program headers.
//!
//! Each `PT_LOAD` header describes a segment: `p_offset` locates its bytes in
//! the file, `p_vaddr` gives its link-time virtual address, `p_filesz` counts
//! file bytes, and `p_memsz` counts memory bytes. The extra memory is zero-filled
//! storage (BSS), used for objects such as zero-initialized global variables.
//! `p_flags` gives read/write/execute permissions; `p_align` constrains placement.
//!
//! See ELF's program-header definitions in <https://man7.org/linux/man-pages/man5/elf.5.html>.

use core::{ops::Range, ptr};

use elf::{
    abi,
    endian::LittleEndian,
    file::{self, Class, FileHeader},
    segment::{ProgramHeader, SegmentTable},
};
use fspy_nostd::{
    CStr, Fat, OwnedFd, fs,
    mm::{self, MapFlags, MprotectFlags, ProtFlags},
};

use crate::mapping::{Mapping, add, align_up, sub, word};

#[cfg(test)]
mod tests;

#[cfg(target_arch = "aarch64")]
const MACHINE: u16 = abi::EM_AARCH64;
#[cfg(target_arch = "x86_64")]
const MACHINE: u16 = abi::EM_X86_64;

/// A temporary read-only view used to parse file offsets, not to execute code.
pub struct File {
    fd: OwnedFd,
    view: Mapping,
    header: FileHeader<LittleEndian>,
    headers: Range<usize>,
}

/// The final memory layout, with addresses the target can use at runtime.
pub struct Image {
    mapping: Mapping,
    /// Add this load bias to an ELF virtual address to get its runtime address.
    pub bias: usize,
    pub entry: usize,
    /// Address of the program-header table inside the mapped image, if present.
    pub phdr: Option<usize>,
    pub phnum: usize,
}

impl Image {
    pub fn retain(self) {
        self.mapping.retain();
    }
}

impl File {
    pub fn new(fd: OwnedFd) -> Self {
        let len = usize::try_from(fs::fstat(fd.as_fd()).expect("cannot stat ELF").st_size)
            .expect("invalid ELF file size");
        assert!(len >= 64, "truncated ELF header");
        let view = Mapping::file(fd.as_fd(), len);
        // SAFETY: this is the unchanged, read-only file view.
        let bytes = unsafe { view.bytes() };
        // The 16-byte identity tells us how to decode the rest of the 64-byte
        // ELF64 header. Section headers describe linker/debugger information;
        // loading these images needs only the separate program-header table.
        let ident = file::parse_ident::<LittleEndian>(&bytes[..16]).expect("invalid ELF identity");
        assert_eq!(ident.1, Class::ELF64, "only ELF64 is supported");
        let header = FileHeader::parse_tail(ident, &bytes[16..64]).expect("invalid ELF header");
        assert_eq!(header.e_machine, MACHINE, "ELF architecture mismatch");
        // ET_DYN permits a load bias chosen for the available address space.
        // Position-independent executables (PIE) and their interpreters use it.
        // ET_EXEC instead uses its declared addresses with no load bias.
        assert!(
            matches!(header.e_type, abi::ET_DYN | abi::ET_EXEC),
            "only ET_DYN and ET_EXEC images are supported"
        );
        assert_eq!(header.version, u32::from(abi::EV_CURRENT), "invalid ELF version");
        assert_eq!(header.e_phentsize, 56, "invalid program-header size");
        // 0xffff is an escape to a count stored in section metadata. This MVP
        // accepts only counts carried in the ELF header itself.
        assert!(
            header.e_phnum != 0 && header.e_phnum != 0xffff,
            "unsupported program-header count"
        );
        let start = word(header.e_phoff);
        let end = add(start, usize::from(header.e_phnum) * 56);
        assert!(bytes.get(start..end).is_some(), "truncated program headers");
        Self { fd, view, header, headers: start..end }
    }

    fn segments(&self) -> SegmentTable<'_, LittleEndian> {
        // SAFETY: the file remains unchanged and mapped read-only.
        SegmentTable::new(
            LittleEndian,
            Class::ELF64,
            &unsafe { self.view.bytes() }[self.headers.clone()],
        )
    }

    pub fn interpreter(&self) -> Option<CStr<'_, Fat>> {
        let mut result = None;
        for segment in self.segments().iter() {
            if segment.p_type == abi::PT_INTERP {
                assert!(result.is_none(), "multiple PT_INTERP entries");
                let start = word(segment.p_offset);
                let end = add(start, word(segment.p_filesz));
                // SAFETY: the file remains unchanged and mapped read-only.
                let bytes =
                    unsafe { self.view.bytes() }.get(start..end).expect("truncated PT_INTERP");
                // Linux requires only that the last byte is NUL, then treats
                // the slice as a C string. Linkers may pad with extra NULs.
                assert!(bytes.last() == Some(&0), "unterminated PT_INTERP");
                let len = bytes.iter().position(|byte| *byte == 0).unwrap() + 1;
                let path =
                    CStr::from_units_with_nul(&bytes[..len]).expect("invalid PT_INTERP path");
                assert!(!path.as_units().is_empty(), "empty PT_INTERP path");
                result = Some(path);
            }
        }
        result
    }

    fn assert_disjoint_load(&self, index: usize, start: usize, end: usize) {
        for (other_index, other) in self.segments().iter().enumerate() {
            if other_index > index && other.p_type == abi::PT_LOAD && other.p_memsz != 0 {
                let other_start = word(other.p_vaddr);
                let other_end = add(other_start, word(other.p_memsz));
                assert!(end <= other_start || other_end <= start, "overlapping ELF segments");
            }
        }
    }

    pub fn map(&self, page: usize) -> Image {
        // First validate the segments and find their page-rounded span.
        // mmap operates on whole pages, even when ELF segments do not.
        let mut low = usize::MAX;
        let mut high = 0;
        let mut alignment = page;
        for (index, segment) in self.segments().iter().enumerate() {
            if segment.p_type == abi::PT_GNU_STACK {
                assert!(segment.p_flags & abi::PF_X == 0, "executable stacks are unsupported");
            }
            if segment.p_type == abi::PT_LOAD {
                let start = word(segment.p_vaddr);
                let end = add(start, word(segment.p_memsz));
                let file_end = add(word(segment.p_offset), word(segment.p_filesz));
                assert!(segment.p_filesz <= segment.p_memsz, "ELF filesz exceeds memsz");
                assert!(file_end <= self.view.len, "truncated ELF segment");
                // Byte ranges must not overlap. Header order need not match
                // vaddr order; mapping below still walks headers so a later
                // MAP_FIXED can share a page with an earlier load.
                if segment.p_memsz != 0 {
                    self.assert_disjoint_load(index, start, end);
                    low = low.min(start & !sub(page, 1));
                    high = high.max(align_up(end, page));
                }
                // ELF treats alignment 0 and 1 as no extra constraint. File
                // and memory offsets must agree within a page for mmap.
                let align = word(segment.p_align).max(1);
                assert!(align.is_power_of_two(), "invalid segment alignment");
                assert_eq!(
                    start % align,
                    word(segment.p_offset) % align,
                    "incongruent ELF alignment"
                );
                assert_eq!(
                    start % page,
                    word(segment.p_offset) % page,
                    "incongruent ELF page offsets"
                );
                alignment = alignment.max(align);
            }
        }
        let span = high.checked_sub(low).expect("ELF has no loadable memory");
        // Reserve the whole image, including holes, before replacing pages
        // with MAP_FIXED. That flag discards existing mappings at its address;
        // restricting it to our reservation protects the loader and old stack.
        let (mapping, bias) = if self.header.e_type == abi::ET_EXEC {
            // Fixed-address executables cannot move. Refuse collisions rather
            // than replacing resident loader code or other process state.
            (Mapping::reserve_at(low, span), 0)
        } else {
            // Extra space aligns the bias even when p_align exceeds a page.
            let mapping = Mapping::reserve(add(span, alignment));
            let base = mapping.ptr.as_ptr().expose_provenance();
            let bias =
                align_up(base.checked_sub(low).expect("unsupported ELF address range"), alignment);
            assert!(add(bias, high) <= add(base, mapping.len), "ELF exceeds reservation");
            (mapping, bias)
        };
        let mut phdr = None;
        let mut executable_entry = false;
        // All segments share one bias, preserving their relative addresses.
        // The executable and interpreter each get their own reservation/bias.
        for segment in
            self.segments().iter().filter(|ph| ph.p_type == abi::PT_LOAD && ph.p_memsz != 0)
        {
            let file_start = word(segment.p_offset);
            // AT_PHDR must point into the final image, not our temporary file
            // view. Find the LOAD covering the table and translate its offset.
            // This also works when the ELF has no explicit PT_PHDR header.
            if self.headers.start >= file_start
                && self.headers.end <= add(file_start, word(segment.p_filesz))
            {
                phdr = Some(add(
                    add(bias, word(segment.p_vaddr)),
                    sub(self.headers.start, file_start),
                ));
            }
            executable_entry |= segment.p_flags & abi::PF_X != 0
                && self.header.e_entry >= segment.p_vaddr
                && self.header.e_entry
                    < segment
                        .p_vaddr
                        .checked_add(segment.p_memsz)
                        .expect("ELF entry range overflow");
            // SAFETY: validation and reservation cover every page this
            // segment replaces. No references to those pages exist yet.
            unsafe { self.map_segment(segment, bias, page) };
        }
        assert!(executable_entry, "ELF entry is not in an executable segment");
        Image {
            mapping,
            bias,
            entry: add(bias, word(self.header.e_entry)),
            phdr,
            phnum: usize::from(self.header.e_phnum),
        }
    }

    unsafe fn map_segment(&self, segment: ProgramHeader, bias: usize, page: usize) {
        let start = add(bias, word(segment.p_vaddr));
        let end = add(start, word(segment.p_memsz));
        let file_end = add(start, word(segment.p_filesz));
        let page_start = start & !sub(page, 1);
        let mut protection = ProtFlags::empty();
        if segment.p_flags & abi::PF_R != 0 {
            protection |= ProtFlags::READ;
        }
        if segment.p_flags & abi::PF_W != 0 {
            protection |= ProtFlags::WRITE;
        }
        if segment.p_flags & abi::PF_X != 0 {
            protection |= ProtFlags::EXEC;
        }
        let fixed = MapFlags::PRIVATE | MapFlags::FIXED;
        // Linux elf_map skips a zero-length mapping, which happens only when
        // filesz is 0 and vaddr is page-aligned. A zero-filesz load that
        // starts mid-page still maps that shared page from the file so a
        // later MAP_FIXED does not anonymous-wipe the previous load's bytes.
        let file_len = sub(align_up(file_end, page), page_start);
        if file_len != 0 {
            // Round both the file offset and address down by the same amount.
            // For example, an offset ending in 0x340 needs those first 0x340
            // bytes of its page mapped too. Round the end up to cover the tail.
            // PRIVATE keeps BSS writes and later relocations out of the file.
            // SAFETY: caller owns the entire reservation being replaced.
            unsafe {
                mm::mmap(
                    ptr::with_exposed_provenance_mut(page_start),
                    file_len,
                    protection,
                    fixed,
                    self.fd.as_fd(),
                    segment.p_offset & !u64::try_from(sub(page, 1)).unwrap(),
                )
            }
            .expect("cannot map ELF segment");
            // Zero through the page boundary (Linux padzero) when the segment
            // has BSS (memsz > filesz) or is writable (glibc's early allocator
            // reads spare bytes after the interpreter's _end even when
            // filesz == memsz). Skip filesz == memsz on read-only/executable
            // segments so we do not COW-dirty code/rodata pages or trigger
            // RW -> RX mprotect transitions under W^X policies.
            //
            // Clamp the zeroed range to the start of any higher-vaddr LOAD
            // sharing this page so unordered segments cannot wipe each other.
            let pad_zero = segment.p_memsz > segment.p_filesz || segment.p_flags & abi::PF_W != 0;
            if pad_zero && !file_end.is_multiple_of(page) {
                let tail_page = file_end & !sub(page, 1);
                let mut zero_end = align_up(file_end, page);
                for other in self.segments().iter() {
                    if other.p_type == abi::PT_LOAD && other.p_memsz != 0 {
                        let other_start = add(bias, word(other.p_vaddr));
                        if other_start >= end && other_start < zero_end {
                            zero_end = other_start;
                        }
                    }
                }
                if zero_end > file_end {
                    // Writable, non-executable pages can be zeroed directly.
                    // Otherwise temporarily make the last file page writable,
                    // without X, and restore its original protection afterwards.
                    let change_protection = !protection.contains(ProtFlags::WRITE)
                        || protection.contains(ProtFlags::EXEC);
                    // SAFETY: this private page was mapped above; it has no live references.
                    unsafe {
                        if change_protection {
                            mm::mprotect(
                                ptr::with_exposed_provenance_mut(tail_page),
                                page,
                                MprotectFlags::READ | MprotectFlags::WRITE,
                            )
                            .expect("cannot zero ELF BSS");
                        }
                        ptr::write_bytes(
                            ptr::with_exposed_provenance_mut::<u8>(file_end),
                            0,
                            sub(zero_end, file_end),
                        );
                        if change_protection {
                            mm::mprotect(
                                ptr::with_exposed_provenance_mut(tail_page),
                                page,
                                MprotectFlags::from_bits_retain(protection.bits()),
                            )
                            .expect("cannot restore ELF protection");
                        }
                    }
                }
            }
        }
        let anonymous_start = align_up(file_end, page);
        let anonymous_end = align_up(end, page);
        if anonymous_start < anonymous_end {
            // Remaining BSS pages have no file bytes. Anonymous pages start
            // at zero; mapping the file past its end could instead SIGBUS.
            // SAFETY: these pages lie inside the reservation and have no live references.
            unsafe {
                mm::mmap_anonymous(
                    ptr::with_exposed_provenance_mut(anonymous_start),
                    sub(anonymous_end, anonymous_start),
                    protection,
                    fixed,
                )
            }
            .expect("cannot map ELF BSS");
        }
    }
}
