//! The `vt_client_napi` cdylib is embedded into the `vp` binary and
//! materialized to disk on first use so tools can `require()` it at runtime.

use std::sync::LazyLock;

use vt_materialized_artifact::artifact;
use vt_path::{AbsolutePath, AbsolutePathBuf};

/// Path to the materialized `vt_client_napi` `.node` addon.
///
/// The file is written to the user's own folder in the system temp dir on first
/// call and reused on every subsequent call (content-addressed filename; no
/// re-writes).
///
/// # Panics
///
/// Panics if the materialization fails on first call — this mirrors fspy's
/// `SPY_IMPL` and the same reasoning applies: if we can't write into the
/// system temp dir, the runner can't run tasks anyway.
#[must_use]
pub fn napi_client_path() -> &'static AbsolutePath {
    static PATH: LazyLock<AbsolutePathBuf> = LazyLock::new(|| {
        let path = artifact!("vt_client_napi", "CARGO_CDYLIB_FILE_VT_CLIENT_NAPI")
            .materialize()
            .suffix(".node")
            .at_user_temp_dir("vp_run")
            .expect("materialize vt_client_napi");
        AbsolutePathBuf::new(path).expect("system temp dir yields an absolute path")
    });
    PATH.as_absolute_path()
}
