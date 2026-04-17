# single_read_write_task_shows_not_cached_message

## `vt run task`

```
~/packages/rw-pkg$ vtt replace-file-content src/data.txt i !

---
vt run: @test/rw-pkg#task not cached because it modified its input. (Run `vt run --last-details` for full details)
```

## `vt run task`

```
~/packages/rw-pkg$ vtt replace-file-content src/data.txt i !

---
vt run: @test/rw-pkg#task not cached because it modified its input. (Run `vt run --last-details` for full details)
```
