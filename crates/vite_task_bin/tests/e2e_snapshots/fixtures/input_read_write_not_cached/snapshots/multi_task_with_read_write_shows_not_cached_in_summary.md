# multi_task_with_read_write_shows_not_cached_in_summary

In a multi-task (`-r`) run, tasks with a read-write overlap should appear in the compact summary stats alongside an `InputModified` notice.

## `vt run -r task`

```
~/packages/normal-pkg$ vtt print hello
hello

~/packages/touch-pkg$ vtt touch-file src/data.txt

~/packages/rw-pkg$ vtt replace-file-content src/data.txt i !

---
vt run: 0/3 cache hit (0%). @test/rw-pkg#task not cached because it modified its input. (Run `vt run --last-details` for full details)
```

## `vt run -r task`

```
~/packages/normal-pkg$ vtt print hello ◉ cache hit, replaying
hello

~/packages/touch-pkg$ vtt touch-file src/data.txt ◉ cache hit, replaying

~/packages/rw-pkg$ vtt replace-file-content src/data.txt i !

---
vt run: 2/3 cache hit (66%). @test/rw-pkg#task not cached because it modified its input. (Run `vt run --last-details` for full details)
```
