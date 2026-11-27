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
    BorrowedFd, CStr, Error, Fat, Result, fs,
    mm::{self, MapFlags, MprotectFlags, ProtFlags},
};

use crate::mapping::{Mapping, add, align_up, sub, try_align_up, word};

#[cfg(test)]
mod tests;

pub const EHDR_SIZE: usize = 64;
pub const PHDR_SIZE: usize = 56;

#[cfg(target_arch = "aarch64")]
const MACHINE: u16 = abi::EM_AARCH64;
#[cfg(target_arch = "x86_64")]
const MACHINE: u16 = abi::EM_X86_64;

/// A temporary read-only view used to parse file offsets, not to execute code.
pub struct File<'fd> {
    fd: BorrowedFd<'fd>,
    view: Mapping,
    header: FileHeader<LittleEndian>,
    headers: Range<usize>,
}

/// Validated page-rounded address span and metadata for an ELF image.
#[derive(Clone, Copy, Debug)]
pub struct ValidatedLayout {
    pub low: usize,
    pub span: usize,
    pub alignment: usize,
    /// The last `PT_GNU_STACK` asks for an executable stack (`PF_X`).
    pub exec_stack: bool,
}

/// The final memory layout, with addresses the target can use at runtime.
pub struct Image {
    mapping: Mapping,
    /// Add this load bias to an ELF virtual address to get its runtime address.
    pub bias: usize,
    pub entry: usize,
    /// Address of the program-header table inside the mapped image (`AT_PHDR`).
    pub phdr: usize,
    pub phnum: usize,
    /// The image's `PT_GNU_STACK` asks for an executable stack.
    pub exec_stack: bool,
}

impl Image {
    pub fn retain(self) {
        self.mapping.retain();
    }

    #[cfg(test)]
    pub fn test_stub(page: usize) -> Self {
        Self {
            mapping: Mapping::reserve(page),
            bias: 0,
            entry: 0x1000,
            phdr: 0x40,
            phnum: 1,
            exec_stack: false,
        }
    }
}

impl<'fd> File<'fd> {
    /// Parses and validates the ELF64 header and program-header table bounds
    /// without taking ownership of `fd`.
    pub fn open(fd: BorrowedFd<'fd>) -> Result<Self> {
        let meta = fs::fstat(fd)?;
        if !meta.is_regular_file() {
            return Err(Error::ACCES);
        }
        let len = usize::try_from(meta.st_size).map_err(|_| Error::NOEXEC)?;
        if len < EHDR_SIZE {
            return Err(Error::NOEXEC);
        }
        let view = Mapping::try_file(fd, len)?;
        // SAFETY: this is the unchanged, read-only file view.
        let bytes = unsafe { view.bytes() };
        // The 16-byte identity tells us how to decode the rest of the 64-byte
        // ELF64 header. Section headers describe linker/debugger information;
        // loading these images needs only the separate program-header table.
        let ident = file::parse_ident::<LittleEndian>(&bytes[..16]).map_err(|_| Error::NOEXEC)?;
        if ident.1 != Class::ELF64 {
            return Err(Error::NOEXEC);
        }
        let header =
            FileHeader::parse_tail(ident, &bytes[16..EHDR_SIZE]).map_err(|_| Error::NOEXEC)?;
        if header.e_machine != MACHINE {
            return Err(Error::NOEXEC);
        }
        // ET_DYN permits a load bias chosen for the available address space.
        // Position-independent executables (PIE) and their interpreters use it.
        // ET_EXEC instead uses its declared addresses with no load bias.
        if !matches!(header.e_type, abi::ET_DYN | abi::ET_EXEC) {
            return Err(Error::NOEXEC);
        }
        if header.version != u32::from(abi::EV_CURRENT) {
            return Err(Error::NOEXEC);
        }
        if usize::from(header.e_phentsize) != PHDR_SIZE {
            return Err(Error::NOEXEC);
        }
        // 0xffff is an escape to a count stored in section metadata. We accept
        // only counts carried in the ELF header itself.
        if header.e_phnum == 0 || header.e_phnum == 0xffff {
            return Err(Error::NOEXEC);
        }
        let start = usize::try_from(header.e_phoff).map_err(|_| Error::NOEXEC)?;
        let table_len = usize::from(header.e_phnum).checked_mul(PHDR_SIZE).ok_or(Error::NOEXEC)?;
        let end = start.checked_add(table_len).ok_or(Error::NOEXEC)?;
        if bytes.get(start..end).is_none() {
            return Err(Error::NOEXEC);
        }
        Ok(Self { fd, view, header, headers: start..end })
    }

