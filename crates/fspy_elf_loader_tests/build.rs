use std::{
    env,
    ffi::OsString,
    process::{Command, Stdio},
};

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    println!("cargo::rerun-if-changed=fixtures/runtime.c");
    println!("cargo::rerun-if-env-changed=CC");
    if let Ok(mut target) = env::var("TARGET") {
        // SAFETY: replacing ASCII '-' with ASCII '_' preserves UTF-8 validity.
        for byte in unsafe { target.as_bytes_mut() } {
            if *byte == b'-' {
                *byte = b'_';
            }
        }
        println!("cargo::rerun-if-env-changed=CC_{target}");
    }

    // Use cc's target-aware compiler discovery (respects CC_<target>, CC, and
    // cross toolchains) without inheriting its library-oriented default flags.
    let compiler = cc::Build::new().cargo_metadata(false).get_compiler();

    let out = env::var_os("OUT_DIR").unwrap();
    let mut source = env::var_os("CARGO_MANIFEST_DIR").unwrap();
    source.push("/fixtures/runtime.c");

    // Compile the single C runtime fixture across all four ELF64 cells with
    // explicit PIE/non-PIE flags so GCC, Clang, and zig cc emit identical shapes.
    let cells = [
        ("FIXTURE_DYN_PIE", "/fixture_dyn_pie", &["-fPIE", "-pie"][..]),
        ("FIXTURE_DYN_EXEC", "/fixture_dyn_exec", &["-fno-pie", "-no-pie"][..]),
        ("FIXTURE_STATIC_EXEC", "/fixture_static_exec", &["-fno-pie", "-static"][..]),
        ("FIXTURE_STATIC_PIE", "/fixture_static_pie", &["-fPIE", "-static-pie"][..]),
    ];

    for (env_name, file_name, flags) in cells {
        let dest = join_out(&out, file_name);
        compile(&compiler, &source, &dest, flags);
        println!("cargo::rustc-env={env_name}={}", dest.to_str().unwrap());
    }
}

fn join_out(out: &OsString, name: &str) -> OsString {
    let mut path = out.clone();
    path.push(name);
    path
}

fn compile(compiler: &cc::Tool, source: &OsString, dest: &OsString, extra: &[&str]) {
    let mut command = Command::new(compiler.path());
    for (key, value) in compiler.env() {
        command.env(key, value);
    }
    let output = command
        .args(["-O1", "-pthread"])
        .args(extra)
        .arg("-o")
        .arg(dest)
        .arg(source)
        .stderr(Stdio::piped())
        .output()
        .expect("cannot invoke C compiler");
    assert!(
        output.status.success(),
        "cannot compile runtime fixture ({extra:?}): {}",
        str::from_utf8(&output.stderr).unwrap_or("non-utf8 C compiler diagnostics")
    );
}
