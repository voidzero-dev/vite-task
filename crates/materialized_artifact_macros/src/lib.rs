//! Proc-macro side of `materialized_artifact`: embed a file and its content
//! hash at compile time, given an env var holding the file's path.
//!
//! The env var is either a Cargo artifact dependency's `CARGO_<KIND>_FILE_<DEP>`
//! (provided only while the consuming crate is compiled — no build script ever
//! sees it) or a path the consuming crate's own build script published via
//! `cargo:rustc-env`. Either way the content hash must be computed here, at
//! macro-expansion time.
//!
//! # Why `proc_macro::tracked`
//!
//! The hash and the embedded bytes are two reads of the same file through
//! different mechanisms: this macro reads it to hash, and the emitted
//! `include_bytes!` reads it to embed. Today both stay in sync because
//! expansion reruns on every compilation, and the emitted
//! `include_bytes!(env!(…))` registers the file and env var in dep-info. But
//! a future compiler that caches proc-macro expansions keyed on input tokens
//! would serve a stale hash next to fresh bytes — a silent mismatch that
//! ships. The [`tracked`] calls declare this macro's reads to the compiler
//! so any such cache invalidates correctly. They are load-bearing, not
//! defensive: do not remove them.
//!
//! [`tracked`]: proc_macro::tracked

#![feature(proc_macro_tracked_env)]
#![feature(proc_macro_tracked_path)]

use std::fs;

use proc_macro::tracked;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    LitStr, Token,
    parse::{Parse, ParseStream},
};

/// Construct a `materialized_artifact::Artifact` from an env var holding a
/// file path at compile time.
///
/// Usage: `artifact!("fspy_preload", "CARGO_CDYLIB_FILE_FSPY_PRELOAD_UNIX")`
/// where the first argument is the artifact name (used in the materialized
/// filename) and the second is the env var holding the file's path: a Cargo
/// artifact dependency's `CARGO_<KIND>_FILE_<DEP>`, or a var the consuming
/// crate's build script published via `cargo:rustc-env`.
///
/// Expands to `Artifact::__new(name, include_bytes!(env!(env_var)), "<hash>")`
/// with the xxh3-128 hex of the file computed during expansion. The expansion
/// deliberately re-references the env var and file through `env!` +
/// `include_bytes!` — embedding needs them anyway, and they double as
/// dep-info registration alongside the `proc_macro::tracked` calls.
///
/// When the env var is unset the expansion branches on `cfg(rust_analyzer)`:
/// under rust-analyzer (which never has artifact-dep env vars) it is a
/// well-typed stub with no diagnostics, while under rustc — where an unset
/// var means the artifact dependency is genuinely missing or the build
/// script didn't emit it — a `compile_error!` fails the build with an
/// actionable message. The cfg-gated tokens exist only in this unset-var
/// expansion, never in a successful build, so consumers need no
/// `check-cfg` declaration for `rust_analyzer`.
#[proc_macro]
pub fn artifact(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    // The single place errors become tokens: every failure inside is a
    // spanned `syn::Error`.
    expand(input).unwrap_or_else(syn::Error::into_compile_error).into()
}

fn expand(input: proc_macro::TokenStream) -> syn::Result<TokenStream> {
    let Args { name, env_var } = syn::parse(input)?;

    // `tracked::env_var` both reads the var and declares the read to the
    // compiler (see crate docs for why that declaration matters).
    let (content, hash, guard) = if let Ok(file_path) = tracked::env_var(env_var.value()) {
        // Declare the file read before attempting it: the dependency must
        // be registered even if the read fails, so a later fix to the file
        // retriggers this macro.
        tracked::path(&file_path);
        let bytes = fs::read(&file_path).map_err(|err| {
            syn::Error::new(
                env_var.span(),
                format!(
                    "`{}` points at {file_path}, which could not be read: {err}",
                    env_var.value()
                ),
            )
        })?;
        let hash = format!("{:x}", xxhash_rust::xxh3::xxh3_128(&bytes));
        (quote!(::core::include_bytes!(::core::env!(#env_var))), hash, TokenStream::new())
    } else {
        // Env var unset. Only the compilation context itself can tell
        // whether that is fine (rust-analyzer never has artifact-dep env
        // vars) or a real error (missing artifact dependency, build script
        // not emitting the var), so the guard lets the emitted tokens branch
        // on `cfg(rust_analyzer)` instead of guessing here.
        let message = format!(
            "`{}` is not set at compile time; declare the artifact as a Cargo artifact \
             dependency under `[dependencies]`, or publish the path from a build script via \
             `cargo:rustc-env`",
            env_var.value()
        );
        let guard = quote! {
            #[cfg(not(rust_analyzer))]
            ::core::compile_error!(#message);
        };
        (quote!(&[]), String::from("rust-analyzer-stub"), guard)
    };

    Ok(quote! {
        {
            #guard
            ::materialized_artifact::Artifact::__new(#name, #content, #hash)
        }
    })
}

/// The two arguments of [`artifact!`]: `"name", "ENV_VAR"`, with an optional
/// trailing comma.
struct Args {
    name: LitStr,
    env_var: LitStr,
}

impl Parse for Args {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name = input.parse()?;
        input.parse::<Token![,]>()?;
        let env_var = input.parse()?;
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
        Ok(Self { name, env_var })
    }
}
