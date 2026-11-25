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
        assert_eq!(image.phdr, Some(image.bias + 64));
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
    let header = &mut bytes[64 + 56..64 + 2 * 56];
    header.fill(0);
    header[..4].copy_from_slice(&abi::PT_LOAD.to_le_bytes());
    header[4..8].copy_from_slice(&(abi::PF_R | abi::PF_W).to_le_bytes());
    for (field, value) in [(8, page), (16, 2 * page), (32, 0x40), (40, 0x40), (48, 2 * page)] {
        header[field..field + 8].copy_from_slice(&u64::try_from(value).unwrap().to_le_bytes());
    }
    bytes[page] = 0x77;
    let image = map_bytes(&bytes);
    // SAFETY: mapped readable segment.
    unsafe {
        assert_eq!(ptr::with_exposed_provenance::<u8>(image.bias + 2 * page).read(), 0x77);
    }
}

#[test]
#[should_panic(expected = "overlapping ELF segments")]
fn rejects_overlapping_load_segments() {
    let page = fspy_nostd::param::page_size();
    let mut bytes = std::vec![0xa5; 2 * page];
    write_header(&mut bytes, 2, 0x80);
    load(&mut bytes, 0, 0, 0x200, 0x200, abi::PF_R | abi::PF_X, page);
    load(&mut bytes, 1, 0x100, 0x200, 0x200, abi::PF_R | abi::PF_W, page);
    map_bytes(&bytes);
}

#[test]
fn reports_no_interpreter_when_pt_interp_is_absent() {
    let page = fspy_nostd::param::page_size();
    let file = File::new(open_bytes(&fixture(page, 0x180, abi::PF_R | abi::PF_W)));
    assert!(file.interpreter().is_none());
}

#[test]
fn accepts_pt_interp_padded_with_extra_nuls() {
    let file = File::new(open_bytes(&interp_image(b"ld.so\0\0\0")));
    let path = file.interpreter().unwrap();
    assert_eq!(path.as_units(), b"ld.so");
}

#[test]
#[should_panic(expected = "unterminated PT_INTERP")]
fn rejects_unterminated_pt_interp() {
    let _ = File::new(open_bytes(&interp_image(b"ld.so"))).interpreter();
}

#[test]
#[should_panic(expected = "empty PT_INTERP path")]
fn rejects_empty_pt_interp() {
    let _ = File::new(open_bytes(&interp_image(b"\0\0\0"))).interpreter();
}

fn fixture(page: usize, memory_size: usize, flags: u32) -> std::vec::Vec<u8> {
    let mut bytes = std::vec![0xa5; 3 * page];
    write_header(&mut bytes, 2, 0x200);
    load(&mut bytes, 0, 0, page + 0x100, page + 0x100, abi::PF_R | abi::PF_X, page);
    load(&mut bytes, 1, page + 0x200, 0x100, memory_size, flags, page);
    bytes
}

fn interp_image(path: &[u8]) -> std::vec::Vec<u8> {
    let start = 64 + 56;
    let mut bytes = std::vec![0; start + path.len()];
    write_header(&mut bytes, 1, 0);
    let header = &mut bytes[64..120];
    header[..4].copy_from_slice(&abi::PT_INTERP.to_le_bytes());
    for (field, value) in [(8, start), (32, path.len()), (40, path.len()), (48, 1)] {
        header[field..field + 8].copy_from_slice(&u64::try_from(value).unwrap().to_le_bytes());
    }
    bytes[start..start + path.len()].copy_from_slice(path);
    bytes
}

fn write_header(bytes: &mut [u8], phnum: u16, entry: u64) {
    bytes[..64].fill(0);
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&abi::ET_DYN.to_le_bytes());
    bytes[18..20].copy_from_slice(&MACHINE.to_le_bytes());
    bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
    bytes[24..32].copy_from_slice(&entry.to_le_bytes());
    bytes[32..40].copy_from_slice(&64_u64.to_le_bytes());
    bytes[52..54].copy_from_slice(&64_u16.to_le_bytes());
    bytes[54..56].copy_from_slice(&56_u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&phnum.to_le_bytes());
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
    let header = &mut bytes[64 + index * 56..64 + index * 56 + 56];
    header.fill(0);
    header[..4].copy_from_slice(&abi::PT_LOAD.to_le_bytes());
    header[4..8].copy_from_slice(&flags.to_le_bytes());
    for (field, value) in
        [(8, start), (16, start), (32, file_size), (40, memory_size), (48, 2 * page)]
    {
        header[field..field + 8].copy_from_slice(&u64::try_from(value).unwrap().to_le_bytes());
    }
}

fn map_bytes(bytes: &[u8]) -> Image {
    File::new(open_bytes(bytes)).map(fspy_nostd::param::page_size())
}

fn open_bytes(bytes: &[u8]) -> OwnedFd {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    let path = std::ffi::CString::new(file.path().as_os_str().as_encoded_bytes()).unwrap();
    let path = CStr::from_units_with_nul(path.as_bytes_with_nul()).unwrap();
    fs::openat(fspy_nostd::CWD, path, fs::OFlags::RDONLY, fs::Mode::empty()).unwrap()
}
