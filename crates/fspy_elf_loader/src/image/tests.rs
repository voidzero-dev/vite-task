use fspy_nostd::OwnedFd;

use super::*;

#[test]
fn zeroes_the_entire_last_file_page_and_anonymous_bss() {
    let page = fspy_nostd::param::page_size();
    // Exercise both direct writes and temporarily writable pages. The final
    // protections must match the ELF in either case.
    for (flags, protection) in
        [(abi::PF_R | abi::PF_W, "rw-p"), (abi::PF_R, "r--p"), (abi::PF_R | abi::PF_X, "r-xp")]
    {
        check_bss(page, flags, protection);
    }
}

fn check_bss(page: usize, flags: u32, expected_protection: &str) {
    for memory_size in [0x180, page + 0x180] {
        let bytes = fixture(page, memory_size, flags);
        let image = map_bytes(&bytes);
        assert_eq!(image.bias % (2 * page), 0);
        assert_eq!(image.phdr, image.bias + EHDR_SIZE);
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        let protection = |address| {
            maps.lines()
                .find_map(|line| {
                    let mut fields = line.split_whitespace();
                    let (start, end) = fields.next()?.split_once('-')?;
                    let range = usize::from_str_radix(start, 16).ok()?
                        ..usize::from_str_radix(end, 16).ok()?;
                    range.contains(&address).then(|| fields.next().unwrap())
                })
                .unwrap()
        };
        assert_eq!(protection(image.entry), "r-xp");
        assert_eq!(protection(image.bias + page + 0x200), expected_protection);
        assert_eq!(protection(image.mapping.ptr.as_ptr().addr() + image.mapping.len - 1), "---p");
        let zero_start = image.bias + page + 0x300;
        let zero_end = align_up(image.bias + page + 0x200 + memory_size, page);
        // SAFETY: the synthetic second LOAD is readable throughout
        // this range, including the file tail and anonymous BSS pages.
        let zeroed = unsafe {
            core::slice::from_raw_parts(
                ptr::with_exposed_provenance::<u8>(zero_start),
                zero_end - zero_start,
            )
        };
        assert!(zeroed.iter().all(|byte| *byte == 0));
        // The two segments share a rounded page. Its earlier segment bytes
        // must survive mapping the later segment at a non-page-aligned vaddr.
        // SAFETY: both addresses are inside mapped readable LOAD pages.
        unsafe {
            assert_eq!(ptr::with_exposed_provenance::<u8>(image.bias + page + 0x80).read(), 0xa5);
            assert_eq!(ptr::with_exposed_provenance::<u8>(image.bias + page + 0x200).read(), 0xa5);
        }
    }
}

#[test]
fn zeroes_spare_file_bytes_on_writable_segment_when_filesz_equals_memsz() {
    let page = fspy_nostd::param::page_size();
    let mut bytes = std::vec![0xa5; 2 * page];
    write_header(&mut bytes, 2, 0x200);
    load(&mut bytes, 0, 0, page, page, abi::PF_R | abi::PF_X, page);
    load(&mut bytes, 1, page + 0x100, 0x200, 0x200, abi::PF_R | abi::PF_W, page);
    let image = map_bytes(&bytes);
    // SAFETY: the second LOAD and its padzero tail are mapped readable.
    unsafe {
        assert_eq!(ptr::with_exposed_provenance::<u8>(image.bias + page + 0x100).read(), 0xa5);
        assert_eq!(ptr::with_exposed_provenance::<u8>(image.bias + page + 0x2ff).read(), 0xa5);
        let tail = core::slice::from_raw_parts(
            ptr::with_exposed_provenance::<u8>(image.bias + page + 0x300),
            page - 0x300,
        );
        assert!(tail.iter().all(|byte| *byte == 0), "writable filesz==memsz must still padzero");
    }
}

