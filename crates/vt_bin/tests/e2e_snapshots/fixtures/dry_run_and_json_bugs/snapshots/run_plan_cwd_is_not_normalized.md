# run_plan_cwd_is_not_normalized

Bug: both tasks run in the package's folder, by a `cwd` of `src/..` or by `cd src && cd ..`. The task graph leaves out a `cwd` that is the package's folder, but the run plan prints `"cwd":"src/.."`, which isn't a normalized path, for a step that runs in the package's folder and so should have no `cwd`.

## `vtt pipe-stdout -- vt run --json`

The task graph: `up-and-back` has no `cwd`.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","packages":{".":{"name":"dry-run-and-json-bugs"}},"tasks":{"build":{"package":".","name":"build","command":"vtt print-file src/a.txt"},"cd-and-back":{"package":".","name":"cd-and-back","command":"cd src && cd .. && vtt print-cwd"},"same-command":{"package":".","name":"same-command","command":"vtt print-file src/a.txt","dependsOn":["build"]},"uncached":{"package":".","name":"uncached","command":"vtt print uncached"},"up-and-back":{"package":".","name":"up-and-back","command":"vtt print-cwd"}}}
```

## `vtt pipe-stdout -- vt run --dry-run --json up-and-back`

Expected: no `cwd`. It's `src/..`.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"up-and-back":{"package":".","name":"up-and-back","steps":[{"program":"<vtt>","args":["print-cwd"],"cwd":"src/..","cache":{"status":"disabled"}}]}}}
```

## `vtt pipe-stdout -- vt run --dry-run --json cd-and-back`

The same after `cd src && cd ..`.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"cd-and-back":{"package":".","name":"cd-and-back","steps":[{"program":"<vtt>","args":["print-cwd"],"cwd":"src/..","cache":{"status":"disabled"}}]}}}
```

## `vt run up-and-back`

The command runs in the package's folder.

```
~/src/..$ vtt print-cwd ⊘ cache disabled
<workspace>
```
