# suggested_exclusions_make_read_write_task_cacheable

Applying the suggested `cache` settings, in both the package-relative and the workspace-based form, lets the task be cached: each second run is a cache hit.

## `vt run task-excluded`

```
~/packages/rw-pkg$ vtt replace-file-content src/data.txt i !
```

## `vt run task-excluded`

```
~/packages/rw-pkg$ vtt replace-file-content src/data.txt i ! ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vt run task-outside-excluded`

```
~/packages/rw-pkg$ vtt replace-file-content ../../shared.txt i !
```

## `vt run task-outside-excluded`

```
~/packages/rw-pkg$ vtt replace-file-content ../../shared.txt i ! ◉ cache hit, replaying

---
vt run: cache hit.
```
