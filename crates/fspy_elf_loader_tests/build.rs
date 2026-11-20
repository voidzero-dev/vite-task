use std::{
    env,
    ffi::OsString,
    process::{Command, Stdio},
};

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    // Keep the same runtime checks in PIE and fixed-address executables.
    // The linker driver must retain dynamic linking while disabling PIE.
    println!("cargo::rustc-link-arg-bin=fixture_exec=-no-pie");
    println!("cargo::rerun-if-changed=fixtures/static_runtime.c");

    let out = env::var_os("OUT_DIR").unwrap();
    let mut source = env::var_os("CARGO_MANIFEST_DIR").unwrap();
    source.push("/fixtures/static_runtime.c");
    let static_bin = join_out(&out, "/fixture_static");
    let static_pie = join_out(&out, "/fixture_static_pie");
    compile(&source, &static_bin, &["-static"]);
    compile(&source, &static_pie, &["-static-pie"]);
    println!("cargo::rustc-env=FIXTURE_STATIC={}", static_bin.to_str().unwrap());
    println!("cargo::rustc-env=FIXTURE_STATIC_PIE={}", static_pie.to_str().unwrap());
}

fn join_out(out: &OsString, name: &str) -> OsString {
    let mut path = out.clone();
    path.push(name);
    path
}

fn compile(source: &OsString, dest: &OsString, extra: &[&str]) {
    let output = Command::new("cc")
        .args(["-O1", "-pthread"])
        .args(extra)
        .arg("-o")
        .arg(dest)
        .arg(source)
        .stderr(Stdio::piped())
        .output()
        .expect("cannot invoke cc");
    assert!(
        output.status.success(),
        "cannot compile static runtime fixture: {}",
        str::from_utf8(&output.stderr).unwrap_or("non-utf8 cc diagnostics")
    );
}
