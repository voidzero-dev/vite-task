# multi_task_with_read_write_shows_not_cached_in_summary

In a multi-task (`-r`) run, tasks with a read-write overlap should appear in
the compact summary stats alongside an `InputModified` notice.

## `vt run -r task`

```
~/packages/normal-pkg$ vtt print hello
hello

~/packages/touch-pkg$ vtt touch-file src/data.txt

~/packages/rw-pkg$ vtt replace-file-content src/data.txt i !

---
vt run: 0/3 cache hit (0%). @test/touch-pkg#task (and 1 more) not cached because they modified their inputs. (Run `vt run --last-details` for full details)
```

## `vt run -r task`

```
~/packages/normal-pkg$ vtt print hello ◉ cache hit, replaying
hello

~/packages/touch-pkg$ vtt touch-file src/data.txt

~/packages/rw-pkg$ vtt replace-file-content src/data.txt i !

---
vt run: 1/3 cache hit (33%). @test/touch-pkg#task (and 1 more) not cached because they modified their inputs. (Run `vt run --last-details` for full details)
```
