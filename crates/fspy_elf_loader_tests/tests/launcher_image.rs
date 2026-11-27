use elf::{ElfBytes, abi, endian::LittleEndian, file::Class};

// Embed the artifact bytes at compile time so cross-compiled test archives
// (such as Windows nextest archives built on Linux) do not depend on the
// build host's path at runtime.
const LAUNCHER: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_FSPY_ELF_LOADER_TEST_LAUNCHER"));

// Inspect the real artifact on every host, including macOS and Windows.
// Linux execution tests separately check startup and fixed-address collisions.
#[test]
fn launcher_uses_high_fixed_addresses_without_runtime_relocations() {
    let image = ElfBytes::<LittleEndian>::minimal_parse(LAUNCHER).unwrap();
    assert_eq!(image.ehdr.class, Class::ELF64);
    assert_eq!(image.ehdr.e_type, abi::ET_EXEC);
    // Keep the entire image inside a sanitizer application region: x86-64
    // C/C++ TSan's 85–90 TiB region, or AArch64 39-bit TSan's low 20 GiB.
    let (base, limit) = match image.ehdr.e_machine {
        abi::EM_X86_64 => (0x5500_0000_0000, 0x5a00_0000_0000),
        abi::EM_AARCH64 => (0x0002_0000_0000, 0x0005_0000_0000),
        machine => panic!("unsupported launcher machine: {machine}"),
    };
    let segments: Vec<_> = image.segments().unwrap().iter().collect();
    assert!(
        !segments
            .iter()
            .any(|ph| { matches!(ph.p_type, abi::PT_INTERP | abi::PT_DYNAMIC | abi::PT_TLS) })
    );
    let first_load = segments.iter().find(|ph| ph.p_type == abi::PT_LOAD).unwrap();
    assert_eq!((first_load.p_vaddr, first_load.p_offset), (base, 0));
    for segment in segments.iter().filter(|ph| ph.p_type == abi::PT_LOAD) {
        assert!(segment.p_vaddr >= base);
        assert!(segment.p_vaddr.checked_add(segment.p_memsz).unwrap() <= limit);
    }
    assert!(segments.iter().any(|ph| {
        ph.p_type == abi::PT_LOAD
            && ph.p_flags & abi::PF_X != 0
            && ph.p_vaddr <= image.ehdr.e_entry
            && image.ehdr.e_entry < ph.p_vaddr + ph.p_memsz
    }));
    assert!(image.dynamic().unwrap().is_none());
}