#[test]
fn does_not_dirty_readonly_or_exec_tail_when_filesz_equals_memsz() {
    let page = fspy_nostd::param::page_size();
    for flags in [abi::PF_R, abi::PF_R | abi::PF_X] {
        let mut bytes = std::vec![0xa5; 2 * page];
        write_header(&mut bytes, 2, 0x200);
        load(&mut bytes, 0, 0, page, page, abi::PF_R | abi::PF_X, page);
        load(&mut bytes, 1, page + 0x100, 0x200, 0x200, flags, page);
        let image = map_bytes(&bytes);
        // SAFETY: the second LOAD's page is mapped readable directly from the file.
        unsafe {
            assert_eq!(
                ptr::with_exposed_provenance::<u8>(image.bias + page + 0x300).read(),
                0xa5,
                "non-writable filesz==memsz must not COW-zero its tail page"
            );
        }
    }
}

#[test]
fn unordered_shared_page_does_not_wipe_higher_segment() {
    let page = fspy_nostd::param::page_size();
    let mut bytes = std::vec![0xa5; 3 * page];
    write_header(&mut bytes, 3, 0x200);
    load(&mut bytes, 0, 0, page, page, abi::PF_R | abi::PF_X, page);
    // Higher vaddr first, lower vaddr second on the same page.
    load(&mut bytes, 1, page + 0x300, 0x100, 0x100, abi::PF_R | abi::PF_W, page);
    load(&mut bytes, 2, page + 0x100, 0x100, 0x100, abi::PF_R | abi::PF_W, page);
    bytes[page + 0x300] = 0x11;
    bytes[page + 0x100] = 0x22;
    let image = map_bytes(&bytes);
    // SAFETY: both segments on the shared page are mapped readable.
    unsafe {
        assert_eq!(ptr::with_exposed_provenance::<u8>(image.bias + page + 0x100).read(), 0x22);
        assert_eq!(ptr::with_exposed_provenance::<u8>(image.bias + page + 0x200).read(), 0);
        assert_eq!(
            ptr::with_exposed_provenance::<u8>(image.bias + page + 0x300).read(),
            0x11,
            "padzero on a lower segment must not wipe a higher segment on the same page"
        );
    }
}

#[test]
fn padzero_clamps_to_higher_segment_overlapping_bss() {
    let page = fspy_nostd::param::page_size();
    let mut bytes = std::vec![0xa5; 3 * page];
    write_header(&mut bytes, 3, 0x200);
    load(&mut bytes, 0, 0, page, page, abi::PF_R | abi::PF_X, page);
    // Higher vaddr first at page + 0x300; lower vaddr second at page + 0x100
    // with filesz = 0x100 (file_end = page + 0x200) and memsz = 0x300
    // (end = page + 0x400 > other_start).
    load(&mut bytes, 1, page + 0x300, 0x100, 0x100, abi::PF_R | abi::PF_W, page);
    load(&mut bytes, 2, page + 0x100, 0x100, 0x300, abi::PF_R | abi::PF_W, page);
    bytes[page + 0x300] = 0x11;
    bytes[page + 0x100] = 0x22;
    let image = map_bytes(&bytes);
    // SAFETY: both segments on the shared page are mapped readable.
    unsafe {
        assert_eq!(ptr::with_exposed_provenance::<u8>(image.bias + page + 0x100).read(), 0x22);
        let bss = core::slice::from_raw_parts(
            ptr::with_exposed_provenance::<u8>(image.bias + page + 0x200),
            0x100,
        );
        assert!(bss.iter().all(|byte| *byte == 0));
        assert_eq!(
            ptr::with_exposed_provenance::<u8>(image.bias + page + 0x300).read(),
            0x11,
            "padzero must clamp at other_start >= file_end even when other_start < end"
        );
    }
}

