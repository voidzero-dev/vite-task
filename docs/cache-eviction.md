# Cache Eviction

Available scoped cache eviction through `cache clear`:

```bash
vp cache clear
vp cache clear --task <task-name>
vp cache clear --package <package-name>
```

Examples:

```bash
vp cache clear
vp cache clear --task build
vp cache clear --package @my/abc
```

## Semantics

- `vp cache clear`: evict all cache entries.
- `vp cache clear --task build`: evict entries for task name `build` across workspace.
- `vp cache clear --package @my/abc`: evict entries for package `@my/abc` across all tasks.

Matching is exact (no wildcard/regex in this RFC).

## CLI constraints

- `--task` and `--package` are mutually exclusive.
- No flag means full clear.

## Output

- Success message includes cleared count.
- Non-zero exit only on invalid args or storage errors.
