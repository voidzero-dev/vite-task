# explicit_auto_output_respects_negative_globs

Exercises `output: [{ auto: true }, "!dist/skip.txt"]`. The runner archives auto-tracked writes except those excluded by output negative globs, so cache hits restore `dist/keep.txt` but not `dist/skip.txt`.

## `vt run auto-output-negative`

first run writes keep and skip files, but skip is excluded by a negative output glob

```
$ node scripts/auto_output_negative.mjs
```

## `vtt rm dist/keep.txt dist/skip.txt`

remove both writes so restoration proves the negative glob was honored

```
```

## `vt run auto-output-negative`

cache hit: only non-excluded auto output is restored

```
$ node scripts/auto_output_negative.mjs ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt print-file dist/keep.txt`

restored from the auto output archive

```
keep
```

## `vtt print-file dist/skip.txt`

not restored because the negative output glob excluded it

```
dist/skip.txt: not found
```