#[test]
fn at_phdr_requires_full_program_header_table_within_p_filesz() {
    let page = fspy_nostd::param::page_size();
    let table_end = EHDR_SIZE + 2 * PHDR_SIZE;

    // p_filesz covers e_phoff (headers.start) but ends inside the second Phdr.
    let mut partial = std::vec![0; 2 * page];
    write_header(&mut partial, 2, 0x200);
    load(&mut partial, 0, 0, table_end - 1, page, abi::PF_R | abi::PF_X, page);
    load(&mut partial, 1, page, 0x40, 0x40, abi::PF_R | abi::PF_W, page);
    let partial_image = map_bytes(&partial);
    assert_eq!(
        partial_image.phdr, partial_image.bias,
        "AT_PHDR must not point to a program-header table partially past p_filesz"
    );

    // Exact boundary: p_filesz == headers.end maps the entire table.
    let mut exact = std::vec![0; 2 * page];
    write_header(&mut exact, 2, 0x200);
    load(&mut exact, 0, 0, table_end, page, abi::PF_R | abi::PF_X, page);
    load(&mut exact, 1, page, 0x40, 0x40, abi::PF_R | abi::PF_W, page);
    let exact_image = map_bytes(&exact);
    assert_eq!(exact_image.phdr, exact_image.bias + EHDR_SIZE);
}

#[test]
fn preserves_previous_load_when_bss_only_starts_mid_page() {
    let page = fspy_nostd::param::page_size();
    for memory_size in [0x180, page + 0x180] {
        let mut bytes = std::vec![0xa5; 3 * page];
        write_header(&mut bytes, 2, 0x200);
        load(&mut bytes, 0, 0, page + 0x200, page + 0x200, abi::PF_R | abi::PF_X, page);
        load(&mut bytes, 1, page + 0x200, 0, memory_size, abi::PF_R | abi::PF_W, page);
        let image = map_bytes(&bytes);
        // SAFETY: both LOADs are mapped readable; the first occupies the
        // shared page below the BSS-only vaddr.
        unsafe {
            assert_eq!(
                ptr::with_exposed_provenance::<u8>(image.bias + page + 0x80).read(),
                0xa5,
                "anonymous MAP_FIXED of a filesz==0 LOAD must not wipe the previous segment"
            );
            assert_eq!(ptr::with_exposed_provenance::<u8>(image.bias + page + 0x1ff).read(), 0xa5);
            let zero_start = image.bias + page + 0x200;
            let zero_end = align_up(zero_start + memory_size, page);
            let zeroed = core::slice::from_raw_parts(
                ptr::with_exposed_provenance::<u8>(zero_start),
                zero_end - zero_start,
            );
            assert!(zeroed.iter().all(|byte| *byte == 0));
        }
    }
}

#[test]
fn maps_unordered_non_overlapping_loads() {
    let page = fspy_nostd::param::page_size();
    let mut bytes = std::vec![0; 3 * page];
    write_header(&mut bytes, 2, 0x200);
    // Higher vaddr first: Linux still loads in header order.
    load(&mut bytes, 0, 2 * page, 0x40, 0x40, abi::PF_R | abi::PF_W, page);
    load(&mut bytes, 1, 0, page, page, abi::PF_R | abi::PF_X, page);
    bytes[2 * page] = 0x11;
    bytes[0x200] = 0x22;
    let image = map_bytes(&bytes);
    assert_eq!(image.entry, image.bias + 0x200);
    // SAFETY: both disjoint LOADs are mapped readable.
    unsafe {
        assert_eq!(ptr::with_exposed_provenance::<u8>(image.bias + 2 * page).read(), 0x11);
        assert_eq!(ptr::with_exposed_provenance::<u8>(image.entry).read(), 0x22);
    }
}

