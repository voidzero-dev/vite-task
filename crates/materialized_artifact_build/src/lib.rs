use std::{fs, path::Path};

/// Namespace prefix for the env vars set by [`register`] and consumed by
/// `materialized_artifact`'s `artifact!` macro. Exported so both crates agree
/// on the same prefix.
pub const ENV_PREFIX: &str = "MATERIALIZED_ARTIFACT_";

/// Publish an artifact at `path` so `materialized_artifact`'s `artifact!($name)`
/// macro can embed it.
///
/// Emits three `cargo:…` directives:
/// `rerun-if-changed={path}`,
/// `rustc-env=MATERIALIZED_ARTIFACT_{name}_PATH={path}`, and
/// `rustc-env=MATERIALIZED_ARTIFACT_{name}_HASH={hex}`. The runtime resolves
/// these at compile time via `include_bytes!(env!(…))` and `env!(…)`.
///
/// `name` is used both as the env-var key and as the on-disk filename prefix
/// (in `Materialize::at`), so it must be a valid identifier-like string
/// that matches the one passed to `artifact!`.
///
/// # Panics
///
/// Panics if `path` is not valid UTF-8 or cannot be read.
pub fn register(name: &str, path: &Path) {
    let path_str = path.to_str().expect("artifact path must be valid UTF-8");
    #[expect(clippy::print_stdout, reason = "cargo build-script directives")]
    {
        // Emit rerun-if-changed before reading so cargo still sees it even if
        // reading the file below panics.
        println!("cargo:rerun-if-changed={path_str}");
        let bytes =
            fs::read(path).unwrap_or_else(|e| panic!("failed to read artifact at {path_str}: {e}"));
        let hash = format!("{:x}", xxhash_rust::xxh3::xxh3_128(&bytes));
        println!("cargo:rustc-env={ENV_PREFIX}{name}_PATH={path_str}");
        println!("cargo:rustc-env={ENV_PREFIX}{name}_HASH={hash}");
    }
}
