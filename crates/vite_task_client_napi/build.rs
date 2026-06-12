#![expect(
    clippy::disallowed_types,
    reason = "build.rs interfaces with std::path and cargo's env-var API"
)]

extern crate napi_build;

use std::{env, fs, path::PathBuf};

fn main() {
    napi_build::setup();

    // Keep this crate's napi-derive type-defs out of any consumer's generated
    // binding.
    //
    // `vite_task_client_napi` is embedded as a cdylib *artifact* dependency of
    // `vite_task`. napi-derive's `type-def` feature is force-enabled by feature
    // unification with consumers that need it (e.g. vite-plus's CLI binding), so
    // disabling the feature here has no effect. By default napi-derive then
    // writes this crate's `#[napi]` items (`RunnerClient`/`load`) into the
    // consumer's shared `NAPI_TYPE_DEF_TMP_FOLDER`, which `@napi-rs/cli` sweeps
    // into the consumer's `index.cjs`/`index.d.cts` as dead exports (the symbols
    // live in the separately-loaded addon, not the consumer's `.node`). The
    // public JS surface is the hand-written `@voidzero-dev/vite-task-client`
    // package, so these generated defs are never needed.
    //
    // `@napi-rs/cli` reuses that folder across builds without pruning it, so
    // first remove any entry a pre-redirect build left there, then redirect this
    // crate's emission to an isolated, clearly-named sink. The override applies
    // only to this crate's rustc invocation, where the napi-derive proc-macro
    // reads the env at expansion time, so consumers' own type-defs are
    // unaffected.
    println!("cargo::rerun-if-env-changed=NAPI_TYPE_DEF_TMP_FOLDER");
    let pkg = env::var("CARGO_PKG_NAME").expect("CARGO_PKG_NAME not set");
    if let Ok(consumer_folder) = env::var("NAPI_TYPE_DEF_TMP_FOLDER") {
        let _ = fs::remove_file(PathBuf::from(consumer_folder).join(&pkg));
    }
    let sink = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"))
        .join("discarded-napi-type-defs");
    fs::create_dir_all(&sink).expect("failed to create napi type-def sink dir");
    println!("cargo::rustc-env=NAPI_TYPE_DEF_TMP_FOLDER={}", sink.display());
}
