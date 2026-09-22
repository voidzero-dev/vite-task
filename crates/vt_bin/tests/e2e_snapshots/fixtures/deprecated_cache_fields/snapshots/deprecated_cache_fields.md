# deprecated_cache_fields

Top-level cache fields still work but print one deprecation warning listing the fields and affected tasks.

## `vt run build`

warning, cache miss

```
warning: Task fields `env`, `input`, `output` are deprecated at the top level; move them under `cache`, e.g. `cache: { env: [...] }`. Affected tasks: build, test
$ vtt print-file input.txt
initial
```

## `vtt replace-file-content input.txt initial modified`

modify declared input

```
```

## `vt run build`

warning, cache miss: input modified

```
warning: Task fields `env`, `input`, `output` are deprecated at the top level; move them under `cache`, e.g. `cache: { env: [...] }`. Affected tasks: build, test
$ vtt print-file input.txt ○ cache miss: 'input.txt' modified, executing
modified
```

## `vt run build`

warning, cache hit

```
warning: Task fields `env`, `input`, `output` are deprecated at the top level; move them under `cache`, e.g. `cache: { env: [...] }`. Affected tasks: build, test
$ vtt print-file input.txt ◉ cache hit, replaying
modified

---
vt run: cache hit.
```
