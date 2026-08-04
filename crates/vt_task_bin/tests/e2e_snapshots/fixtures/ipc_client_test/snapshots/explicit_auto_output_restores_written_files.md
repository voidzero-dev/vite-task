# explicit_auto_output_restores_written_files

Exercises `output: [{ auto: true }]` with explicit inputs disabled. The runner attaches fspy for output tracking, archives the written file, and restores it on cache hit.

## `vt run auto-output-explicit`

first run writes dist/auto.txt and archives fspy-tracked outputs

```
$ vtt write-file dist/auto.txt ok
```

## `vtt rm dist/auto.txt`

remove the output so restoration is observable

```
```

## `vt run auto-output-explicit`

cache hit: fspy-tracked output is restored

```
$ vtt write-file dist/auto.txt ok ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt print-file dist/auto.txt`

restored from the auto output archive

```
ok
```
