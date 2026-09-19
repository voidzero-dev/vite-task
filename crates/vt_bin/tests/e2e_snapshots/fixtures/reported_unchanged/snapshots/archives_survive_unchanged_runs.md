# archives_survive_unchanged_runs

An unchanged run records the new inputs and preserves automatically inferred outputs for restoration.

## `vt run build`

```
$ vtt report-unchanged build input.txt artifact.txt
built artifact
```

## `vtt write-file input.txt unchanged`

```
```

## `vt run build`

```
$ vtt report-unchanged build input.txt artifact.txt ○ cache miss: 'input.txt' modified, executing
command ran
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).
```

## `vtt rm artifact.txt`

```
```

## `vt run build`

```
$ vtt report-unchanged build input.txt artifact.txt ◉ cache hit, replaying
command ran

---
vt run: cache hit.
```

## `vtt print-file artifact.txt`

```
artifact
```
