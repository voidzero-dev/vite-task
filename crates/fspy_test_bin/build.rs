fn main() {
    // fspy_test_bin must be a statically-linked executable so fspy can test
    // its seccomp-based tracing path (used for static binaries that make raw
    // syscalls instead of going through a preloaded libc shim).
    // Force +crt-static even when the global RUSTFLAGS contain -crt-static.
    println!("cargo::rustc-link-arg=-static");
}
