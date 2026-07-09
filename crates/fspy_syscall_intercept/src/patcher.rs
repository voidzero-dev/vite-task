/*
 * This module rewrites selected syscall instructions that are already mapped
 * into the process. No object file on disk is modified. The high-level flow is:
 *
 *   1. Ask the dynamic loader for every ELF image currently in memory.
 *   2. Read each image's ELF file to find named functions that may issue one of
 *      the filesystem syscalls understood by the caller.
 *   3. Let the architecture module identify instruction sequences that can be
 *      replaced without splitting an instruction or changing copied code.
 *   4. Allocate a new executable region near the original instructions and
 *      write one trampoline per accepted site into it.
 *   5. Replace the original syscall with a short branch to its trampoline.
 *
 * A trampoline is a small generated function. It saves enough machine state to
 * call the shared dispatcher, executes the syscall through either the allowed
 * or fallback gate selected by that dispatcher, replays any instruction bytes
 * displaced by the branch, and then resumes the original function.
 *
 * ELF files describe addresses relative to an image. ASLR chooses a different
 * base address when the image is loaded; `load_bias + symbol.address()` converts
 * an ELF symbol address into the live process address. Program headers describe
 * mapped segments, while symbols and relocations help prove that replacing a
 * byte range will not strand a known branch target in the middle of the patch.
 *
 * Code pages are normally readable and executable, not writable. Installation
 * follows W^X discipline: generated trampolines are written as RW and changed
 * to RX; original text is changed from RX to RW only for the copy itself and is
 * immediately restored to RX. Raw mapping syscalls are used so patching libc's
 * own wrappers cannot remove execute permission from code the patcher is using.
 */

use std::{
    ffi::{CStr, OsStr},
    os::unix::ffi::OsStrExt as _,
};

use object::{
    Object as _, ObjectSection as _, ObjectSymbol, Relocation, RelocationTarget, SymbolKind,
};

use super::arch;

#[derive(Clone, Debug)]
pub struct PatchSite {
    /// Live address at which the original syscall instruction begins.
    pub address: usize,
    /// Number of original bytes replaced by the branch instruction.
    pub overwrite_length: usize,
    /// Complete instructions after the syscall that must run in the trampoline.
    pub displaced: Vec<u8>,
    /// Page protection restored after writing the replacement branch.
    pub restore_protection: libc::c_int,
}

#[derive(Clone, Copy)]
struct ExecutableSegment {
    start: usize,
    end: usize,
    protection: libc::c_int,
}

struct Image {
    load_bias: usize,
    path: Vec<u8>,
    segments: Vec<ExecutableSegment>,
}

pub fn patch_initial_elf_mappings() {
    // `dl_iterate_phdr` runs the callback while loader metadata is stable. Copy
    // only addresses and paths here; parsing files and generating code happens
    // after the callback returns, outside the loader callback.
    let mut images = Vec::new();
    // SAFETY: the callback copies every borrowed loader value into `images`
    // before returning. It never unwinds across the C boundary.
    unsafe {
        libc::dl_iterate_phdr(
            Some(collect_image),
            std::ptr::from_mut(&mut images).cast::<libc::c_void>(),
        );
    }
    for image in images {
        patch_image(&image);
    }
}