#[test]
fn accepts_page_congruent_segment_incongruent_with_large_p_align() {
    let page = fspy_nostd::param::page_size();
    let mut bytes = std::vec![0; 3 * page];
    write_header(&mut bytes, 2, 0x200);
    load(&mut bytes, 0, 0, page, page, abi::PF_R | abi::PF_X, page);
    // Offset = page, vaddr = 2 * page: congruent mod page, incongruent mod 2 * page.
    Phdr {
        p_type: abi::PT_LOAD,
        p_flags: abi::PF_R | abi::PF_W,
        p_offset: u64::try_from(page).unwrap(),
        p_vaddr: u64::try_from(2 * page).unwrap(),
        p_filesz: 0x40,
        p_memsz: 0x40,
        p_align: u64::try_from(2 * page).unwrap(),
    }
    .write_to(&mut bytes, 1);
    bytes[page] = 0x77;
    let image = map_bytes(&bytes);
    // SAFETY: mapped readable segment.
    unsafe {
        assert_eq!(ptr::with_exposed_provenance::<u8>(image.bias + 2 * page).read(), 0x77);
    }
}

#[test]
fn reports_no_interpreter_when_pt_interp_is_absent() {
    let page = fspy_nostd::param::page_size();
    let fd = open_bytes(&fixture(page, 0x180, abi::PF_R | abi::PF_W));
    let file = File::open(fd.as_fd()).unwrap();
    assert!(file.interpreter().unwrap().is_none());
}

#[test]
fn accepts_pt_interp_padded_with_extra_nuls() {
    let fd = open_bytes(&interp_image(b"ld.so\0\0\0"));
    let file = File::open(fd.as_fd()).unwrap();
    let path = file.interpreter().unwrap().unwrap();
    assert_eq!(path.as_units(), b"ld.so");
}

#[test]
fn rejects_unterminated_and_empty_pt_interp() {
    let fd_unterm = open_bytes(&interp_image(b"ld.so"));
    assert_eq!(File::open(fd_unterm.as_fd()).unwrap().interpreter().err(), Some(Error::NOEXEC));

    let fd_empty = open_bytes(&interp_image(b"\0\0\0"));
    assert_eq!(File::open(fd_empty.as_fd()).unwrap().interpreter().err(), Some(Error::NOEXEC));
}

