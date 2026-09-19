fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        // Keep the same runtime checks in PIE and fixed-address executables.
        // The linker driver must retain dynamic linking while disabling PIE.
        println!("cargo::rustc-link-arg-bin=fixture_exec=-no-pie");
    }
}
