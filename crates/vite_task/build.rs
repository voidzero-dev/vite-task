// Why `cfg(fspy)` instead of matching on `target_os` directly at each use site:
// "fspy is available" is a single semantic predicate, but the underlying reason
// (the `fspy` crate builds on windows/macos/linux) is a three-OS list that
// would otherwise have to be repeated — as `any(target_os = "windows", "macos",
// "linux")` — everywhere `fspy::*` is touched. Naming it `fspy` keeps the
// source self-documenting: code reads `#[cfg(fspy)]` instead of a disjunction
// over OSes. The OS allowlist lives in two spots that must stay in sync: this
// file (for the rustc cfg) and the target-scoped dep block in Cargo.toml
// (which Cargo resolves before build.rs runs, so it can't reuse this cfg).
fn main() {
    println!("cargo::rustc-check-cfg=cfg(fspy)");
    println!("cargo::rerun-if-changed=build.rs");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if matches!(target_os.as_str(), "windows" | "macos" | "linux") {
        println!("cargo::rustc-cfg=fspy");
    }
}
