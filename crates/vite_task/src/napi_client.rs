//! The `vite_task_client_napi` cdylib is embedded into the `vp` binary and
//! materialized to disk on first use so tools can `require()` it at runtime.

use std::{env, fs, sync::LazyLock};

use materialized_artifact::artifact;
use vite_path::{AbsolutePath, AbsolutePathBuf};

/// Path to the materialized `vite_task_client_napi` `.node` addon.
///
/// The file is written to a process-wide temp directory on first call and
/// reused on every subsequent call (content-addressed filename; no re-writes).
///
/// # Panics
///
/// Panics if the materialization fails on first call — this mirrors fspy's
/// `SPY_IMPL` and the same reasoning applies: if we can't write into the
/// system temp dir, the runner can't run tasks anyway.
#[must_use]
pub fn napi_client_path() -> &'static AbsolutePath {
    static PATH: LazyLock<AbsolutePathBuf> = LazyLock::new(|| {
        let dir = env::temp_dir().join("vite_task_client_napi");
        let _ = fs::create_dir(&dir);
        let path = artifact!("vite_task_client_napi")
            .materialize()
            .suffix(".node")
            .at(&dir)
            .expect("materialize vite_task_client_napi");
        AbsolutePathBuf::new(path).expect("system temp dir yields an absolute path")
    });
    PATH.as_absolute_path()
}
