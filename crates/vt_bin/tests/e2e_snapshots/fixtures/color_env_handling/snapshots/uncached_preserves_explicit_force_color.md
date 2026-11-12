# uncached_preserves_explicit_force_color

## `FORCE_COLOR=3 vt run --log labeled uncached-force`

```
[color-env-handling#uncached-force] $ vtt print-env FORCE_COLOR ⊘ cache disabled
[color-env-handling#uncached-force] 3
```

## `FORCE_COLOR=3 vt run --log grouped uncached-force`

```
[color-env-handling#uncached-force] $ vtt print-env FORCE_COLOR ⊘ cache disabled
── [color-env-handling#uncached-force] ──
3
```
