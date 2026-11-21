# unknown

A step that would be a hit or changed is `unknown` when a step it waits on would write files first: by running, or by being a hit that restores outputs. The text output doesn't show it.

## `vt run after-emit`

```
$ vtt write-file dist/out.txt built

$ vtt print checked
checked

---
vt run: 0/2 cache hit (0%), 2/2 successful. (Run `vt run --last-details` for full details)
```

## `vtt pipe-stdout -- vt run --dry-run --json after-emit`

`emit` is a hit that restores `dist/out.txt`, so `after-emit` is unknown.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"after-emit":{"package":".","name":"after-emit","dependsOn":["emit"],"steps":[{"program":"<vtt>","args":["print","checked"],"cache":{"status":"unknown","baseline":{"durationMs":<ms>}}}]},"emit":{"package":".","name":"emit","steps":[{"program":"<vtt>","args":["write-file","dist/out.txt","built"],"cache":{"status":"hit","baseline":{"durationMs":<ms>}}}]}}}
```

## `vt run after-build`

```
$ vtt print-file a.txt b.txt
a
b

$ vtt print checked ◉ cache hit, replaying
checked

---
vt run: 1/2 cache hit (50%), 2/2 successful. (Run `vt run --last-details` for full details)
```

## `vtt pipe-stdout -- vt run --dry-run --json after-build`

`build` has no outputs, so `after-build` stays a hit.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"after-build":{"package":".","name":"after-build","dependsOn":["build"],"steps":[{"program":"<vtt>","args":["print","checked"],"cache":{"status":"hit","baseline":{"durationMs":<ms>}}}]},"build":{"package":".","name":"build","steps":[{"program":"<vtt>","args":["print-file","a.txt","b.txt"],"cache":{"status":"hit","baseline":{"durationMs":<ms>}}}]}}}
```

## `vt run emit-then-print`

```
$ vtt write-file dist/steps.txt built

$ vtt print after
after

---
vt run: 0/2 cache hit (0%), 2/2 successful. (Run `vt run --last-details` for full details)
```

## `vtt pipe-stdout -- vt run --dry-run --json emit-then-print`

The first step restores outputs, so the step after it is unknown.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"emit-then-print":{"package":".","name":"emit-then-print","steps":[{"program":"<vtt>","args":["write-file","dist/steps.txt","built"],"cache":{"status":"hit","baseline":{"durationMs":<ms>}}},{"program":"<vtt>","args":["print","after"],"cache":{"status":"unknown","baseline":{"durationMs":<ms>}}}]}}}
```

## `vt run nested-then-print`

```
$ vtt write-file dist/out.txt built ◉ cache hit, replaying

$ vtt print after ◉ cache hit, replaying
after

---
vt run: 2/2 cache hit (100%), 2/2 successful. (Run `vt run --last-details` for full details)
```

## `vtt pipe-stdout -- vt run --dry-run --json nested-then-print`

The nested plan restores outputs, so the step after it is unknown.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"nested-then-print":{"package":".","name":"nested-then-print","steps":[{"expandedFrom":"vt run emit","plan":{"emit":{"package":".","name":"emit","steps":[{"program":"<vtt>","args":["write-file","dist/out.txt","built"],"cache":{"status":"hit","baseline":{"durationMs":<ms>}}}]}}},{"program":"<vtt>","args":["print","after"],"cache":{"status":"unknown","baseline":{"durationMs":<ms>}}}]}}}
```

## `vt run --dry-run after-emit`

The text shows today's check.

```
$ vtt write-file dist/out.txt built ◉ cache hit
$ vtt print checked ◉ cache hit

---
vt run: 2/2 cache hit (100%), nothing was run.
```
