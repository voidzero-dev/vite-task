# uncached_redirected_stdout

## `vtt pipe-stdout -- vt run --log labeled uncached-force`

```
--- stdout ---
[color-env-handling#uncached-force] $ vtt print-env FORCE_COLOR ⊘ cache disabled
[color-env-handling#uncached-force] (undefined)
```

## `vtt pipe-stdout -- vt run --log grouped uncached-force`

```
--- stdout ---
[color-env-handling#uncached-force] $ vtt print-env FORCE_COLOR ⊘ cache disabled
── [color-env-handling#uncached-force] ──
(undefined)
```
