# nested_plan

A nested `vt run` is a step with the plan of that run.

## `vt run build`

```
$ vtt print-file a.txt b.txt
a
b
```

## `vtt pipe-stdout -- vt run --dry-run --json nested`

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"nested":{"package":".","name":"nested","steps":[{"expandedFrom":"vt run build","plan":{"build":{"package":".","name":"build","steps":[{"program":"<vtt>","args":["print-file","a.txt","b.txt"],"cache":{"status":"hit","baseline":{"durationMs":<ms>}}}]}}}]}}}
```
