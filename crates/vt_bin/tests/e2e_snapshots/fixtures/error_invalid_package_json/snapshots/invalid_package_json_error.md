# invalid_package_json_error

Tests that package parse errors display paths without Rust debug formatting

## `vtt cp packages/broken/package.invalid packages/broken/package.json`

```
```

## `vt run build`

**Exit code:** 1

```
error: Failed to load task graph
* Failed to load package graph
* Failed to parse JSON file at <workspace>/packages/broken/package.json
* expected value at line 3 column 1
```
