# run_plan_paths_are_not_normalized

Bug: the run plan prints a step's `cwd` and `program` as they were joined, with `..` left in, where the task graph cleans its paths. So a step that runs in its package's folder gets a `cwd`, which should be absent, and a path outside the workspace is written relative to the workspace root, where it should be absolute.

## `vtt pipe-stdout -- vt run --json`

The task graph: `up-and-back` has no `cwd`, and the `cwd` of `outside` is absolute.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","packages":{".":{"name":"dry-run-and-json-bugs"}},"tasks":{"build":{"package":".","name":"build","command":"vtt print-file src/a.txt"},"cd-and-back":{"package":".","name":"cd-and-back","command":"cd src && cd .. && vtt print-cwd"},"check":{"package":".","name":"check","command":"vtt print check"},"emit":{"package":".","name":"emit","command":"vtt write-file dist/out.txt built"},"outside":{"package":".","name":"outside","cwd":"<absolute path>","command":"vtt print-cwd"},"postcheck":{"package":".","name":"postcheck","command":"vtt print post"},"precheck":{"package":".","name":"precheck","command":"vt run build"},"same-command":{"package":".","name":"same-command","command":"vtt print-file src/a.txt","dependsOn":["build"]},"tool":{"package":".","name":"tool","command":"src/../bin/tool"},"up-and-back":{"package":".","name":"up-and-back","command":"vtt print-cwd"}}}
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

## `vtt pipe-stdout -- vt run --dry-run --json outside`

Expected: an absolute `cwd`, as in the task graph. It's `..`, relative to the workspace root.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"outside":{"package":".","name":"outside","steps":[{"program":"<vtt>","args":["print-cwd"],"cwd":"..","cache":{"status":"disabled"}}]}}}
```

## `vtt pipe-stdout -- vt run --dry-run --json tool`

Expected: `bin/tool`. It's `src/../bin/tool`.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"tool":{"package":".","name":"tool","steps":[{"program":"src/../bin/tool","cache":{"status":"disabled"}}]}}}
```

## `vt run up-and-back`

The command runs in the package's folder.

```
~/src/..$ vtt print-cwd ⊘ cache disabled
<workspace>
```
