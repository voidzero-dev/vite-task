<p align="center">
  <br>
  <br>
  <b>Vite Task</b>
  <br>
  <br>
  <br>
</p>

<div align="center">

[![MIT licensed][badge-license]][url-license]
[![Discord chat][badge-discord]][discord-url]

</div>

# Vite Task

Vite Task is a high-performance monorepo task runner written in Rust, designed for JavaScript/TypeScript projects. Think [Nx](https://nx.dev/) or [Turborepo](https://turbo.build/), but faster — with intelligent caching, dependency resolution, and precise file system tracking.

> **Note:** Vite Task is officially released as part of [Vite+](https://github.com/voidzero-dev/vite-plus), the unified toolchain for the web. Vite+ uses the `vite_task` library crate to power its `vp run` command. This repository contains the standalone source code for development and contribution purposes — see the [Contributing Guide](CONTRIBUTING.md) for details.

## Features

- **Fast**: Written in Rust for maximum performance
- **Intelligent Caching**: Skips tasks whose inputs haven't changed, using precise file system access tracking via `fspy`
- **Dependency Resolution**: Automatically resolves task dependencies — both explicit (`dependsOn`) and topological (based on `package.json` dependencies)
- **Cross-Platform**: Works on macOS, Linux, and Windows
- **Workspace-Aware**: Detects monorepo workspaces and package dependency graphs
- **Built-in Tool Runners**: Run `vitest`, `oxlint`, and other tools from `node_modules/.bin` without extra configuration

## Quick Start

### Run a task

```bash
# Run a task in the current package
vp run build

# Run a task in a specific package
vp run app#build

# Run a task across all packages
vp run build -r

# Run a task in the current package and its dependencies
vp run build -t
```

### Built-in commands

```bash
vp test [args...]     # run vitest
vp lint [args...]     # run oxlint
```

## Task Configuration

Tasks are defined in `vite-task.json` in each package:

```json
{
  "tasks": {
    "build": {
      "command": "tsc -b",
      "dependsOn": ["^build"],
      "cache": true
    },
    "test": {
      "command": "vitest run",
      "dependsOn": ["build"],
      "cache": true
    }
  }
}
```

## CLI Flags

| Flag | Description |
| --- | --- |
| `-r, --recursive` | Run across all packages in the workspace |
| `-t, --transitive` | Run in the current package and its dependencies |
| `--ignore-depends-on` | Skip explicit `dependsOn` dependencies |

## Architecture

Vite Task is structured as a Rust workspace with the following crates:

| Crate | Description |
| --- | --- |
| `vite_task` | Core task runner with caching and session management |
| `vite_task_bin` | CLI binary (`vp` command) and task synthesizer |
| `vite_task_graph` | Task dependency graph construction and config loading |
| `vite_task_plan` | Execution planning (resolves env vars, working dirs, commands) |
| `vite_workspace` | Workspace detection and package dependency graph |
| `fspy` | File system access tracking for precise cache invalidation |
| `vite_path` | Type-safe path abstractions (`AbsolutePath` / `RelativePath`) |
| `vite_str` | Optimized string types |
| `vite_glob` | Glob pattern matching |
| `pty_terminal` | PTY-based terminal management |

## VoidZero Inc.

Vite Task is a project of [VoidZero](https://voidzero.dev/).

## Contributing

We would love to have more contributors involved!

To get started, please read our [Contributing Guide](CONTRIBUTING.md).

## License

This project is licensed under the [MIT License](LICENSE).

[badge-license]: https://img.shields.io/badge/license-MIT-blue.svg
[url-license]: https://github.com/nicolo-ribaudo/nicolo-ribaudo/blob/main/LICENSE
[badge-discord]: https://img.shields.io/discord/1079625926024900739?logo=discord&label=Discord
[discord-url]: https://chat.vitejs.dev
