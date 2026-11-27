# nested_runs

## `vt run --dry-run nested`

A nested `vt run` is part of the dry run.

```
$ vtt print-file src/a.txt
```

## `vt run --dry-run nested-dry-run`

A nested `vt run --dry-run` is a process of its own, which a dry run doesn't start.

```
$ vt run --dry-run build ⊘ cache disabled
```

## `vt run nested-dry-run`

A real run starts it, and it prints its own dry run.

```
$ vt run --dry-run build ⊘ cache disabled
$ vtt print-file src/a.txt
```
