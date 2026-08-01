# atomic_directory_publish_is_cached_and_restored

A task that stages output under `dist.tmp` and renames the directory into place must still have its outputs collected. Every write lands on the staging path and only the directory itself carries the rename, so a tracker that ignores directory renames archives nothing and a later cache hit restores an incomplete tree. The third run removes `dist` first to prove the outputs really came back from the archive rather than from the previous run leaving them behind.

## `vt run task`

```
~/packages/atomic-pkg$ vtt publish-dir atomic src/data.txt dist
```

## `vt run task`

```
~/packages/atomic-pkg$ vtt publish-dir atomic src/data.txt dist ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt rm -rf dist`

```
```

## `vt run task`

```
~/packages/atomic-pkg$ vtt publish-dir atomic src/data.txt dist ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt print-file dist/out.txt`

```
atomic
```
