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

<table>
  <tbody>
    <tr valign="top" align="left">
      <td>
        <ul>
          <li><code>app#build</code>
            <ul>
              <li><code>vite build</code></li>
              <li><code>vite preview</code></li>
              <ul>
          </li>
        </ul>
      </td>
      <td>
      <pre>
$ vite build

<small>(cache hit, replaying)</small><br />
VITE+ v1.0.0 building for production
transforming...
✓ 32 modules transformed...
rendering chunks...
computing gzip size...
dist/index.html 0.46 kB | gzip: 0.30 kB
dist/assets/react-CHdo91hT.svg 4.13 kB | gzip: 2.05 kB
dist/assets/index-D8b4DHJx.css 1.39 kB | gzip: 0.71 kB
dist/assets/index-CAl1KfkQ.js188.06 kB | gzip: 59.21 kB
✓ built in 308ms
</pre>

</td>
</tr>

</tbody>
</table>

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

<table>
  <tbody>
    <tr valign="top" align="left">
      <td>
        <ul>
          <li><code>monorepoRoot#ready</code>
            <ul>
              <li><code>vite run format</code>
              <ul>
                <li><code>dprint fmt</code></li>
                <li><code>vite fmt</code></li>
              </ul>
              </li>
              <li><code>vite run -r build</code>
              <ul>
              <li><code>pkg1#build</code></li>
              <li><code>pkg2#build</code></li>
              <li><code>pkg3#build</code></li>
          </li>
        </ul>
      </td>
      <td>
      <pre>
$ vite lint

<small>(cache hit, replaying)</small><br />
VITE+ v1.0.0 lint
Found 0 warnings and 0 errors.
✓ Finished in 1ms on 3 files with 88 rules using 10 threads.
</pre>

</td>
</tr>

</tbody>
</table>

### Supported Syntaxes

In order for multi-step and nested tasks to be recognized correctly, Vite Task currently supports a subset of bash syntaxes:

- Simple commands: `program arg1 arg2 ...`
- Commands prefixed with environment variables: `VAR=value program arg1 arg2`
- Referencing variables with `$`: `program $FOO a${BAR}b ${BAZ:42}`
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
      "command": "if [ -f file.txt ]; then vite run -r build; fi"
    }
  }
}
```

## Task Dependencies

Task dependencies can be defined in `vite-task.json` file. You can specify which tasks need to be executed before a particular task runs:

```json
{
  "tasks": {
    "build": {
      "command": "vite build",
      "dependsOn": ["lint", "ui#test", "^build"]
    },
    "lint": {
      "script": "vite lint"
    }
  }
}
```

- `lint` refers to the `lint` task in the same package.
- `ui#test` refers to the `test` task in the `ui` package.
- `^build` refers to all the tasks named `build` in the dependencies of the current package.
