# flags

Run flags that combine with `--dry-run --json`. Like them, they go before the task name.

## `vtt pipe-stdout -- vt run --dry-run --json --cache uncached`

`--cache` leaves a task with `cache: false` uncached.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"uncached":{"package":".","name":"uncached","steps":[{"program":"<vtt>","args":["print","uncached"],"cache":{"status":"disabled"}}]}}}
```

## `vtt pipe-stdout -- vt run --dry-run --json --concurrency-limit 1 --ignore-depends-on other`

Flags that only change scheduling or `dependsOn` are accepted too.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"other":{"package":".","name":"other","steps":[{"program":"<vtt>","args":["print","other"],"cache":{"status":"noBaseline"}}]}}}
```

## `vt run other --dry-run --json`

After the task name, `--dry-run` and `--json` are passed to the task, which runs.

```
$ vtt print other --dry-run --json
other --dry-run --json
```
