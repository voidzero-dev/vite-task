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
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), bytes).unwrap();
        let path = std::ffi::CString::new(file.path().as_os_str().as_encoded_bytes()).unwrap();
        let path = CStr::from_units_with_nul(path.as_bytes_with_nul()).unwrap();
        let image = File::new(
            fs::openat(fspy_nostd::CWD, path, fs::OFlags::RDONLY, fs::Mode::empty()).unwrap(),
        );
        let image = image.map(page);
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

fn fixture(page: usize, memory_size: usize, flags: u32) -> std::vec::Vec<u8> {
    let mut bytes = std::vec![0xa5; 3 * page];
    bytes[..64].fill(0);
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&abi::ET_DYN.to_le_bytes());
    bytes[18..20].copy_from_slice(&MACHINE.to_le_bytes());
    bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
    bytes[24..32].copy_from_slice(&0x200_u64.to_le_bytes());
    bytes[32..40].copy_from_slice(&64_u64.to_le_bytes());
    bytes[52..54].copy_from_slice(&64_u16.to_le_bytes());
    bytes[54..56].copy_from_slice(&56_u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&2_u16.to_le_bytes());
    segment(&mut bytes, 0, 0, page + 0x100, page + 0x100, abi::PF_R | abi::PF_X, page);
    segment(&mut bytes, 1, page + 0x200, 0x100, memory_size, flags, page);
    bytes
}

fn segment(
    bytes: &mut [u8],
    index: usize,
    start: usize,
    file_size: usize,
    memory_size: usize,
    flags: u32,
    page: usize,
) {
    let offset = 64 + index * 56;
    let header = &mut bytes[offset..offset + 56];
    header.fill(0);
    header[..4].copy_from_slice(&abi::PT_LOAD.to_le_bytes());
    header[4..8].copy_from_slice(&flags.to_le_bytes());
    for (offset, value) in
        [(8, start), (16, start), (32, file_size), (40, memory_size), (48, 2 * page)]
    {
        header[offset..offset + 8].copy_from_slice(&u64::try_from(value).unwrap().to_le_bytes());
    }
}
