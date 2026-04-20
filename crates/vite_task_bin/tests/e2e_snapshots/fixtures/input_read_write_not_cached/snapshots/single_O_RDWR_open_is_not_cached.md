# single_O_RDWR_open_is_not_cached

Opening a single file with `O_RDWR` (e.g. `touch` keeping the file) should
count as a read-write overlap and prevent caching, just like separate
read+write syscalls.

## `vt run task`

```
~/packages/touch-pkg$ vtt touch-file src/data.txt

---
vt run: @test/touch-pkg#task not cached because it modified its input. (Run `vt run --last-details` for full details)
```

## `vt run task`

```
~/packages/touch-pkg$ vtt touch-file src/data.txt

---
vt run: @test/touch-pkg#task not cached because it modified its input. (Run `vt run --last-details` for full details)
```
