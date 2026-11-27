include!("src/launcher_region.rs");

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=src/launcher_region.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        // Leave the low 4 GiB available while staying in application regions
        // of the current LLVM Linux ASan and C/C++ TSan layouts. AArch64's
        // base also fits smaller virtual address spaces, including 39-bit.
        // Older AArch64 TSan runtimes can reject 8 GiB; see the README.
        // This is a placement convention; fixed targets can still overlap it.
        let image_base = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("x86_64") => LAUNCHER_REGION_X86_64.0,   // 85 TiB
            Ok("aarch64") => LAUNCHER_REGION_AARCH64.0, // 8 GiB
            _ => panic!("unsupported launcher architecture"),
        };
        // Fixed linking resolves pointers without a startup self-relocator.
        println!("cargo::metadata=link_args=--no-pie --image-base={image_base:#x}");
    }
}
