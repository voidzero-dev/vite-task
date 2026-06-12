#![expect(
    clippy::disallowed_types,
    clippy::disallowed_macros,
    reason = "build.rs interfaces with std::path and cargo's env-var API"
)]

use std::{env, path::Path};

use anyhow::Context;

// Why `cfg(fspy)` instead of matching on `target_os` directly at each use site:
// "fspy is available" is a single semantic predicate, but the underlying reason
// (the `fspy` crate builds on windows/macos/linux) is a three-OS list that
// would otherwise have to be repeated — as `any(target_os = "windows", "macos",
// "linux")` — everywhere `fspy::*` is touched. Naming it `fspy` keeps the
// source self-documenting: code reads `#[cfg(fspy)]` instead of a disjunction
// over OSes. The OS allowlist lives in two spots that must stay in sync: this
// file (for the rustc cfg) and the target-scoped dep block in Cargo.toml
// (which Cargo resolves before build.rs runs, so it can't reuse this cfg).
fn main() -> anyhow::Result<()> {
    println!("cargo::rustc-check-cfg=cfg(fspy)");
    println!("cargo::rerun-if-changed=build.rs");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    if matches!(target_os.as_str(), "windows" | "macos" | "linux") {
        println!("cargo::rustc-cfg=fspy");
    }

    let env_name = "CARGO_CDYLIB_FILE_VITE_TASK_CLIENT_NAPI";
    println!("cargo:rerun-if-env-changed={env_name}");
    let dylib_path = env::var_os(env_name).with_context(|| format!("{env_name} not set"))?;
    materialized_artifact_build::register("vite_task_client_napi", Path::new(&dylib_path));
    Ok(())
}