#[test]
fn validate_exec_checks_valid_and_invalid_images_without_closing_fd() {
    let page = fspy_nostd::param::page_size();

    // Valid ET_DYN (PIE) image.
    let valid_pie = open_bytes(&fixture(page, 0x180, abi::PF_R | abi::PF_W));
    assert_eq!(crate::validate_exec(valid_pie.as_fd()), Ok(()));
    // Verify `fd` was NOT closed by `validate_exec`.
    assert!(fs::fstat(valid_pie.as_fd()).is_ok());

    // Valid ET_EXEC image at a non-zero fixed load address.
    let exec_base = 0x40_0000_u64;
    let mut exec_bytes = std::vec![0xa5; 2 * page];
    write_header(&mut exec_bytes, 1, exec_base + 0x200);
    exec_bytes[16..18].copy_from_slice(&abi::ET_EXEC.to_le_bytes());
    Phdr {
        p_type: abi::PT_LOAD,
        p_flags: abi::PF_R | abi::PF_X,
        p_offset: 0,
        p_vaddr: exec_base,
        p_filesz: u64::try_from(page + 0x100).unwrap(),
        p_memsz: u64::try_from(page + 0x100).unwrap(),
        p_align: u64::try_from(page).unwrap(),
    }
    .write_to(&mut exec_bytes, 0);
    let valid_exec = open_bytes(&exec_bytes);
    assert_eq!(crate::validate_exec(valid_exec.as_fd()), Ok(()));
    assert!(fs::fstat(valid_exec.as_fd()).is_ok());

    // Zero-address ET_EXEC image -> NOEXEC (cannot reserve address 0).
    let mut zero_exec_bytes = fixture(page, 0x180, abi::PF_R | abi::PF_W);
    zero_exec_bytes[16..18].copy_from_slice(&abi::ET_EXEC.to_le_bytes());
    let zero_exec = open_bytes(&zero_exec_bytes);
    assert_eq!(crate::validate_exec(zero_exec.as_fd()), Err(Error::NOEXEC));

    // Non-ELF -> NOEXEC, fd stays open.
    let non_elf = open_bytes(b"#!/bin/sh\necho hi\n");
    assert_eq!(crate::validate_exec(non_elf.as_fd()), Err(Error::NOEXEC));
    assert!(fs::fstat(non_elf.as_fd()).is_ok());

    // Truncated ELF header -> NOEXEC.
    let truncated = open_bytes(&exec_bytes[..32]);
    assert_eq!(crate::validate_exec(truncated.as_fd()), Err(Error::NOEXEC));

    // Wrong architecture -> NOEXEC.
    let mut bad_arch = fixture(page, 0x180, abi::PF_R | abi::PF_W);
    bad_arch[18..20].copy_from_slice(&0_u16.to_le_bytes());
    let bad_arch_fd = open_bytes(&bad_arch);
    assert_eq!(crate::validate_exec(bad_arch_fd.as_fd()), Err(Error::NOEXEC));

    // Executable PT_GNU_STACK is accepted, as Linux does, and recorded.
    let mut exec_stack = std::vec![0; 2 * page];
    write_header(&mut exec_stack, 2, 0x200);
    load(&mut exec_stack, 0, 0, page, page, abi::PF_R | abi::PF_X, page);
    Phdr {
        p_type: abi::PT_GNU_STACK,
        p_flags: abi::PF_R | abi::PF_W | abi::PF_X,
        p_offset: 0,
        p_vaddr: 0,
        p_filesz: 0,
        p_memsz: 0,
        p_align: 16,
    }
    .write_to(&mut exec_stack, 1);
    let exec_stack_fd = open_bytes(&exec_stack);
    assert_eq!(crate::validate_exec(exec_stack_fd.as_fd()), Ok(()));
    let exec_stack_file = File::open(exec_stack_fd.as_fd()).unwrap();
    assert!(exec_stack_file.validate_segments(page).unwrap().exec_stack);

    // Missing PT_INTERP -> NOENT.
    let missing_interp_path = b"/nonexistent-fspy-interp-for-test\0";
    let mut missing_interp = std::vec![0; 2 * page];
    write_header(&mut missing_interp, 2, 0x200);
    load(&mut missing_interp, 0, 0, page, page, abi::PF_R | abi::PF_X, page);
    let path_off = page - missing_interp_path.len();
    missing_interp[path_off..page].copy_from_slice(missing_interp_path);
    Phdr {
        p_type: abi::PT_INTERP,
        p_flags: abi::PF_R,
        p_offset: u64::try_from(path_off).unwrap(),
        p_vaddr: u64::try_from(path_off).unwrap(),
        p_filesz: u64::try_from(missing_interp_path.len()).unwrap(),
        p_memsz: u64::try_from(missing_interp_path.len()).unwrap(),
        p_align: 1,
    }
    .write_to(&mut missing_interp, 1);
    let missing_interp_fd = open_bytes(&missing_interp);
    assert_eq!(crate::validate_exec(missing_interp_fd.as_fd()), Err(Error::NOENT));
    assert!(fs::fstat(missing_interp_fd.as_fd()).is_ok());

    // Valid PT_INTERP pointing to a valid ET_DYN or (as Linux allows) ET_EXEC
    // interpreter succeeds.
    let dyn_interp_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(dyn_interp_file.path(), fixture(page, 0x180, abi::PF_R | abi::PF_W)).unwrap();
    let dyn_interp_path =
        std::ffi::CString::new(dyn_interp_file.path().as_os_str().as_encoded_bytes()).unwrap();
    let mut with_valid_interp = std::vec![0; 2 * page];
    write_header(&mut with_valid_interp, 2, 0x200);
    load(&mut with_valid_interp, 0, 0, page, page, abi::PF_R | abi::PF_X, page);
    let pbytes = dyn_interp_path.as_bytes_with_nul();
    let p_off = page - pbytes.len();
    with_valid_interp[p_off..page].copy_from_slice(pbytes);
    Phdr {
        p_type: abi::PT_INTERP,
        p_flags: abi::PF_R,
        p_offset: u64::try_from(p_off).unwrap(),
        p_vaddr: u64::try_from(p_off).unwrap(),
        p_filesz: u64::try_from(pbytes.len()).unwrap(),
        p_memsz: u64::try_from(pbytes.len()).unwrap(),
        p_align: 1,
    }
    .write_to(&mut with_valid_interp, 1);
    let valid_dyn_fd = open_bytes(&with_valid_interp);
    assert_eq!(crate::validate_exec(valid_dyn_fd.as_fd()), Ok(()));

    std::fs::write(dyn_interp_file.path(), &exec_bytes).unwrap();
    assert_eq!(crate::validate_exec(valid_dyn_fd.as_fd()), Ok(()));
}

