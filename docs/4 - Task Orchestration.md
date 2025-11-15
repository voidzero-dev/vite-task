# Task Orchestration

## Compositing Tasks Inside Scripts

Vite Task allows you to composite tasks with bash-like syntaxes right inside your scripts defined in `package.json`/`vite-task.json`.

### Multi-step Tasks

You may have already use `&&` in your scripts to run multiple commands in sequence. Vite Task regognizes this pattern and caches each step individually.

For example:

```json
// package.json
{
  "name": "app",
  "scripts": {
    "build": "vite build && vite preview"
  }
}
```

Vite Task will show `vite build` and `vite preview` as individual commands with their own cache status under the `build` task.

- `app#build`
  - `vite build`
  - `vite preview`

### Nested Tasks

Vite Task recursively expands `vite run ...` in scripts to run nested tasks directly instead of spawning a new vite task subprocess. This gives you a cleaner overview of all the executions and avoids unnecessary process spawning overhead.

```json
// package.json
{
  "name": "monorepoRoot",
  "scripts": {
    "ready": "vite run format && vite run -r build",
    "format": "dprint fmt && vite fmt"
  }
}
```

Vite Task will show:

- `monorepoRoot#ready`
  - `monorepoRoot#format`
    - `dprint fmt`
    - `vite fmt`

### Supported Syntaxes

In order for multi-step and nested tasks to be recognized correctly, Vite Task currently supports a subset of bash syntaxes:

- Simple commands: `program arg1 arg2 ...`
- Commands prefixed with environment variables: `VAR=value program arg1 arg2`
- Referencing variables with `$`, supporting default values: `program $FOO a${BAR}b ${BAZ:42}`
- Sequential commands: `program1 && VAR=value program2 $FOO && ...`

If a script contains syntaxes beyoud these, Vite Task falls back to normal script execution with system shells. For example, the following script will not be split into multiple steps because of the `if` statement:

```json
{
  "scripts": {
    "complex": "if [ -f file.txt ]; then vite lint && vite build ; fi"
  }
}
```

Note that even if a script is not expanded, Vite Task is still able to **cache the entire script execution as a single unit**.

If you put a `vite run ...` command inside a script with unsupported syntax, like the example below, the **inner `vite run ...` will fail** at execution time, because it is currently not supported to cache both `build` tasks and `complex` as a single unit at the same time.

```bash
{
    "scripts": {
        "complex": "if [ -f file.txt ]; then vite run -r build; fi"
    }
}
```

To make it work, you can disable caching for the outer task by adding `"cache": false` in `vite-task.json`:

```json
/// vite-task.json
{
  "tasks": {
    "complex": {
      "cache": false,
      "script": "if [ -f file.txt ]; then vite run -r build; fi"
    }
  }
}
```

## Task Dependencies

Task dependencies can be defined in `vite-task.json` file. You can specify which tasks need to be executed before a particular task runs.

```json
{
  "tasks": {
    "build": {
      "dependsOn": ["lint", "test"],
      "script": "vite build"
    },
    "lint": {
      "script": "eslint src/"
    },
    "test": {
      "script": "jest"
    }
  }
}
```
