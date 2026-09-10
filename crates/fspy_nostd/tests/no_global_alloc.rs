use std::process::Command;

#[test]
fn global_allocator_is_checked_at_link_time() {
    let source = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/no_global_alloc.rs");
    // Compile and link freestanding fixtures, but never execute them. Both
    // targets are installed by rust-toolchain.toml; no Linux VM is needed.
    for target in ["aarch64-unknown-none-softfloat", "x86_64-unknown-none"] {
        // Optimization once erased a forbidden allocation when the undefined
        // function returned `!`. Check both optimized and unoptimized builds.
        for optimization in ["0", "3"] {
            for operation in ["unused", "alloc", "alloc_zeroed", "dealloc", "realloc", "global_vec"]
            {
                let directory = tempfile::tempdir().unwrap();
                // Run a full link: cargo check cannot detect undefined symbols.
                // --cfg selects the fixture's operation. Abort avoids an unwind
                // runtime, and --no-pie matches our fixed-address launcher.
                let output = Command::new("rustc")
                    .args([source, "--edition=2024", "--target", target])
                    .args(["--cfg", operation, "-C", "panic=abort", "-C"])
                    .arg(["opt-level=", optimization].concat())
                    .args(["-C", "link-arg=--no-pie", "-o"])
                    .arg(directory.path().join("probe"))
                    .output()
                    .expect("cannot run rustc");
                let stderr = std::str::from_utf8(&output.stderr).unwrap();
                if operation == "unused" {
                    // Linking alloc alone must work when its global allocator
                    // is unused and the linker can discard the forbidden calls.
                    assert!(output.status.success(), "{target}, opt={optimization}: {stderr}");
                } else {
                    assert!(
                        !output.status.success(),
                        "{target}, opt={optimization}: {operation} unexpectedly linked"
                    );
                    // An unrelated compiler or linker error is not a pass.
                    assert!(
                        stderr
                            .contains("undefined symbol: __fspy_global_allocator_must_not_be_used"),
                        "{target}, opt={optimization}, {operation}: {stderr}"
                    );
                }
            }
        }
    }
}
