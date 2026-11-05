# invalid_json_in_unreached_package

`packages/broken/vite-task.json` isn't valid JSON. `vt run -t build` in `packages/app` never needs `@test/broken`'s config; `-r` reaches it.

## `vtt cp ../broken/vite-task.invalid ../broken/vite-task.json`

```
```

## `vt run -t build`

app and lib only

**Exit code:** 1

```
error: Failed to load task graph
* Failed to load task config file for package at <workspace>/packages/broken
* unexpected end of input on line 1 column 1
```

## `vt run -r build`

reaches @test/broken

**Exit code:** 1

```
error: Failed to load task graph
* Failed to load task config file for package at <workspace>/packages/broken
* unexpected end of input on line 1 column 1
```
