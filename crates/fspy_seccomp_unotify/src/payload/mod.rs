mod filter;
use wincode::{SchemaRead, SchemaWrite};
pub use filter::Filter;

#[derive(Debug, SchemaWrite, SchemaRead, Clone)]
pub struct SeccompPayload {
    pub(crate) ipc_path: Vec<u8>,
    pub(crate) filter: Filter,
}
