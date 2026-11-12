# uncached_stdio_color_detection

## `vt run --log labeled shell-force`

```
[color-env-handling#shell-force] $ vtt print-env FORCE_COLOR > shell-force.txt ⊘ cache disabled
```

## `vtt print-file shell-force.txt`

```
(undefined)
```

## `vt run --log grouped shell-force`

```
[color-env-handling#shell-force] $ vtt print-env FORCE_COLOR > shell-force.txt ⊘ cache disabled
```

## `vtt print-file shell-force.txt`

```
(undefined)
```

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
