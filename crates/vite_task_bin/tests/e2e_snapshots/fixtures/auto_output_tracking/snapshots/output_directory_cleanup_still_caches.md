# output_directory_cleanup_still_caches

Build tools empty their output directory before writing to it, which means they stat and list it. Those reads must not make the directory an input: its contents are the task's own product, and a fingerprint of them can never match on a clean checkout where the directory does not exist yet.

## `vt run task`

```
~/packages/clean-pkg$ vtt publish-dir rebuild src/data.txt dist
```

## `vt run task`

```
~/packages/clean-pkg$ vtt publish-dir rebuild src/data.txt dist ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt rm -rf dist`

```
```

## `vt run task`

```
~/packages/clean-pkg$ vtt publish-dir rebuild src/data.txt dist ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt print-file dist/out.txt`

```
clean
```
