# Contributing

Thank you for your interest in contributing to Vite Task!

We welcome and appreciate any form of contributions.

## Development Binary vs Official Release

This repository builds the `vp` development binary via the `vite_task_bin` crate. This binary is a **standalone dev build** for testing Vite Task in isolation.

In production, Vite Task is integrated into [Vite+](https://github.com/voidzero-dev/vite-plus) — the unified toolchain for the web. End users access Vite Task functionality through the `vp run` command in Vite+, where it provides monorepo task execution with caching and dependency-aware scheduling alongside Vite+'s other capabilities (dev server, testing, linting, building, etc.).

When contributing to this repository, use the dev binary built by `vite_task_bin` (`cargo run -p vite_task_bin`) to test your changes locally.

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) (see `rust-toolchain.toml` for the required version)
- [just](https://github.com/casey/just) (command runner)
- [pnpm](https://pnpm.io/) (for JavaScript tooling and integration tests)

### Setup

```bash
# Install build tools and dependencies
just init

# Install JS dependencies for integration tests
pnpm install --dir packages/tools
```

### Development Workflow

```bash
# Run the full quality check (format, compile check, test, lint, docs)
just ready

# Or run individual steps:
just fmt       # Format code (cargo fmt, cargo shear, dprint)
just check     # Check compilation with all features
just test      # Run all tests
just lint      # Clippy linting
just doc       # Documentation generation
```

### Running Tests

```bash
# All tests
cargo test

# E2E snapshot tests
cargo test -p vite_task_bin --test e2e_snapshots

# Plan snapshot tests
cargo test -p vite_task_plan --test plan_snapshots

# Filter by test name
cargo test --test e2e_snapshots -- stdin

# Update snapshots
INSTA_UPDATE=always cargo test
```

Integration tests (e2e, plan, fspy) require `pnpm install` in `packages/tools` first.

### Cross-Platform Testing

This project must work on both Unix (macOS/Linux) and Windows. You can cross-compile and test on Windows from macOS using:

```bash
cargo xtest --builder cargo-xwin --target aarch64-pc-windows-msvc -p <package> --test <test>
```

## Code Guidelines

### Required Patterns

These patterns are enforced by `.clippy.toml`:

| Instead of | Use |
| --- | --- |
| `HashMap`/`HashSet` | `FxHashMap`/`FxHashSet` from `rustc-hash` |
| `std::path::Path`/`PathBuf` | `vite_path::AbsolutePath`/`RelativePath` |
| `std::format!` | `vite_str::format!` |
| `String` (for small strings) | `vite_str::Str` |
| `std::env::current_dir` | `vite_path::current_dir` |
| `.to_lowercase()`/`.to_uppercase()` | `cow_utils` methods |

### Cross-Platform Requirements

All code must work on both Unix and Windows:

- Use `#[cfg(unix)]` / `#[cfg(windows)]` for platform-specific implementations
- Always test on both platforms — skipping tests on either platform is **not acceptable**
- Platform differences should be handled gracefully, not skipped

### Cross-Platform Linting

After major changes (especially to `fspy*` or platform-specific crates), run cross-platform clippy:

```bash
just lint          # native (host platform)
just lint-linux    # Linux via cargo-zigbuild
just lint-windows  # Windows via cargo-xwin
```

## AI Usage Policy

When using AI tools (including LLMs like ChatGPT, Claude, Copilot, etc.) to contribute:

- **Please disclose AI usage** to reduce maintainer fatigue
- **You are responsible** for all AI-generated issues or PRs you submit
- **Low-quality or unreviewed AI content will be closed immediately**

We encourage the use of AI tools to assist with development, but all contributions must be thoroughly reviewed and tested by the contributor before submission.
