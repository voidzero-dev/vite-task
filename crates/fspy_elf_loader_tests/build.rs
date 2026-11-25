use std::{
    env,
    ffi::{OsStr, OsString},
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
    let is_zigcc = compiler
        .path()
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.starts_with("zigcc-"));

    let out = env::var_os("OUT_DIR").unwrap();
    let mut source = env::var_os("CARGO_MANIFEST_DIR").unwrap();
    source.push("/fixtures/runtime.c");

    // Compile the single C runtime fixture across all four ELF64 cells with
    // explicit PIE/non-PIE flags so GCC, Clang, and zig cc emit identical shapes.
    // cargo-zigbuild's gnu wrapper cannot link static glibc executables, so use
    // `cargo-zigbuild zig cc` targeting musl for the two static cells.
    let musl_target = if is_zigcc {
        Some(match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("x86_64") => "x86_64-linux-musl",
            Ok("aarch64") => "aarch64-linux-musl",
            other => panic!("unsupported Linux target arch for zig cc static fixtures: {other:?}"),
        })
    } else {
        None
    };

    for (env_name, file_name, flags, is_static) in [
        ("FIXTURE_DYN_PIE", "/fixture_dyn_pie", &["-fPIE", "-pie"][..], false),
        ("FIXTURE_DYN_EXEC", "/fixture_dyn_exec", &["-fno-pie", "-no-pie"][..], false),
        ("FIXTURE_STATIC_EXEC", "/fixture_static_exec", &["-fno-pie", "-static"][..], true),
        (
            "FIXTURE_STATIC_PIE",
            "/fixture_static_pie",
            if is_zigcc { &["-fPIE", "-static", "-pie"][..] } else { &["-fPIE", "-static-pie"] },
            true,
        ),
    ] {
        let dest = join_out(&out, file_name);
        if is_static && let Some(musl_target) = musl_target {
            compile_zig_musl(musl_target, &source, &dest, flags);
        } else {
            compile(&compiler, &source, &dest, flags);
        }
        println!("cargo::rustc-env={env_name}={}", dest.to_str().unwrap());
    }
}

fn join_out(out: &OsString, name: &str) -> OsString {
    let mut path = out.clone();
    path.push(name);
    path
}

fn compile_zig_musl(musl_target: &str, source: &OsString, dest: &OsString, extra: &[&str]) {
    let mut command = Command::new("cargo-zigbuild");
    command.args(["zig", "cc", "--", "-target", musl_target]);
    run_compile(command, source, dest, extra);
}

fn compile(compiler: &cc::Tool, source: &OsString, dest: &OsString, extra: &[&str]) {
    let mut command = Command::new(compiler.path());
    for (key, value) in compiler.env() {
        command.env(key, value);
    }
    run_compile(command, source, dest, extra);
}

fn run_compile(mut command: Command, source: &OsString, dest: &OsString, extra: &[&str]) {
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
