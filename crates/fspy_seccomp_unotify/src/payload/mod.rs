mod filter;
pub use filter::{Architecture, Filter};
use wincode::{SchemaRead, SchemaWrite};

#[derive(Debug, SchemaWrite, SchemaRead, Clone)]
pub struct SeccompPayload {
    pub(crate) ipc_path: Vec<u8>,
    pub(crate) filter: Filter,
    pub(crate) post_syscall_pc: u64,
}

impl SeccompPayload {
    #[must_use]
    pub const fn post_syscall_pc(&self) -> u64 {
        self.post_syscall_pc
    }
}
