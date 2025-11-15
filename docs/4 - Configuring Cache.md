# Configuring Cache

Fields `cache`, `envs`, `passthroughEnvs`, `inputs`, and `outputs` in `vite-task.json` can be used to configure caching behavior for tasks. All of them are optional. The default values are:

```jsonc
{
  "cache": true,
  "envs": [],
  "passthroughEnvs": [
    "PATH",
    "HOME" /* ... */
  ],
  "inputs": "inferred",
  "outputs": [],
  "idempotent": false
}
```

> You generally don't need to configure caching for vite+ subcommands like `vite build` and `vite test`. They are automatically adjusted. The default values above are for 3rd party tools only.

### `cache` (boolean, default: `true`)

Caching is enabled by default, so scripts in `package.json` are cached unless explicitly disabled.

Caching is automatically disabled for tasks that fail or receive user input from stdin (such as prompts or `Ctrl-C`), so it's generally safe to enable caching for long-running tasks like dev servers, because they won't be cached anyway when they exit on `Ctrl-C`.

> If cache is disabled, all the other fields mentioned in this pages are irrelevant.

Here are some scenarios where you may want to disable caching explicitly:

```jsonc
// vite-task.json
{
  "tasks": {
    "server": {
      "command": "node start-server.js",
      // If this server may exit normally without any stdin interaction,
      // Vite Task cannot determine its long-running nature automatically.
      // We have to disable caching explicitly.
      "cache": false
    },
    "fetch": {
      "command": "wgets http://example.com/data",
      // This task produces non-deterministic outputs from network requests.
      "cache": false
    },
    "roll-dice": {
      "command": "shuf -i 1-6 -n 1",
      // This task produces non-deterministic outputs from RNG.
      "cache": false
    }
  }
}
```

### `envs` (array of strings, default: `[]`)

`envs` specifies a list of environment variables that are passed to the task and are considered when fingerprinting for caching. If any of these environment variables change, the cache will be invalidated and the task will be re-executed.

By default, no environment variables are included in the cache fingerprint, and only those listed in `passthroughEnvs` are passed to the task.

> The decision to pass no environment variables by default avoids unintentional cache misses due to unrelated changes. Some environment variables are updated by the shell very frequently, even after every command execution, which would make caching useless.

### `passthroughEnvs` (array of strings, default: see above)

`passthroughEnvs` specifies a list of environment variables that are always passed to the task, but are not considered when fingerprinting for caching. This is useful for variables that are necessary for the task to run but should not affect caching, such as `PATH` and `HOME`.

Example:

```jsonc
// vite-task.json
{
  "tasks": {
    "build": {
      "command": "webpack",
      // This task reads NODE_ENV,
      // and changes to NODE_ENV should invalidate the cache.
      "envs": ["NODE_ENV"],
      // This task reads GITHUB_TOKEN,
      // but changes to GITHUB_TOKEN should not invalidate the cache.
      "passthroughEnvs": ["GITHUB_TOKEN"]
    }
  }
}
```

### `inputs` (array of strings or `"inferred"`, default: `"inferred"`)

`inputs` specifies the input files or directories that are considered when fingerprinting for caching. You can specify specific files or directories, or use glob patterns. If set to `"inferred"`, Vite Task will automatically infer the input files.

> `inferred` mode is guaranteed to work for vite+ subcommands like `vite build` and `vite test`. For other tools, it may not infer all input files correctly or may be too cautious and include unnecessary files. In such cases, you can manually specify the input files.

Example:

```jsonc
// vite-task.json
{
  "tasks": {
    "install": {
      "command": "pnpm install",
      // Changing these files invalidates the cache for `pnpm install`
      "inputs": [
        "package.json",
        "pnpm-lock.yaml",
        "patches/**"
      ]
    }
  }
}
```

### `outputs` (array of strings, default: `[]`)

`outputs` specifies the output files or directories that are produced by the task. Vite Task will cache these output files and restore them when the cache is hit. You can specify specific files or directories, or use glob patterns. If not specified, no output files are cached.

> stdout and stderr are always cached regardless of this setting.

> `outputs` for `vite build` are automatically configured without the need for manual setup.

Example:

```jsonc
// vite-task.json
{
  "tasks": {
    "build": {
      "command": "webpack",
      // Saving files in folder "dist" in cache,
      // and restoring them when cache is hit.
      "outputs": [
        "dist/**"
      ]
    }
  }
}
```

### `idempotent` (boolean, default: `false`)

Vite Task infers all filesystem reads and writes during task execution. If it detects that a task reads and writes the same file, Vite Task will not cache the task because an immediate re-execution may produce different results.

However, some tools are designed to be idempotent, meaning that re-executing them will not change the file again. For example, a code formatter may read and write the same source files, but running it multiple times will not change the files after the first run.

Vite Task cannot infer idempotency automatically for third-party tools, so you can explicitly mark a task as idempotent to allow caching even if it reads and writes the same file.

> `vite lint` and `vite format` are automatically marked as idempotent.

Example:

```jsonc
// vite-task.json
{
  "tasks": {
    // Formatters and autofix linters are usually idempotent
    "format": {
      "command": "prettier --write src/**",
      "idempotent": true
    },
    "autofix-lint": {
      "command": "eslint --fix src/**",
      "idempotent": true
    }
  }
}
```

## Cache Restrictions on Compound Tasks

When a task is expanded into multiple steps or nested tasks (see [Task Orchestration](3%20-%20Task%20Orchestration.md) for details), Vite Task only supports caching each individual step separately. The overall task cannot be cached as a whole, and trying to enable caching for such tasks will result in an error.

Example:

```jsonc
// vite-task.json
{
  "tasks": {
    "build-and-test": {
      "command": "vite build && vite test",
      // Error: caching is not supported for multi-step tasks.
      // It's not necessary anyway, as each step is already cached separately.
      "cache": true
    }
  }
}
```