    fn segments(&self) -> SegmentTable<'_, LittleEndian> {
        // SAFETY: the file remains unchanged and mapped read-only.
        SegmentTable::new(
            LittleEndian,
            Class::ELF64,
            &unsafe { self.view.bytes() }[self.headers.clone()],
        )
    }

    /// Returns the first `PT_INTERP` path; like Linux, later ones are ignored.
    pub fn interpreter(&self) -> Result<Option<CStr<'_, Fat>>> {
        for segment in self.segments().iter() {
            if segment.p_type == abi::PT_INTERP {
                let start = usize::try_from(segment.p_offset).map_err(|_| Error::NOEXEC)?;
                let len = usize::try_from(segment.p_filesz).map_err(|_| Error::NOEXEC)?;
                let end = start.checked_add(len).ok_or(Error::NOEXEC)?;
                // SAFETY: the file remains unchanged and mapped read-only.
                let bytes = unsafe { self.view.bytes() }.get(start..end).ok_or(Error::NOEXEC)?;
                // Linux requires only that the last byte is NUL, then treats
                // the slice as a C string. Linkers may pad with extra NULs.
                if bytes.last() != Some(&0) {
                    return Err(Error::NOEXEC);
                }
                let nul_len = bytes.iter().position(|byte| *byte == 0).ok_or(Error::NOEXEC)? + 1;
                let path = CStr::from_units_with_nul(&bytes[..nul_len]).ok_or(Error::NOEXEC)?;
                if path.as_units().is_empty() {
                    return Err(Error::NOEXEC);
                }
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    /// Validates the `PT_LOAD` headers (bounds, page congruence, and entry
    /// point) and records `PT_GNU_STACK`, without mapping any execution
    /// segments. Like Linux `binfmt_elf`, overlapping loads, non-power-of-two
    /// `p_align`, and an executable stack are accepted.
    pub fn validate_segments(&self, page: usize) -> Result<ValidatedLayout> {
        if !page.is_power_of_two() {
            return Err(Error::INVAL);
        }
        let mut low = usize::MAX;
        let mut high = 0_usize;
        let mut alignment = page;
        let mut exec_stack = false;
        let mut executable_entry = false;

        for segment in self.segments().iter() {
            if segment.p_type == abi::PT_GNU_STACK {
                exec_stack = (segment.p_flags & abi::PF_X) != 0;
            }
            if segment.p_type == abi::PT_LOAD {
                let start = usize::try_from(segment.p_vaddr).map_err(|_| Error::NOEXEC)?;
                let memsz = usize::try_from(segment.p_memsz).map_err(|_| Error::NOEXEC)?;
                let filesz = usize::try_from(segment.p_filesz).map_err(|_| Error::NOEXEC)?;
                let offset = usize::try_from(segment.p_offset).map_err(|_| Error::NOEXEC)?;
                let end = start.checked_add(memsz).ok_or(Error::NOEXEC)?;
                let file_end = offset.checked_add(filesz).ok_or(Error::NOEXEC)?;
                if segment.p_filesz > segment.p_memsz || file_end > self.view.len {
                    return Err(Error::NOEXEC);
                }
                // Header order need not match vaddr order; mapping below walks
                // headers so a later MAP_FIXED wins where loads share pages.
                if segment.p_memsz != 0 {
                    low = low.min(start & !(page - 1));
                    let aligned_end = try_align_up(end, page).ok_or(Error::NOEXEC)?;
                    high = high.max(aligned_end);
                    let entry_end =
                        segment.p_vaddr.checked_add(segment.p_memsz).ok_or(Error::NOEXEC)?;
                    executable_entry |= (segment.p_flags & abi::PF_X) != 0
                        && self.header.e_entry >= segment.p_vaddr
                        && self.header.e_entry < entry_end;
                }
                // ELF treats alignment 0 and 1 as no extra constraint, and
                // Linux ignores a non-power-of-two `p_align`. File and memory
                // offsets must agree within a page for mmap; like Linux
                // binfmt_elf, do not reject binaries (e.g. patchelf output)
                // that are page-congruent but incongruent mod p_align.
                let align = usize::try_from(segment.p_align).map_err(|_| Error::NOEXEC)?;
                if start % page != offset % page {
                    return Err(Error::NOEXEC);
                }
                if align.is_power_of_two() {
                    alignment = alignment.max(align);
                }
            }
        }

        let span = high.checked_sub(low).ok_or(Error::NOEXEC)?;
        if span == 0 || isize::try_from(span).is_err() {
            return Err(Error::NOEXEC);
        }
        if self.header.e_type == abi::ET_EXEC && low == 0 {
            return Err(Error::NOEXEC);
        }
        if self.header.e_type == abi::ET_DYN {
            let reserved = span.checked_add(alignment).ok_or(Error::NOEXEC)?;
            if isize::try_from(reserved).is_err() {
                return Err(Error::NOEXEC);
            }
        }
        if !executable_entry {
            return Err(Error::NOEXEC);
        }
        Ok(ValidatedLayout { low, span, alignment, exec_stack })
    }

    pub fn map(&self, page: usize, layout: &ValidatedLayout) -> Image {
        // Reserve the whole image, including holes, before replacing pages
        // with MAP_FIXED. That flag discards existing mappings at its address;
        // restricting it to our reservation protects the loader and old stack.
        let (mapping, bias) = if self.header.e_type == abi::ET_EXEC {
            // Fixed-address executables cannot move. Refuse collisions rather
            // than replacing resident loader code or other process state.
            (Mapping::reserve_at(layout.low, layout.span), 0)
        } else {
            // Extra space aligns the load bias even when `p_align` exceeds a
            // page or `layout.low > base`. Compute `mapped_low` inside the
            // reservation with `mapped_low % alignment == layout.low % alignment`
            // so `bias = mapped_low.wrapping_sub(layout.low)` is aligned to
            // `layout.alignment` and never underflows.
            let mapping = Mapping::reserve(add(layout.span, layout.alignment));
            let base = mapping.ptr.as_ptr().expose_provenance();
            let desired_mod = layout.low % layout.alignment;
            let cur_mod = base % layout.alignment;
            let delta = (desired_mod + layout.alignment - cur_mod) % layout.alignment;
            let mapped_low = add(base, delta);
            let bias = mapped_low.wrapping_sub(layout.low);
            assert!(
                add(mapped_low, layout.span) <= add(base, mapping.len),
                "ELF exceeds reservation"
            );
            (mapping, bias)
        };
        // Like Linux, AT_PHDR is the table's address in the LOAD whose file
        // bytes contain the program-header table, or the load bias when no LOAD
        // does. This also works when the ELF has no explicit PT_PHDR header.
        let mut phdr = bias;
        // All segments share one bias, preserving their relative addresses.
        // The executable and interpreter each get their own reservation/bias.
        for segment in
            self.segments().iter().filter(|ph| ph.p_type == abi::PT_LOAD && ph.p_memsz != 0)
        {
            let file_start = word(segment.p_offset);
            if let Some(file_end) = file_start.checked_add(word(segment.p_filesz))
                && self.headers.start >= file_start
                && self.headers.end <= file_end
            {
                phdr = add(
                    bias.wrapping_add(word(segment.p_vaddr)),
                    sub(self.headers.start, file_start),
                );
            }
            // SAFETY: validation and reservation cover every page this
            // segment replaces. No references to those pages exist yet.
            unsafe { self.map_segment(segment, bias, page) };
        }
        Image {
            mapping,
            bias,
            entry: bias.wrapping_add(word(self.header.e_entry)),
            phdr,
            phnum: usize::from(self.header.e_phnum),
            exec_stack: layout.exec_stack,
        }
    }

    unsafe fn map_segment(&self, segment: ProgramHeader, bias: usize, page: usize) {
        let start = bias.wrapping_add(word(segment.p_vaddr));
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
                    self.fd,
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
                        let other_start = bias.wrapping_add(word(other.p_vaddr));
                        if other_start > start && other_start >= file_end && other_start < zero_end
                        {
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

/// Validates an executable descriptor and its `PT_INTERP` (if present) using
/// the same rules as [`crate::exec_elf`], without mapping execution segments or
/// closing `fd`.
pub fn validate_exec_fd(fd: BorrowedFd<'_>, page: usize) -> Result<()> {
    let executable = File::open(fd)?;
    let _ = executable.validate_segments(page)?;
    if let Some(interp_path) = executable.interpreter()? {
        let interp_fd = fs::openat(
            fspy_nostd::CWD,
            interp_path,
            fs::OFlags::RDONLY | fs::OFlags::CLOEXEC,
            fs::Mode::empty(),
        )?;
        // Like Linux, accept an ET_EXEC interpreter and ignore its own
        // PT_INTERP and PT_GNU_STACK.
        let interp = File::open(interp_fd.as_fd())?;
        let _ = interp.validate_segments(page)?;
    }
    Ok(())
}
