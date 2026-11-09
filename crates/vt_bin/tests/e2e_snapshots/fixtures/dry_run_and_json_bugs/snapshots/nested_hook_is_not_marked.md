# nested_hook_is_not_marked

Bug: each step of a `pre` or `post` script has a `hook`, but a `pre` script that runs a nested `vt run` doesn't, so it reads as the task's own command.

## `vtt pipe-stdout -- vt run --dry-run --json check`

Expected: the first step has `"hook":"precheck"`, like the `postcheck` step.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"check":{"package":".","name":"check","steps":[{"expandedFrom":"vt run build","plan":{"build":{"package":".","name":"build","steps":[{"program":"<vtt>","args":["print-file","src/a.txt"],"cache":{"status":"noBaseline"}}]}}},{"program":"<vtt>","args":["print","check"],"cache":{"status":"disabled"}},{"program":"<vtt>","args":["print","post"],"hook":"postcheck","cache":{"status":"disabled"}}]}}}
```
