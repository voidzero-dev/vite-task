# multi_task_with_read_write_shows_not_cached_in_summary

Tests that tasks modifying their own inputs (read-write overlap) are not cached.
vtt replace-file-content reads then writes the same file — fspy detects both.

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