unsafe extern "C" fn collect_image(
    info: *mut libc::dl_phdr_info,
    _size: libc::size_t,
    data: *mut libc::c_void,
) -> libc::c_int {
    if info.is_null() || data.is_null() {
        return 0;
    }
    // SAFETY: dl_iterate_phdr provides valid pointers for the callback duration.
    let info = unsafe { &*info };
    // SAFETY: `data` was created from `&mut Vec<Image>` in the caller.
    let images = unsafe { &mut *data.cast::<Vec<Image>>() };
    let phnum = info.dlpi_phnum as usize;
    if info.dlpi_phdr.is_null() {
        return 0;
    }
    // SAFETY: the loader reports `dlpi_phnum` valid program headers.
    let headers = unsafe { std::slice::from_raw_parts(info.dlpi_phdr, phnum) };
    let Ok(load_bias) = usize::try_from(info.dlpi_addr) else {
        return 0;
    };
    let mut segments = Vec::new();
    for header in headers {
        if header.p_type != libc::PT_LOAD
            || header.p_flags & libc::PF_X == 0
            || header.p_flags & libc::PF_R == 0
            || header.p_flags & libc::PF_W != 0
        {
            continue;
        }
        let Ok(virtual_address) = usize::try_from(header.p_vaddr) else {
            continue;
        };
        let Ok(memory_size) = usize::try_from(header.p_memsz) else {
            continue;
        };
        let Some(start) = load_bias.checked_add(virtual_address) else {
            continue;
        };
        let Some(end) = start.checked_add(memory_size) else {
            continue;
        };
        if start == end {
            continue;
        }
        let protection = libc::PROT_READ | libc::PROT_EXEC;
        segments.push(ExecutableSegment { start, end, protection });
    }
    if segments.is_empty() {
        return 0;
    }

    // Never rewrite the preload DSO itself: it contains the fallback gate and
    // dispatcher, and recursive patching would invalidate the escape path.
    let own_code = arch::dispatch_entry_address();
    if segments.iter().any(|segment| (segment.start..segment.end).contains(&own_code)) {
        return 0;
    }

    let path = if info.dlpi_name.is_null() {
        b"/proc/self/exe".to_vec()
    } else {
        // SAFETY: dlpi_name is a loader-owned C string for this callback.
        let name = unsafe { CStr::from_ptr(info.dlpi_name) }.to_bytes();
        if name.is_empty() { b"/proc/self/exe".to_vec() } else { name.to_vec() }
    };
    images.push(Image { load_bias, path, segments });
    0
}

fn patch_image(image: &Image) {
    let Ok(file_bytes) = std::fs::read(OsStr::from_bytes(&image.path)) else {
        return;
    };
    let Ok(file) = object::File::parse(file_bytes.as_slice()) else {
        return;
    };
    if !file.is_little_endian() || !target_architecture_matches(file.architecture()) {
        return;
    }
    let mut known_targets = collect_relocation_targets(&file, image.load_bias);
    known_targets.extend(collect_known_code_targets(&file, &file_bytes, image));
    known_targets.sort_unstable();
    known_targets.dedup();

    let mut sites = Vec::new();
    for symbol in file.symbols().chain(file.dynamic_symbols()) {
        if symbol.kind() != SymbolKind::Text
            || !symbol.is_definition()
            || symbol.size() == 0
            || !symbol.name_bytes().is_ok_and(is_monitored_symbol)
        {
            continue;
        }
        let Ok(symbol_address) = usize::try_from(symbol.address()) else {
            continue;
        };
        let Ok(symbol_size) = usize::try_from(symbol.size()) else {
            continue;
        };
        let Some(start) = image.load_bias.checked_add(symbol_address) else {
            continue;
        };
        let Some(end) = start.checked_add(symbol_size) else {
            continue;
        };
        let Some(segment) =
            image.segments.iter().find(|segment| start >= segment.start && end <= segment.end)
        else {
            continue;
        };
        let Some(bytes) = symbol_file_bytes(&file, &file_bytes, &symbol) else {
            continue;
        };
        let mut function_sites = arch::collect_sites(start, bytes);
        function_sites.retain(|site| {
            let Some(displaced_start) = site.address.checked_add(arch::SYSCALL_INSTRUCTION_LENGTH)
            else {
                return false;
            };
            let Some(displaced_end) = site.address.checked_add(site.overwrite_length) else {
                return false;
            };
            !known_targets.iter().any(|target| (displaced_start..displaced_end).contains(target))
        });
        for site in &mut function_sites {
            site.restore_protection = segment.protection;
        }
        sites.extend(function_sites);
    }

    sites.sort_unstable_by_key(|site| site.address);
    sites.dedup_by_key(|site| site.address);
    let mut previous_end = 0;
    sites.retain(|site| {
        let Some(end) = site.address.checked_add(site.overwrite_length) else {
            return false;
        };
        if site.address < previous_end {
            return false;
        }
        previous_end = end;
        true
    });
    if sites.is_empty() {
        return;
    }
    install_patches(&sites);
}

