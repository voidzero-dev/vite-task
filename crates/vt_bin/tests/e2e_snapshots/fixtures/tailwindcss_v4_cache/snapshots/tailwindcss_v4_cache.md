# tailwindcss_v4_cache

`@tailwindcss/cli` v4 (Rust `oxide` + `lightningcss` rayon globbing) scans `src/index.html` and compiles `src/input.css` to `dist/output.css`. Editing `src/index.html` invalidates the cache.

## `vt run --cache build`

first run: cache miss

```
$ tailwindcss -i src/input.css -o dist/output.css
≈ tailwindcss v<version>

Done in <duration>
```

## `vtt grep-file dist/output.css text-red-500`

```
dist/output.css: found "text-red-500"
```

## `vt run --cache build`

second run: cache hit

```
$ tailwindcss -i src/input.css -o dist/output.css ◉ cache hit, replaying
≈ tailwindcss v<version>

Done in <duration>

---
vt run: cache hit.
```

## `vtt replace-file-content src/index.html text-red-500 text-blue-500`

```
```

## `vt run --cache build`

third run: cache miss after editing src/index.html

```
$ tailwindcss -i src/input.css -o dist/output.css ○ cache miss: 'src/index.html' modified, executing
≈ tailwindcss v<version>

Done in <duration>
```

## `vtt grep-file dist/output.css text-blue-500`

```
dist/output.css: found "text-blue-500"
```