#[test]
fn maps_et_dyn_with_high_nonzero_p_vaddr_without_underflow() {
    let page = fspy_nostd::param::page_size();
    let high_vaddr = 0x7000_0000_0000_u64;
    let mut bytes = std::vec![0x3c; 2 * page];
    write_header(&mut bytes, 1, high_vaddr + 0x200);
    Phdr {
        p_type: abi::PT_LOAD,
        p_flags: abi::PF_R | abi::PF_X,
        p_offset: 0,
        p_vaddr: high_vaddr,
        p_filesz: u64::try_from(page + 0x100).unwrap(),
        p_memsz: u64::try_from(page + 0x100).unwrap(),
        p_align: u64::try_from(page).unwrap(),
    }
    .write_to(&mut bytes, 0);

    let file = open_bytes(&bytes);
    let parsed = File::open(file.as_fd()).unwrap();
    let layout = parsed.validate_segments(page).unwrap();
    let image = parsed.try_map(page, &layout).unwrap();
    let mapped_base = image.bias.wrapping_add(layout.low);
    assert_eq!(image.entry, mapped_base + 0x200);
    assert_eq!(image.phdr, mapped_base + EHDR_SIZE);
}

#[test]
fn accepts_non_ev_current_ident_and_header_versions() {
    let page = fspy_nostd::param::page_size();
    for (ei_version, e_version) in [(0_u8, 0_u32), (0, 1), (1, 0), (2, 2), (2, 0)] {
        let mut bytes = fixture(page, 0x180, abi::PF_R | abi::PF_W);
        bytes[abi::EI_VERSION] = ei_version;
        bytes[20..24].copy_from_slice(&e_version.to_le_bytes());

        let fd = open_bytes(&bytes);
        let parsed = File::open(fd.as_fd()).expect("non-EV_CURRENT ELF should open");
        let _layout = parsed.validate_segments(page).expect("non-EV_CURRENT ELF should validate");
        assert_eq!(crate::validate_exec(fd.as_fd()), Ok(()));
    }

    // Magic, ELF64 class, and little-endian data checks must still reject invalid images.
    for (offset, bad_byte) in
        [(0_usize, 0x00_u8), (abi::EI_CLASS, abi::ELFCLASS32), (abi::EI_DATA, abi::ELFDATA2MSB)]
    {
        let mut bytes = fixture(page, 0x180, abi::PF_R | abi::PF_W);
        bytes[offset] = bad_byte;
        let fd = open_bytes(&bytes);
        assert_eq!(File::open(fd.as_fd()).err(), Some(Error::NOEXEC));
    }
}

fn fixture(page: usize, memory_size: usize, flags: u32) -> std::vec::Vec<u8> {
    let mut bytes = std::vec![0xa5; 3 * page];
    write_header(&mut bytes, 2, 0x200);
    load(&mut bytes, 0, 0, page + 0x100, page + 0x100, abi::PF_R | abi::PF_X, page);
    load(&mut bytes, 1, page + 0x200, 0x100, memory_size, flags, page);
    bytes
}

