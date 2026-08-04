# default_auto_output_restores_written_files

Exercises the default output behavior. When `output` is omitted, the runner automatically tracks writes, archives the generated file, and restores it on cache hit.

## `vt run auto-output-default`

first run writes dist/default.txt and archives fspy-tracked outputs

```
$ vtt write-file dist/default.txt ok
```

## `vtt rm dist/default.txt`

remove the output so restoration is observable

```
```

## `vt run auto-output-default`

cache hit: default auto output is restored

```
$ vtt write-file dist/default.txt ok ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt print-file dist/default.txt`

restored from the default auto output archive

```
ok
```