fn is_monitored_symbol(name: &[u8]) -> bool {
    matches!(
        name,
        b"open"
            | b"open64"
            | b"openat"
            | b"openat64"
            | b"__open"
            | b"__open64"
            | b"__open_2"
            | b"__open64_2"
            | b"__open_nocancel"
            | b"__open64_nocancel"
            | b"__openat_2"
            | b"__openat64_2"
            | b"access"
            | b"faccessat"
            | b"stat"
            | b"lstat"
            | b"fstatat"
            | b"statx"
            | b"getdents"
            | b"getdents64"
            | b"__getdents"
            | b"__getdents64"
            | b"execve"
            | b"execveat"
            | b"fspy_test_accelerated_openat"
    ) || name.starts_with(b"__xstat")
        || name.starts_with(b"__lxstat")
        || name.starts_with(b"__fxstatat")
}

fn collect_known_code_targets(
    file: &object::File<'_>,
    file_bytes: &[u8],
    image: &Image,
) -> Vec<usize> {
    let mut targets = file
        .symbols()
        .chain(file.dynamic_symbols())
        .filter(ObjectSymbol::is_definition)
        .filter_map(|symbol| image.load_bias.checked_add(usize::try_from(symbol.address()).ok()?))
        .collect::<Vec<_>>();
    for symbol in file.symbols().chain(file.dynamic_symbols()) {
        if symbol.kind() != SymbolKind::Text || !symbol.is_definition() || symbol.size() == 0 {
            continue;
        }
        let Ok(symbol_address) = usize::try_from(symbol.address()) else {
            continue;
        };
        let Ok(symbol_size) = usize::try_from(symbol.size()) else {
            continue;
        };
        let Some(start) = image.load_bias.checked_add(symbol_address) else {
            continue;
        };
        let Some(end) = start.checked_add(symbol_size) else {
            continue;
        };
        if !image.segments.iter().any(|segment| start >= segment.start && end <= segment.end) {
            continue;
        }
        let Some(bytes) = symbol_file_bytes(file, file_bytes, &symbol) else {
            continue;
        };
        targets.extend(arch::collect_known_direct_targets(start, bytes));
    }
    targets
}

fn collect_relocation_targets(file: &object::File<'_>, load_bias: usize) -> Vec<usize> {
    let mut targets = Vec::new();
    for section in file.sections() {
        for (_, relocation) in section.relocations() {
            if let Some(target) = relocation_target(file, load_bias, &relocation) {
                targets.push(target);
            }
        }
    }
    if let Some(relocations) = file.dynamic_relocations() {
        for (_, relocation) in relocations {
            if let Some(target) = relocation_target(file, load_bias, &relocation) {
                targets.push(target);
            }
        }
    }
    targets.sort_unstable();
    targets.dedup();
    targets
}

fn relocation_target(
    file: &object::File<'_>,
    load_bias: usize,
    relocation: &Relocation,
) -> Option<usize> {
    let base = match relocation.target() {
        RelocationTarget::Symbol(index) => file.symbol_by_index(index).ok()?.address(),
        RelocationTarget::Section(index) => file.section_by_index(index).ok()?.address(),
        RelocationTarget::Absolute => 0,
        _ => return None,
    };
    let target = base.checked_add_signed(relocation.addend())?;
    load_bias.checked_add(usize::try_from(target).ok()?)
}

