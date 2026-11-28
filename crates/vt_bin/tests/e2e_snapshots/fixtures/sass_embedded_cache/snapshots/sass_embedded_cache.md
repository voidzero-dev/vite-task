# sass_embedded_cache

`sass-embedded` (Dart VM AOT binary) compiles `src/main.scss` (which `@use`s `src/_vars.scss`) to `dist/main.css`. Editing `src/_vars.scss` invalidates the cache.

## `vt run --cache build`

first run: cache miss

```
$ node build.mjs
```

## `vtt print-file dist/main.css`

```
.box {
  color: #ff0000;
}
```

## `vt run --cache build`

second run: cache hit

```
$ node build.mjs ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt replace-file-content src/_vars.scss #ff0000 #0000ff`

```
```

## `vt run --cache build`

third run: cache miss after editing src/_vars.scss

```
$ node build.mjs ○ cache miss: 'src/_vars.scss' modified, executing
```

## `vtt print-file dist/main.css`

```
.box {
  color: #0000ff;
}
```