#[expect(clippy::struct_field_names, reason = "matches ELF64 Elf64_Phdr field names")]
struct Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

impl Phdr {
    fn write_to(&self, bytes: &mut [u8], index: usize) {
        let start = EHDR_SIZE + index * PHDR_SIZE;
        let out = &mut bytes[start..start + PHDR_SIZE];
        out.fill(0);
        out[0..4].copy_from_slice(&self.p_type.to_le_bytes());
        out[4..8].copy_from_slice(&self.p_flags.to_le_bytes());
        out[8..16].copy_from_slice(&self.p_offset.to_le_bytes());
        out[16..24].copy_from_slice(&self.p_vaddr.to_le_bytes());
        out[32..40].copy_from_slice(&self.p_filesz.to_le_bytes());
        out[40..48].copy_from_slice(&self.p_memsz.to_le_bytes());
        out[48..56].copy_from_slice(&self.p_align.to_le_bytes());
    }
}

fn interp_image(path: &[u8]) -> std::vec::Vec<u8> {
    let start = EHDR_SIZE + PHDR_SIZE;
    let mut bytes = std::vec![0; start + path.len()];
    write_header(&mut bytes, 1, 0);
    let len = u64::try_from(path.len()).unwrap();
    Phdr {
        p_type: abi::PT_INTERP,
        p_flags: 0,
        p_offset: u64::try_from(start).unwrap(),
        p_vaddr: 0,
        p_filesz: len,
        p_memsz: len,
        p_align: 1,
    }
    .write_to(&mut bytes, 0);
    bytes[start..start + path.len()].copy_from_slice(path);
    bytes
}

fn write_header(bytes: &mut [u8], phnum: u16, entry: u64) {
    let out = &mut bytes[..EHDR_SIZE];
    out.fill(0);
    out[0..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    out[16..18].copy_from_slice(&abi::ET_DYN.to_le_bytes());
    out[18..20].copy_from_slice(&MACHINE.to_le_bytes());
    out[20..24].copy_from_slice(&1_u32.to_le_bytes());
    out[24..32].copy_from_slice(&entry.to_le_bytes());
    out[32..40].copy_from_slice(&u64::try_from(EHDR_SIZE).unwrap().to_le_bytes());
    out[52..54].copy_from_slice(&u16::try_from(EHDR_SIZE).unwrap().to_le_bytes());
    out[54..56].copy_from_slice(&u16::try_from(PHDR_SIZE).unwrap().to_le_bytes());
    out[56..58].copy_from_slice(&phnum.to_le_bytes());
}

fn load(
    bytes: &mut [u8],
    index: usize,
    start: usize,
    file_size: usize,
    memory_size: usize,
    flags: u32,
    page: usize,
) {
    let addr = u64::try_from(start).unwrap();
    Phdr {
        p_type: abi::PT_LOAD,
        p_flags: flags,
        p_offset: addr,
        p_vaddr: addr,
        p_filesz: u64::try_from(file_size).unwrap(),
        p_memsz: u64::try_from(memory_size).unwrap(),
        p_align: u64::try_from(2 * page).unwrap(),
    }
    .write_to(bytes, index);
}

fn map_bytes(bytes: &[u8]) -> Image {
    let page = fspy_nostd::param::page_size();
    let fd = open_bytes(bytes);
    let file = File::open(fd.as_fd()).unwrap();
    let layout = file.validate_segments(page).unwrap();
    file.try_map(page, &layout).unwrap()
}

fn open_bytes(bytes: &[u8]) -> OwnedFd {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    let path = std::ffi::CString::new(file.path().as_os_str().as_encoded_bytes()).unwrap();
    let path = CStr::from_units_with_nul(path.as_bytes_with_nul()).unwrap();
    fs::openat(fspy_nostd::CWD, path, fs::OFlags::RDONLY, fs::Mode::empty()).unwrap()
}
