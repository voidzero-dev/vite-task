# Task Listing

`vp run --list` shows all available tasks across the workspace and exits without running anything. Combined with `--json`, it produces machine-readable output for CI pipelines, shell completion, and tooling integration.

## `--list`

```sh
vp run --list
```

Prints a flat list of all tasks with their commands:

```
@vrowzer/fs#build           tsdown
@vrowzer/fs#build:docs      typedoc --excludeInternal
@vrowzer/example-vue#build  vite build
@vrowzer/example-vue#dev    vite
test:unit:vite              pnpm run --color "/^test:unit:vite:/"
test:unit:vrowzer           vitest run --project vrowzer:unit
typecheck                   pnpm -r --if-present typecheck
```

Tasks from named packages use the `package#task` format. Tasks from the workspace root (when the root `package.json` has no `name` field) are shown without a prefix.

The flag is mutually exclusive with `--last-details`.

## `--json`

```sh
vp run --list --json
```

Outputs the task list as a JSON array. Requires `--list`.

```json
[
  {
    "package": "@vrowzer/fs",
    "task": "build",
    "command": "tsdown",
    "packagePath": "packages/fs"
  },
  {
    "package": "@vrowzer/fs",
    "task": "build:docs",
    "command": "typedoc --excludeInternal",
    "packagePath": "packages/fs"
  },
  {
    "package": "@vrowzer/example-vue",
    "task": "build",
    "command": "vite build",
    "packagePath": "examples/vue"
  },
  {
    "package": "@vrowzer/example-vue",
    "task": "dev",
    "command": "vite",
    "packagePath": "examples/vue"
  },
  {
    "package": null,
    "task": "test:unit:vite",
    "command": "pnpm run --color \"/^test:unit:vite:/\"",
    "packagePath": "."
  },
  {
    "package": null,
    "task": "test:unit:vrowzer",
    "command": "vitest run --project vrowzer:unit",
    "packagePath": "."
  },
  {
    "package": null,
    "task": "typecheck",
    "command": "pnpm -r --if-present typecheck",
    "packagePath": "."
  }
]
```

The `package` field is `null` when the task belongs to a workspace root whose `package.json` has no `name` field. `packagePath` is the package directory relative to the workspace root.

## Filtering

`--list` works with the standard package filter flags to scope the output:

```sh
vp run --list -r                         # all packages (same as bare --list)
vp run --list -F app                     # only tasks in "app" package
vp run --list --filter "./packages/*"    # packages under packages/
vp run --list -w                         # workspace root only
```

Without any filter flag, `--list` shows tasks from all packages in the workspace.

## Use Cases

### Onboarding

New developers can quickly see what tasks are available across the monorepo:

```sh
vp run --list
```

### CI/CD Task Verification

Check whether a task exists before running it:

```sh
if vp run --list --json | jq -e '.[] | select(.task == "typecheck")' > /dev/null; then
  vp run typecheck
fi
```

### Shell Completion

Dynamically populate tab-completion candidates:

```sh
# zsh completion function
_vp_run_tasks() {
  local tasks=$(vp run --list --json | jq -r '.[].task')
  compadd $tasks
}
```

### AI/LLM Integration

LLMs and AI agents can retrieve structured task information to understand the project:

```sh
vp run --list --json
# Returns task names, commands, and package paths as structured JSON
```

### Documentation Generation

Generate a Markdown task inventory from JSON:

```sh
vp run --list --json | jq -r '.[] | "- `\(.package // "(root)")#\(.task)`: \(.command)"'
```

### Package-Scoped Inspection

Inspect tasks for a specific package or directory:

```sh
# Tasks in the "app" package
vp run --list -F app

# Tasks under the infra directory
vp run --list --filter "./infra/*"
```

## Flag Interactions

| Combined with    | Behavior                                   |
| ---------------- | ------------------------------------------ |
| `--json`         | Output as JSON array instead of plain text |
| `-r`             | Show all packages (same as bare `--list`)  |
| `-F <pattern>`   | Scope to matching packages                 |
| `--filter <pat>` | Scope to matching packages                 |
| `-w`             | Show workspace root tasks only             |
| `-t`             | Show current package and transitive deps   |
| `--last-details` | **Error** — mutually exclusive             |

`--json` without `--list` is an error.
