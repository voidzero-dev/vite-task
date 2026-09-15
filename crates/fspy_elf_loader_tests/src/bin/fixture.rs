#[cfg(not(target_os = "linux"))]
const fn main() {}

#[cfg(target_os = "linux")]
fn main() {
    use std::{
        cell::Cell,
        io::Write as _,
        os::unix::ffi::OsStrExt as _,
        sync::atomic::{AtomicU8, Ordering},
    };

    static BSS: [AtomicU8; 128 * 1024] = [const { AtomicU8::new(0) }; 128 * 1024];
    static DATA: AtomicU8 = AtomicU8::new(42);
    thread_local! { static TLS: Cell<usize> = const { Cell::new(17) }; }

    let args: Vec<_> = std::env::args_os().collect();
    assert_eq!(args.len(), 3);
    assert_eq!(args[0], "custom-argv-zero");
    assert_eq!(args[1], "");
    assert_eq!(args[2].as_bytes(), b"\xff");
    assert_eq!(std::env::var_os("EMPTY").unwrap(), "");
    assert_eq!(std::env::var_os("BYTES").unwrap().as_bytes(), b"\xff");
    for byte in &BSS {
        assert_eq!(byte.load(Ordering::Relaxed), 0);
    }
    BSS[BSS.len() - 1].store(9, Ordering::Relaxed);
    assert_eq!(DATA.swap(43, Ordering::Relaxed), 42);
    TLS.set(23);
    let values = std::thread::spawn(|| {
        assert_eq!(TLS.get(), 17);
        vec![7_u8; 128 * 1024]
    })
    .join()
    .unwrap();
    assert!(values.iter().all(|value| *value == 7));
    assert_eq!(TLS.get(), 23);

    // SAFETY: successful runtime startup supplies readable auxiliary data.
    unsafe {
        let execfn = aux(libc::AT_EXECFN);
        let execfn = std::ffi::CStr::from_ptr(std::ptr::with_exposed_provenance(execfn));
        assert_eq!(execfn.to_bytes(), std::env::var_os("FIXTURE_IMAGE").unwrap().as_bytes());
        assert_eq!(aux(libc::AT_PHENT), size_of::<libc::Elf64_Phdr>());
        let headers = std::slice::from_raw_parts(
            std::ptr::with_exposed_provenance::<libc::Elf64_Phdr>(aux(libc::AT_PHDR)),
            aux(libc::AT_PHNUM),
        );
        let phdr = headers.iter().find(|ph| ph.p_type == libc::PT_PHDR).unwrap();
        let bias = aux(libc::AT_PHDR).checked_sub(usize::try_from(phdr.p_vaddr).unwrap()).unwrap();
        let entry = aux(libc::AT_ENTRY);
        assert!(headers.iter().any(|ph| {
            ph.p_type == libc::PT_LOAD
                && ph.p_flags & libc::PF_X != 0
                && (bias + usize::try_from(ph.p_vaddr).unwrap()
                    ..bias + usize::try_from(ph.p_vaddr + ph.p_memsz).unwrap())
                    .contains(&entry)
        }));
        let interpreter = std::slice::from_raw_parts(
            std::ptr::with_exposed_provenance::<u8>(aux(libc::AT_BASE)),
            4,
        );
        assert_eq!(interpreter, b"\x7fELF");
        let random = std::slice::from_raw_parts(
            std::ptr::with_exposed_provenance::<u8>(aux(libc::AT_RANDOM)),
            16,
        );
        assert!(random.iter().any(|byte| *byte != 0));
    }
    let image = std::env::var_os("FIXTURE_IMAGE").unwrap();
    for entry in std::fs::read_dir("/proc/self/fd").unwrap() {
        if let Ok(path) = std::fs::read_link(entry.unwrap().path()) {
            assert_ne!(path.as_os_str(), image, "executable fd leaked");
        }
    }
    std::io::stdout().write_all(b"runtime-ok\n").unwrap();
    std::process::exit(37);
}

#[cfg(target_os = "linux")]
fn aux(key: libc::c_ulong) -> usize {
    // SAFETY: libc is initialized and getauxval only reads its startup data.
    usize::try_from(unsafe { libc::getauxval(key) }).unwrap()
}
