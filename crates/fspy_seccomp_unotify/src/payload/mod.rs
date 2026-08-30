mod filter;
pub use filter::Filter;
use wincode::{SchemaRead, SchemaWrite};

#[derive(Debug, SchemaWrite, SchemaRead, Clone)]
pub struct SeccompPayload {
    pub(crate) ipc_path: Vec<u8>,
    pub(crate) filter: Filter,
}

impl SeccompPayload {
    /// Builds a payload whose installation is guaranteed to fail: the filter
    /// is empty (the kernel refuses it) and nothing listens on `ipc_path`.
    ///
    /// Test support for the untracked-exec fallback: integration tests in
    /// dependent crates use it to exercise a failing [`crate::target`]
    /// install without a restricted sandbox.
    #[doc(hidden)]
    #[must_use]
    pub fn unreachable(ipc_path: Vec<u8>) -> Self {
        Self { ipc_path, filter: Filter(Vec::new()) }
    }
}
