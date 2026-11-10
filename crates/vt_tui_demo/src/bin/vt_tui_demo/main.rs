//! `vt_tui_demo`: plays a scripted run in this terminal, the way `vp run`
//! would show it: the UI when stdin and stdout are terminals, plain output
//! otherwise.

#[cfg(not(target_arch = "wasm32"))]
mod cli;
#[cfg(not(target_arch = "wasm32"))]
mod terminal;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    cli::main()
}

/// On WebAssembly the demo runs in the browser page instead; see `web/`.
#[cfg(target_arch = "wasm32")]
const fn main() {}
