# uncached_explicit_color_controls

## `FORCE_COLOR=0 vt run --log labeled uncached-force`

```
[color-env-handling#uncached-force] $ vtt print-env FORCE_COLOR ⊘ cache disabled
[color-env-handling#uncached-force] 0
```

## `FORCE_COLOR=3 vt run --log labeled uncached-force`

```
[color-env-handling#uncached-force] $ vtt print-env FORCE_COLOR ⊘ cache disabled
[color-env-handling#uncached-force] 3
```

## `NO_COLOR=1 vt run --log labeled uncached-force`

```
[color-env-handling#uncached-force] $ vtt print-env FORCE_COLOR ⊘ cache disabled
[color-env-handling#uncached-force] (undefined)
```

## `NODE_DISABLE_COLORS=1 vt run --log labeled uncached-force`

```
[color-env-handling#uncached-force] $ vtt print-env FORCE_COLOR ⊘ cache disabled
[color-env-handling#uncached-force] (undefined)
```

## `vt run --log labeled explicit-no-color`

```
[color-env-handling#explicit-no-color] $ FORCE_COLOR=0 vtt print-env FORCE_COLOR ⊘ cache disabled
[color-env-handling#explicit-no-color] 0
```
