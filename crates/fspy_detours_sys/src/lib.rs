#![cfg(windows)]
#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    clippy::wildcard_imports,
    reason = "non-vite crate; generated FFI bindings use wildcard imports"
)]

#[expect(non_camel_case_types, non_snake_case, reason = "generated FFI bindings")]
#[rustfmt::skip] // generated code is formatted by prettyplease, not rustfmt
mod generated_bindings;

pub use generated_bindings::*;
