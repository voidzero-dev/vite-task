# Vite Task

A monorepo task runner with caching and dependency resolution, distributed as `vp run` in [Vite+](https://github.com/voidzero-dev/vite-plus).

## Context by Task

- Task execution flows through `vt_bin` → `vt_graph` → `vt_plan` → `vt`; `vt_workspace` supplies the package graph, and `fspy*` traces file access.
- For crate-specific behavior, use the relevant `crates/*/README.md` and nearby implementation. For input tracking, concurrency, cancellation, or output behavior, use the corresponding guide in `docs/`.
- For task configuration and dependency resolution, use `crates/vt_graph` and the plan fixtures below. Internal configs are `vite-task.json`; task references use `package#task`.
- For build commands, use `justfile`. Toolchain and version requirements live in `rust-toolchain.toml`, `Cargo.toml`, `.node-version`, and `package.json`.

## Public Naming

Use `vp` and `vite.config.*` in code comments, documentation, and user-facing strings. The internal names `vt` and `vite-task.json` are permitted in implementation code, test fixtures, and `AGENTS.md`.

## Code Constraints

Use the project types and helpers from `vt_path`, `vt_str`, `rustc_hash`, and `cow_utils`. [`.clippy.toml`](.clippy.toml) lists the disallowed standard-library alternatives and their replacements.

- Keep paths absolute during internal data flow; convert to relative paths when saving to cache.
- Use `vt_path` operations such as `strip_prefix` and `join`. Convert to std paths only at std-library boundaries; add missing operations to `vt_path`.
- Use `vt_str::Str` and `vt_str::format!` for small strings.

## Checks and Tests

`just check`, `just lint`, and `just doc` cover compilation, Clippy, and documentation. `just fmt` runs the formatters and Cargo dependency cleanup. `just ready` runs the full quality suite and rejects unstaged tracked changes at entry.

```bash
cargo test -p vt_plan --test plan_snapshots       # Graph, config, command paths, cwd, env
cargo test -p vt_bin --test e2e_snapshots          # Execution, caching, output styling
cargo test                                      # Default suite (also: just test)
```

- [Plan snapshot tests](crates/vt_plan/tests/plan_snapshots/README.md): fixtures in `crates/vt_plan/tests/plan_snapshots/fixtures/`. Prefer these when execution is not needed to verify the behavior.
- [E2E snapshot tests](crates/vt_bin/tests/e2e_snapshots/README.md): fixtures in `crates/vt_bin/tests/e2e_snapshots/fixtures/`.
- Default `cargo test` runs tests needing only the Rust toolchain. Tests requiring Node.js or workspace packages are ignored by default. Run `pnpm install` at the workspace root, then use `cargo test -- --include-ignored` or `--ignored`; fixture directories do not need separate installs.
- For intended snapshot changes, set `UPDATE_SNAPSHOTS=1` on the affected test command and review the diff.
- There are no known failing or flaky tests. Treat failures during your changes as regressions and fix the cause; do not skip, ignore, or work around failing tests.

## Cross-Platform Requirements

- Support macOS, Linux, and Windows. Use `#[cfg(unix)]` / `#[cfg(windows)]` for platform-specific implementations, including within tests, and prefer cross-platform libraries for shared operations.
- Tests for changed behavior must exercise that behavior on both Unix and Windows; CI provides execution on each platform. The only platform-skip exception is musl when an essential dependency or capability is unavailable; document the requirement and scope the skip to musl.
- After major changes to `fspy*` or platform-specific crates, run `just lint-linux` (requires `cargo-zigbuild`) and `just lint-windows` (requires `cargo-xwin`).

## Commits, Pull Requests, and Changelog

- Use `gh pr` for standalone pull requests and `gh stack` for pull request stacks.
- PR titles use [Conventional Commits](https://www.conventionalcommits.org): `type(scope): summary`, with optional scope.
- AI-assisted commits require a `Co-authored-by:` trailer naming the actual model and version.
- PR descriptions require a `Motivation` section. Ask before creating the PR only if the request and context do not establish its motivation. Do not add `Validation`, `Test plan`, or similar sections.
- For user-facing features, behavior changes, fixes, removals, or performance improvements, update `CHANGELOG.md` following [these instructions](.claude/skills/update-changelog.md). Internal refactors, CI, dependency bumps, test fixes, and documentation changes do not need changelog entries.
