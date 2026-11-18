# uncached_piped_colors

## `vt run --log labeled uncached-color`

```
\x1b[90m[color-env-handling#uncached-color] \x1b[34m$ vtt print-color green coverage-row \x1b[30m\xe2\x8a\x98 \x1b[90mcache disabled
\x1b[90m[color-env-handling#uncached-color] \x1b[32mcoverage-row
```

## `vt run --log grouped uncached-color`

```
\x1b[90m[color-env-handling#uncached-color] \x1b[34m$ vtt print-color green coverage-row \x1b[30m\xe2\x8a\x98 \x1b[90mcache disabled
\x1b[90m\xe2\x94\x80\xe2\x94\x80 \x1b[90m[color-env-handling#uncached-color] \x1b[90m\xe2\x94\x80\xe2\x94\x80
\x1b[32mcoverage-row
```
