# sharp_image_cache

`sharp` (`libvips-cpp.so` via `$ORIGIN`) renders `src/icon.svg` to a resized PNG `dist/out.png` and records its decoded dimensions and pixel RGBA in `dist/meta.txt`. Editing `src/icon.svg` invalidates the cache.

## `vt run --cache build`

first run: cache miss

```
$ node build.mjs
```

## `vtt print-file dist/meta.txt`

```
8x8:255,0,0,255
```

## `vt run --cache build`

second run: cache hit

```
$ node build.mjs ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt replace-file-content src/icon.svg #ff0000 #00ff00`

```
```

## `vt run --cache build`

third run: cache miss after editing src/icon.svg

```
$ node build.mjs ○ cache miss: 'src/icon.svg' modified, executing
```

## `vtt print-file dist/meta.txt`

```
8x8:0,255,0,255
```
