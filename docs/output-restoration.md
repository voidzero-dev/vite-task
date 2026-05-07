# Output Restoration

When a cached task is replayed, vp doesn't just replay the terminal output — it also restores any files the task produced. So if your `build` task writes to `dist/`, a cache hit will put those files right back where they belong.

## How It Works

By default, vp watches which files a task writes during execution. On the next run, if the cache hits, those files are extracted back into the workspace automatically. You don't need to configure anything.

```json
{
  "tasks": {
    "build": {
      "command": "tsc --outDir dist"
    }
  }
}
```

After `vp run build` runs once, the compiled files in `dist/` are archived. On subsequent runs that hit the cache, `dist/` is restored from that archive instead of recompiling.

## The `output` Field

The `output` field lets you control which files get archived and restored. It works exactly like `input` — same glob patterns, same `auto` directive, same negative patterns.

### Default (omitted)

Auto-detects written files. This is usually all you need:

```json
{
  "tasks": {
    "build": {
      "command": "tsc --outDir dist"
    }
  }
}
```

### Explicit Globs

If you only want specific files restored:

```json
{
  "tasks": {
    "build": {
      "command": "tsc --outDir dist",
      "output": ["dist/**"]
    }
  }
}
```

This ignores any other files the task might write (temp files, logs, etc.) and only archives what's under `dist/`.

### Negative Patterns

Exclude certain output files from being cached:

```json
{
  "tasks": {
    "build": {
      "command": "webpack",
      "output": [{ "auto": true }, "!dist/cache/**"]
    }
  }
}
```

Here, everything the task writes is archived _except_ files under `dist/cache/`.

### Disable Output Restoration

If you don't want any files restored on cache hit — only terminal output replayed — pass an empty array:

```json
{
  "tasks": {
    "test": {
      "command": "vitest run",
      "output": []
    }
  }
}
```

## `output` and `input` Are Independent

The `output` field has its own auto-detection, separate from `input`. You can disable auto-inference for inputs while keeping it for outputs:

```json
{
  "tasks": {
    "build": {
      "command": "tsc --outDir dist",
      "input": ["src/**/*.ts", "tsconfig.json"],
      "output": [{ "auto": true }]
    }
  }
}
```

This tracks inputs via explicit globs only (no file-read inference), but still auto-detects which files the task writes for output restoration.

## Configuration Summary

| Configuration                         | Behavior                            |
| ------------------------------------- | ----------------------------------- |
| `output` omitted                      | Auto-detect written files (default) |
| `output: [{ "auto": true }]`          | Same as omitted                     |
| `output: ["dist/**"]`                 | Only restore files under `dist/`    |
| `output: [{ "auto": true }, "!tmp/"]` | Auto-detect, but skip `tmp/`        |
| `output: []`                          | Don't restore any files             |

## Notes

- Changing the `output` configuration invalidates the cache — if you switch from `["dist/**"]` to `["build/**"]`, the next run will be a cache miss.
- Output archives are stored alongside the cache database. Running `vp cache clean` removes them.
- The `output` field can't be used with `cache: false`, same as `input`.
