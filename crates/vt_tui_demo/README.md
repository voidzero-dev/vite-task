# vt_tui_demo

Scripted runs played into `vt_tui`, compiled to WebAssembly for the RFC page in `web/`, where ghostty-web draws them. The scripts cover a long-running `vp run -r dev` and a `vp run verify` made of lint, build and test steps, behind a small shell that can start them again and answer `vp run --last-details`.

```bash
pnpm install
pnpm --dir crates/vt_tui_demo/web dev     # builds the wasm, then serves the page on :5199
```

The same runs play in a real terminal with the `vt_tui_demo` binary, the way `vp run` would show them: the UI when stdin and stdout are terminals, and each task's lines prefixed with its label otherwise. It exits with 1 if a task failed and 130 on Ctrl-C. AI agents are detected from the environment, as `vp run` would; `--agent` and `--no-agent` override it.

```bash
cargo run -p vt_tui_demo -- verify        # a test fails; `--pass` lets them all pass
cargo run -p vt_tui_demo -- dev           # runs until Ctrl-C
cargo run -p vt_tui_demo -- verify | cat  # plain output
```

Building the page needs the `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`) and `wasm-bindgen` 0.2.130, matching the `wasm-bindgen` crate.
