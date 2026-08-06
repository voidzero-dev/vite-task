# output_negative_glob_prevents_read_write_overlap

Exercises `output: [{ auto: true }, "!scratch/**"]` on a task that reads and writes `scratch/overlap.txt`. Because the write is excluded from the auto-output set, it should not count as a read-write overlap, while non-excluded outputs are still archived and restored.

## `vt run auto-output-negative-overlap`

first run reads and writes an output-negative path

```
$ node scripts/auto_output_negative_overlap.mjs
```

## `vtt rm dist/negative-overlap.txt`

remove the non-excluded output so restoration is observable

```
```

## `vt run auto-output-negative-overlap`

cache hit: the output-negative write did not block caching

```
$ node scripts/auto_output_negative_overlap.mjs ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt print-file dist/negative-overlap.txt`

restored from the auto output archive

```
keep
```
