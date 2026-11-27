# pagefind_cache

`pagefind` (static-PIE ELF64 on Linux x86-64) indexes `site/index.html` into `dist/pagefind/`. Deleting `dist` and re-running restores outputs from cache; editing `site/index.html` invalidates the cache.

## `vt run --cache index`

first run: cache miss

```
$ pagefind --site site --output-path dist/pagefind --silent
```

## `vtt grep-file dist/pagefind/pagefind-entry.json '"page_count":1'`

```
dist/pagefind/pagefind-entry.json: found "\"page_count\":1"
```

## `vtt rm -r dist`

```
```

## `vt run --cache index`

second run: cache hit restores dist/pagefind

```
$ pagefind --site site --output-path dist/pagefind --silent ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt grep-file dist/pagefind/pagefind-entry.json '"page_count":1'`

```
dist/pagefind/pagefind-entry.json: found "\"page_count\":1"
```

## `vtt replace-file-content site/index.html initial-pagefind edited-pagefind`

```
```

## `vt run --cache index`

third run: cache miss after editing site/index.html

```
$ pagefind --site site --output-path dist/pagefind --silent ○ cache miss: 'site/index.html' modified, executing
```

## `vtt grep-file dist/pagefind/pagefind-entry.json '"page_count":1'`

```
dist/pagefind/pagefind-entry.json: found "\"page_count\":1"
```
