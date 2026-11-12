# uncached_stdio_color_detection

## `vtt pipe-stdout -- vt run --log labeled uncached-force`

```
--- stdout ---
[color-env-handling#uncached-force] $ vtt print-env FORCE_COLOR ⊘ cache disabled
[color-env-handling#uncached-force] (undefined)
```

## `vt run --log interleaved uncached-force`

```
$ vtt print-env FORCE_COLOR ⊘ cache disabled
(undefined)
```
