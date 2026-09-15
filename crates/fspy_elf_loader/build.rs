fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        // main! has no self-relocator. x86_64-unknown-none otherwise links a
        // static PIE with unresolved load-bias relocations, even for panics.
        // The kernel maps PT_LOAD segments but does not fix up pointers in a
        // static PIE. With --no-pie, the linker resolves them for fixed addresses.
        println!("cargo::rustc-link-arg-bin=fspy_elf_loader_test_launcher=--no-pie");
    }
}
