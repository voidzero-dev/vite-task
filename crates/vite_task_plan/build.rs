#![expect(clippy::disallowed_types, reason = "build script uses std types")]

fn main() {
    // OUT_DIR is something like `<target>/<profile>/build/<crate>-<hash>/out`.
    // Navigate up to `<target>/<profile>` to find workspace binaries.
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_path = std::path::Path::new(&out_dir);
    // out -> build/<crate>-<hash> -> build -> <profile>
    let profile_dir = out_path.parent().unwrap().parent().unwrap().parent().unwrap();

    for (env_name, bin_name) in [
        ("COMPILE_TIME_VT_PATH", if cfg!(windows) { "vt.exe" } else { "vt" }),
        ("COMPILE_TIME_VTT_PATH", if cfg!(windows) { "vtt.exe" } else { "vtt" }),
    ] {
        println!("cargo::rustc-env={env_name}={}", profile_dir.join(bin_name).display());
    }
}