fn symbol_file_bytes<'data, S>(
    file: &object::File<'data>,
    file_bytes: &'data [u8],
    symbol: &S,
) -> Option<&'data [u8]>
where
    S: ObjectSymbol<'data>,
{
    let section_index = symbol.section_index()?;
    let Ok(section) = file.section_by_index(section_index) else {
        return None;
    };
    let (section_file_offset, section_file_size) = section.file_range()?;
    let symbol_section_offset = symbol.address().checked_sub(section.address())?;
    let symbol_section_end = symbol_section_offset.checked_add(symbol.size())?;
    if symbol_section_end > section_file_size {
        return None;
    }
    let file_start = section_file_offset.checked_add(symbol_section_offset)?;
    let file_end = file_start.checked_add(symbol.size())?;
    let Ok(file_start) = usize::try_from(file_start) else {
        return None;
    };
    let Ok(file_end) = usize::try_from(file_end) else {
        return None;
    };
    file_bytes.get(file_start..file_end)
}

fn target_architecture_matches(architecture: object::Architecture) -> bool {
    #[cfg(target_arch = "aarch64")]
    return architecture == object::Architecture::Aarch64;
    #[cfg(target_arch = "x86_64")]
    return architecture == object::Architecture::X86_64;
}

fn install_patches(sites: &[PatchSite]) {
    /*
     * All trampolines for one image share a newly allocated region:
     *
     *   [ trampoline 0 ][ padding ][ trampoline 1 ] ... [ page padding ]
     *
     * The branch instruction available at an original site has a limited
     * displacement. `map_near_sites` therefore tries candidate mappings around
     * the image and accepts one only if every site can encode a branch to its
     * assigned trampoline. Generation is completed while the region is RW;
     * only after every trampoline and site-branch byte sequence is valid does
     * the region become RX and installation begin.
     */
    let Some(page_size) = page_size() else {
        return;
    };
    let mut offsets = Vec::with_capacity(sites.len());
    let mut used = 0usize;
    for site in sites {
        used = align_up(used, 16);
        offsets.push(used);
        let Some(next) = used.checked_add(arch::trampoline_size(site)) else {
            return;
        };
        used = next;
    }
    let Some(mapping_size) = align_up_checked(used, page_size) else {
        return;
    };
    let Some(first_site) = sites.first() else {
        return;
    };
    let Some(last_site) = sites.last() else {
        return;
    };
    let Some(mapping) = map_near_sites(
        first_site.address,
        last_site.address,
        mapping_size,
        page_size,
        sites,
        &offsets,
    ) else {
        return;
    };

    let mut site_patches = Vec::with_capacity(sites.len());
    for (site, offset) in sites.iter().zip(&offsets) {
        let trampoline = mapping + offset;
        // SAFETY: the mapping is writable and `offset + size` was included in
        // the checked `used` total above.
        let output = unsafe {
            std::slice::from_raw_parts_mut(trampoline as *mut u8, arch::trampoline_size(site))
        };
        if arch::emit_trampoline(output, trampoline, site).is_none() {
            // SAFETY: this mapping was created by map_near_sites.
            unsafe { raw_munmap(mapping as *mut libc::c_void, mapping_size) };
            return;
        }
        let Some(patch) = arch::encode_site_branch(site, trampoline) else {
            // SAFETY: this mapping was created by map_near_sites.
            unsafe { raw_munmap(mapping as *mut libc::c_void, mapping_size) };
            return;
        };
        site_patches.push(patch);
    }

    // SAFETY: the emitted range is within the writable trampoline mapping.
    unsafe { arch::clear_cache(mapping, mapping + used) };
    // SAFETY: transition the private mapping from RW to RX; it is never RWX.
    if unsafe {
        raw_mprotect(mapping as *mut libc::c_void, mapping_size, libc::PROT_READ | libc::PROT_EXEC)
    } != 0
    {
        // SAFETY: this mapping was created by map_near_sites.
        unsafe { raw_munmap(mapping as *mut libc::c_void, mapping_size) };
        return;
    }

    for (site, patch) in sites.iter().zip(site_patches) {
        if !patch_site(site, &patch, page_size) {
            // Already-installed sites remain valid and conservative. Stopping
            // avoids touching more text after an unexpected mprotect failure.
            break;
        }
    }
}

