# entry_stored_earlier_in_the_run

Bug: `same-command` runs the same command, with the same inputs, as `build`, which it waits on, so they share one cache entry. On a fresh cache, a real run runs `build` and stores the entry, and `same-command` then hits it. A dry run checks both against today's cache, finds no entry for either, and predicts two misses. The JSON shows `same-command` as `noBaseline`, where it should be `unknown`, as for a step whose files an earlier step may change.

## `vt run --dry-run same-command`

Predicts 0/2 cache hits.

```
$ vtt print-file src/a.txt
$ vtt print-file src/a.txt

---
vt run: 0/2 cache hit (0%), nothing was run.
```

## `vtt pipe-stdout -- vt run --dry-run --json same-command`

Expected: `same-command` is `unknown`. It's `noBaseline`.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"build":{"package":".","name":"build","steps":[{"program":"<vtt>","args":["print-file","src/a.txt"],"cache":{"status":"noBaseline"}}]},"same-command":{"package":".","name":"same-command","dependsOn":["build"],"steps":[{"program":"<vtt>","args":["print-file","src/a.txt"],"cache":{"status":"noBaseline"}}]}}}
```

## `vt run same-command`

The real run hits 1/2.

```
$ vtt print-file src/a.txt
original

$ vtt print-file src/a.txt ◉ cache hit, replaying
original

---
vt run: 1/2 cache hit (50%), 2/2 successful. (Run `vt run --last-details` for full details)
```
