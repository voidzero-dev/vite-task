# cache_disabled

Tasks with caching off are reported as disabled, even when the cache has an entry.

## `vt run --dry-run uncached`

cache: false

```
dry-run#uncached: $ vtt print uncached → Cache disabled
```

## `vt run test`

```
$ vtt print-file src.txt
source

$ vtt print-file test.txt
tests

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vt run --dry-run --no-cache test`

--no-cache turns caching off for every task

```
dry-run#build: $ vtt print-file src.txt → Cache disabled
dry-run#test: $ vtt print-file test.txt → Cache disabled
```
