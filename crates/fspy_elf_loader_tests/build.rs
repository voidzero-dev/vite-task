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
    println!("cargo::rerun-if-changed=fixtures/virtualization.c");
    println!("cargo::rerun-if-env-changed=CC");

    let compiler = resolve_compiler();
    let is_zigcc = compiler
        .to_str()
        .is_some_and(|path| path.rsplit('/').next().is_some_and(|name| name.starts_with("zigcc-")));

    let out = env::var_os("OUT_DIR").unwrap();
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR").unwrap();
    let mut source = manifest_dir.clone();
    source.push("/fixtures/runtime.c");
    let mut virt_source = manifest_dir;
    virt_source.push("/fixtures/virtualization.c");

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
            run_compile(Command::new(&compiler), &source, &dest, flags);
        }
        println!("cargo::rustc-env={env_name}={}", dest.to_str().unwrap());
    }

    for (env_name, file_name, flags, is_static) in [
        ("FIXTURE_VIRT_DYN_PIE", "/fixture_virt_dyn_pie", &["-fPIE", "-pie"][..], false),
        (
            "FIXTURE_VIRT_STATIC_EXEC",
            "/fixture_virt_static_exec",
            &["-fno-pie", "-static"][..],
            true,
        ),
    ] {
        let dest = join_out(&out, file_name);
        if is_static && let Some(musl_target) = musl_target {
            compile_zig_musl(musl_target, &virt_source, &dest, flags);
        } else {
            run_compile(Command::new(&compiler), &virt_source, &dest, flags);
        }
        println!("cargo::rustc-env={env_name}={}", dest.to_str().unwrap());
    }
}

fn resolve_compiler() -> OsString {
    if let Ok(mut target) = env::var("TARGET") {
        // SAFETY: replacing ASCII '-' with ASCII '_' preserves UTF-8 validity.
        for byte in unsafe { target.as_bytes_mut() } {
            if *byte == b'-' {
                *byte = b'_';
            }
        }
        let mut key = OsString::from("CC_");
        key.push(&target);
        println!("cargo::rerun-if-env-changed=CC_{target}");
        if let Some(cc) = env::var_os(&key).filter(|value| !value.is_empty()) {
            return cc;
        }
    }
    env::var_os("CC").filter(|value| !value.is_empty()).unwrap_or_else(|| OsString::from("cc"))
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
