//! Build-script guard for crates that are consumed as artifact dependencies.
//!
//! This is the producer side of the artifact contract, and it has to live
//! here rather than in `materialized_artifact_build`: that crate runs from the
//! *consuming* crate's build script, which only ever sees its own profile.
//! A consumer's build script reports `OPT_LEVEL=3` while the artifact it just
//! embedded was built at 0, and the artifact's path records nothing about how
//! it was compiled. Only the artifact crate's own build script can tell.

use std::env;

/// Fails the build when the calling crate is compiled without optimizations
/// in a release build.
///
/// Cargo compiles artifact dependencies declared under `[build-dependencies]`
/// with the build-override profile — opt-level 0 — even in release builds, and
/// it only reads profiles from the root manifest of the workspace being built.
/// A workspace that depends on this repo therefore has to opt in explicitly,
/// and nothing tells it otherwise: the output still lands under
/// `target/<triple>/release/`, and the only symptom is a slower binary. See
/// <https://github.com/rust-lang/cargo/issues/16719>.
///
/// Call this from the artifact crate's own build script, where `OPT_LEVEL`
/// reports that crate's own profile rather than its parent's.
///
/// Only the optimization level is checked. Cargo passes `OPT_LEVEL`, `DEBUG`
/// and `PROFILE` to build scripts but not `codegen-units` or `strip`, so the
/// rest of the recommended profile block cannot be verified here — which is
/// why the failure prints the whole block rather than just the one setting.
/// `strip` in particular must be restated by anyone adding an override at all:
/// a package override that omits it silently resets an inherited
/// `strip = "symbols"` to `"debuginfo"`.
///
/// # Panics
///
/// Panics — failing the build — when `PROFILE` is `release` and `OPT_LEVEL` is
/// `0`.
pub fn require_optimized_in_release() {
    // `PROFILE` is `release` for release-like profiles and `debug` otherwise,
    // so debug builds — where opt-level 0 is correct — never trip this.
    if env::var("PROFILE").as_deref() != Ok("release")
        || env::var("OPT_LEVEL").as_deref() != Ok("0")
    {
        return;
    }
    let package = env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "this crate".into());
    panic!(
        "\n\n\
         `{package}` was built without optimizations in a release build.\n\n\
         It is an artifact dependency, which Cargo compiles with the\n\
         build-override profile (opt-level 0) even in release builds. Profiles\n\
         only take effect from the root manifest of the workspace being built,\n\
         so add this to the root Cargo.toml of *your* workspace:\n\n    \
         [profile.release.package.{package}]\n    \
         opt-level = 3\n    \
         codegen-units = 1\n    \
         strip = \"symbols\"\n\n\
         Keep all three: an override that omits `strip` resets it to\n\
         \"debuginfo\", and `codegen-units` is dropped by the build-override\n\
         profile. Without this block the crate ships unoptimized and nothing\n\
         else reports it.\n"
    );
}
