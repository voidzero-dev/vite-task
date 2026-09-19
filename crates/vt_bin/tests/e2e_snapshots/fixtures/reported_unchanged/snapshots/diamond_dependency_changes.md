# diamond_dependency_changes

An unchanged prerequisite does not hide another prerequisite changing the dependent input.

## `vt run --concurrency-limit 1 diamond`

```
$ vtt report-unchanged build a.txt artifact.txt
built artifact

$ vtt cp d.txt d-output.txt

$ vtt print-file b.txt artifact.txt d-output.txt
own source
artifact
other dependency

---
vt run: 0/3 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vtt write-file a.txt unchanged`

```
```

## `vtt write-file d.txt 'changed other dependency'`

```
```

## `vt run --concurrency-limit 1 diamond`

```
$ vtt report-unchanged build a.txt artifact.txt ○ cache miss: 'a.txt' modified, executing
command ran
◉ unchanged (reported by task)

$ vtt cp d.txt d-output.txt ○ cache miss: 'd.txt' modified, executing

$ vtt print-file b.txt artifact.txt d-output.txt ○ cache miss: 'd-output.txt' modified, executing
own source
artifact
changed other dependency
---
vt run: 0/3 cache hit (0%), 1 unchanged. (Run `vt run --last-details` for full details)
```
