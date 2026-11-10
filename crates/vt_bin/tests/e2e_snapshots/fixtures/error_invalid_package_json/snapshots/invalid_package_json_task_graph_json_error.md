# invalid_package_json_task_graph_json_error

With `--json`, a load error goes to stderr and nothing goes to stdout

## `vtt cp packages/broken/package.invalid packages/broken/package.json`

```
```

## `vtt pipe-stdout -- vt run --json`

**Exit code:** 1

```
error: Failed to load package graph
* Failed to parse JSON file at <workspace>/packages/broken/package.json
* expected value at line 3 column 1
--- stdout ---
```
