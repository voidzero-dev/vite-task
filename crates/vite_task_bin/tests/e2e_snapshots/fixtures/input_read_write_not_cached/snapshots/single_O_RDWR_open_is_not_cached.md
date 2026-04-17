# single_O_RDWR_open_is_not_cached

Tests that tasks modifying their own inputs (read-write overlap) are not cached.
vtt replace-file-content reads then writes the same file — fspy detects both.

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