fn map_near_sites(
    first: usize,
    last: usize,
    mapping_size: usize,
    page_size: usize,
    sites: &[PatchSite],
    offsets: &[usize],
) -> Option<usize> {
    let after = align_up_checked(last.checked_add(page_size)?, page_size)?;
    let before = first.checked_sub(mapping_size + page_size).map(|value| value & !(page_size - 1));
    let step = 1024 * 1024;

    for attempt in 0..256usize {
        for hint in [
            after.checked_add(attempt.checked_mul(step)?)?,
            before.and_then(|base| base.checked_sub(attempt.checked_mul(step)?)).unwrap_or(0),
        ] {
            if hint == 0 {
                continue;
            }
            // A non-fixed hint cannot replace an existing mapping. Its actual
            // result is accepted only after every architecture branch checks.
            // SAFETY: mmap receives no borrowed pointers, and a non-fixed hint
            // cannot replace an existing target mapping.
            let mapping = unsafe {
                raw_mmap(
                    hint as *mut libc::c_void,
                    mapping_size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            if mapping == libc::MAP_FAILED {
                continue;
            }
            let address = mapping as usize;
            let in_range = sites.iter().zip(offsets).all(|(site, offset)| {
                address
                    .checked_add(*offset)
                    .is_some_and(|trampoline| arch::encode_site_branch(site, trampoline).is_some())
            });
            if in_range {
                return Some(address);
            }
            // SAFETY: the mapping was returned by mmap above.
            unsafe { raw_munmap(mapping, mapping_size) };
        }
    }
    None
}

fn patch_site(site: &PatchSite, patch: &[u8], page_size: usize) -> bool {
    const MAX_PATCH_SIZE: usize = 32;

    if patch.len() != site.overwrite_length || patch.len() > MAX_PATCH_SIZE {
        return false;
    }
    // Discovery used immutable ELF bytes and may race file replacement or
    // process mutation. Re-read this small live range immediately before
    // changing page permissions and require it to match the expected syscall
    // plus displaced instructions.
    let mut original = [0; MAX_PATCH_SIZE];
    // SAFETY: PF_R was required and the patch range was checked inside the
    // readable executable segment while collecting the site.
    unsafe {
        std::ptr::copy_nonoverlapping(
            site.address as *const u8,
            original.as_mut_ptr(),
            patch.len(),
        );
    }
    let instruction_length = arch::ALLOWED_GATE_INSTRUCTION.len();
    if patch.len() != instruction_length + site.displaced.len()
        || original[..instruction_length] != *arch::ALLOWED_GATE_INSTRUCTION
        || original[instruction_length..patch.len()] != site.displaced
    {
        return false;
    }
    let page_start = site.address & !(page_size - 1);
    let Some(site_end) = site.address.checked_add(site.overwrite_length) else {
        return false;
    };
    let Some(page_end) = align_up_checked(site_end, page_size) else {
        return false;
    };
    let length = page_end - page_start;
    // Remove execute permission while writing to preserve W^X.
    // SAFETY: the page-aligned range belongs to the executable PT_LOAD mapping
    // that was bounds-checked while collecting this site.
    if unsafe {
        raw_mprotect(page_start as *mut libc::c_void, length, libc::PROT_READ | libc::PROT_WRITE)
    } != 0
    {
        return false;
    }
    // SAFETY: mprotect made the complete patch range writable.
    unsafe {
        std::ptr::copy_nonoverlapping(patch.as_ptr(), site.address as *mut u8, patch.len());
        arch::clear_cache(site.address, site_end);
    }
    // SAFETY: restore the PT_LOAD protection recorded by the loader scan.
    if unsafe { raw_mprotect(page_start as *mut libc::c_void, length, site.restore_protection) }
        == 0
    {
        return true;
    }

    // A failed final transition should leave the mapping writable. Restore the
    // original instructions before retrying the original protection so a
    // caller never observes a half-installed branch if recovery succeeds.
    // SAFETY: the first mprotect succeeded, and a failed mprotect leaves the
    // prior protection in place.
    unsafe {
        std::ptr::copy_nonoverlapping(original.as_ptr(), site.address as *mut u8, patch.len());
        arch::clear_cache(site.address, site_end);
    }
    // SAFETY: best-effort retry of the original segment protection after the
    // original bytes have been restored.
    let _ =
        unsafe { raw_mprotect(page_start as *mut libc::c_void, length, site.restore_protection) };
    false
}

fn page_size() -> Option<usize> {
    usize::try_from(
        // SAFETY: `sysconf` has no pointer preconditions.
        unsafe { libc::sysconf(libc::_SC_PAGESIZE) },
    )
    .ok()
    .filter(|size| size.is_power_of_two())
}

unsafe fn raw_mmap(
    address: *mut libc::c_void,
    length: usize,
    protection: libc::c_int,
    flags: libc::c_int,
    fd: libc::c_int,
    offset: libc::off_t,
) -> *mut libc::c_void {
    let Ok(number) = usize::try_from(libc::SYS_mmap) else {
        return libc::MAP_FAILED;
    };
    // SAFETY: the caller supplies mmap-compatible arguments.
    let result = unsafe {
        arch::raw_syscall6(
            number,
            address as usize,
            length,
            usize::try_from(protection).unwrap_or_default(),
            usize::try_from(flags).unwrap_or_default(),
            usize::from_ne_bytes(i64::from(fd).to_ne_bytes()),
            usize::from_ne_bytes(offset.to_ne_bytes()),
        )
    };
    if (-4095..0).contains(&result) { libc::MAP_FAILED } else { result as *mut libc::c_void }
}

unsafe fn raw_mprotect(
    address: *mut libc::c_void,
    length: usize,
    protection: libc::c_int,
) -> libc::c_int {
    let Ok(number) = usize::try_from(libc::SYS_mprotect) else {
        return -1;
    };
    // SAFETY: the caller supplies a mapped, page-aligned range.
    let result = unsafe {
        arch::raw_syscall6(
            number,
            address as usize,
            length,
            usize::try_from(protection).unwrap_or_default(),
            0,
            0,
            0,
        )
    };
    if result < 0 { -1 } else { libc::c_int::try_from(result).unwrap_or(-1) }
}

unsafe fn raw_munmap(address: *mut libc::c_void, length: usize) -> libc::c_int {
    let Ok(number) = usize::try_from(libc::SYS_munmap) else {
        return -1;
    };
    // SAFETY: the caller supplies a mapping previously returned by mmap.
    let result = unsafe { arch::raw_syscall6(number, address as usize, length, 0, 0, 0, 0) };
    if result < 0 { -1 } else { libc::c_int::try_from(result).unwrap_or(-1) }
}

const fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

fn align_up_checked(value: usize, alignment: usize) -> Option<usize> {
    Some(value.checked_add(alignment - 1)? & !(alignment - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_alignment_rejects_overflow() {
        assert_eq!(align_up_checked(1, 4096), Some(4096));
        assert_eq!(align_up_checked(4096, 4096), Some(4096));
        assert_eq!(align_up_checked(usize::MAX, 4096), None);
    }

    #[test]
    fn branch_limit_constant_matches_architecture_encoder() {
        let site = PatchSite {
            address: 0x1_0000_0000,
            overwrite_length: if cfg!(target_arch = "x86_64") { 5 } else { 4 },
            displaced: Vec::new(),
            restore_protection: libc::PROT_READ | libc::PROT_EXEC,
        };
        assert!(
            arch::encode_site_branch(&site, site.address + arch::MAX_BRANCH_DISTANCE).is_some()
        );
    }
}
