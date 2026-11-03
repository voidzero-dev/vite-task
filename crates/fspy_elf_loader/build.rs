fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        // Leave the low 4 GiB available while staying in application regions
        // of the current LLVM Linux ASan and C/C++ TSan layouts. AArch64's
        // base also fits smaller virtual address spaces, including 39-bit.
        // Older AArch64 TSan runtimes can reject 8 GiB; see the README.
        // This is a placement convention; fixed targets can still overlap it.
        let image_base = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("x86_64") => "--image-base=0x550000000000", // 85 TiB
            Ok("aarch64") => "--image-base=0x200000000",   // 8 GiB
            _ => panic!("unsupported launcher architecture"),
        };
        // Fixed linking resolves pointers without a startup self-relocator.
        for argument in ["--no-pie", image_base] {
            println!("cargo::rustc-link-arg-bin=fspy_elf_loader_test_launcher={argument}");
        }
    }
}
