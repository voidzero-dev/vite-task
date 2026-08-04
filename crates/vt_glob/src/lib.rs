//! Glob matching, split by use case:
//!
//! - [`mod@env`] — environment-variable **name** matching (flat strings),
//!   backed by `globset` with path semantics disabled.
//! - [`mod@path`] — filesystem **path** matching with gitignore semantics,
//!   backed by `wax`.
//!
//! Each module owns its own error type ([`env::EnvGlobError`] /
//! [`path::PathGlobError`]).

pub mod env;
pub mod path;
