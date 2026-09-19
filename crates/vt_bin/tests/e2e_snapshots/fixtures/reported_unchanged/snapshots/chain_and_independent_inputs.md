# chain_and_independent_inputs

A report preserves downstream cache validation, independent input changes, direct requests, and uncached dependents.

## `vt run cold-dependent`

```
$ vtt report-unchanged
command ran
◉ unchanged (reported by task)

$ vtt print-file b.txt
own source

---
vt run: 0/2 cache hit (0%), 1 unchanged. (Run `vt run --last-details` for full details)
```

## `vt run c`

```
$ vtt report-unchanged build a.txt artifact.txt
built artifact

$ vtt print-file b.txt artifact.txt
own source
artifact

$ vtt print-file c.txt
last task

---
vt run: 0/3 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vtt write-file a.txt unchanged`

```
```

## `vt run c`

```
$ vtt report-unchanged build a.txt artifact.txt ○ cache miss: 'a.txt' modified, executing
command ran
◉ unchanged (reported by task)

$ vtt print-file b.txt artifact.txt ◉ cache hit, replaying
own source
artifact

$ vtt print-file c.txt ◉ cache hit, replaying
last task

---
vt run: 2/3 cache hit (66%), 1 unchanged. (Run `vt run --last-details` for full details)
```

## `vtt write-file b.txt 'changed own source'`

```
```

## `vt run c`

```
$ vtt report-unchanged build a.txt artifact.txt ◉ cache hit, replaying
command ran

$ vtt print-file b.txt artifact.txt ○ cache miss: 'b.txt' modified, executing
changed own sourceartifact

$ vtt print-file c.txt ◉ cache hit, replaying
last task

---
vt run: 2/3 cache hit (66%). (Run `vt run --last-details` for full details)
```

## `vt run --no-cache b`

```
$ vtt report-unchanged build a.txt artifact.txt ⊘ cache disabled
command ran
◉ unchanged (reported by task)

$ vtt print-file b.txt artifact.txt ⊘ cache disabled
changed own sourceartifact

---
vt run: 0/2 cache hit (0%), 1 unchanged. (Run `vt run --last-details` for full details)
```

## `vt run uncached-dependent`

```
$ vtt report-unchanged ◉ cache hit, replaying
command ran

$ vtt print-file b.txt ⊘ cache disabled
changed own source
---
vt run: 1/2 cache hit (50%). (Run `vt run --last-details` for full details)
```

## `vt run uncached-dependent`

```
$ vtt report-unchanged ◉ cache hit, replaying
command ran

$ vtt print-file b.txt ⊘ cache disabled
changed own source
---
vt run: 1/2 cache hit (50%). (Run `vt run --last-details` for full details)
```
