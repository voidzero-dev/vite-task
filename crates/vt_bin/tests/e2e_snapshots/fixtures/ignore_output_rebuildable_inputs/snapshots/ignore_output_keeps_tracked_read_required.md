# ignore_output_keeps_tracked_read_required

## `vtt write-file state.json '{"value":"original"}'`

```
```

## `vt run read-state`

```
$ node read-state.mjs
```

## `vtt rm state.json result.json`

```
```

## `vt run read-state`

```
$ node read-state.mjs ○ cache miss: 'state.json' removed from workspace root, executing
```

## `vtt print-file result.json`

```
{"read":"missing"}
```
