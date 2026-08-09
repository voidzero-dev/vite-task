fn main() {
    // Injected into every traced process; see the unix preload's build.rs.
    artifact_profile::require_optimized_in_release();

    if std::env::var_os("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:DetourFinishHelperProcess,@1,NONAME");
    }
}
